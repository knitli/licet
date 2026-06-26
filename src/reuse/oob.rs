//! Out-of-band REUSE metadata: read `REUSE.toml` (current spec) and `.reuse/dep5`
//! (legacy) for detection, and write `REUSE.toml` annotations for non-annotatable
//! files (FR-015, FR-003a, research §7).

use std::path::Path;

use globset::{Glob, GlobMatcher};
use serde::Deserialize;

use crate::domain::{OobSource, OutOfBandEntry, Precedence};

/// All out-of-band annotations discovered under a repository root.
#[derive(Default)]
pub struct OutOfBand {
    entries: Vec<OobAnnotation>,
}

struct OobAnnotation {
    matchers: Vec<GlobMatcher>,
    license: Option<String>,
    copyrights: Vec<String>,
    source: OobSource,
    precedence: Precedence,
}

#[derive(Debug, Deserialize)]
struct ReuseToml {
    #[serde(default)]
    annotations: Vec<ReuseAnnotation>,
}

#[derive(Debug, Deserialize)]
struct ReuseAnnotation {
    #[serde(default)]
    path: PathList,
    #[serde(rename = "SPDX-License-Identifier")]
    license: Option<String>,
    #[serde(rename = "SPDX-FileCopyrightText", default)]
    copyright: PathList,
    /// REUSE 3.3 `precedence`: `closest` (default) | `aggregate` | `override`.
    precedence: Option<String>,
}

/// Map a REUSE.toml `precedence` string to [`Precedence`] (default `Closest`).
fn parse_precedence(raw: Option<&str>) -> Precedence {
    match raw.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("override") => Precedence::Override,
        Some(s) if s.eq_ignore_ascii_case("aggregate") => Precedence::Aggregate,
        _ => Precedence::Closest,
    }
}

/// `path` / copyright may be a single string or a list in REUSE.toml.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum PathList {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl PathList {
    fn into_vec(self) -> Vec<String> {
        match self {
            PathList::None => Vec::new(),
            PathList::One(s) => vec![s],
            PathList::Many(v) => v,
        }
    }
}

impl OutOfBand {
    /// Load `REUSE.toml` and `.reuse/dep5` from `root`, if present.
    pub fn load(root: &Path) -> Self {
        let mut oob = OutOfBand::default();
        let reuse_toml = root.join("REUSE.toml");
        if let Ok(text) = std::fs::read_to_string(&reuse_toml) {
            oob.parse_reuse_toml(&text);
        }
        let dep5 = root.join(".reuse").join("dep5");
        if let Ok(text) = std::fs::read_to_string(&dep5) {
            oob.parse_dep5(&text);
        }
        oob
    }

    fn parse_reuse_toml(&mut self, text: &str) {
        let parsed: ReuseToml = match toml::from_str(text) {
            Ok(p) => p,
            Err(_) => return,
        };
        for ann in parsed.annotations {
            let precedence = parse_precedence(ann.precedence.as_deref());
            let matchers = compile_globs(ann.path.into_vec());
            self.entries.push(OobAnnotation {
                matchers,
                license: ann.license,
                copyrights: ann.copyright.into_vec(),
                source: OobSource::ReuseToml,
                precedence,
            });
        }
    }

