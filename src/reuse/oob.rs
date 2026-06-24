//! Out-of-band REUSE metadata: read `REUSE.toml` (current spec) and `.reuse/dep5`
//! (legacy) for detection, and write `REUSE.toml` annotations for non-annotatable
//! files (FR-015, FR-003a, research §7).

use std::path::Path;

use globset::{Glob, GlobMatcher};
use serde::Deserialize;

use crate::domain::{OobSource, OutOfBandEntry};

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
            let matchers = compile_globs(ann.path.into_vec());
            self.entries.push(OobAnnotation {
                matchers,
                license: ann.license,
                copyrights: ann.copyright.into_vec(),
                source: OobSource::ReuseToml,
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
/// Creates the file with a `version = 1` header if it does not exist.
pub fn write_annotation(
    root: &Path,
    rel_path: &str,
    license: &str,
    copyrights: &[String],
) -> std::io::Result<()> {
    let reuse_toml = root.join("REUSE.toml");
    let mut out = String::new();
    if !reuse_toml.exists() {
        out.push_str("version = 1\n\n");
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

    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&reuse_toml)?;
    f.write_all(out.as_bytes())
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
}
