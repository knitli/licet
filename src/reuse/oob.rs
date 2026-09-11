//! Out-of-band REUSE metadata: read `REUSE.toml` (current spec) and `.reuse/dep5`
//! (legacy) for detection, and write `REUSE.toml` annotations for non-annotatable
//! files (FR-015, FR-003a, research §7).
//!
//! `REUSE.toml` files are discovered at every directory depth through the same
//! content snapshot the scan evaluates, so staged checks observe staged
//! metadata. Hierarchy resolution mirrors the reference REUSE tool: documents
//! are consulted root-first and stop after the rootmost `override` table;
//! `aggregate` tables always contribute; `closest` tables are a per-field
//! fallback. Every contributing table keeps its provenance in
//! [`crate::domain::MetadataOrigin`].

use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};
use serde::Deserialize;

use crate::domain::{MetadataOrigin, OobSource, OutOfBandEntry, Precedence};
use crate::walk::Snapshot;

/// All out-of-band metadata discovered under a repository root.
#[derive(Debug, Default)]
pub struct OutOfBand {
    /// Parsed `REUSE.toml` documents, shallowest directory first.
    docs: Vec<ReuseDoc>,
    /// Parsed `.reuse/dep5` paragraphs in document order.
    dep5: Vec<Dep5Para>,
    /// Whether `.reuse/dep5` exists in the snapshot (even when it carries no
    /// paragraphs — mere coexistence with any `REUSE.toml` is an error).
    dep5_present: bool,
}

/// One parsed `REUSE.toml` document plus where it lives.
#[derive(Debug)]
struct ReuseDoc {
    /// Repo-relative path of the document (`REUSE.toml`, `sub/REUSE.toml`).
    rel: PathBuf,
    /// Directory containing the document (empty for the project root);
    /// annotation paths are relative to this base and can never escape it.
    base: PathBuf,
    tables: Vec<OobTable>,
}

/// One validated `[[annotations]]` table with its matchers precompiled.
#[derive(Debug)]
struct OobTable {
    index: usize,
    matchers: Vec<ReuseMatcher>,
    licenses: Vec<String>,
    copyrights: Vec<String>,
    precedence: Precedence,
}

/// One parsed `.reuse/dep5` paragraph (always `aggregate` per REUSE 3.3
/// §"Order of precedence": dep5 licensing adds to file-level licensing).
#[derive(Debug)]
struct Dep5Para {
    index: usize,
    matchers: Vec<ReuseMatcher>,
    licenses: Vec<String>,
    copyrights: Vec<String>,
}

/// A path matcher: either a compiled glob or a literal fallback for patterns
/// the glob compiler rejects (never a silent drop).
#[derive(Debug)]
enum ReuseMatcher {
    Glob(GlobMatcher),
    Literal(String),
}

