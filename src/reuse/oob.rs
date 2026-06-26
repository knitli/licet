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
                    // dep5 predates the `precedence` field; preserve the legacy
                    // authoritative behavior by treating it as an override.
                    precedence: Precedence::Override,
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
    pub fn lookup(&self, rel_path: &Path) -> Option<OutOfBandEntry> {
        self.entries
            .iter()
            .find(|e| e.matchers.iter().any(|m| m.is_match(rel_path)))
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

/// Append a `REUSE.toml` annotation for a non-annotatable file (FR-015).
///
/// Creates the file with a `version = 1` header if it does not exist, and is
/// idempotent: if a block with the same `path =` already exists it is left
/// untouched. The whole file is rewritten atomically (FR-024).
///
/// Returns `true` when the file was modified, `false` when the path was already
/// annotated.
pub fn write_annotation(
    root: &Path,
    rel_path: &str,
    license: &str,
    copyrights: &[String],
) -> std::io::Result<bool> {
    let reuse_toml = root.join("REUSE.toml");
    let existing = std::fs::read_to_string(&reuse_toml).unwrap_or_default();

    // Idempotency: skip if this exact path is already annotated.
    let path_line = format!("path = {}", toml_string(rel_path));
    if existing.lines().any(|l| l.trim() == path_line) {
        return Ok(false);
    }

    let mut out = existing.clone();
    if out.is_empty() {
        out.push_str("version = 1\n\n");
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("[[annotations]]\n");
    out.push_str(&format!("{path_line}\n"));
    for c in copyrights {
        out.push_str(&format!("SPDX-FileCopyrightText = {}\n", toml_string(c)));
    }
    out.push_str(&format!(
        "SPDX-License-Identifier = {}\n\n",
        toml_string(license)
    ));

    crate::reuse::atomic_write(&reuse_toml, &out)?;
    Ok(true)
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
    fn dep5_is_override_precedence() {
        let mut oob = OutOfBand::default();
        oob.parse_dep5("Files: img/*\nLicense: MIT\n");
        assert_eq!(
            oob.lookup(&PathBuf::from("img/x")).unwrap().precedence,
            Precedence::Override
        );
    }

    #[test]
    fn write_annotation_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cprs = vec!["2026 Acme".to_string()];
        assert!(write_annotation(root, "logo.png", "CC0-1.0", &cprs).unwrap());
        let first = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        // Second write of the same path is a no-op.
        assert!(!write_annotation(root, "logo.png", "CC0-1.0", &cprs).unwrap());
        let second = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.matches("path = \"logo.png\"").count(), 1);
        assert!(first.starts_with("version = 1"));
    }
}
