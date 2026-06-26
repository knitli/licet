//! Comment-style registry: built-in table seeded to the REUSE-known set, overlaid by
//! user-defined associations from config (FR-010, FR-011).
//!
//! Resolution precedence: exact filename → extension → built-in default.

mod comment_style;

pub use comment_style::by_alias;

use crate::config::{CommentStyleAssociation, CommentStyleRef, LicensingConfiguration};
use crate::domain::{CommentSyntax, Selector};
use std::path::Path;

/// Render an SPDX header block in `syntax` for the given license and copyright lines
/// (write side, FR-011). Lines are terminated with `\n`; the caller adapts line endings.
///
/// Policy: prefer the line form when the language supports it (REUSE convention is a
/// single `# SPDX-License-Identifier:` line); fall back to the block form otherwise.
pub fn render_header(syntax: &CommentSyntax, license: &str, copyrights: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for c in copyrights {
        lines.push(format!("SPDX-FileCopyrightText: {c}"));
    }
    lines.push(format!("SPDX-License-Identifier: {license}"));

    if let Some(line) = syntax.line() {
        let prefix = &line.prefix;
        let body: Vec<String> = lines.iter().map(|l| format!("{prefix} {l}")).collect();
        format!("{}\n", body.join("\n"))
    } else {
        // Block-only: the sum type guarantees a block exists when no line form does.
        let block = syntax
            .block()
            .expect("CommentSyntax has neither line nor block");
        let inner_prefix = &block.line_prefix;
        let mut out = format!("{}\n", block.open);
        for l in &lines {
            out.push_str(&format!("{inner_prefix}{l}\n"));
        }
        out.push_str(&format!("{}\n", block.close));
        out
    }
}

/// Resolves a path to a comment style, overlaying config associations on built-ins.
pub struct CommentResolver<'a> {
    associations: &'a [CommentStyleAssociation],
}

impl<'a> CommentResolver<'a> {
    pub fn new(config: &'a LicensingConfiguration) -> Self {
        CommentResolver {
            associations: &config.comment_styles,
        }
    }

    /// Resolve the comment syntax for `path` (FR-011 precedence:
    /// exact filename association → extension association → built-in filename →
    /// built-in extension).
    pub fn resolve(&self, path: &Path) -> Option<CommentSyntax> {
        let filename = path.file_name().and_then(|n| n.to_str());
        let ext = path.extension().and_then(|e| e.to_str());

        // 1. Config association by exact filename.
        if let Some(fname) = filename
            && let Some(style) = self.assoc_for_filename(fname)
        {
            return Some(style);
        }
        // 2. Config association by extension.
        if let Some(e) = ext
            && let Some(style) = self.assoc_for_ext(e)
        {
            return Some(style);
        }
        // 3. Built-in by filename.
        if let Some(fname) = filename
            && let Some(c) = comment_style::by_filename(fname)
        {
            return Some(c.syntax.clone());
        }
        // 4. Built-in by extension.
        if let Some(e) = ext
            && let Some(c) = comment_style::by_extension(e)
        {
            return Some(c.syntax.clone());
        }
        None
    }

    fn assoc_for_filename(&self, filename: &str) -> Option<CommentSyntax> {
        self.associations.iter().find_map(|a| match &a.selector {
            Selector::Filename(f) if f == filename => self.materialize(&a.style),
            Selector::ExactPath(p) if p.rsplit('/').next() == Some(filename) => {
                self.materialize(&a.style)
            }
            _ => None,
        })
    }

    fn assoc_for_ext(&self, ext: &str) -> Option<CommentSyntax> {
        self.associations.iter().find_map(|a| match &a.selector {
            Selector::Extension(e) if e.eq_ignore_ascii_case(ext) => self.materialize(&a.style),
            _ => None,
        })
    }

    fn materialize(&self, style: &CommentStyleRef) -> Option<CommentSyntax> {
        match style {
            CommentStyleRef::Named(name) => by_alias(name).map(|c| c.syntax.clone()),
            CommentStyleRef::Inline(s) => Some(s.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cfg_with(assoc: Vec<CommentStyleAssociation>) -> LicensingConfiguration {
        LicensingConfiguration {
            comment_styles: assoc,
            ..Default::default()
        }
    }

    fn line_prefix_of(syntax: &CommentSyntax) -> &str {
        &syntax.line().expect("expected a line style").prefix
    }

    #[test]
    fn builtin_rust_is_slashes() {
        let cfg = cfg_with(vec![]);
        let r = CommentResolver::new(&cfg);
        let s = r.resolve(&PathBuf::from("src/lib.rs")).unwrap();
        assert_eq!(line_prefix_of(&s), "//");
    }

    #[test]
    fn config_ext_overrides_when_unknown() {
        let cfg = cfg_with(vec![CommentStyleAssociation {
            selector: Selector::Extension("pkl".into()),
            style: CommentStyleRef::Named("c".into()),
        }]);
        let r = CommentResolver::new(&cfg);
        let s = r.resolve(&PathBuf::from("hk.pkl")).unwrap();
        assert_eq!(line_prefix_of(&s), "//");
    }

    #[test]
    fn filename_assoc_beats_ext_assoc() {
        let cfg = cfg_with(vec![
            CommentStyleAssociation {
                selector: Selector::Extension("pkl".into()),
                style: CommentStyleRef::Named("hash".into()),
            },
            CommentStyleAssociation {
                selector: Selector::Filename("hk.pkl".into()),
                style: CommentStyleRef::Inline(CommentSyntax::line_only("//")),
            },
        ]);
        let r = CommentResolver::new(&cfg);
        let s = r.resolve(&PathBuf::from("hk.pkl")).unwrap();
        assert_eq!(line_prefix_of(&s), "//");
    }

    #[test]
    fn unknown_ext_no_config_is_none() {
        let cfg = cfg_with(vec![]);
        let r = CommentResolver::new(&cfg);
        assert!(r.resolve(&PathBuf::from("mystery.zzz")).is_none());
    }

    /// Invariant: detection must parse a superset of what rendering emits, so every
    /// built-in style round-trips through `detect` — a header `licet` writes is always
    /// recognized again, never re-flagged as missing/wrong (render ⊆ parse).
    #[test]
    fn rendered_headers_round_trip_through_detection() {
        use crate::reuse::oob::OutOfBand;

        let oob = OutOfBand::default();
        for c in comment_style::COMMENTS {
            let rendered = render_header(&c.syntax, "MIT", &["2026 Acme".to_string()]);
            // Synthetic file: the rendered header followed by a blank line and a body.
            let content = format!("{rendered}\nbody\n");
            let actual = crate::detect::detect(&PathBuf::from("sample"), content.as_bytes(), &oob);

            assert_eq!(
                actual.detected_license.as_deref(),
                Some("MIT"),
                "license did not round-trip for family `{}`\n---\n{rendered}---",
                c.family
            );
            assert!(
                actual
                    .detected_copyrights
                    .iter()
                    .any(|cp| cp.contains("2026 Acme")),
                "copyright did not round-trip for family `{}` (got {:?})\n---\n{rendered}---",
                c.family,
                actual.detected_copyrights
            );
        }
    }
}