impl ReuseMatcher {
    fn is_match(&self, path: &str) -> bool {
        match self {
            ReuseMatcher::Glob(g) => g.is_match(path),
            ReuseMatcher::Literal(l) => l == path,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReuseToml {
    version: u32,
    #[serde(default)]
    annotations: Vec<ReuseAnnotation>,
}

#[derive(Debug, Deserialize)]
struct ReuseAnnotation {
    path: StringList,
    #[serde(rename = "SPDX-License-Identifier", default)]
    license: StringList,
    #[serde(rename = "SPDX-FileCopyrightText", default)]
    copyright: StringList,
    /// REUSE 3.3 `precedence`: exactly `closest` (default) | `aggregate` |
    /// `override`. Anything else is a malformed document, not a silent default.
    precedence: Option<String>,
}

/// A TOML string-or-list field (`path`, licensing, copyright).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringList {
    One(String),
    Many(Vec<String>),
}

impl Default for StringList {
    fn default() -> Self {
        StringList::Many(Vec::new())
    }
}

impl StringList {
    fn into_vec(self) -> Vec<String> {
        match self {
            StringList::One(s) => vec![s],
            StringList::Many(v) => v,
        }
    }
}

impl OutOfBand {
    /// Load `REUSE.toml` documents at every depth plus `.reuse/dep5` from the
    /// working tree at `root`. Fallible: malformed metadata is an error with
    /// document path and location, never silent absence (F08, F09, F13).
    pub fn load(root: &Path) -> crate::error::Result<Self> {
        Self::load_snapshot(&Snapshot::Worktree {
            root: root.to_path_buf(),
        })
    }

    /// Load metadata through a content snapshot (staged checks observe staged
    /// metadata, never the working copy).
    ///
    /// Read failures and non-UTF-8 metadata are errors, never silent absence
    /// (F04, F13). Malformed documents, a `version` other than 1, a missing
    /// `path`, an invalid `precedence`, an invalid license expression, and
    /// `REUSE.toml`+`.reuse/dep5` coexistence all fail here — before any
    /// write — carrying the document path and parse location.
    pub fn load_snapshot(snapshot: &Snapshot) -> crate::error::Result<Self> {
        use crate::error::LicetError;
        let mut oob = OutOfBand::default();
        for rel in discover_doc_paths(snapshot)? {
            let bytes = snapshot.read(&rel)?.ok_or_else(|| {
                LicetError::Config(format!(
                    "REUSE metadata {} disappeared during the scan",
                    rel.display()
                ))
            })?;
            let text = String::from_utf8(bytes).map_err(|e| {
                LicetError::Config(format!(
                    "REUSE metadata {} is not valid UTF-8: {e}",
                    rel.display()
                ))
            })?;
            oob.parse_reuse_toml(&rel, &text)?;
        }
        let dep5_rel = Path::new(".reuse/dep5");
        if let Some(bytes) = snapshot.read(dep5_rel)? {
            oob.dep5_present = true;
            let text = String::from_utf8(bytes).map_err(|e| {
                LicetError::Config(format!(
                    "REUSE metadata .reuse/dep5 is not valid UTF-8: {e}"
                ))
            })?;
            oob.parse_dep5(&text)?;
        }
        if oob.dep5_present && !oob.docs.is_empty() {
            return Err(LicetError::Config(format!(
                "found both '{}' and '.reuse/dep5': REUSE.toml and DEP5 are mutually \
                 exclusive, you cannot keep both simultaneously",
                display_rel(&oob.docs[0].rel)
            )));
        }
        Ok(oob)
    }

    /// Parse one `REUSE.toml` document: structural TOML errors, a `version`
    /// other than 1, a missing/empty `path`, an invalid `precedence`, and an
    /// invalid license expression all fail with document path and table index.
    /// Unknown keys and tables are preserved-ignored per the REUSE schema's
    /// extension allowance.
    fn parse_reuse_toml(&mut self, rel: &Path, text: &str) -> crate::error::Result<()> {
        use crate::error::LicetError;
        let loc = display_rel(rel);
        let parsed: ReuseToml = toml::from_str(text)
            .map_err(|e| LicetError::Config(format!("{loc}: malformed REUSE.toml: {e}")))?;
        if parsed.version != 1 {
            return Err(LicetError::Config(format!(
                "{loc}: unsupported REUSE.toml version {} (this tool implements version 1)",
                parsed.version
            )));
        }
        let base = rel.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut tables = Vec::with_capacity(parsed.annotations.len());
        for (index, ann) in parsed.annotations.into_iter().enumerate() {
            let tag = format!("{loc} table [[annotations]] #{index}");
            let paths = ann.path.into_vec();
            if paths.is_empty() {
                return Err(LicetError::Config(format!(
                    "{tag}: 'path' must not be empty"
                )));
            }
            let precedence = match ann.precedence.as_deref() {
                None => Precedence::Closest,
                Some("closest") => Precedence::Closest,
                Some("aggregate") => Precedence::Aggregate,
                Some("override") => Precedence::Override,
                Some(other) => {
                    return Err(LicetError::Config(format!(
                        "{tag}: invalid precedence {other:?}: must be one of \
                         'closest', 'aggregate', 'override'"
                    )));
                }
            };
            let mut licenses = Vec::new();
            for raw in ann.license.into_vec() {
                crate::spdx::validate_expression(&raw).map_err(|reason| {
                    LicetError::Config(format!("{tag}: invalid SPDX-License-Identifier: {reason}"))
                })?;
                licenses.push(raw.trim().to_string());
            }
            let copyrights = ann
                .copyright
                .into_vec()
                .into_iter()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
            let matchers = paths.iter().map(|p| compile_reuse_pattern(p)).collect();
            tables.push(OobTable {
                index,
                matchers,
                licenses,
                copyrights,
                precedence,
            });
        }
        self.docs.push(ReuseDoc {
            rel: rel.to_path_buf(),
            base,
            tables,
        });
        // Keep documents ordered shallowest-first so lookup consults them
        // root-first (mirrors the reference tool).
        self.docs.sort_by_key(|d| d.base.components().count());
        Ok(())
    }

    /// Debian dep5 (`.reuse/dep5`) parser: `Files:`/`Copyright:`/`License:`
    /// paragraphs (field names ASCII-case-insensitive) with continuation-line
    /// unfolding — a line starting with whitespace continues the current
    /// field, and a lone `.` is a blank. Unknown fields are preserved-ignored
    /// without being interpreted. An invalid `License:` expression fails like
    /// any other malformed metadata.
    fn parse_dep5(&mut self, text: &str) -> crate::error::Result<()> {
        use crate::error::LicetError;
        #[derive(Default)]
        struct Para {
            files: String,
            copyright: Vec<String>,
            license: String,
            field: Option<Dep5Field>,
        }
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Dep5Field {
            Files,
            Copyright,
            License,
            Other,
        }
        let mut paras: Vec<Para> = vec![Para::default()];
        for raw_line in text.lines() {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            if line.trim().is_empty() {
                paras.push(Para::default());
                continue;
            }
            let para = paras.last_mut().expect("paragraphs never empty");
            if line.starts_with(' ') || line.starts_with('\t') {
                // Continuation of the current field (a lone `.` is blank).
                let cont = line.trim();
                if cont.is_empty() || cont == "." {
                    continue;
                }
                match para.field {
                    Some(Dep5Field::Files) => {
                        para.files.push(' ');
                        para.files.push_str(cont);
                    }
                    Some(Dep5Field::Copyright) => para.copyright.push(cont.to_string()),
                    Some(Dep5Field::License) => {
                        para.license.push(' ');
                        para.license.push_str(cont);
                    }
                    Some(Dep5Field::Other) | None => {}
                }
                continue;
            }
            if cont_is_dot_only(line) {
                continue;
            }
            let (name, value) = match line.split_once(':') {
                Some((n, v)) => (n.trim(), v.trim()),
                None => continue,
            };
            if name.eq_ignore_ascii_case("Files") {
                para.field = Some(Dep5Field::Files);
                if !para.files.is_empty() {
                    para.files.push(' ');
                }
                para.files.push_str(value);
            } else if name.eq_ignore_ascii_case("Copyright") {
                para.field = Some(Dep5Field::Copyright);
                if !value.is_empty() {
                    para.copyright.push(value.to_string());
                }
            } else if name.eq_ignore_ascii_case("License") {
                para.field = Some(Dep5Field::License);
                if !para.license.is_empty() {
                    para.license.push(' ');
                }
                para.license.push_str(value);
            } else {
                para.field = Some(Dep5Field::Other);
            }
        }
        for (index, para) in paras.into_iter().enumerate() {
            if para.files.trim().is_empty() {
                continue;
            }
            let license = para.license.trim();
            let mut licenses = Vec::new();
            if !license.is_empty() {
                crate::spdx::validate_expression(license).map_err(|reason| {
                    LicetError::Config(format!(
                        ".reuse/dep5 paragraph #{index}: invalid License: {reason}"
                    ))
                })?;
                licenses.push(license.to_string());
            }
            let matchers = para
                .files
                .split_whitespace()
                .map(compile_dep5_pattern)
                .collect();
            self.dep5.push(Dep5Para {
                index,
                matchers,
                licenses,
                copyrights: para.copyright,
            });
        }
        Ok(())
    }

    /// Out-of-band coverage for a repo-relative path, if any.
    ///
    /// Each document contributes exclusively its last matching table; tables
    /// are consulted root-first and consultation stops after the rootmost
    /// `override` table (mirroring the reference tool). The returned entry
    /// carries unconditional (`aggregate`/barrier) values, per-field
    /// `closest` fallbacks, and every contributing table's provenance.
    pub fn lookup(&self, rel_path: &Path) -> Option<OutOfBandEntry> {
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        // Per-document last match, root-first, stopping after an override.
        let mut consulted: Vec<(&ReuseDoc, &OobTable)> = Vec::new();
        for doc in &self.docs {
            let Some(remainder) = under_base(&rel_str, &doc.base) else {
                continue;
            };
            if let Some(table) = doc
                .tables
                .iter()
                .rev()
                .find(|t| matches_any(&t.matchers, remainder))
            {
                let is_override = table.precedence == Precedence::Override;
                consulted.push((doc, table));
                if is_override {
                    break;
                }
            }
        }
        // dep5 paragraphs aggregate; the last matching paragraph wins (as in
        // the reference tool), contributing alongside any REUSE tables.
        let dep5_hit = self
            .dep5
            .iter()
            .rev()
            .find(|p| matches_any(&p.matchers, &rel_str));
        if consulted.is_empty() && dep5_hit.is_none() {
            return None;
        }
        let barrier = consulted
            .iter()
            .any(|(_, t)| t.precedence == Precedence::Override);
        let mut licenses: Vec<String> = Vec::new();
        let mut copyrights: Vec<String> = Vec::new();
        let mut origins: Vec<MetadataOrigin> = Vec::new();
        let mut primary_source = OobSource::ReuseToml;
        let mut has_aggregate = false;
        // Unconditional contributors: the barrier table first (if any), then
        // every consulted `aggregate` table shallowest-first, then dep5 —
        // matching the reference tool's override-before-aggregate order.
        let mut unconditional: Vec<(&ReuseDoc, &OobTable)> = Vec::new();
        if barrier {
            let (doc, table) = consulted
                .last()
                .expect("barrier implies a consulted override table");
            debug_assert_eq!(table.precedence, Precedence::Override);
            unconditional.push((*doc, *table));
        }
        for (doc, table) in &consulted {
            if table.precedence == Precedence::Aggregate {
                has_aggregate = true;
                unconditional.push((*doc, *table));
            }
        }
        for (doc, table) in unconditional {
            push_unique(&mut licenses, table.licenses.iter().cloned());
            push_unique(&mut copyrights, table.copyrights.iter().cloned());
            origins.push(MetadataOrigin {
                metadata_path: doc.rel.clone(),
                table_index: table.index,
                precedence: table.precedence,
                licenses: table.licenses.clone(),
                copyrights: table.copyrights.clone(),
            });
        }
        if let Some(para) = dep5_hit {
            has_aggregate = true;
            if origins.is_empty() {
                primary_source = OobSource::Dep5;
            }
            push_unique(&mut licenses, para.licenses.iter().cloned());
            push_unique(&mut copyrights, para.copyrights.iter().cloned());
            origins.push(MetadataOrigin {
                metadata_path: PathBuf::from(".reuse/dep5"),
                table_index: para.index,
                precedence: Precedence::Aggregate,
                licenses: para.licenses.clone(),
                copyrights: para.copyrights.clone(),
            });
        }
        // Per-field nearest-outward `closest` fallback (deepest consulted
        // table wins each field independently).
        let mut fallback_licenses: Vec<String> = Vec::new();
        let mut fallback_copyrights: Vec<String> = Vec::new();
        let mut fallback_lic_origin: Option<MetadataOrigin> = None;
        let mut fallback_cpr_origin: Option<MetadataOrigin> = None;
        for (doc, table) in consulted.iter().rev() {
            if table.precedence != Precedence::Closest {
                continue;
            }
            if fallback_licenses.is_empty() && !table.licenses.is_empty() {
                fallback_licenses = table.licenses.clone();
                fallback_lic_origin = Some(MetadataOrigin {
                    metadata_path: doc.rel.clone(),
                    table_index: table.index,
                    precedence: table.precedence,
                    licenses: table.licenses.clone(),
                    copyrights: table.copyrights.clone(),
                });
            }
            if fallback_copyrights.is_empty() && !table.copyrights.is_empty() {
                fallback_copyrights = table.copyrights.clone();
                fallback_cpr_origin = Some(MetadataOrigin {
                    metadata_path: doc.rel.clone(),
                    table_index: table.index,
                    precedence: table.precedence,
                    licenses: table.licenses.clone(),
                    copyrights: table.copyrights.clone(),
                });
            }
        }
        if fallback_lic_origin
            .as_ref()
            .is_some_and(|o| Some(o) != fallback_cpr_origin.as_ref())
        {
            origins.push(fallback_lic_origin.expect("checked"));
        }
        if let Some(o) = fallback_cpr_origin {
            origins.push(o);
        }
        if licenses.is_empty()
            && copyrights.is_empty()
            && fallback_licenses.is_empty()
            && fallback_copyrights.is_empty()
        {
            // Tables matched but none carries any licensing information — an
            // override barrier still suppresses the file (reference behavior),
            // otherwise there is no coverage at all.
            if !barrier {
                return None;
            }
        }
        // Origins read shallowest-document first (dep5 is root-level).
        origins.sort_by_key(|o| (o.metadata_path.clone(), o.table_index));
        let precedence = if barrier {
            Precedence::Override
        } else if has_aggregate {
            Precedence::Aggregate
        } else {
            Precedence::Closest
        };
        Some(OutOfBandEntry {
            source: primary_source,
            licenses,
            copyrights,
            fallback_licenses,
            fallback_copyrights,
            suppresses_file: barrier,
            precedence,
            origins,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty() && self.dep5.is_empty()
    }

    /// Whether `.reuse/dep5` exists in the loaded snapshot (used to refuse
    /// writes that would create a mutually-exclusive `REUSE.toml` next to it).
    pub fn has_dep5(&self) -> bool {
        self.dep5_present
    }
}

/// A lone `.` line outside a field continuation is blank padding, not content.
fn cont_is_dot_only(line: &str) -> bool {
    line.trim() == "."
}

/// Display a snapshot-relative path with forward slashes.
fn display_rel(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

/// The portion of repo-relative `path` under document base `base`, or `None`
/// when the file is not inside that directory. Containment holds by
/// construction: a pattern can never match outside its own document's tree.
fn under_base<'a>(path: &'a str, base: &Path) -> Option<&'a str> {
    let base_str = display_rel(base);
    if base_str.is_empty() {
        return Some(path);
    }
    path.strip_prefix(base_str.as_str())
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
}

fn matches_any(matchers: &[ReuseMatcher], path: &str) -> bool {
    matchers.iter().any(|m| m.is_match(path))
}

fn push_unique(target: &mut Vec<String>, iter: impl Iterator<Item = String>) {
    for item in iter {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

/// Compile one `REUSE.toml` path pattern with the REUSE 3.3 grammar: `*`
/// never crosses `/`, `**` (and `**/`) does, only `\`-asterisk and
/// `\`-backslash escapes are special, and `?`, brackets, and braces are
/// literal. Everything else is passed through to globset with `/` as the
/// separator, so a pattern can only ever match inside its document's tree.
fn compile_reuse_pattern(pattern: &str) -> ReuseMatcher {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                // `\` followed by any other character is that character
                // verbatim (spec §REUSE.toml); escape it for globset.
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                }
                None => out.push_str("[\\\\]"),
            },
            '*' => {
                let mut run = 1;
                while chars.peek() == Some(&'*') {
                    chars.next();
                    run += 1;
                }
                out.push_str(if run >= 2 { "**" } else { "*" });
            }
            '?' | '[' | ']' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    let full = if out.is_empty() {
        "**".to_string()
    } else {
        out
    };
    match GlobBuilder::new(&full).literal_separator(true).build() {
        Ok(g) => ReuseMatcher::Glob(g.compile_matcher()),
        Err(_) => ReuseMatcher::Literal(pattern.to_string()),
    }
}

/// Compile one `.reuse/dep5` `Files:` pattern: shell-style wildcards where
/// `*` crosses `/` (verified against the reference tool), unlike REUSE.toml.
fn compile_dep5_pattern(pattern: &str) -> ReuseMatcher {
    match GlobBuilder::new(pattern).literal_separator(false).build() {
        Ok(g) => ReuseMatcher::Glob(g.compile_matcher()),
        Err(_) => ReuseMatcher::Literal(pattern.to_string()),
    }
}

/// Repo-relative paths of every `REUSE.toml` document visible in `snapshot`:
/// tracked-or-present files at any depth that are not VCS-ignored. Index
/// snapshots observe tracked entries (tracked files are never VCS-ignored);
/// worktree snapshots walk the filesystem honoring ignore rules, never
/// following symlinks.
fn discover_doc_paths(snapshot: &Snapshot) -> crate::error::Result<Vec<PathBuf>> {
    match snapshot {
        Snapshot::Index(index) => {
            let mut paths: Vec<PathBuf> = index
                .entries
                .keys()
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n == "REUSE.toml")
                })
                .cloned()
                .collect();
            paths.sort();
            Ok(paths)
        }
        Snapshot::Worktree { root } => {
            use ignore::{WalkBuilder, overrides::OverrideBuilder};
            let mut ob = OverrideBuilder::new(root);
            ob.add("!.git/")
                .expect("valid built-in skip override for metadata discovery");
            let overrides = ob.build().expect("valid metadata-discovery overrides");
            let walker = WalkBuilder::new(root)
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .parents(true)
                .overrides(overrides)
                .follow_links(false)
                .build();
            let mut paths = Vec::new();
            for entry in walker {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        return Err(crate::error::LicetError::Io(std::io::Error::other(
                            format!("metadata discovery traversal failed: {e}"),
                        )));
                    }
                };
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                if entry.file_name().to_str().is_none_or(|n| n != "REUSE.toml") {
                    continue;
                }
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    paths.push(rel.to_path_buf());
                }
            }
            paths.sort();
            Ok(paths)
        }
    }
}

