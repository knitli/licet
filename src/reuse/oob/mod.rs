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

use globset::GlobMatcher;
use serde::Deserialize;

use crate::domain::Precedence;

mod lookup;
mod parse;
mod write;

pub use write::{
    AnnotationBatchOutcome, AnnotationDestination, AnnotationRequest, AnnotationWrite, DocPatch,
    annotation_destination, write_annotation, write_annotations,
};

#[cfg(test)]
mod tests;

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
