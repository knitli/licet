//! Built-in comment registry: one row per language family, ported from the
//! REUSE-known set (data-model §4, FR-010). This is **pure data** — adding a
//! language is a new [`Comment`] row, never new code.
//!
//! Three lookup namespaces are kept separate, matching the resolver's precedence
//! (`comment/mod.rs`): exact filename, file extension, and config `style = "..."`
//! aliases. `c` (alias → `//`) and `c` (extension → `//`) never collide because
//! they live in different indices.
//!
//! The `by_*` helpers are the lookup boundary: they scan the table linearly today,
//! which is negligible for its size, and can swap to prebuilt maps behind the same
//! signatures with zero caller changes if the table grows large.

use crate::domain::{Comment, CommentSyntax};

/// The complete built-in registry. Rows are grouped by syntax family; `family` is
/// a descriptive label, not a lookup key.
pub static COMMENTS: &[Comment] = &[
    // C-style: `//` line + `/* … */` block. Render prefers the line form, so output
    // is identical to the legacy `//`-only mapping while modeling block support.
    Comment {
        family: "C-style",
        extensions: &[
            "rs", "c", "h", "cpp", "cc", "hpp", "cxx", "java", "js", "jsx", "ts", "tsx", "go",
            "swift", "kt", "kts", "scala", "dart", "php", "cs",
        ],
        filenames: &["Jenkinsfile"],
        aliases: &["c", "cpp", "rust", "java", "js", "slashes"],
        syntax: CommentSyntax::both("//", "/*", "*/", " * "),
    },
    // Hash: `#` line. Covers most scripting/config languages and the `#`-commented
    // special filenames.
    Comment {
        family: "hash",
        extensions: &[
            "py",
            "rb",
            "sh",
            "bash",
            "zsh",
            "pl",
            "pm",
            "r",
            "toml",
            "yaml",
            "yml",
            "cfg",
            "conf",
            "mk",
            "tf",
            "dockerfile",
        ],
        filenames: &[
            "Makefile",
            "Dockerfile",
            "Containerfile",
            ".gitignore",
            ".gitattributes",
            "Gemfile",
            "Rakefile",
            "CMakeLists.txt",
        ],
        aliases: &["hash", "python", "shell", "ruby", "toml", "yaml"],
        syntax: CommentSyntax::line_only("#"),
    },
    // Semicolon: `;` line (Lisp dialects, assembly, INI).
    Comment {
        family: "semicolon",
        extensions: &["el", "lisp", "clj", "scm", "asm", "ini"],
        filenames: &[],
        aliases: &["semicolon", "lisp", "ini"],
        syntax: CommentSyntax::line_only(";"),
    },
    // Dashes: `--` line (SQL, Lua, Haskell, Elm). Kept line-only; per-language block
    // forms (`{- -}`, `--[[ ]]`) differ and aren't represented by this shared row.
    Comment {
        family: "dashes",
        extensions: &["lua", "sql", "hs", "elm"],
        filenames: &[],
        aliases: &["lua", "sql"],
        syntax: CommentSyntax::line_only("--"),
    },
    // C-block / CSS: `/* … */` block with ` * ` alignment, no line form.
    Comment {
        family: "css",
        extensions: &["css", "scss", "less"],
        filenames: &[],
        aliases: &["cblock", "css"],
        syntax: CommentSyntax::block_only("/*", "*/", " * "),
    },
    // Markup: `<!-- … -->` block, no per-line prefix.
    Comment {
        family: "markup",
        extensions: &["html", "htm", "xml", "svg", "vue", "md", "markdown"],
        filenames: &[],
        aliases: &["html", "xml", "markdown"],
        syntax: CommentSyntax::block_only("<!--", "-->", ""),
    },
    // TeX: `%` line.
    Comment {
        family: "tex",
        extensions: &["tex", "sty", "cls"],
        filenames: &[],
        aliases: &["tex"],
        syntax: CommentSyntax::line_only("%"),
    },
    // Vim script: `"` line.
    Comment {
        family: "vim",
        extensions: &["vim"],
        filenames: &[],
        aliases: &["vim"],
        syntax: CommentSyntax::line_only("\""),
    },
    // Batch: `REM` line. Extensions added (the legacy `batch` alias had no extension
    // mapping, so `.bat`/`.cmd` files resolved to nothing).
    Comment {
        family: "batch",
        extensions: &["bat", "cmd"],
        filenames: &[],
        aliases: &["batch"],
        syntax: CommentSyntax::line_only("REM"),
    },
];

/// Look up a row by file extension (case-insensitive, no leading dot).
pub fn by_extension(ext: &str) -> Option<&'static Comment> {
    COMMENTS
        .iter()
        .find(|c| c.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// Look up a row by exact filename.
pub fn by_filename(name: &str) -> Option<&'static Comment> {
    COMMENTS.iter().find(|c| c.filenames.contains(&name))
}

/// Look up a row by config `style = "..."` alias (case-insensitive).
pub fn by_alias(alias: &str) -> Option<&'static Comment> {
    COMMENTS
        .iter()
        .find(|c| c.aliases.iter().any(|a| a.eq_ignore_ascii_case(alias)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn rust_resolves_to_slashes() {
        let c = by_extension("rs").unwrap();
        assert_eq!(c.syntax.line().unwrap().prefix, "//");
    }

    #[test]
    fn c_supports_both_line_and_block() {
        let c = by_extension("c").unwrap();
        assert!(c.syntax.line().is_some());
        let b = c.syntax.block().unwrap();
        assert_eq!(b.open, "/*");
        assert_eq!(b.close, "*/");
        assert_eq!(b.line_prefix, " * ");
    }

    #[test]
    fn css_is_block_only() {
        let c = by_extension("scss").unwrap();
        assert!(c.syntax.line().is_none());
        assert!(c.syntax.block().is_some());
    }

    #[test]
    fn markup_block_has_no_line_prefix() {
        let c = by_extension("html").unwrap();
        assert_eq!(c.syntax.block().unwrap().line_prefix, "");
    }

    #[test]
    fn special_filenames_resolve() {
        assert_eq!(by_filename("Makefile").unwrap().family, "hash");
        assert_eq!(by_filename("Jenkinsfile").unwrap().family, "C-style");
    }

    #[test]
    fn alias_namespace_is_independent_of_extensions() {
        // `slashes` is alias-only; `rs` is extension-only.
        assert!(by_alias("slashes").is_some());
        assert!(by_extension("slashes").is_none());
    }

    #[test]
    fn extensions_are_unique_across_rows() {
        let mut seen = HashSet::new();
        for c in COMMENTS {
            for e in c.extensions {
                assert!(seen.insert(*e), "extension `{e}` is mapped by two rows");
            }
        }
    }

    #[test]
    fn aliases_are_unique_across_rows() {
        let mut seen = HashSet::new();
        for c in COMMENTS {
            for a in c.aliases {
                assert!(seen.insert(*a), "alias `{a}` is mapped by two rows");
            }
        }
    }
}