/// Outcome of [`write_annotation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationWrite {
    /// The file's effective `REUSE.toml` license already matched; nothing written.
    Unchanged,
    /// A new exact-path annotation was appended — covering a previously
    /// uncovered file, overriding a broader glob entry, or superseding an
    /// older exact-path entry. Being last, it wins per REUSE 3.3.
    Appended,
}

impl AnnotationWrite {
    /// Whether the file was actually rewritten.
    pub fn modified(self) -> bool {
        !matches!(self, AnnotationWrite::Unchanged)
    }
}

/// One exact-path annotation the caller wants covered by `REUSE.toml`.
pub struct AnnotationRequest<'a> {
    pub rel_path: &'a str,
    pub license: &'a str,
    pub copyrights: &'a [String],
}

/// Where one annotation patch belongs: the document that wins for the path,
/// the path relative to that document's base, and whether the stanza needs
/// `override` precedence to govern there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationDestination {
    /// Repo-relative document path (`REUSE.toml`, `sub/REUSE.toml`).
    pub doc_rel: PathBuf,
    /// Request path relative to the document's base.
    pub base_path: String,
    /// True when only an `override` stanza governs at the destination: under
    /// an `override` barrier, or when an `aggregate` table contributes
    /// unconditionally (a default-`closest` stanza would neither break the
    /// barrier nor silence the aggregate).
    pub use_override: bool,
}

