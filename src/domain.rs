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
    /// Language-specific first-line requirements
    /// BibTeX
    BibTex,
    /// PHP
    PhpTag,
    /// Haskell
    HaskellCabal,
    /// TeX
    Tex,
}

/// One parsed in-file SPDX header occurrence (data-model §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderBlock {
    /// Byte span `[start, end)` of the block within the file (enables targeting, FR-008).
    pub byte_range: (usize, usize),
    /// License identifiers/expressions declared in this block.
    pub license_ids: Vec<String>,
    /// Value byte spans `[start, end)` of each entry in [`Self::license_ids`],
    /// parallel to it: `license_spans[i]` is exactly the SPDX value bytes that
    /// produced `license_ids[i]` (leading whitespace and trailing comment
    /// closers excluded). Reconciliation edits these spans instead of
    /// re-deriving positions from rendered text, so trailing code on the same
    /// line is never reparsed or touched (FR-007).
    pub license_spans: Vec<(usize, usize)>,
    /// `SPDX-FileCopyrightText` lines found in this block.
    pub copyrights: Vec<String>,
    /// Value byte spans `[start, end)` of each entry in [`Self::copyrights`],
    /// parallel to it. Copyright values are never validated as SPDX (any text
    /// may follow the marker), so spans additionally bound what a copyright
    /// replacement may touch (FR-007, FR-009).
    pub copyright_spans: Vec<(usize, usize)>,
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

/// How an out-of-band annotation combines with file-level licensing info, per the
/// REUSE 3.3 `precedence` field (FR-003a, data-model §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Precedence {
    /// File-level info (in-file header or its `.license` sidecar) wins; the
    /// annotation is only a fallback. The REUSE 3.3 default.
    #[default]
    Closest,
    /// The annotation's info is always associated, then `closest` logic applies —
    /// effectively the union of annotation and file-level info.
    Aggregate,
    /// The annotation wins and any file-level info is ignored.
    Override,
}

impl Precedence {
    /// Lower-snake string used in configuration and JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Precedence::Closest => "closest",
            Precedence::Aggregate => "aggregate",
            Precedence::Override => "override",
        }
    }
}

/// Provenance of one metadata table contributing to a file's effective
/// licensing: which document, which table, and what it contributed (FR-003a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataOrigin {
    /// Repo-relative path of the metadata document (`REUSE.toml`,
    /// `sub/REUSE.toml`, or `.reuse/dep5`).
    pub metadata_path: PathBuf,
    /// `[[annotations]]` (or dep5 paragraph) index within the document.
    pub table_index: usize,
    /// The table's own precedence (`dep5` is always `Aggregate`).
    pub precedence: Precedence,
    /// License expressions contributed by this table (validated, raw form).
    pub licenses: Vec<String>,
    /// Copyright notices contributed by this table.
    pub copyrights: Vec<String>,
}

/// License/copyright covering a path from out-of-band sources, already
/// resolved across the whole `REUSE.toml` hierarchy (data-model §5).
///
/// Resolution mirrors the reference REUSE tool: documents are consulted from
/// the project root toward the file and stop after the first (`rootmost`)
/// `override` table; `aggregate` tables always contribute; `closest` tables
/// are a per-field fallback used only when file-level info lacks that field
/// (and, when the file carries exactly one of the two fields, supply the
/// other). `.reuse/dep5` paragraphs aggregate like any other table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutOfBandEntry {
    /// Source of the primary contributor (first override/aggregate table, or
    /// the fallback table when only `closest` tables matched).
    pub source: OobSource,
    /// Effective unconditional OOB licenses: the barrier table plus every
    /// consulted `aggregate` table. Empty when no table supplied a license.
    pub licenses: Vec<String>,
    /// Effective unconditional OOB copyrights (same contributors as above).
    pub copyrights: Vec<String>,
    /// Nearest-outward `closest` fallback licenses, used only when
    /// file-level info carries no license for the path.
    pub fallback_licenses: Vec<String>,
    /// Nearest-outward `closest` fallback copyrights, used only when
    /// file-level info carries no copyright for the path.
    pub fallback_copyrights: Vec<String>,
    /// True when a `rootmost` override barrier suppresses file/sidecar info
    /// (and every deeper table) for this path.
    pub suppresses_file: bool,
    /// Governing precedence: `Override` under a barrier, else `Aggregate`
    /// when any aggregate contributor exists, else `Closest`.
    pub precedence: Precedence,
    /// Every contributing table, shallowest document first.
    pub origins: Vec<MetadataOrigin>,
}

impl OutOfBandEntry {
    /// Canonical single-value presentation of the effective OOB licenses:
    /// one expression, or the `AND`-combination of several (data-model §5).
    pub fn license(&self) -> Option<String> {
        combine_licenses(&self.licenses)
    }
}