    /// Minimal Debian dep5 (`.reuse/dep5`) parser: paragraphs with `Files:`,
    /// `Copyright:`, `License:`.
    fn parse_dep5(&mut self, text: &str) {
        let mut files: Vec<String> = Vec::new();
        let mut license: Option<String> = None;
        let mut copyrights: Vec<String> = Vec::new();

        let flush = |files: &mut Vec<String>,
                     license: &mut Option<String>,
                     copyrights: &mut Vec<String>,
                     entries: &mut Vec<OobAnnotation>| {
            if !files.is_empty() {
                entries.push(OobAnnotation {
                    matchers: compile_globs(std::mem::take(files)),
                    license: license.take(),
                    copyrights: std::mem::take(copyrights),
                    source: OobSource::Dep5,
                    // dep5 has no `precedence` field; REUSE 3.3 (§"Order of precedence")
                    // specifies its information is *aggregated* with file-level info, so
                    // both the header's and dep5's licenses apply.
                    precedence: Precedence::Aggregate,
                });
            }
            files.clear();
            *license = None;
            copyrights.clear();
        };

        for line in text.lines() {
            if line.trim().is_empty() {
                flush(&mut files, &mut license, &mut copyrights, &mut self.entries);
            } else if let Some(rest) = line.strip_prefix("Files:") {
                files = rest.split_whitespace().map(dep5_glob).collect();
            } else if let Some(rest) = line.strip_prefix("License:") {
                license = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("Copyright:") {
                copyrights.push(rest.trim().to_string());
            }
        }
        flush(&mut files, &mut license, &mut copyrights, &mut self.entries);
    }

    /// Out-of-band coverage for a repo-relative path, if any.
    ///
    /// When several annotations match, REUSE 3.3 resolves the overlap by **last match**
    /// ("exclusively the last matching table in the file is used"), so we scan in reverse.
    /// `REUSE.toml` stays authoritative over legacy `.reuse/dep5`: we prefer the last
    /// matching `REUSE.toml` annotation and only fall back to dep5 when none matches.
    pub fn lookup(&self, rel_path: &Path) -> Option<OutOfBandEntry> {
        let matches = |e: &&OobAnnotation| e.matchers.iter().any(|m| m.is_match(rel_path));
        self.entries
            .iter()
            .rev()
            .find(|e| e.source == OobSource::ReuseToml && matches(e))
            .or_else(|| self.entries.iter().rev().find(matches))
            .map(|e| OutOfBandEntry {
                source: e.source,
                license: e.license.clone(),
                copyrights: e.copyrights.clone(),
                precedence: e.precedence,
            })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Translate a dep5 glob (`foo/*`) into a globset pattern.
fn dep5_glob(s: &str) -> String {
    s.to_string()
}

fn compile_globs(patterns: Vec<String>) -> Vec<GlobMatcher> {
    patterns
        .iter()
        .filter_map(|p| Glob::new(p).ok().map(|g| g.compile_matcher()))
        .collect()
}

/// Outcome of [`write_annotation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationWrite {
    /// The file's effective `REUSE.toml` license already matched; nothing written.
    Unchanged,
    /// An existing exact-path annotation's license was rewritten in place.
    Updated,
    /// A new exact-path annotation was appended (covers a previously uncovered file,
    /// or overrides a broader glob entry — last match wins per REUSE 3.3).
    Appended,
}

impl AnnotationWrite {
    /// Whether the file was actually rewritten.
    pub fn modified(self) -> bool {
        !matches!(self, AnnotationWrite::Unchanged)
    }
}

/// Ensure a `REUSE.toml` annotation covers `rel_path` with `license` (FR-015, FR-003a).
///
/// Creates the file with a `version = 1` header if absent. The behavior matches REUSE 3.3
/// last-match semantics and is idempotent:
/// - if the annotation that currently wins for this file already declares `license`, it is
///   left untouched ([`AnnotationWrite::Unchanged`]);
/// - if the winning annotation is an exact single-path block for this file, its
///   `SPDX-License-Identifier` is rewritten in place ([`AnnotationWrite::Updated`]) —
///   copyright lines are preserved (FR-009);
/// - otherwise (uncovered, or covered only by a broader glob) a new exact-path block is
///   appended ([`AnnotationWrite::Appended`]); being last, it wins.
///
/// The whole file is rewritten atomically (FR-024).
pub fn write_annotation(
    root: &Path,
    rel_path: &str,
    license: &str,
    copyrights: &[String],
) -> std::io::Result<AnnotationWrite> {
    let reuse_toml = root.join("REUSE.toml");
    let existing = std::fs::read_to_string(&reuse_toml).unwrap_or_default();
    let lines: Vec<&str> = existing.lines().collect();
    let blocks = parse_blocks(&lines);

    // The annotation that currently determines this file's license is the *last* one whose
    // path globs match it (REUSE 3.3 overlap resolution).
    let winner = blocks
        .iter()
        .rev()
        .find(|b| b.paths.iter().any(|p| glob_matches(p, rel_path)));

    if let Some(b) = winner {
        if b.license.as_deref() == Some(license) {
            return Ok(AnnotationWrite::Unchanged);
        }
        // Rewrite in place only when the winner is an exact single-path block for this
        // file carrying a license line — it still wins afterward, so this is idempotent.
        if b.paths.len() == 1
            && b.paths[0] == rel_path
            && let Some(li) = b.license_line
        {
            let mut new_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
            new_lines[li] = format!("SPDX-License-Identifier = {}", toml_string(license));
            let mut out = new_lines.join("\n");
            if existing.ends_with('\n') {
                out.push('\n');
            }
            crate::reuse::atomic_write(&reuse_toml, &out)?;
            return Ok(AnnotationWrite::Updated);
        }
        // Glob/array/license-less winner: fall through and append an exact override.
    }

    let mut out = existing.clone();
    if out.is_empty() {
        out.push_str("version = 1\n\n");
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("[[annotations]]\n");
    out.push_str(&format!("path = {}\n", toml_string(rel_path)));
    for c in copyrights {
        out.push_str(&format!("SPDX-FileCopyrightText = {}\n", toml_string(c)));
    }
    out.push_str(&format!(
        "SPDX-License-Identifier = {}\n\n",
        toml_string(license)
    ));

    crate::reuse::atomic_write(&reuse_toml, &out)?;
    Ok(AnnotationWrite::Appended)
}

/// A parsed `[[annotations]]` block: its `path` values, current license, and the source
/// line index of the `SPDX-License-Identifier` (for in-place rewrites).
struct AnnBlock {
    paths: Vec<String>,
    license: Option<String>,
    license_line: Option<usize>,
}

/// Light line-oriented parser for the `[[annotations]]` blocks of a `REUSE.toml`. It only
/// needs `path` and `SPDX-License-Identifier`; anything else is ignored.
fn parse_blocks(lines: &[&str]) -> Vec<AnnBlock> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "[[annotations]]" {
            i += 1;
            continue;
        }
        let mut paths = Vec::new();
        let mut license = None;
        let mut license_line = None;
        let mut j = i + 1;
        while j < lines.len() && lines[j].trim() != "[[annotations]]" {
            if let Some((key, val)) = lines[j].split_once('=') {
                match key.trim() {
                    "path" => paths = quoted_values(val),
                    "SPDX-License-Identifier" => {
                        license = quoted_values(val).into_iter().next();
                        license_line = Some(j);
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        blocks.push(AnnBlock {
            paths,
            license,
            license_line,
        });
        i = j;
    }
    blocks
}

/// Match a `REUSE.toml` path glob against a repo-relative path, falling back to exact
/// equality if the pattern is not a valid glob.
fn glob_matches(pattern: &str, rel_path: &str) -> bool {
    match Glob::new(pattern) {
        Ok(g) => g.compile_matcher().is_match(rel_path),
        Err(_) => pattern == rel_path,
    }
}

/// Extract the double-quoted string values from a TOML scalar or inline-array tail,
/// unescaping `\\` and `\"` (sufficient for the small value space REUSE.toml uses here).
fn quoted_values(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in s.chars() {
        if !in_str {
            if c == '"' {
                in_str = true;
            }
            continue;
        }
        if esc {
            cur.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            out.push(std::mem::take(&mut cur));
            in_str = false;
        } else {
            cur.push(c);
        }
    }
    out
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_reuse_toml_glob() {
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(
            "version = 1\n[[annotations]]\npath = \"assets/**\"\n\
             SPDX-License-Identifier = \"CC0-1.0\"\nSPDX-FileCopyrightText = \"2026 Acme\"\n",
        );
        let e = oob.lookup(&PathBuf::from("assets/logo.png")).unwrap();
        assert_eq!(e.license.as_deref(), Some("CC0-1.0"));
        assert_eq!(e.copyrights, vec!["2026 Acme".to_string()]);
    }

    #[test]
    fn parses_dep5() {
        let mut oob = OutOfBand::default();
        oob.parse_dep5("Files: img/*\nCopyright: 2026 Acme\nLicense: MIT\n");
        let e = oob.lookup(&PathBuf::from("img/x.jpg")).unwrap();
        assert_eq!(e.license.as_deref(), Some("MIT"));
        assert_eq!(e.source, OobSource::Dep5);
    }

    #[test]
    fn no_match_is_none() {
        let oob = OutOfBand::default();
        assert!(oob.lookup(&PathBuf::from("x")).is_none());
    }

    #[test]
    fn precedence_defaults_to_closest() {
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml("[[annotations]]\npath = \"a\"\nSPDX-License-Identifier = \"MIT\"\n");
        assert_eq!(
            oob.lookup(&PathBuf::from("a")).unwrap().precedence,
            Precedence::Closest
        );
    }

    #[test]
    fn precedence_override_parsed() {
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(
            "[[annotations]]\npath = \"a\"\nprecedence = \"override\"\nSPDX-License-Identifier = \"MIT\"\n",
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("a")).unwrap().precedence,
            Precedence::Override
        );
    }

    #[test]
    fn dep5_is_aggregate_precedence() {
        let mut oob = OutOfBand::default();
        oob.parse_dep5("Files: img/*\nLicense: MIT\n");
        assert_eq!(
            oob.lookup(&PathBuf::from("img/x")).unwrap().precedence,
            Precedence::Aggregate
        );
    }

    #[test]
    fn write_annotation_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cprs = vec!["2026 Acme".to_string()];
        assert_eq!(
            write_annotation(root, "logo.png", "CC0-1.0", &cprs).unwrap(),
            AnnotationWrite::Appended
        );
        let first = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        // Second write of the same path with the same license is a no-op.
        assert_eq!(
            write_annotation(root, "logo.png", "CC0-1.0", &cprs).unwrap(),
            AnnotationWrite::Unchanged
        );
        let second = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.matches("path = \"logo.png\"").count(), 1);
        assert!(first.starts_with("version = 1"));
    }

    #[test]
    fn write_annotation_updates_exact_path_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cprs = vec!["2026 Acme".to_string()];
        write_annotation(root, "logo.png", "CC0-1.0", &cprs).unwrap();
        // A new intent for the same exact path rewrites the license line in place.
        assert_eq!(
            write_annotation(root, "logo.png", "CC-BY-4.0", &cprs).unwrap(),
            AnnotationWrite::Updated
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert_eq!(text.matches("path = \"logo.png\"").count(), 1);
        assert!(text.contains("SPDX-License-Identifier = \"CC-BY-4.0\""));
        assert!(!text.contains("CC0-1.0"));
        // Copyright is preserved across the in-place license change (FR-009).
        assert!(text.contains("SPDX-FileCopyrightText = \"2026 Acme\""));

        // And it round-trips through lookup at the new license.
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(&text);
        assert_eq!(
            oob.lookup(&PathBuf::from("logo.png")).unwrap().license,
            Some("CC-BY-4.0".to_string())
        );
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
            write_annotation(root, "logo.png", "CC-BY-4.0", &[]).unwrap(),
            AnnotationWrite::Appended
        );
        let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(&text);
        assert_eq!(
            oob.lookup(&PathBuf::from("logo.png")).unwrap().license,
            Some("CC-BY-4.0".to_string()),
            "exact override must win over the glob"
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("other.png")).unwrap().license,
            Some("MIT".to_string()),
            "the glob still governs its other files"
        );
    }

    #[test]
    fn lookup_is_last_match() {
        let mut oob = OutOfBand::default();
        oob.parse_reuse_toml(
            "[[annotations]]\npath = \"*.png\"\nSPDX-License-Identifier = \"MIT\"\n\
             [[annotations]]\npath = \"logo.png\"\nSPDX-License-Identifier = \"CC-BY-4.0\"\n",
        );
        assert_eq!(
            oob.lookup(&PathBuf::from("logo.png")).unwrap().license,
            Some("CC-BY-4.0".to_string())
        );
    }
}