/// Route an annotation to its winning document (FR-003a).
///
/// The walk mirrors [`OutOfBand::lookup`] over the already-loaded hierarchy:
/// - no coverage, or `closest` governing → the nearest consulted document
///   (the project root when nothing consults), default precedence — nearest
///   wins the per-field `closest` fallback;
/// - `override` or `aggregate` governing → the project root with `override`
///   precedence — root is consulted first, so the appended stanza breaks
///   before every deeper table and supersedes root tables by last match.
pub fn annotation_destination(oob: &OutOfBand, rel_path: &str) -> AnnotationDestination {
    let mut consulted: Vec<(usize, Precedence)> = Vec::new();
    for (di, doc) in oob.docs.iter().enumerate() {
        let Some(remainder) = under_base(rel_path, &doc.base) else {
            continue;
        };
        if let Some(table) = doc
            .tables
            .iter()
            .rev()
            .find(|t| matches_any(&t.matchers, remainder))
        {
            let is_override = table.precedence == Precedence::Override;
            consulted.push((di, table.precedence));
            if is_override {
                break;
            }
        }
    }
    let governing = if consulted.iter().any(|(_, p)| *p == Precedence::Override) {
        Some(Precedence::Override)
    } else if consulted.iter().any(|(_, p)| *p == Precedence::Aggregate) {
        Some(Precedence::Aggregate)
    } else if consulted.is_empty() {
        None
    } else {
        Some(Precedence::Closest)
    };
    match governing {
        Some(Precedence::Override) | Some(Precedence::Aggregate) => AnnotationDestination {
            doc_rel: PathBuf::from("REUSE.toml"),
            base_path: rel_path.to_string(),
            use_override: true,
        },
        Some(Precedence::Closest) => {
            let (di, _) = consulted.last().copied().expect("consulted non-empty");
            let doc = &oob.docs[di];
            AnnotationDestination {
                doc_rel: doc.rel.clone(),
                base_path: under_base(rel_path, &doc.base)
                    .unwrap_or(rel_path)
                    .to_string(),
                use_override: false,
            }
        }
        None => AnnotationDestination {
            doc_rel: PathBuf::from("REUSE.toml"),
            base_path: rel_path.to_string(),
            use_override: false,
        },
    }
}

/// Ensure a `REUSE.toml` annotation covers `rel_path` with `license` (FR-015, FR-003a).
///
/// Single-request form of [`write_annotations`]; `oob` is the scan's loaded
/// hierarchy, used for coverage and destination routing.
pub fn write_annotation(
    root: &Path,
    rel_path: &str,
    license: &str,
    copyrights: &[String],
    oob: &OutOfBand,
) -> Result<AnnotationWrite, crate::reuse::WriteError> {
    let outcomes = write_annotations(
        root,
        &[AnnotationRequest {
            rel_path,
            license,
            copyrights,
        }],
        oob,
    )?;
    Ok(outcomes
        .per_request
        .into_iter()
        .next()
        .unwrap_or(AnnotationWrite::Unchanged))
}

/// One patched metadata document: the before/after bytes plus which caller
/// requests its stanzas carry, so the caller files one write record per
/// document with the covered assets attached.
#[derive(Debug, Clone)]
pub struct DocPatch {
    pub doc_rel: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
    pub requests: Vec<usize>,
}

/// Outcome of [`write_annotations`]: per-request dispositions plus the
/// document patches actually committed (one per touched document).
#[derive(Debug, Clone)]
pub struct AnnotationBatchOutcome {
    pub per_request: Vec<AnnotationWrite>,
    pub patches: Vec<DocPatch>,
}