/// Combine several license expressions into one `AND` expression, parenthesizing
/// compound operands so `MIT OR Apache-2.0` plus `CC0-1.0` reads as
/// `(MIT OR Apache-2.0) AND CC0-1.0` rather than changing meaning.
pub fn combine_licenses(exprs: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for e in exprs {
        let t = e.trim();
        if t.is_empty() {
            continue;
        }
        // Any multi-token expression is parenthesized (`WITH` binds tighter
        // than `AND`, so this is conservative but never changes meaning).
        let needs_parens = t.chars().any(char::is_whitespace);
        if needs_parens && !(t.starts_with('(') && t.ends_with(')')) {
            parts.push(format!("({t})"));
        } else {
            parts.push(t.to_string());
        }
    }
    match parts.len() {
        0 => None,
        1 => Some(parts.remove(0)),
        _ => Some(parts.join(" AND ")),
    }
}

/// How `apply` covers a file that cannot carry an in-file comment header
/// (non-annotatable / binary). Configured via `[output] non_annotatable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NonAnnotatableStrategy {
    /// Write a `<file>.license` sidecar next to the asset (default).
    #[default]
    Sidecar,
    /// Append a `[[annotations]]` block to the central `REUSE.toml`.
    ReuseToml,
}

/// Where the detected actual license was read from (report contract `actual_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActualSource {
    Header,
    /// A `<file>.license` sidecar (counts as file-level info per the REUSE spec).
    Sidecar,
    ReuseToml,
    Dep5,
}

impl ActualSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActualSource::Header => "header",
            ActualSource::Sidecar => "license_file",
            ActualSource::ReuseToml => "reuse_toml",
            ActualSource::Dep5 => "dep5",
        }
    }
}

/// One rejected `SPDX-License-Identifier` value: kept for diagnosis, never silently
/// dropped (F13). The line is 1-based nearest-line; columns are deliberately not
/// claimed (byte offsets shift under multibyte text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidLicenseValue {
    /// 1-based nearest line number of the tag.
    pub line: usize,
    /// The offending tag value verbatim.
    pub value: String,
    /// Why it was rejected (dependency parse error, summarized).
    pub reason: String,
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
    /// Licenses declared for in-file SPDX snippets (`SPDX-SnippetBegin`..`SPDX-SnippetEnd`).
    /// These describe snippets, not the file, so they never satisfy declared
    /// policy drift — but their texts are still referenced for `LICENSES/`
    /// completeness (FR-030), and REUSE validation counts them like the
    /// reference tool does.
    pub snippet_licenses: Vec<String>,
    /// Copyright notices declared inside SPDX snippet regions. Like snippet
    /// licenses, these never satisfy declared policy, but REUSE validation
    /// counts them toward the file's copyright requirement (reference parity).
    pub snippet_copyrights: Vec<String>,
    /// False when the file is not valid UTF-8 (drives `Unreadable` — FR-025).
    pub encoding_ok: bool,
    /// Rejected license values with their location (diagnosed, never dropped).
    pub invalid_license_values: Vec<InvalidLicenseValue>,
}

/// Exhaustive, mutually-exclusive drift classification (FR-004, data-model §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftClass {
    /// Declared intent satisfied.
    Compliant,
    /// A header exists but the license differs from intent.
    WrongLicense { declared: String, actual: String },
    /// The license matches but the copyright policy is unsatisfied.
    CopyrightMismatch { declared: String, actual: String },
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
            DriftClass::CopyrightMismatch { .. } => "copyright_mismatch",
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

/// Which content a scan evaluates: the working tree or the Git index (F06).
/// Carried through every dependent read (source bytes, sidecars, metadata,
/// configuration, license texts) so a snapshot never mixes the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSource {
    /// Current working-tree bytes.
    Worktree,
    /// Git index blobs (`check --staged`).
    Index,
}

impl ContentSource {
    /// Lower-snake label used in report `snapshot` metadata.
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentSource::Worktree => "worktree",
            ContentSource::Index => "index",
        }
    }
}

/// Normalize a copyright notice for comparison: trim and collapse every
/// whitespace run to a single space, so `2026   Acme` and `2026 Acme` compare
/// equal without changing what is written (FR-009).
pub fn normalize_copyright_notice(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether effective copyright notices satisfy a policy: `preserve` imposes no
/// requirement; `add` needs existing notices plus the requested normalized
/// notice; `replace` needs exactly the requested normalized notice.
pub fn copyright_policy_satisfied(policy: &CopyrightPolicy, effective: &[String]) -> bool {
    match policy {
        CopyrightPolicy::Preserve => true,
        CopyrightPolicy::PreserveAndAdd(want) => {
            let want = normalize_copyright_notice(want);
            !effective.is_empty()
                && effective
                    .iter()
                    .any(|c| normalize_copyright_notice(c) == want)
        }
        CopyrightPolicy::Replace(want) => {
            let want = normalize_copyright_notice(want);
            let mut have: Vec<String> = effective
                .iter()
                .map(|c| normalize_copyright_notice(c))
                .collect();
            have.sort();
            have == vec![want]
        }
    }
}

/// Whether two intents are identical: semantically equal license expressions
/// and equal copyright policies over normalized text. Equal-specificity rules
/// with differing full intent are a conflict; identical intent resolves to
/// earliest declaration order (FR-002, FR-022).
pub fn intents_equal(a: &LicenseIntent, b: &LicenseIntent) -> bool {
    if !crate::spdx::expressions_equal(&a.license_expression, &b.license_expression) {
        return false;
    }
    match (&a.copyright_policy, &b.copyright_policy) {
        (CopyrightPolicy::Preserve, CopyrightPolicy::Preserve) => true,
        (CopyrightPolicy::PreserveAndAdd(x), CopyrightPolicy::PreserveAndAdd(y))
        | (CopyrightPolicy::Replace(x), CopyrightPolicy::Replace(y)) => {
            normalize_copyright_notice(x) == normalize_copyright_notice(y)
        }
        _ => false,
    }
}

/// A retained copyright mismatch: the license matched but the copyright policy
/// did not (primary drift), or neither matched (kept as a diagnostic next to
/// `WrongLicense`, data-model §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyrightDrift {
    /// The policy requirement, e.g. `add:2026 Acme`.
    pub declared: String,
    /// Effective notices joined with `; `, or `<none>`.
    pub actual: String,
}

