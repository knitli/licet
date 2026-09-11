//! Parsing for out-of-band REUSE metadata: `REUSE.toml` documents at every
//! depth plus legacy `.reuse/dep5` paragraphs, loaded through the same
//! content snapshot the scan evaluates (FR-003a, research §7).

use super::*;

use crate::walk::Snapshot;
use globset::GlobBuilder;

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
    pub(crate) fn parse_reuse_toml(&mut self, rel: &Path, text: &str) -> crate::error::Result<()> {
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
    pub(crate) fn parse_dep5(&mut self, text: &str) -> crate::error::Result<()> {
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
        for line in text.lines() {
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
}

/// A lone `.` line outside a field continuation is blank padding, not content.
fn cont_is_dot_only(line: &str) -> bool {
    line.trim() == "."
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
    // Force backslash escapes on every platform: globset disables them by
    // default where `\` is a path separator (Windows), which would turn our
    // emitted `\?`, `\[`, `\X` literals into separators. licet generates
    // this pattern text itself and matches `/`-normalized candidates, so
    // matching must be identical on all platforms.
    match GlobBuilder::new(&full)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
    {
        Ok(g) => ReuseMatcher::Glob(g.compile_matcher()),
        Err(_) => ReuseMatcher::Literal(pattern.to_string()),
    }
}

/// Compile one `.reuse/dep5` `Files:` pattern: shell-style wildcards where
/// `*` crosses `/` (verified against the reference tool), unlike REUSE.toml.
fn compile_dep5_pattern(pattern: &str) -> ReuseMatcher {
    // Same platform-independence requirement as REUSE.toml patterns above:
    // dep5 `Files:` entries must match identically on every OS.
    match GlobBuilder::new(pattern)
        .literal_separator(false)
        .backslash_escape(true)
        .build()
    {
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