/// Ensure `REUSE.toml` annotations cover every request (FR-015, FR-003a).
///
/// `oob` is the scan's loaded hierarchy: coverage and destination routing read
/// it, so decisions match what detection sees. Each request whose effective
/// license *and* copyrights already match is left untouched
/// ([`AnnotationWrite::Unchanged`]); every other request gains one appended
/// exact-path stanza ([`AnnotationWrite::Appended`]) in its winning document
/// ([`annotation_destination`]). Creates the project root document with a
/// `version = 1` header if absent; never creates deeper documents.
///
/// The patch is strictly append-only: existing documents are never reparsed
/// with a lossy line scan and never reformatted, so comments, ordering, and
/// unrelated stanzas survive byte-for-byte, and a repeated run converges.
/// Requests group by destination document, so each document is read, patched,
/// and written exactly once. Every patched document is re-parsed — and every
/// appended coverage verified — before anything is committed. Copyrights
/// serialize as one string/list field, never repeated keys; exact paths escape
/// REUSE glob metacharacters so odd filenames cannot become new patterns;
/// appended stanzas reuse their document's own newline convention.
///
/// An unreadable or non-UTF-8 document is an error, never silently replaced
/// with fresh content (F04).
pub fn write_annotations(
    root: &Path,
    requests: &[AnnotationRequest<'_>],
    oob: &OutOfBand,
) -> Result<AnnotationBatchOutcome, crate::reuse::WriteError> {
    use crate::reuse::{WriteError, atomic_write, read_expected_for_write};
    use std::collections::BTreeMap;

    let mut per_request = vec![AnnotationWrite::Unchanged; requests.len()];
    // Requests needing a stanza, grouped by destination document so each
    // document is read, patched, and written exactly once.
    let mut pending: BTreeMap<PathBuf, Vec<(usize, AnnotationDestination)>> = BTreeMap::new();
    for (i, req) in requests.iter().enumerate() {
        let entry = oob.lookup(Path::new(req.rel_path));
        let covered = entry.as_ref().and_then(|e| {
            e.license()
                .or_else(|| crate::domain::combine_licenses(&e.fallback_licenses))
        });
        let copyrights_match = entry.as_ref().is_some_and(|e| {
            norm_set(effective_copyrights(e)) == norm_set(req.copyrights.to_vec())
        });
        if covered.as_deref() == Some(req.license) && copyrights_match {
            continue;
        }
        let dest = annotation_destination(oob, req.rel_path);
        pending
            .entry(dest.doc_rel.clone())
            .or_default()
            .push((i, dest));
    }
    if pending.is_empty() {
        return Ok(AnnotationBatchOutcome {
            per_request,
            patches: Vec::new(),
        });
    }

    let mut patches: Vec<DocPatch> = Vec::new();

    for (doc_rel, jobs) in &pending {
        let existing_bytes =
            read_expected_for_write(&root.join(doc_rel)).map_err(WriteError::for_read_failure)?;
        let existing = match &existing_bytes {
            Some(bytes) => std::str::from_utf8(bytes).map_err(|e| {
                WriteError::for_read_failure(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("existing {} is not valid UTF-8: {e}", doc_rel.display()),
                ))
            })?,
            // Only the project root document may be created; a missing deeper
            // document means the hierarchy moved under us — refuse, don't invent.
            None if doc_rel == Path::new("REUSE.toml") => "",
            None => {
                return Err(WriteError::for_read_failure(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "annotated document {} disappeared during the scan",
                        doc_rel.display()
                    ),
                )));
            }
        };

        let nl = if existing.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let mut out = existing.to_owned();
        if out.is_empty() {
            out.push_str("version = 1");
            out.push_str(nl);
            out.push_str(nl);
        } else if !out.ends_with('\n') {
            out.push_str(nl);
        }
        for (i, dest) in jobs {
            let req = &requests[*i];
            out.push_str("[[annotations]]");
            out.push_str(nl);
            out.push_str(&format!(
                "path = {}",
                toml_string(&escape_reuse_path(&dest.base_path))
            ));
            out.push_str(nl);
            if dest.use_override {
                out.push_str("precedence = \"override\"");
                out.push_str(nl);
            }
            match req.copyrights.len() {
                0 => {}
                1 => {
                    out.push_str(&format!(
                        "SPDX-FileCopyrightText = {}",
                        toml_string(&req.copyrights[0])
                    ));
                    out.push_str(nl);
                }
                _ => {
                    let list = req
                        .copyrights
                        .iter()
                        .map(|s| toml_string(s))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!("SPDX-FileCopyrightText = [{list}]"));
                    out.push_str(nl);
                }
            }
            out.push_str(&format!(
                "SPDX-License-Identifier = {}",
                toml_string(req.license)
            ));
            out.push_str(nl);
            out.push_str(nl);
            per_request[*i] = AnnotationWrite::Appended;
        }

        // Re-parse the complete proposed document before committing, and
        // verify every appended coverage through the same lookup detection
        // uses (this also validates the escaping and list serialization).
        let mut verify = OutOfBand::default();
        verify.parse_reuse_toml(doc_rel, &out).map_err(|e| {
            WriteError::for_read_failure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("proposed {} failed to re-parse: {e}", doc_rel.display()),
            ))
        })?;
        for (i, _) in jobs {
            let req = &requests[*i];
            // Verify through the document's own base, exactly as detection
            // will read it back.
            let got = verify.lookup(Path::new(req.rel_path)).and_then(|e| {
                e.license()
                    .or_else(|| crate::domain::combine_licenses(&e.fallback_licenses))
            });
            if got.as_deref() != Some(req.license) {
                return Err(WriteError::for_read_failure(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "proposed {} does not cover {} with {}",
                        doc_rel.display(),
                        req.rel_path,
                        req.license
                    ),
                )));
            }
        }

        atomic_write(root, doc_rel, existing_bytes.as_deref(), out.as_bytes())?;
        patches.push(DocPatch {
            doc_rel: doc_rel.clone(),
            before: existing_bytes.clone(),
            after: out.into_bytes(),
            requests: jobs.iter().map(|(i, _)| *i).collect(),
        });
    }
    Ok(AnnotationBatchOutcome {
        per_request,
        patches,
    })
}

/// Effective OOB copyrights for a lookup entry, mirroring the license logic:
/// unconditional contributors first, else the `closest` fallback.
fn effective_copyrights(entry: &crate::domain::OutOfBandEntry) -> Vec<String> {
    if entry.copyrights.is_empty() {
        entry.fallback_copyrights.clone()
    } else {
        entry.copyrights.clone()
    }
}

