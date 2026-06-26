//! Core domain types mirroring `data-model.md` §1–§6. Language-agnostic engine
//! vocabulary shared by config, rules, detection, classification, and reconciliation.

use std::path::PathBuf;

use smol_str::SmolStr;

/// A matcher key for a rule or comment-style association (data-model §2).
///
/// Specificity ordering (most → least specific): exact path/filename > glob > extension.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Selector {
    /// Bare extension, e.g. `rs`, `pkl` (no leading dot).
    Extension(String),
    /// Glob pattern, e.g. `examples/**/*.rs`.
    Glob(String),
    /// Exact repo-relative path, e.g. `examples/demo.rs`.
    ExactPath(String),
    /// Exact file name regardless of directory, e.g. `hk.pkl`.
    Filename(String),
}

impl Selector {
    /// Derived specificity rank (higher = more specific) for precedence (FR-002).
    pub fn specificity(&self) -> u32 {
        match self {
            Selector::ExactPath(_) => 40,
            Selector::Filename(_) => 30,
            Selector::Glob(_) => 20,
            Selector::Extension(_) => 10,
        }
    }

    /// Stable human/JSON identifier for the selector (used in reports and `--explain`).
    pub fn label(&self) -> String {
        match self {
            Selector::Extension(e) => format!("ext={e}"),
            Selector::Glob(g) => format!("glob={g}"),
            Selector::ExactPath(p) => format!("path={p}"),
            Selector::Filename(f) => format!("file={f}"),
        }
    }
}

/// How copyright/authorship lines are treated by a license change (data-model §3).
///
/// Default is [`CopyrightPolicy::Preserve`] — copyright is never erased by a license
/// change (FR-009, SC-004).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CopyrightPolicy {
    /// Keep existing copyright lines untouched.
    #[default]
    Preserve,
    /// Keep existing copyright lines and add the given text if absent.
    PreserveAndAdd(String),
    /// Replace copyright lines with the given text.
    Replace(String),
}

/// The licensing outcome a rule or default confers on a file (data-model §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseIntent {
    /// SPDX expression, e.g. `MIT OR Apache-2.0`, or a `LicenseRef-*`.
    pub license_expression: String,
    /// Copyright handling policy.
    pub copyright_policy: CopyrightPolicy,
}

/// Line-comment syntax, e.g. `//`, `#`, `;` (data-model §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineStyle {
    /// Prefix opening a line comment.
    pub prefix: SmolStr,
}

/// Block-comment syntax, e.g. `/* … */`, `<!-- … -->` (data-model §4).
///
/// `line_prefix` is the **internal alignment** prefix applied to each content line
/// (e.g. ` * ` for a C block); empty when the block carries no per-line decoration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStyle {
    /// Block opener, e.g. `/*`, `<!--`, `(*`.
    pub open: SmolStr,
    /// Block closer, e.g. `*/`, `-->`, `*)`.
    pub close: SmolStr,
    /// Per-line alignment prefix inside the block (empty = none).
    pub line_prefix: SmolStr,
}

/// What comment forms a language supports (data-model §4).
///
/// A sum type so "line-only" / "block-only" / "both" are exhaustive and illegal
/// states — neither form available, or a half-specified block (`open` without
/// `close`) — are unrepresentable. The render side prefers [`LineStyle`] when
/// present (REUSE convention is a single `# SPDX-License-Identifier:` line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentSyntax {
    LineOnly(LineStyle),
    BlockOnly(BlockStyle),
    Both { line: LineStyle, block: BlockStyle },
}

impl CommentSyntax {
    /// Line syntax, if this language supports line comments.
    pub fn line(&self) -> Option<&LineStyle> {
        match self {
            CommentSyntax::LineOnly(l) | CommentSyntax::Both { line: l, .. } => Some(l),
            CommentSyntax::BlockOnly(_) => None,
        }
    }

    /// Block syntax, if this language supports block comments.
    pub fn block(&self) -> Option<&BlockStyle> {
        match self {
            CommentSyntax::BlockOnly(b) | CommentSyntax::Both { block: b, .. } => Some(b),
            CommentSyntax::LineOnly(_) => None,
        }
    }

    /// Const line-only style (terse table authoring). `prefix` must be ≤ 23 bytes.
    pub const fn line_only(prefix: &'static str) -> Self {
        CommentSyntax::LineOnly(LineStyle {
            prefix: SmolStr::new_inline(prefix),
        })
    }

    /// Const block-only style. Each token must be ≤ 23 bytes.
    pub const fn block_only(
        open: &'static str,
        close: &'static str,
        line_prefix: &'static str,
    ) -> Self {
        CommentSyntax::BlockOnly(BlockStyle {
            open: SmolStr::new_inline(open),
            close: SmolStr::new_inline(close),
            line_prefix: SmolStr::new_inline(line_prefix),
        })
    }

    /// Const "both" style for C-like languages. Each token must be ≤ 23 bytes.
    pub const fn both(
        line: &'static str,
        open: &'static str,
        close: &'static str,
        line_prefix: &'static str,
    ) -> Self {
        CommentSyntax::Both {
            line: LineStyle {
                prefix: SmolStr::new_inline(line),
            },
            block: BlockStyle {
                open: SmolStr::new_inline(open),
                close: SmolStr::new_inline(close),
                line_prefix: SmolStr::new_inline(line_prefix),
            },
        }
    }
}

