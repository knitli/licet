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

use crate::comment::extensions::{
    APPLESCRIPT_EXTS, ASPX_EXTS, BATCH_EXTS, BIBTEX_EXTS, BLADE_EXTS, C_EXTS, CPP_SINGLE_ONLY_EXTS,
    CSS_EXTS, FTL_EXTS, HANDLEBARS_EXTS, HASH_FILENAMES, HASH_STYLE_EXTS, HASKELL_EXTS, HTML_EXTS,
    JINJA_EXTS, JULIA_EXTS, LEAN_EXTS, LEGACY_FORTRAN_EXTS, LISP_EXTS, M4_EXTS, ML_EXTS,
    MODERN_FORTRAN_EXTS, PASCAL_EXTS, PLANTUML_EXTS, RESTRUCTURED_TEXT_EXTS, SEMICOLON_EXTS,
    TEX_EXTS, UNIX_MANUAL_EXTS, VELOCITY_EXTS, VIM_EXTS, XQUERY_EXTS,
};
use crate::domain::{Comment, CommentSyntax};

/// The complete built-in registry. Rows are grouped by syntax family; `family` is
/// a descriptive label, not a lookup key.
pub static COMMENTS: &[Comment] = &[
    // Applescript: `--` line.
    Comment {
        family: "applescript",
        extensions: APPLESCRIPT_EXTS,
        filenames: &[],
        aliases: &["scpt"],
        syntax: CommentSyntax::both("--", "(*", "*)", ""),
    },
    // ASPX: `<%-- … --%>` block.
    Comment {
        family: "aspx",
        extensions: ASPX_EXTS,
        filenames: &[],
        aliases: &["asp.net"],
        syntax: CommentSyntax::block_only("<%--", "--%>", ""),
    },
    // BibTeX: `@Comment{ … }` block.
    Comment {
        family: "bibtex",
        extensions: BIBTEX_EXTS,
        filenames: &[],
        aliases: &["bib"],
        syntax: CommentSyntax::block_only("@Comment{", "}", ""),
    },
    // Blade: `{{-- … --}}` block.
    Comment {
        family: "blade",
        extensions: BLADE_EXTS,
        filenames: &[],
        aliases: &["blade", "laravel blade"],
        syntax: CommentSyntax::block_only("{{--", "--}}", ""),
    },
    // CSS-style: `/* … */` block, no line form (CSS/LESS/SASS/SCSS and other
    // block-only languages). C/C++ source live in the `C` row below (both forms).
    Comment {
        family: "css",
        extensions: CSS_EXTS,
        filenames: &[],
        aliases: &[
            "c-block", "cblock", "css", "gas", "less", "qss", "sass", "scss",
        ],
        syntax: CommentSyntax::block_only("/*", "*/", " * "),
    },
    // C-style: `//` line + `/* … */` block. Render prefers the line form, so output
    // is identical to the legacy `//`-only mapping while modeling block support.
    Comment {
        family: "C",
        extensions: C_EXTS,
        filenames: &["Jenkinsfile", "go.mod"],
        aliases: &[
            "c",
            "cpp",
            "c++",
            "c#",
            "csharp",
            "c-sharp",
            "dart",
            "go",
            "java",
            "javascript",
            "js",
            "jsonc",
            "odin",
            "jsx",
            "php",
            "react",
            "rust",
            "scala",
            "slashes",
            "swift",
            "ts",
            "tsx",
            "typescript",
        ],
        syntax: CommentSyntax::both("//", "/*", "*/", " * "),
    },
    Comment {
        family: "cpp-single",
        extensions: CPP_SINGLE_ONLY_EXTS,
        filenames: &[],
        aliases: &[
            "cpp-single-only",
            "cpp-line",
            "c++-line",
            "gleam",
            "zig",
            "zon",
        ],
        syntax: CommentSyntax::line_only("//"),
    },
    // Hash: `#` line. Covers most scripting/config languages and the `#`-commented
    // special filenames.
    Comment {
        family: "hash",
        extensions: HASH_STYLE_EXTS,
        filenames: HASH_FILENAMES,
        aliases: &[
            "hash",
            "bazel",
            "bash",
            "bitbake",
            "cmake",
            "fish",
            "graphql",
            "nim",
            "python",
            "shell",
            "ruby",
            "terraform",
            "toml",
            "yaml",
            "zsh",
        ],
        syntax: CommentSyntax::line_only("#"),
    },
    // Lisp family `;;;` line
    Comment {
        family: "lisp",
        extensions: LISP_EXTS,
        filenames: &[],
        aliases: &["lisp", "assembly", "clojure", "elisp", "emacs", "scheme"],
        syntax: CommentSyntax::line_only(";;;"),
    },
    // Semicolon: `;` line (INI).
    Comment {
        family: "semicolon",
        extensions: SEMICOLON_EXTS,
        filenames: &[".npmrc", "dune", "dune-project", "dune-workspace"],
        aliases: &["semicolon", "dune", "ini"],
        syntax: CommentSyntax::line_only(";"),
    },
    // Haskell: `--` line (SQL, Lua, Haskell, Elm). Kept line-only; per-language block
    // forms (`{- -}`, `--[[ ]]`) differ and aren't represented by this shared row.
    Comment {
        family: "haskell",
        extensions: HASKELL_EXTS,
        filenames: &["cabal.project"],
        aliases: &["elm", "lua", "sql"],
        syntax: CommentSyntax::line_only("--"),
    },
    // Html: `<!-- … -->` block, no per-line prefix.
    Comment {
        family: "html",
        extensions: HTML_EXTS,
        filenames: &[],
        aliases: &[
            "astro",
            "html",
            "hypertext",
            "xml",
            "markup",
            "markdown",
            "svelte",
            "vue",
        ],
        syntax: CommentSyntax::block_only("<!--", "-->", ""),
    },
    // TeX: `%` line.
    Comment {
        family: "tex",
        extensions: TEX_EXTS,
        filenames: &[],
        aliases: &["tex", "latex"],
        syntax: CommentSyntax::line_only("%"),
    },
    // Vim script: `"` line.
    Comment {
        family: "vim",
        extensions: VIM_EXTS,
        filenames: &["vimrc", ".vimrc"],
        aliases: &["vim", "vi", "nvim"],
        syntax: CommentSyntax::line_only("\""),
    },
    // Batch: `REM` line. Extensions added (the legacy `batch` alias had no extension
    // mapping, so `.bat`/`.cmd` files resolved to nothing).
    Comment {
        family: "batch",
        extensions: BATCH_EXTS,
        filenames: &["batchfile", ".batchfile"],
        aliases: &["batch"],
        syntax: CommentSyntax::line_only("REM"),
    },
    // Legacy Fortran: `C` line.
    Comment {
        family: "legacy_fortran",
        extensions: LEGACY_FORTRAN_EXTS,
        filenames: &[],
        aliases: &["fortran", "legacy fortran", "f77"],
        syntax: CommentSyntax::line_only("c"),
    },
    // Modern Fortran: `!` line.
    Comment {
        family: "modern_fortran",
        extensions: MODERN_FORTRAN_EXTS,
        filenames: &[],
        aliases: &["modern fortran", "f90", "f95", "f03", "f08", "f18"],
        syntax: CommentSyntax::line_only("!"),
    },
    // FreeMarker: `<#-- … -->` block.
    Comment {
        family: "ftl",
        extensions: FTL_EXTS,
        filenames: &[],
        aliases: &["freemarker", "freemarker template language"],
        syntax: CommentSyntax::block_only("<#--", "--#>", ""),
    },
    // Handlebars: `{{!-- … --}}` block.
    Comment {
        family: "handlebars",
        extensions: HANDLEBARS_EXTS,
        filenames: &[],
        aliases: &["hbs"],
        syntax: CommentSyntax::block_only("{{!--", "--}}", ""),
    },
    // Jinja: `{# … #}` block.
    Comment {
        family: "jinja",
        extensions: JINJA_EXTS,
        filenames: &[],
        aliases: &["jinja2", "j2"],
        syntax: CommentSyntax::block_only("{#", " #}", ""),
    },
    // Julia: `#` line + `#= … =#` block.
    Comment {
        family: "julia",
        extensions: JULIA_EXTS,
        filenames: &[],
        aliases: &["jl"],
        syntax: CommentSyntax::both("#", "#=", "=#", ""),
    },
    // Lean: `/- … -/` block.
    Comment {
        family: "lean",
        extensions: LEAN_EXTS,
        filenames: &[],
        aliases: &[],
        syntax: CommentSyntax::block_only("/-", "-/", "-"),
    },
    // M4: `dnl` line.
    Comment {
        family: "m4",
        extensions: M4_EXTS,
        filenames: &["configure.ac"],
        aliases: &[],
        syntax: CommentSyntax::line_only("dnl"),
    },
    // ML: `(* … *)` block.
    Comment {
        family: "ml",
        extensions: ML_EXTS,
        filenames: &["ROOT"],
        aliases: &["ocaml"],
        syntax: CommentSyntax::block_only("(*", "*)", "*"),
    },
    // Pascal: `//` line + `{ … }` block.
    Comment {
        family: "pascal",
        extensions: PASCAL_EXTS,
        filenames: &[],
        aliases: &[],
        syntax: CommentSyntax::both("//", "{", "}", ""),
    },
    // PlantUML: `'` line + `/' … '/` block.
    Comment {
        family: "plantuml",
        extensions: PLANTUML_EXTS,
        filenames: &[],
        aliases: &["puml"],
        syntax: CommentSyntax::both("'", "/'", "'/", "'"),
    },
    // reStructuredText: `..` line.
    Comment {
        family: "restructuredtext",
        extensions: RESTRUCTURED_TEXT_EXTS,
        filenames: &[],
        aliases: &["rst"],
        syntax: CommentSyntax::line_only(".."),
    },
    // Unix Manual: `.\` line.
    Comment {
        family: "unix-manual",
        extensions: UNIX_MANUAL_EXTS,
        filenames: &[],
        aliases: &["manpage", "man"],
        syntax: CommentSyntax::line_only(".\\"),
    },
    // Velocity: `#* … *#` block.
    Comment {
        family: "velocity",
        extensions: VELOCITY_EXTS,
        filenames: &[],
        aliases: &["vst"],
        syntax: CommentSyntax::block_only("#*", "*#", " "),
    },
    // XQuery: `(: … :)` block.
    Comment {
        family: "xquery",
        extensions: XQUERY_EXTS,
        filenames: &[],
        aliases: &["x-query"],
        syntax: CommentSyntax::block_only("(:", ":)", " "),
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
        assert_eq!(by_filename("Jenkinsfile").unwrap().family, "C");
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