/// Order-insensitive comparison form for notice lists.
fn norm_set(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

/// Escape the REUSE glob metacharacters in an exact path so the emitted stanza
/// cannot act as a pattern: a file named `a*b.png` must not start covering
/// its siblings. Only `\` and `*` are escaped: both engines treat `\`-asterisk
/// and `\\`-backslash as escapes, while `?`, brackets, and braces are literal
/// in both grammars and must stay bare (this crate's matcher and the reference
/// tool's pathspec grammar agree on all of this).
fn escape_reuse_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '*') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Quote a value for emission into a `REUSE.toml` stanza (write-side escaping only).
fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// REUSE-IgnoreStart — SPDX tags in the tests below are fixtures, not this file's licensing.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn load_doc(text: &str) -> OutOfBand {
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(Path::new("REUSE.toml"), text).unwrap();
        oob
    }

    #[test]
    fn parses_reuse_toml_glob() {
        // A `closest` table is fallback only: its values land in the fallback
        // fields, leaving the unconditional ones empty for detection to fill
        // from file-level info first.
        let oob = load_doc(
            "version = 1\n[[annotations]]\npath = \"assets/**\"\n\
             SPDX-License-Identifier = \"CC0-1.0\"\nSPDX-FileCopyrightText = \"2026 Acme\"\n",
        );
        let e = oob.lookup(&PathBuf::from("assets/logo.png")).unwrap();
        assert!(e.licenses.is_empty());
        assert_eq!(e.fallback_licenses, vec!["CC0-1.0".to_string()]);
        assert_eq!(e.fallback_copyrights, vec!["2026 Acme".to_string()]);
        assert_eq!(e.precedence, Precedence::Closest);
        assert!(!e.suppresses_file);
    }

    #[test]
    fn license_array_stays_together() {
        // Multiple expressions in one table apply together (aggregate here so
        // they are unconditional); the presentation form AND-combines them.
        let oob = load_doc(
            "version = 1\n[[annotations]]\npath = \"a.bin\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = [\"MIT\", \"Apache-2.0\"]\n",
        );
        let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
        assert_eq!(
            e.licenses,
            vec!["MIT".to_string(), "Apache-2.0".to_string()]
        );
        assert_eq!(e.license().as_deref(), Some("MIT AND Apache-2.0"));
    }

    #[test]
    fn parses_dep5() {
        let mut oob = OutOfBand::default();
        oob.parse_dep5("Files: img/*\nCopyright: 2026 Acme\nLicense: MIT\n")
            .unwrap();
        let e = oob.lookup(&PathBuf::from("img/x.jpg")).unwrap();
        assert_eq!(e.licenses, vec!["MIT".to_string()]);
        assert_eq!(e.source, OobSource::Dep5);
        assert_eq!(e.precedence, Precedence::Aggregate);
    }

    #[test]
    fn dep5_star_crosses_directories() {
        // Unlike REUSE.toml, dep5 `*` matches across `/` (reference behavior).
        let mut oob = OutOfBand::default();
        oob.parse_dep5("Files: *.bin\nLicense: MIT\n").unwrap();
        assert!(oob.lookup(&PathBuf::from("sub/b.bin")).is_some());
    }

    #[test]
    fn dep5_continuation_lines_unfold() {
        let mut oob = OutOfBand::default();
        oob.parse_dep5(
            "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\
             \n\
             Files: a.bin\n sub/b.bin\n\
             Copyright: 2026 A\n 2026 B\n .\n\
             License: MIT\n",
        )
        .unwrap();
        let e = oob.lookup(&PathBuf::from("sub/b.bin")).unwrap();
        assert_eq!(e.licenses, vec!["MIT".to_string()]);
        assert_eq!(
            e.copyrights,
            vec!["2026 A".to_string(), "2026 B".to_string()]
        );
    }

    #[test]
    fn dep5_last_paragraph_wins() {
        // Two paragraphs covering the same file: the last one governs, exactly
        // as the reference tool reports.
        let mut oob = OutOfBand::default();
        oob.parse_dep5(
            "Files: a.bin\nCopyright: 2026 A\nLicense: MIT\n\nFiles: a.bin\nCopyright: 2026 B\nLicense: Apache-2.0\n",
        )
        .unwrap();
        let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
        assert_eq!(e.licenses, vec!["Apache-2.0".to_string()]);
        assert_eq!(e.copyrights, vec!["2026 B".to_string()]);
        assert_eq!(e.origins.len(), 1);
        assert_eq!(e.origins[0].table_index, 1);
    }

    #[test]
    fn dep5_field_names_are_case_insensitive() {
        let mut oob = OutOfBand::default();
        oob.parse_dep5("files: a.bin\ncopyright: 2026 A\nlicense: MIT\n")
            .unwrap();
        let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
        assert_eq!(e.licenses, vec!["MIT".to_string()]);
    }

    #[test]
    fn no_match_is_none() {
        let oob = OutOfBand::default();
        assert!(oob.lookup(&PathBuf::from("x")).is_none());
    }

    #[test]
    fn malformed_documents_fail_with_location() {
        for (name, text) in [
            (
                "bad-toml",
                "version = 1\n[[annotations]]\npath = \nSPDX-License-Identifier = \"MIT\"\n",
            ),
            ("version-2", "version = 2\n[[annotations]]\npath = \"a\"\n"),
            (
                "missing-path",
                "version = 1\n[[annotations]]\nSPDX-License-Identifier = \"MIT\"\n",
            ),
            (
                "empty-path",
                "version = 1\n[[annotations]]\npath = []\nSPDX-License-Identifier = \"MIT\"\n",
            ),
            (
                "bad-precedence",
                "version = 1\n[[annotations]]\npath = \"a\"\nprecedence = \"bogus\"\n",
            ),
            (
                "capital-precedence",
                "version = 1\n[[annotations]]\npath = \"a\"\nprecedence = \"Closest\"\n",
            ),
            (
                "bad-expression",
                "version = 1\n[[annotations]]\npath = \"a\"\nSPDX-License-Identifier = \"NOT-A-LICENSE\"\n",
            ),
        ] {
            let mut oob = OutOfBand::default();
            let err = oob
                .parse_reuse_toml(Path::new("REUSE.toml"), text)
                .expect_err(&format!("{name} must fail"));
            let msg = err.to_string();
            assert!(
                msg.contains("REUSE.toml"),
                "{name}: error names the document: {msg}"
            );
        }
        let mut oob = OutOfBand::default();
        let err = oob
            .parse_dep5("Files: a.bin\nLicense: NOT-A-LICENSE\n")
            .expect_err("bad dep5 License must fail");
        assert!(err.to_string().contains(".reuse/dep5"), "{err}");
    }

    #[test]
    fn missing_version_fails() {
        let mut oob = OutOfBand::default();
        let err = oob
            .parse_reuse_toml(Path::new("REUSE.toml"), "[[annotations]]\npath = \"a\"\n")
            .expect_err("missing version must fail");
        assert!(err.to_string().contains("REUSE.toml"), "{err}");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        // The REUSE schema explicitly permits extension keys.
        let oob = load_doc(
            "version = 1\n[tool.extra]\nnote = 1\n[[annotations]]\npath = \"a\"\n\
             SPDX-License-Identifier = \"MIT\"\nSPDX-FileComment = \"hi\"\n",
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("a")).unwrap().fallback_licenses,
            vec!["MIT".to_string()]
        );
    }

    #[test]
    fn nested_documents_resolve_root_first() {
        let mut oob = load_doc(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nSPDX-License-Identifier = \"MIT\"\n\
             SPDX-FileCopyrightText = \"2026 Root\"\n",
        );
        oob.parse_reuse_toml(
            Path::new("sub/REUSE.toml"),
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        )
        .unwrap();
        // Per-field nearest-outward fallback: the child's license wins, but
        // the child says nothing about copyright so the root still supplies it.
        let e = oob.lookup(&PathBuf::from("sub/f.rs")).unwrap();
        assert_eq!(e.fallback_licenses, vec!["Apache-2.0".to_string()]);
        assert_eq!(e.fallback_copyrights, vec!["2026 Root".to_string()]);
        assert_eq!(e.origins.len(), 2);
        assert_eq!(
            e.origins[0].metadata_path,
            PathBuf::from("REUSE.toml"),
            "origins read shallowest-document first"
        );
        assert_eq!(e.origins[1].metadata_path, PathBuf::from("sub/REUSE.toml"));
    }

    #[test]
    fn nested_patterns_cannot_escape_their_directory() {
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(
            Path::new("sub/REUSE.toml"),
            "version = 1\n[[annotations]]\npath = \"**\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
        )
        .unwrap();
        assert!(oob.lookup(&PathBuf::from("sub/a.rs")).is_some());
        assert!(
            oob.lookup(&PathBuf::from("other.rs")).is_none(),
            "a nested document never covers its parent"
        );
        assert!(
            oob.lookup(&PathBuf::from("REUSE.toml")).is_none(),
            "metadata documents are not covered by nested globs"
        );
    }

    #[test]
    fn override_barrier_suppresses_deeper_tables() {
        let mut oob = load_doc(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nprecedence = \"override\"\n\
             SPDX-License-Identifier = \"MIT\"\nSPDX-FileCopyrightText = \"2026 Root\"\n",
        );
        oob.parse_reuse_toml(
            Path::new("sub/REUSE.toml"),
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = \"Apache-2.0\"\n",
        )
        .unwrap();
        let e = oob.lookup(&PathBuf::from("sub/f.rs")).unwrap();
        assert!(e.suppresses_file);
        assert_eq!(e.precedence, Precedence::Override);
        assert_eq!(e.licenses, vec!["MIT".to_string()]);
        assert!(
            !e.licenses.contains(&"Apache-2.0".to_string()),
            "the deeper aggregate is suppressed by the rootmost override"
        );
    }

    #[test]
    fn empty_override_still_suppresses() {
        // A field-less override table contributes nothing but still
        // suppresses file-level info (reference behavior).
        let oob =
            load_doc("version = 1\n[[annotations]]\npath = \"a.bin\"\nprecedence = \"override\"\n");
        let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
        assert!(e.suppresses_file);
        assert!(e.licenses.is_empty() && e.copyrights.is_empty());
    }

    #[test]
    fn reuse_pattern_grammar() {
        // `*` never crosses `/`; `**` does; `?[]{} ` are literal; `\` escapes.
        let oob = load_doc(
            "version = 1\n\
             [[annotations]]\npath = \"*.png\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"MIT\"\n\
             [[annotations]]\npath = \"a?.png\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"Apache-2.0\"\n\
             [[annotations]]\npath = \"deep/**\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"CC0-1.0\"\n",
        );
        // Last match wins within the document.
        assert_eq!(
            oob.lookup(&PathBuf::from("a?.png")).unwrap().licenses,
            vec!["Apache-2.0".to_string()],
            "`?` is literal: the exact file matches the literal pattern"
        );
        // `*.png` must not have matched `a?.png` via wildcard, or last-match
        // would still hold — check a file only `*` can match instead.
        assert!(
            oob.lookup(&PathBuf::from("sub/a.png")).is_none(),
            "`*` must not cross `/`"
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("deep/nest/a.png"))
                .unwrap()
                .licenses,
            vec!["CC0-1.0".to_string()],
            "`**` crosses directories"
        );
    }

    #[test]
    fn reuse_escapes_match_verbatim() {
        let mut oob = OutOfBand::default();
        // TOML `"star\\\\*.bin"` is the pattern `star\*.bin`: a literal star.
        oob.parse_reuse_toml(
            Path::new("REUSE.toml"),
            "version = 1\n[[annotations]]\npath = \"star\\\\*.bin\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
        )
        .unwrap();
        assert!(oob.lookup(&PathBuf::from("star*.bin")).is_some());
        assert!(oob.lookup(&PathBuf::from("starX.bin")).is_none());
    }

    #[test]
    fn braces_and_brackets_are_literal() {
        let oob = load_doc(
            "version = 1\n[[annotations]]\npath = \"a[0].{png,bin}\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
        );
        assert!(oob.lookup(&PathBuf::from("a[0].{png,bin}")).is_some());
        assert!(oob.lookup(&PathBuf::from("a0.png")).is_none());
    }

    #[test]
    fn coexistence_with_dep5_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("REUSE.toml"),
            "version = 1\n[[annotations]]\npath = \"a\"\nSPDX-License-Identifier = \"MIT\"\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join(".reuse")).unwrap();
        std::fs::write(
            dir.path().join(".reuse/dep5"),
            "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n",
        )
        .unwrap();
        let err = OutOfBand::load(dir.path()).expect_err("coexistence must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("REUSE.toml") && msg.contains(".reuse/dep5"),
            "{msg}"
        );
    }

    /// The scan's hierarchy as the write probe sees it.
    fn load_root(root: &std::path::Path) -> OutOfBand {
        OutOfBand::load(root).unwrap()
    }

    #[test]
    fn write_annotation_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cprs = vec!["2026 Acme".to_string()];
        assert_eq!(
            write_annotation(root, "logo.png", "CC0-1.0", &cprs, &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        let first = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        // Second write of the same path with the same license is a no-op.
        assert_eq!(
            write_annotation(root, "logo.png", "CC0-1.0", &cprs, &load_root(root)).unwrap(),
            AnnotationWrite::Unchanged
        );
        let second = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.matches("path = \"logo.png\"").count(), 1);
        assert!(first.starts_with("version = 1"));
    }

    #[test]
    fn write_annotation_appends_superseding_stanza_for_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cprs = vec!["2026 Acme".to_string()];
        write_annotation(root, "logo.png", "CC0-1.0", &cprs, &load_root(root)).unwrap();
        // A new intent for the same exact path appends a superseding stanza;
        // the old one is left byte-for-byte (last match wins per REUSE 3.3).
        assert_eq!(
            write_annotation(root, "logo.png", "CC-BY-4.0", &cprs, &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert_eq!(text.matches("path = \"logo.png\"").count(), 2);
        assert!(text.contains("SPDX-License-Identifier = \"CC0-1.0\""));
        assert!(text.contains("SPDX-License-Identifier = \"CC-BY-4.0\""));

        // The appended stanza wins through lookup (a `closest` table surfaces
        // as the fallback until file-level info exists) …
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(Path::new("REUSE.toml"), &text)
            .unwrap();
        assert_eq!(
            oob.lookup(&PathBuf::from("logo.png"))
                .unwrap()
                .fallback_licenses,
            vec!["CC-BY-4.0".to_string()]
        );
        // … and the rerun converges to a no-op.
        assert_eq!(
            write_annotation(root, "logo.png", "CC-BY-4.0", &cprs, &load_root(root)).unwrap(),
            AnnotationWrite::Unchanged
        );
    }

    #[test]
    fn write_annotation_refuses_malformed_document() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("REUSE.toml"), "[[annotations\npath = \n").unwrap();
        let before = std::fs::read(root.join("REUSE.toml")).unwrap();
        // The probe hierarchy is empty (the scan would already have failed on
        // this document); the re-parse gate still refuses to append to it.
        let err = write_annotation(root, "logo.png", "CC0-1.0", &[], &OutOfBand::default())
            .expect_err("malformed REUSE.toml must fail closed");
        assert!(err.to_string().contains("re-parse"), "{err}");
        // The broken document is left untouched — nothing appended.
        assert_eq!(std::fs::read(root.join("REUSE.toml")).unwrap(), before);
    }

    #[test]
    fn write_annotation_appends_override_for_glob() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("REUSE.toml"),
            "version = 1\n\n[[annotations]]\npath = \"*.png\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
        )
        .unwrap();
        // A glob covers the file; we cannot edit it without affecting siblings, so we
        // append a more-specific exact-path block. Last match wins.
        assert_eq!(
            write_annotation(root, "logo.png", "CC-BY-4.0", &[], &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(Path::new("REUSE.toml"), &text)
            .unwrap();
        // Both tables are `closest` fallbacks without file-level info, so the
        // last match governs each file.
        assert_eq!(
            oob.lookup(&PathBuf::from("logo.png"))
                .unwrap()
                .fallback_licenses,
            vec!["CC-BY-4.0".to_string()],
            "exact override must win over the glob"
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("other.png"))
                .unwrap()
                .fallback_licenses,
            vec!["MIT".to_string()],
            "the glob still governs its other files"
        );
    }

    #[test]
    fn write_annotations_batch_many_files_single_doc_write() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let empty: Vec<String> = vec![];
        let acme = vec!["2026 Acme".to_string()];
        let outcome = write_annotations(
            root,
            &[
                AnnotationRequest {
                    rel_path: "a.png",
                    license: "CC0-1.0",
                    copyrights: &empty,
                },
                AnnotationRequest {
                    rel_path: "b.png",
                    license: "MIT",
                    copyrights: &acme,
                },
            ],
            &load_root(root),
        )
        .unwrap();
        assert_eq!(
            outcome.per_request,
            vec![AnnotationWrite::Appended, AnnotationWrite::Appended]
        );
        // One document touched, one patch carrying both requests.
        assert_eq!(outcome.patches.len(), 1);
        assert_eq!(outcome.patches[0].doc_rel, PathBuf::from("REUSE.toml"));
        assert_eq!(outcome.patches[0].requests, vec![0, 1]);
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert_eq!(text.matches("[[annotations]]").count(), 2);
        // Both resolve, and a rerun over the same batch converges entirely.
        let oob = load_root(root);
        assert_eq!(
            oob.lookup(&PathBuf::from("a.png"))
                .unwrap()
                .fallback_licenses,
            vec!["CC0-1.0".to_string()]
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("b.png"))
                .unwrap()
                .fallback_licenses,
            vec!["MIT".to_string()]
        );
        let rerun = write_annotations(
            root,
            &[
                AnnotationRequest {
                    rel_path: "a.png",
                    license: "CC0-1.0",
                    copyrights: &empty,
                },
                AnnotationRequest {
                    rel_path: "b.png",
                    license: "MIT",
                    copyrights: &acme,
                },
            ],
            &oob,
        )
        .unwrap();
        assert_eq!(
            rerun.per_request,
            vec![AnnotationWrite::Unchanged, AnnotationWrite::Unchanged]
        );
        assert!(rerun.patches.is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("REUSE.toml")).unwrap(),
            text
        );
    }

    #[test]
    fn multiple_copyrights_serialize_as_list() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cprs = vec!["2026 Acme".to_string(), "2027 Acme".to_string()];
        assert_eq!(
            write_annotation(root, "logo.png", "CC0-1.0", &cprs, &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        // One string/list field, never repeated keys (repeated keys are
        // invalid TOML and would fail the re-parse gate).
        assert_eq!(text.matches("SPDX-FileCopyrightText").count(), 1, "{text}");
        assert!(
            text.contains("SPDX-FileCopyrightText = [\"2026 Acme\", \"2027 Acme\"]"),
            "{text}"
        );
        let oob = load_root(root);
        assert_eq!(
            oob.lookup(&PathBuf::from("logo.png"))
                .unwrap()
                .fallback_copyrights,
            cprs
        );
    }

    #[test]
    fn glob_metachar_path_is_escaped_to_literal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(
            write_annotation(root, "a*b.png", "CC0-1.0", &[], &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        // Two layers of escaping: TOML decodes `\\` to `\`, leaving the
        // REUSE-level `\`-asterisk escape the matcher (and the reference
        // tool) reads as a literal star.
        assert!(text.contains("path = \"a\\\\*b.png\""), "{text}");
        let oob = load_root(root);
        assert!(oob.lookup(&PathBuf::from("a*b.png")).is_some());
        assert!(
            oob.lookup(&PathBuf::from("aXb.png")).is_none(),
            "escaped path must not act as a glob"
        );
    }

    #[test]
    fn crlf_document_keeps_crlf_on_append() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("REUSE.toml"),
            "version = 1\r\n\r\n[[annotations]]\r\npath = \"a.png\"\r\n\
             SPDX-License-Identifier = \"MIT\"\r\n",
        )
        .unwrap();
        assert_eq!(
            write_annotation(root, "b.png", "CC0-1.0", &[], &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert!(!text.contains('\n') || text.contains("\r\n"), "{text:?}");
        assert!(
            text.contains("[[annotations]]\r\npath = \"b.png\"\r\n"),
            "{text:?}"
        );
        // Still parses: the appended stanza reuses the document convention.
        load_root(root)
            .lookup(&PathBuf::from("b.png"))
            .expect("b.png covered");
    }

    #[test]
    fn two_holder_broad_annotation_gets_exact_exception() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let before = "version = 1\n\n[[annotations]]\npath = [\"a.png\", \"b.png\"]\n\
             SPDX-FileCopyrightText = [\"2026 Acme\", \"2027 Acme\"]\n\
             SPDX-License-Identifier = \"MIT\"\n";
        std::fs::write(root.join("REUSE.toml"), before).unwrap();
        // Narrowing a.png must not rewrite the shared stanza: append an exact
        // exception carrying both holders.
        assert_eq!(
            write_annotation(
                root,
                "a.png",
                "CC-BY-4.0",
                &["2026 Acme".to_string(), "2027 Acme".to_string()],
                &load_root(root),
            )
            .unwrap(),
            AnnotationWrite::Appended
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert!(
            text.starts_with(before),
            "shared stanza byte-identical: {text}"
        );
        let oob = load_root(root);
        assert_eq!(
            oob.lookup(&PathBuf::from("a.png"))
                .unwrap()
                .fallback_licenses,
            vec!["CC-BY-4.0".to_string()]
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("a.png"))
                .unwrap()
                .fallback_copyrights,
            vec!["2026 Acme".to_string(), "2027 Acme".to_string()]
        );
        // The sibling still rides the broad annotation untouched.
        assert_eq!(
            oob.lookup(&PathBuf::from("b.png"))
                .unwrap()
                .fallback_licenses,
            vec!["MIT".to_string()]
        );
    }

    #[test]
    fn matching_license_missing_copyright_appends() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("REUSE.toml"),
            "version = 1\n\n[[annotations]]\npath = \"a.png\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
        )
        .unwrap();
        // Same license but a new requested notice is a real change, not Unchanged.
        assert_eq!(
            write_annotation(
                root,
                "a.png",
                "MIT",
                &["2026 Acme".to_string()],
                &load_root(root),
            )
            .unwrap(),
            AnnotationWrite::Appended
        );
        let oob = load_root(root);
        assert_eq!(
            oob.lookup(&PathBuf::from("a.png"))
                .unwrap()
                .fallback_copyrights,
            vec!["2026 Acme".to_string()]
        );
    }

    #[test]
    fn unknown_keys_comments_and_literal_path_survive_append() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let before = "# Hand-authored header comment.\nversion = 1\ncustom = 42\n\n\
             [[annotations]]\npath = 'a.png'\n# inline comment\nunknown-key = true\n\
             SPDX-License-Identifier = \"MIT\"\n";
        std::fs::write(root.join("REUSE.toml"), before).unwrap();
        assert_eq!(
            write_annotation(root, "b.png", "CC0-1.0", &[], &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert!(
            text.starts_with(before),
            "hand-authored content byte-identical: {text}"
        );
    }

    #[test]
    fn subdir_closest_routes_to_subdir_doc_with_base_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(
            root.join("sub/REUSE.toml"),
            "version = 1\n\n[[annotations]]\npath = \"*.png\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
        )
        .unwrap();
        // The subdir document wins the `closest` fallback for its tree, so the
        // exact exception lands there with a base-relative path — a root
        // stanza would lose to the nearer document.
        let dest = annotation_destination(&load_root(root), "sub/logo.png");
        assert_eq!(dest.doc_rel, PathBuf::from("sub/REUSE.toml"));
        assert_eq!(dest.base_path, "logo.png");
        assert!(!dest.use_override);
        assert_eq!(
            write_annotation(root, "sub/logo.png", "CC-BY-4.0", &[], &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        assert!(
            !root.join("REUSE.toml").exists(),
            "nothing appended at the root"
        );
        let text = std::fs::read_to_string(root.join("sub/REUSE.toml")).unwrap();
        assert!(text.contains("path = \"logo.png\""), "{text}");
        let oob = load_root(root);
        assert_eq!(
            oob.lookup(&PathBuf::from("sub/logo.png"))
                .unwrap()
                .fallback_licenses,
            vec!["CC-BY-4.0".to_string()]
        );
    }

    #[test]
    fn subdir_override_barrier_gets_root_override_stanza() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(
            root.join("sub/REUSE.toml"),
            "version = 1\n\n[[annotations]]\npath = \"logo.png\"\nprecedence = \"override\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
        )
        .unwrap();
        // A nearer `override` barrier can only be superseded from the root:
        // root is consulted first, so a root override stanza breaks before the
        // subdir document is ever read.
        let dest = annotation_destination(&load_root(root), "sub/logo.png");
        assert_eq!(dest.doc_rel, PathBuf::from("REUSE.toml"));
        assert!(dest.use_override);
        assert_eq!(
            write_annotation(root, "sub/logo.png", "CC-BY-4.0", &[], &load_root(root)).unwrap(),
            AnnotationWrite::Appended
        );
        let oob = load_root(root);
        let entry = oob.lookup(&PathBuf::from("sub/logo.png")).unwrap();
        assert_eq!(entry.license().as_deref(), Some("CC-BY-4.0"));
        assert!(entry.suppresses_file);
    }

    #[test]
    fn lookup_is_last_match() {
        let oob = load_doc(
            "version = 1\n[[annotations]]\npath = \"*.png\"\nSPDX-License-Identifier = \"MIT\"\n\
             [[annotations]]\npath = \"logo.png\"\nSPDX-License-Identifier = \"CC-BY-4.0\"\n",
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("logo.png"))
                .unwrap()
                .fallback_licenses,
            vec!["CC-BY-4.0".to_string()]
        );
    }
}
// REUSE-IgnoreEnd
