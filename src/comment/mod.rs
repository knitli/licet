//! Comment-style registry: built-in table seeded to the REUSE-known set, overlaid by
//! user-defined associations from config (FR-010, FR-011).
//!
//! Resolution precedence: exact filename → extension → built-in default.

use std::path::Path;

use crate::config::{CommentStyleAssociation, CommentStyleRef, LicensingConfiguration};
use crate::domain::{CommentStyle, Selector};

/// Render an SPDX header block in `style` for the given license and copyright lines
/// (write side, FR-011). Lines are terminated with `\n`; the caller adapts line endings.
pub fn render_header(style: &CommentStyle, license: &str, copyrights: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for c in copyrights {
        lines.push(format!("SPDX-FileCopyrightText: {c}"));
    }
    lines.push(format!("SPDX-License-Identifier: {license}"));

    if let Some(prefix) = &style.line_prefix {
        let body: Vec<String> = lines.iter().map(|l| format!("{prefix} {l}")).collect();
        format!("{}\n", body.join("\n"))
    } else if let (Some(start), Some(end)) = (&style.block_start, &style.block_end) {
        let inner_prefix = style.block_line_prefix.as_deref().unwrap_or("");
        let mut out = format!("{start}\n");
        for l in &lines {
            out.push_str(&format!("{inner_prefix}{l}\n"));
        }
        out.push_str(&format!("{end}\n"));
        out
    } else {
        // Empty style: fall back to bare lines (should not happen — validated).
        format!("{}\n", lines.join("\n"))
    }
}

/// Look up a built-in comment style by name (e.g. `c`, `hash`, `html`).
pub fn builtin_named(name: &str) -> Option<CommentStyle> {
    let s = match name.to_ascii_lowercase().as_str() {
        // Line styles.
        "c" | "cpp" | "rust" | "java" | "js" | "slashes" => CommentStyle::line("//"),
        "hash" | "python" | "shell" | "ruby" | "toml" | "yaml" => CommentStyle::line("#"),
        "semicolon" | "lisp" | "ini" => CommentStyle::line(";"),
        "lua" | "sql" => CommentStyle::line("--"),
        "vim" => CommentStyle::line("\""),
        "tex" => CommentStyle::line("%"),
        "batch" => CommentStyle::line("REM"),
        // Block styles.
        "cblock" | "css" => CommentStyle::block("/*", "*/", Some(" * ")),
        "html" | "xml" | "markdown" => CommentStyle::block("<!--", "-->", None),
        _ => return None,
    };
    Some(s)
}

/// Built-in style for a bare extension (no leading dot), seeded to the REUSE-known set.
fn builtin_for_ext(ext: &str) -> Option<CommentStyle> {
    let name = match ext.to_ascii_lowercase().as_str() {
        "rs" | "c" | "h" | "cpp" | "cc" | "hpp" | "cxx" | "java" | "js" | "jsx" | "ts" | "tsx"
        | "go" | "swift" | "kt" | "kts" | "scala" | "dart" | "php" | "cs" => {
            CommentStyle::line("//")
        }
        "py" | "rb" | "sh" | "bash" | "zsh" | "pl" | "pm" | "r" | "toml" | "yaml" | "yml"
        | "cfg" | "conf" | "mk" | "tf" | "dockerfile" => CommentStyle::line("#"),
        "el" | "lisp" | "clj" | "scm" | "asm" | "ini" => CommentStyle::line(";"),
        "lua" | "sql" | "hs" | "elm" => CommentStyle::line("--"),
        "css" | "scss" | "less" => CommentStyle::block("/*", "*/", Some(" * ")),
        "html" | "htm" | "xml" | "svg" | "vue" | "md" | "markdown" => {
            CommentStyle::block("<!--", "-->", None)
        }
        "tex" | "sty" | "cls" => CommentStyle::line("%"),
        "vim" => CommentStyle::line("\""),
        _ => return None,
    };
    Some(name)
}

/// Built-in style for an exact filename (REUSE-known special files).
fn builtin_for_filename(name: &str) -> Option<CommentStyle> {
    let style = match name {
        "Makefile" | "Dockerfile" | "Containerfile" | ".gitignore" | ".gitattributes"
        | "Gemfile" | "Rakefile" | "CMakeLists.txt" => CommentStyle::line("#"),
        "Jenkinsfile" => CommentStyle::line("//"),
        _ => return None,
    };
    Some(style)
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

    /// Resolve the comment style for `path` (FR-011 precedence:
    /// exact filename association → extension association → built-in filename →
    /// built-in extension).
    pub fn resolve(&self, path: &Path) -> Option<CommentStyle> {
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
            && let Some(style) = builtin_for_filename(fname)
        {
            return Some(style);
        }
        // 4. Built-in by extension.
        if let Some(e) = ext
            && let Some(style) = builtin_for_ext(e)
        {
            return Some(style);
        }
        None
    }

    fn assoc_for_filename(&self, filename: &str) -> Option<CommentStyle> {
        self.associations.iter().find_map(|a| match &a.selector {
            Selector::Filename(f) if f == filename => self.materialize(&a.style),
            Selector::ExactPath(p) if p.rsplit('/').next() == Some(filename) => {
                self.materialize(&a.style)
            }
            _ => None,
        })
    }

    fn assoc_for_ext(&self, ext: &str) -> Option<CommentStyle> {
        self.associations.iter().find_map(|a| match &a.selector {
            Selector::Extension(e) if e.eq_ignore_ascii_case(ext) => self.materialize(&a.style),
            _ => None,
        })
    }

    fn materialize(&self, style: &CommentStyleRef) -> Option<CommentStyle> {
        match style {
            CommentStyleRef::Named(name) => builtin_named(name),
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

    #[test]
    fn builtin_rust_is_slashes() {
        let cfg = cfg_with(vec![]);
        let r = CommentResolver::new(&cfg);
        assert_eq!(
            r.resolve(&PathBuf::from("src/lib.rs")).unwrap().line_prefix,
            Some("//".to_string())
        );
    }

    #[test]
    fn config_ext_overrides_when_unknown() {
        let cfg = cfg_with(vec![CommentStyleAssociation {
            selector: Selector::Extension("pkl".into()),
            style: CommentStyleRef::Named("c".into()),
        }]);
        let r = CommentResolver::new(&cfg);
        assert_eq!(
            r.resolve(&PathBuf::from("hk.pkl")).unwrap().line_prefix,
            Some("//".to_string())
        );
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
                style: CommentStyleRef::Inline(CommentStyle::line("//")),
            },
        ]);
        let r = CommentResolver::new(&cfg);
        assert_eq!(
            r.resolve(&PathBuf::from("hk.pkl")).unwrap().line_prefix,
            Some("//".to_string())
        );
    }

    #[test]
    fn unknown_ext_no_config_is_none() {
        let cfg = cfg_with(vec![]);
        let r = CommentResolver::new(&cfg);
        assert!(r.resolve(&PathBuf::from("mystery.zzz")).is_none());
    }
}