impl CopyrightDrift {
    pub fn new(policy: &CopyrightPolicy, effective: &[String]) -> Self {
        let declared = match policy {
            CopyrightPolicy::Preserve => "preserve".to_string(),
            CopyrightPolicy::PreserveAndAdd(t) => format!("add:{t}"),
            CopyrightPolicy::Replace(t) => format!("replace:{t}"),
        };
        let actual = if effective.is_empty() {
            "<none>".to_string()
        } else {
            effective.join("; ")
        };
        CopyrightDrift { declared, actual }
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
    /// Copyright mismatch detail: primary when the license matched but the
    /// copyright policy did not, diagnostic when both differ.
    pub copyright_drift: Option<CopyrightDrift>,
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

/// What a planned write mutates (report contract v2, FR-021).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    /// An in-file source edit (header insert/replace/append).
    Source,
    /// A `<file>.license` sidecar body.
    Sidecar,
    /// A `REUSE.toml` document patch (one or more exact-path stanzas).
    ReuseToml,
    /// A `LICENSES/<id>.txt` text install.
    LicenseText,
    /// A generated configuration file (`init`, task 8).
    Config,
}

impl WriteKind {
    /// Lower-snake string used in JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteKind::Source => "source",
            WriteKind::Sidecar => "sidecar",
            WriteKind::ReuseToml => "reuse_toml",
            WriteKind::LicenseText => "license_text",
            WriteKind::Config => "config",
        }
    }
}

/// One concrete filesystem mutation, independent of file states: the same
/// records drive dry-run previews and real execution (FR-021). Byte buffers
/// stay internal; JSON serializes text/diffs only for text being written.
#[derive(Debug, Clone)]
pub struct PlannedWrite {
    /// Destination written (the document itself for metadata patches).
    pub path: PathBuf,
    pub kind: WriteKind,
    /// Expected current bytes (`None` = the destination must not exist).
    /// Evaluated before every write; a mismatch blocks it.
    pub before: Option<Vec<u8>>,
    /// Bytes to install (`None` for blocked writes with no known result,
    /// e.g. a fetch that never ran).
    pub after: Option<Vec<u8>>,
    /// Selected files this write covers (the assets behind a metadata patch).
    pub affected_files: Vec<PathBuf>,
}

/// Outcome of one planned write (report contract v2, FR-021).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStatus {
    /// Previewed but not executed (dry-run).
    Planned,
    /// Bytes committed.
    Applied,
    /// Evaluated and left alone (converged rerun — normally absent, since
    /// converged runs emit no write records at all).
    Unchanged,
    /// Attempted and failed; `message` says why.
    Failed,
    /// Refused before any attempt (a missing custom text, an unpermitted
    /// fetch); `message` names the requirement.
    Blocked,
}

impl WriteStatus {
    /// Lower-snake string used in JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteStatus::Planned => "planned",
            WriteStatus::Applied => "applied",
            WriteStatus::Unchanged => "unchanged",
            WriteStatus::Failed => "failed",
            WriteStatus::Blocked => "blocked",
        }
    }
}

/// One executed (or refused) write plus its outcome for the report.
#[derive(Debug, Clone)]
pub struct ExecutedWrite {
    pub write: PlannedWrite,
    pub status: WriteStatus,
    /// Human-readable reason for `Failed`/`Blocked` (fetch URL, missing id…).
    pub message: Option<String>,
    /// True when the replacement bytes were committed even though the outcome
    /// is otherwise a failure (durability sync after a successful rename):
    /// such a write counts as a change for partial-result accounting.
    pub replacement_completed: bool,
}

impl ExecutedWrite {
    /// A previewed-but-unexecuted write (dry-run).
    pub fn planned(write: PlannedWrite) -> Self {
        ExecutedWrite {
            write,
            status: WriteStatus::Planned,
            message: None,
            replacement_completed: false,
        }
    }

    /// A refused write (missing text, unpermitted fetch).
    pub fn blocked(write: PlannedWrite, message: impl Into<String>) -> Self {
        ExecutedWrite {
            write,
            status: WriteStatus::Blocked,
            message: Some(message.into()),
            replacement_completed: false,
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
