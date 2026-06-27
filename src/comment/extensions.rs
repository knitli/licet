//! This module contains the file extension lists for each comment style family.
//!
//! It also includes the filenames list for the hash-style comment family, because it's extensive.

pub static APPLESCRIPT_EXTS: &[&str; 3] = &["applescript", "scpt", "scptd"];

pub static ASPX_EXTS: &[&str; 5] = &["asax", "asmx", "aspx", "axd", "jsp"];

pub static BATCH_EXTS: &[&str; 2] = &["bat", "cmd"];

pub static BIBTEX_EXTS: &[&str; 1] = &["bib"];

pub static BLADE_EXTS: &[&str; 2] = &["blade", "blade.php"];

pub static CSS_EXTS: &[&str; 7] = &["css", "ld", "less", "qss", "s", "sass", "scss"];

pub static C_EXTS: &[&str; 62] = &[
    "adoc",
    "aidl",
    "asc",
    "asciidoc",
    "c",
    "cc",
    "cjs",
    "code-workspace",
    "cpp",
    "cs",
    "cts",
    "cu",
    "cuh",
    "cxx",
    "d",
    "dart",
    "di",
    "dts",
    "dtsi",
    "fs",
    "fsx",
    "go",
    "gperf",
    "gradle",
    "groovy",
    "h",
    "hh",
    "hjson",
    "hpp",
    "hx",
    "hxsl",
    "ino",
    "java",
    "js",
    "json5",
    "jsonc",
    "jsx",
    "kt",
    "kts",
    "mjs",
    "mts",
    "odin",
    "php",
    "php3",
    "php4",
    "php5",
    "pkl",
    "proto",
    "qbs",
    "qml",
    "rs",
    "sbt",
    "sc",
    "scad",
    "scala",
    "soy",
    "swift",
    "ts",
    "tsx",
    "v",
    "vala",
    "vsh",
];

pub static CPP_SINGLE_ONLY_EXTS: &[&str; 4] = &["gleam", "ha", "zig", "zon"];

pub static FTL_EXTS: &[&str; 1] = &["ftl"];

pub static HANDLEBARS_EXTS: &[&str; 1] = &["hbs"];

pub static HASH_STYLE_EXTS: &[&str; 67] = &[
    "awk",
    "bash",
    "bats",
    "bb",
    "bbappend",
    "bbclass",
    "bxl",
    "bzl",
    "cfg",
    "cmake",
    "coffee",
    "conf",
    "cr",
    "cson",
    "csh",
    "dockerfile",
    "ex",
    "exs",
    "env",
    "fish",
    "gemspec",
    "graphql",
    "graphqls",
    "gqls",
    "hcl",
    "jy",
    "ksh",
    "mk",
    "nim",
    "nimble",
    "nimrod",
    "nix",
    "org",
    "pl",
    "pm",
    "po",
    "pod",
    "pot",
    "pri",
    "pro",
    "properties",
    "ps1",
    "psm1",
    "pxd",
    "py",
    "pyi",
    "pyw",
    "pyx",
    "r",
    "rake",
    "rb",
    "rbw",
    "rbx",
    "sh",
    "sky",
    "smk",
    "star",
    "t",
    "tcl",
    "tf",
    "tfvars",
    "toml",
    "ttl",
    "xsh",
    "yaml",
    "yml",
    "zsh",
];

pub static HASH_FILENAMES: &[&str; 61] = &[
    ".bash_profile",
    ".bashrc",
    ".bazelignore",
    ".bazelrc",
    ".browserslist",
    ".clang-format",
    ".clang-tidy",
    ".coveragerc",
    ".dockerignore",
    ".earthlyignore",
    ".editorconfig",
    ".envrc",
    ".eslintignore",
    ".eslintrc",
    ".gitattributes",
    ".gitignore",
    ".gitmodules",
    ".htaccess",
    ".mailmap",
    ".mdlrc",
    ".npmignore",
    ".prettierignore",
    ".pylintrc",
    ".python-version",
    ".Renviron",
    ".Rprofile",
    ".ruby-version",
    ".shellcheckrc",
    ".taprc",
    ".tool-versions",
    ".yamllint",
    ".yarnrc",
    ".zprofile",
    ".zshenv",
    ".zshrc",
    "ansible.cfg",
    "BUCK",
    "BUILD",
    "CMakeLists.txt",
    "CODEOWNERS",
    "Containerfile",
    "Dockerfile",
    "Doxyfile",
    "Earthfile",
    "Gemfile",
    "gradlew",
    "Makefile.am",
    "Makefile",
    "MANIFEST.in",
    "manifest",
    "matplotlibrc",
    "meson_options.txt",
    "meson.build",
    "nim.cfg",
    "PACKAGE",
    "py.typed",
    "pylintrc",
    "Rakefile",
    "requirements.txt",
    "setup.cfg",
    "Snakefile",
];

pub static HASKELL_EXTS: &[&str; 8] = &["adb", "ads", "cabal", "elm", "hs", "lua", "sql", "vhdl"];

pub static HTML_EXTS: &[&str; 23] = &[
    "astro", "csl", "csproj", "dtd", "fsproj", "html", "htm", "md", "markdown", "props", "qrc",
    "Rmd", "rss", "slnx", "svg", "svelte", "textile", "ui", "vbproj", "vue", "xhtml", "xml", "xsl",
];

pub static JINJA_EXTS: &[&str; 3] = &["j2", "jinja", "jinja2"];

pub static JULIA_EXTS: &[&str; 2] = &["jl", "julia"];

pub static LEAN_EXTS: &[&str; 1] = &["lean"];

pub static LEGACY_FORTRAN_EXTS: &[&str; 4] = &["f", "for", "fpp", "ftn"];

pub static LISP_EXTS: &[&str; 15] = &[
    "asm", "cl", "clj", "cljc", "cljs", "el", "fnl", "l", "lisp", "lsp", "rkt", "scm", "sld",
    "sls", "sps",
];

pub static M4_EXTS: &[&str; 1] = &["m4"];

pub static ML_EXTS: &[&str; 2] = &["ml", "mli"];

pub static MODERN_FORTRAN_EXTS: &[&str; 5] = &["f90", "f95", "f03", "f08", "f18"];

pub static PASCAL_EXTS: &[&str; 3] = &["dpr", "lpr", "pas"];

pub static PLANTUML_EXTS: &[&str; 4] = &["iuml", "pu", "puml", "plantuml"];

pub static RESTRUCTURED_TEXT_EXTS: &[&str; 3] = &["rst", "rest", "restx"];

pub static SEMICOLON_EXTS: &[&str; 3] = &["ahk", "ahkl", "ini"];

pub static TEX_EXTS: &[&str; 13] = &[
    "aux", "cls", "erl", "escript", "es", "hrl", "latex", "m", "sty", "tex", "toc", "xrl", "yrl",
];

pub static UNIX_MANUAL_EXTS: &[&str; 10] = &["1", "2", "3", "4", "5", "6", "7", "8", "9", "man"];

pub static VELOCITY_EXTS: &[&str; 2] = &["vm", "vtl"];

pub static VIM_EXTS: &[&str; 2] = &["vim", "nvim"];

pub static XQUERY_EXTS: &[&str; 5] = &["xq", "xql", "xqm", "xqy", "xquery"];