/// One row of the built-in comment registry (data-model §4).
///
/// Identity slices stay `&'static` because languages are never authored at
/// runtime — user customization flows through config [`Selector`]s, which lower
/// into a [`CommentSyntax`] directly. Only the rendering payload needs to be both
/// const-constructible and runtime-ownable (hence [`SmolStr`], not `&'static str`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Human-facing family label, e.g. `"C-style"`, `"hash"` (diagnostics).
    pub family: &'static str,
    /// File extensions (no leading dot) this row covers.
    pub extensions: &'static [&'static str],
    /// Exact filenames this row covers, e.g. `Makefile`, `Dockerfile`.
    pub filenames: &'static [&'static str],
    /// Names usable from config `style = "..."`, e.g. `c`, `hash`, `slashes`.
    pub aliases: &'static [&'static str],
    /// Comment syntax this family renders/parses.
    pub syntax: CommentSyntax,
}

/// First-line context controlling safe header insertion (data-model §5, FR-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionAfter {
    /// Insert at the very top of the file.
    FileStart,
    /// Insert after a `#!...` shebang line.
    Shebang,
    /// Insert after an encoding/XML declaration.
    EncodingDecl,
    /// Insert after a byte-order mark.
    Bom,
}

/// One parsed in-file SPDX header occurrence (data-model §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderBlock {
    /// Byte span `[start, end)` of the block within the file (enables targeting, FR-008).
    pub byte_range: (usize, usize),
    /// License identifiers/expressions declared in this block.
    pub license_ids: Vec<String>,
    /// `SPDX-FileCopyrightText` lines found in this block.
    pub copyrights: Vec<String>,
    /// First-line context preceding the block.
    pub position_after: PositionAfter,
}

/// Where an out-of-band entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobSource {
    /// `REUSE.toml` (current spec).
    ReuseToml,
    /// `.reuse/dep5` (legacy).
    Dep5,
}

/// License/copyright covering a path from an out-of-band source (data-model §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutOfBandEntry {
    pub source: OobSource,
    pub license: Option<String>,
    pub copyrights: Vec<String>,
}

/// Where the detected actual license was read from (report contract `actual_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActualSource {
    Header,
    ReuseToml,
    Dep5,
}

impl ActualSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActualSource::Header => "header",
            ActualSource::ReuseToml => "reuse_toml",
            ActualSource::Dep5 => "dep5",
        }
    }
}

/// Everything detection found for a file (data-model §5, FR-003a).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActualLicenseState {
    /// Every parsed in-file header occurrence (not just the first — FR-008).
    pub headers: Vec<HeaderBlock>,
    /// Out-of-band coverage from `REUSE.toml`/`.reuse/dep5`, if any.
    pub out_of_band: Option<OutOfBandEntry>,
    /// Canonical detected license expression (header and/or out-of-band).
    pub detected_license: Option<String>,
    /// Source of the detected license (out-of-band wins on disagreement — FR-003a).
    pub detected_source: Option<ActualSource>,
    /// All copyright lines found across sources.
    pub detected_copyrights: Vec<String>,
    /// False when the file is not valid UTF-8 (drives `Unreadable` — FR-025).
    pub encoding_ok: bool,
}

/// Exhaustive, mutually-exclusive drift classification (FR-004, data-model §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftClass {
    /// Declared intent satisfied.
    Compliant,
    /// A header exists but the license differs from intent.
    WrongLicense { declared: String, actual: String },
    /// Covered by intent but no header/out-of-band license present.
    MissingHeader,
    /// No rule, no default — not covered by any intent.
    Uncovered,
    /// Explicitly excluded from coverage (FR-016).
    Excluded,
    /// Cannot be safely parsed or written, e.g. non-UTF-8 (FR-025).
    Unreadable,
}

impl DriftClass {
    /// Lower-snake string used in JSON output (report.schema.json `drift` enum).
    pub fn as_str(&self) -> &'static str {
        match self {
            DriftClass::Compliant => "compliant",
            DriftClass::WrongLicense { .. } => "wrong_license",
            DriftClass::MissingHeader => "missing_header",
            DriftClass::Uncovered => "uncovered",
            DriftClass::Excluded => "excluded",
            DriftClass::Unreadable => "unreadable",
        }
    }

    /// Whether this class fails the gate (FR-012a, FR-025). `Excluded`/`Compliant` pass.
    pub fn is_failure(&self) -> bool {
        !matches!(self, DriftClass::Compliant | DriftClass::Excluded)
    }
}

/// An unresolved equal-specificity rule conflict on one file (FR-022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleConflict {
    /// Labels of the rules that tied.
    pub rules: Vec<String>,
    /// Human-readable explanation.
    pub message: String,
}

/// Per-file join of declared intent vs detected actual (data-model §6).
#[derive(Debug, Clone)]
pub struct FileLicensingState {
    pub path: PathBuf,
    /// Label of the winning rule, or `None` if default/uncovered.
    pub matched_rule: Option<String>,
    pub declared_intent: Option<LicenseIntent>,
    pub actual: ActualLicenseState,
    pub drift: DriftClass,
    pub conflict: Option<RuleConflict>,
}

/// Additive vs destructive reconciliation mode (FR-007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeMode {
    /// Append the declared id without removing existing ones (opt-in).
    Additive,
    /// Replace the license id to match config (default).
    Destructive,
}

impl ChangeMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChangeMode::Additive => "additive",
            ChangeMode::Destructive => "destructive",
        }
    }
}

/// Record of a single file's reconciliation (data-model §8).
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub mode: ChangeMode,
    /// Which header block index was replaced when multiple exist (FR-008).
    pub target_header: Option<usize>,
    /// Whether a header was written/inserted.
    pub wrote_header: bool,
    /// Count of copyright lines preserved across the change (SC-004).
    pub preserved_copyrights: usize,
    /// False on dry-run or when this file's write failed (FR-021).
    pub applied: bool,
}
