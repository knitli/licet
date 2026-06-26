# Feature Specification: Declarative License Header Management

**Feature Branch**: `001-declarative-license-headers`

**Created**: 2026-06-24

**Status**: Draft

**Input**: User description: "I use Reuse (the REUSE specification and python tool) with all of my projects. I like the *idea* of it... but there are some things I don't like about it: 1) It doesn't work declaratively... 2) It doesn't enforce the lines... 3) It's slow... 4) It supports a few dozen comment styles for file types it knows but doesn't let you define new associations... 5) It's not clear how to destructively vs. additively annotate a file..."

## Overview

A licensing tool that manages SPDX/REUSE-style license and copyright information for every file in a repository, driven by a **single declarative configuration** rather than per-file manual annotation. The configuration expresses intent ("Rust source is license A, except under `vendor/` where it is license B; everything else defaults to license C"), and the tool can **project that intent onto the repository**, **report drift** between intent and reality, and **reconcile** files to match — while remaining output-compatible with the REUSE specification so existing REUSE tooling, CI, and consumers continue to work.

This is a superset/replacement for the day-to-day workflow of the existing REUSE tool, targeting the same end-state (compliant headers + a `LICENSES/` tree) but solving the gaps the user identified: declarativeness, enforcement, speed, extensible comment styles, and precise additive-vs-destructive header control.

## Clarifications

### Session 2026-06-24

- Q: When `apply` runs without an explicitly stated mode, should reconciliation default to additive or destructive? → A: Destructive by default for the license identifier (replace to match config); copyright/authorship is always preserved; additive is opt-in.
- Q: What is the source of truth for a file's "actual" license when detecting drift? → A: Both in-file SPDX headers and out-of-band entries (`REUSE.toml`/`.reuse/dep5`); either satisfies intent.
- Q: For the CI/pre-commit gate, does an "uncovered" file (matched by no rule, exclusion, or default) fail the gate? → A: Yes — uncovered counts as a failure; the gate fails until every file is covered by a rule, an explicit exclusion, or the default.
- Q: How does the tool acquire a referenced-but-missing license text? → A: Bundle standard SPDX license texts and supply them offline by default; reach the network only for identifiers not in the bundle, and only when explicitly enabled. Offline by default (hermetic CI).
- Q: What is the concrete full-repository performance target? → A: A full scan of a ~10,000-file repository completes in under 1 second (warm cache).

### Session 2026-06-24 (specification review panel)

- Q: CI checks out fresh, so the warm-cache target doesn't apply there. What is the cold-scan performance bar? → A: A full **cold** scan (empty cache) of a ~10,000-file repository completes in under **3 seconds** on a 4-core 2020-era runner; the sub-1-second figure is the **warm-cache** (repeat/local) target.
- Q: When a file's in-file SPDX header and an out-of-band entry (`REUSE.toml`/`.reuse/dep5`) disagree on the license, which is authoritative for detection? → A: It depends on the annotation's REUSE 3.3 **`precedence`** field. `closest` (the spec **default**) makes file-level info — the in-file header or its `.license` sidecar — authoritative, with the annotation a fallback; `override` makes the annotation authoritative and emits a non-failing `source_override` diagnostic; `aggregate` treats both as satisfying. `.reuse/dep5` is legacy and treated as `override`. The declarative config (`license.toml`) remains the source of truth for *intent*; out-of-band is read for interop/detection only, never the authoring surface. (Note: the maintainer considers `REUSE.toml` poorly suited to declarative authoring.)
- Q: How are non-UTF-8 files (UTF-16, Latin-1, etc.), where byte-offset insertion could corrupt content, handled? → A: Detect and **skip** them without writing, classify them as a gate failure (`Unreadable`), and warn. Transcoding is deferred beyond v1.
- Q: Can the file-selection flags (`--staged`, `--changed`, `--files`) be combined? → A: No — they are **mutually exclusive**; combining them is a usage error (exit 2).
- Q: What protects source files during destructive `apply`? → A: `apply` **refuses to run on a dirty working tree** unless `--allow-dirty` is passed, and every file write is **atomic** (write-temp-then-rename) so an interruption cannot leave a partially written source file.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Declare licensing rules and see drift (Priority: P1)

A maintainer writes a configuration file that declares licensing intent for the whole repository using ordered rules — by file type, by path/glob, with more-specific rules overriding less-specific ones, plus a repository-wide default. They run a **check** that projects those rules onto every tracked file and reports, per file, the **declared** license/copyright versus what is **actually** present in the file (or its associated metadata), highlighting every file that drifts in either direction.

**Why this priority**: This is the core differentiator and the minimum viable product. Even with no automatic fixing, a maintainer who can express intent once and get an authoritative, repository-wide drift report has already solved the two biggest pain points (no declarative model, no enforcement). It is independently valuable as a read-only audit.

**Independent Test**: Author a config with a default rule and at least one override rule, run the check against a repo containing both compliant and drifted files, and confirm the report correctly classifies each file (compliant / drifted-wrong-license / missing-header / not-covered-by-any-rule).

**Acceptance Scenarios**:

1. **Given** a config that declares `*.rs` as `LicenseRef-MarqueLicense-1.0` via an extension rule, overrides `*.rs` under `examples/` to `MIT OR Apache-2.0`, and sets a repository-wide default of `MIT OR Apache-2.0`, **When** the maintainer runs the check, **Then** a Rust file in `examples/` carrying `LicenseRef-MarqueLicense-1.0` is reported as drifted with both the declared and actual identifiers shown.
2. **Given** a file matched by a rule but containing no license information at all, **When** the check runs, **Then** the file is reported as non-compliant (missing) rather than silently ignored.
3. **Given** a file that no rule matches and for which no default applies, **When** the check runs, **Then** the file is reported as "uncovered" so the maintainer can decide whether to extend the rules.
4. **Given** a fully compliant repository, **When** the check runs, **Then** it reports zero drift and signals success.

---

### User Story 2 - Reconcile files to declared intent (Priority: P2)

The maintainer runs an **apply** operation that brings drifted files into compliance with the declared configuration: writing missing headers, and correcting headers whose license disagrees with intent. The maintainer controls whether reconciliation is **additive** (add the declared header while leaving existing annotations in place) or **destructive** (replace conflicting header content with the declared header), and can target which existing header content is replaced versus preserved.

**Why this priority**: This is the enforcement teeth that stops copy-paste drift. P1 tells you what's wrong; P2 fixes it consistently from a single source of truth, eliminating the "look at another file, paste the header, call it done" failure mode.

**Independent Test**: Take the drifted repo from Story 1, run apply, and confirm every previously drifted file now matches its declared intent, that a re-run of the check reports zero drift, and that additive vs destructive mode produces the documented difference on a file that already has a conflicting header.

**Acceptance Scenarios**:

1. **Given** a Rust file under `examples/` declared as `MIT OR Apache-2.0` but carrying `LicenseRef-MarqueLicense-1.0`, **When** apply runs in destructive mode, **Then** the license identifier is replaced so the file now declares `MIT OR Apache-2.0` and the subsequent check passes.
2. **Given** the same file, **When** apply runs in additive mode, **Then** the declared header is added without removing the prior license line, and the maintainer is warned that the file now carries conflicting licenses.
3. **Given** a file with a license header AND copyright/authorship lines, **When** apply replaces the license, **Then** the copyright/authorship lines are preserved by default (only the targeted license content is changed).
4. **Given** a file with multiple existing headers, **When** apply runs, **Then** the maintainer can target a specific header for replacement rather than only the first one encountered.
5. **Given** a check run in a non-writing mode, **When** it finds drift, **Then** no file is modified and the operation reports a non-success result suitable for gating.

---

### User Story 3 - Define comment styles for unknown file types (Priority: P3)

The maintainer associates a comment style with file types the tool would not otherwise recognize (for example, "use C-style block comments for `hk.pkl` and all `*.pkl` files"). The association is stored in configuration so it persists: subsequent checks and applies read and write headers in that file's correct comment syntax automatically, with no per-file flags.

**Why this priority**: Without this, files like `hk.pkl` either can't be annotated or require fragile one-off flags that aren't remembered, leaving permanent gaps in enforcement. It's gated behind P1/P2 because it extends, rather than defines, the core workflow.

**Independent Test**: Add a comment-style association for a previously unknown extension, run apply on a file of that type, and confirm the header is written using the configured comment syntax and is then recognized (round-trips) on the next check.

**Acceptance Scenarios**:

1. **Given** a config that maps `*.pkl` to a C-style comment style, **When** apply annotates an `hk.pkl` file, **Then** the header is written using that comment syntax.
2. **Given** that same file on a later run, **When** the check reads it, **Then** the previously written header is recognized correctly (the association persists; no per-file flag is needed).
3. **Given** a file type the tool already recognizes, **When** no override is configured, **Then** its built-in comment style continues to be used.
4. **Given** a comment-style association by exact filename (not just extension), **When** that file is processed, **Then** the filename-specific style takes precedence over an extension-based one.

---

### User Story 4 - Enforce in pre-commit and CI without slowing the workflow (Priority: P4)

The maintainer wires the check into a commit hook and CI so that drift blocks the change. The check is fast enough to run on every commit without being a noticeable bump, supports operating on only the changed/staged subset of files, and returns a clear pass/fail result with actionable output.

**Why this priority**: The user's whole motivation is enforcing intent automatically; a gate that's too slow gets disabled. Speed and a clean exit-code contract turn the declarative model into an always-on guarantee. It depends on P1 (the check) existing.

**Independent Test**: Run the check over a representative repository and over a small changed subset, confirm it completes within the performance target and returns the correct success/failure result for compliant and drifted inputs respectively.

**Acceptance Scenarios**:

1. **Given** a staged change that introduces a drifted file, **When** the commit hook runs the check on the staged set, **Then** the commit is blocked with output naming the offending file(s) and the declared-vs-actual difference.
2. **Given** a compliant staged change, **When** the hook runs, **Then** it passes quickly and does not impede the commit.
3. **Given** a full-repository check in CI, **When** it runs, **Then** it completes within the performance target defined in Success Criteria and returns a single authoritative pass/fail.
4. **Given** the check is asked to operate only on a provided list of files, **When** it runs, **Then** it evaluates only those files against the applicable rules.
5. **Given** a *contributor* (not the config maintainer) whose commit is blocked because a file they edited drifted, **When** the gate fails, **Then** the output identifies the offending file(s), shows declared-vs-actual, and states the remediation command (e.g. run `apply` then re-commit) so the contributor can self-serve without understanding the full rule set.

**Note on actors**: although the *maintainer* authors the configuration, the person most often blocked by this gate is a *contributor* who touched a file incidentally. The gate's output is therefore written to be actionable by someone who did not write the rules.

---

### User Story 5 - Stay REUSE/SPDX compatible and migrate from an existing setup (Priority: P5)

The tool produces output that conforms to the REUSE specification (SPDX identifiers in file headers, a `LICENSES/` directory holding referenced license texts, and a recognized fallback for non-annotatable files), so existing REUSE consumers and compliance checks keep working. A maintainer with an existing REUSE project (including `REUSE.toml`) can bootstrap the declarative configuration from the current state rather than starting from scratch.

**Why this priority**: Compatibility protects the user's investment in REUSE's ecosystem and the "idea" they like; migration lowers adoption cost. It is last because the core value (P1–P4) is realizable before perfect interop and import are complete.

**Independent Test**: Run the tool on an existing REUSE-compliant repository, confirm its output still passes a standard REUSE compliance check, and confirm a bootstrap step can derive an initial config that reproduces the repository's current licensing intent.

**Acceptance Scenarios**:

1. **Given** a repository the tool has reconciled, **When** a standard REUSE compliance check is run, **Then** it reports the repository as compliant.
2. **Given** an existing project with a `REUSE.toml` and annotated files, **When** the maintainer runs the bootstrap step, **Then** an initial declarative configuration is generated that, when projected, matches the project's current licensing.
3. **Given** a referenced license identifier whose text is not present, **When** the tool runs, **Then** it reports the missing license text (consistent with REUSE expectations) and can fetch or scaffold it.
4. **Given** a referenced license identifier whose standard text is bundled but absent from `LICENSES/`, **When** the maintainer runs the materialize command (`licet add-license <id>`, or `--all` for every missing text), **Then** the text is written into `LICENSES/` from the embedded bundle without contacting the network and without modifying any source file or the configuration.

---

### Edge Cases

- **Conflicting rules**: When two rules of equal specificity match the same file, the tool MUST resolve deterministically (documented precedence — e.g., declaration order) and MUST be able to surface the conflict rather than silently picking one.
- **Binary / non-annotatable files**: Files that cannot carry an inline header (images, binaries, formats without comments) MUST be coverable through a REUSE-compatible mechanism rather than byte-edited or skipped. `apply` establishes coverage by writing a `<file>.license` sidecar (default) or appending a `REUSE.toml` annotation (configurable), and the asset itself is never modified. An uncovered non-annotatable file remains a gate failure.
- **Generated / vendored / ignored content**: The configuration MUST allow excluding paths from coverage (and such exclusions MUST be distinguishable in reporting from "uncovered by accident").
- **Pre-existing non-conforming header text**: A file whose header was hand-written in an unexpected format MUST be classified as drifted (not crash the parser), and destructive apply MUST be able to normalize it.
- **Multiple/compound licenses**: Files legitimately carrying a compound expression (e.g., `MIT OR Apache-2.0`) MUST be compared as an expression, not as raw text, so equivalent expressions are treated as equal.
- **Additive mode creating contradictions**: Adding a declared header to a file that already declares a different license MUST warn that the result is internally contradictory.
- **Shebang / encoding / leading directives**: Headers MUST be inserted in the correct position relative to shebangs, encoding declarations, or other required-first lines.
- **Partial apply failure**: If reconciliation fails partway (e.g., a file becomes unwritable), the tool MUST report what changed and what didn't, leaving the repository in a describable state.
- **Symlinks and duplicate content**: The tool MUST avoid double-annotating the same underlying file reached via a symlink.
- **In-file vs out-of-band disagreement**: When a file's inline header and its out-of-band entry (`REUSE.toml`/`.reuse/dep5`) declare different licenses, the out-of-band entry is authoritative for the *detected actual* license; the tool MUST emit a non-failing diagnostic recording that the inline header was overridden, rather than silently dropping the discrepancy.
- **Non-UTF-8 / unusual encodings**: Files that are not valid UTF-8 (e.g. UTF-16, Latin-1) MUST NOT be modified by byte-offset insertion (which would corrupt them); the tool MUST detect them, skip writing, classify them as `Unreadable`, and surface a warning. `Unreadable` counts as a gate failure.
- **Line-ending preservation**: When writing a header the tool MUST preserve the file's existing newline convention (LF vs CRLF) so reconciliation does not produce whole-file diffs or mixed endings.
- **Destructive apply safety**: `apply` MUST refuse to modify files when the working tree has uncommitted changes unless `--allow-dirty` is given, and each file modification MUST be atomic (write to a temp file, then rename) so an interruption cannot leave a source file partially written.
- **Stale cache after configuration change**: The scan cache MUST be keyed so that changing the configuration, rule set, comment-style registry, or tool version invalidates affected entries; a cached result MUST NOT survive a change that would alter a file's classification.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST allow licensing intent to be expressed in a single declarative configuration using ordered rules that match files by file type/extension, by path/glob, and by exact filename, plus a repository-wide default.
- **FR-002**: The system MUST resolve overlapping rules deterministically, with more-specific matches overriding less-specific ones, and MUST document the precedence model.
- **FR-003**: The system MUST project the declared configuration onto every covered file to compute that file's intended license (and copyright handling) without modifying any file.
- **FR-003a**: When detecting a file's actual license, the system MUST consider in-file SPDX headers, `<file>.license` sidecars, and out-of-band REUSE entries (`REUSE.toml` / `.reuse/dep5`); licensing information from any of these satisfies the declared intent for drift purposes. How an out-of-band annotation combines with file-level info (the in-file header or its sidecar) MUST follow the annotation's REUSE 3.3 `precedence` field: `closest` (default — file-level wins, annotation is a fallback), `aggregate` (both satisfy), or `override` (annotation wins). A sidecar is file-level and takes precedence over an in-file header. Only when an `override` annotation suppresses a disagreeing in-file header MUST the system emit a non-failing `source_override` diagnostic. `.reuse/dep5` has no `precedence` field and is treated as `override`.
- **FR-004**: The system MUST report drift per file, distinguishing at least: compliant, wrong-license (declared ≠ actual), missing header, uncovered-by-rules, explicitly-excluded, and unreadable (cannot be safely parsed or written, e.g. non-UTF-8).
- **FR-005**: The system MUST compare license expressions by parsed canonical form — equal up to operator commutativity, associativity, whitespace, parenthesization, and case-insensitive identifiers (so `Apache-2.0 OR MIT` equals `MIT OR Apache-2.0`) — rather than by raw string match. Full logical/distributive equivalence (e.g. `A AND (B OR C)` ≡ `(A AND B) OR (A AND C)`) is explicitly out of scope for v1.
- **FR-006**: The system MUST be able to reconcile (apply) files so their headers match declared intent, including writing missing headers and correcting incorrect ones.
- **FR-007**: The system MUST support both additive reconciliation (add declared content, retain existing) and destructive reconciliation (replace conflicting content with declared content), selectable by the maintainer. When no mode is specified, `apply` MUST default to destructive reconciliation of the license identifier (replacing it to match the configuration); additive reconciliation MUST be explicitly opted into.
- **FR-008**: The system MUST allow targeting which existing header content is replaced versus preserved, including selecting a specific header among multiple rather than only the first.
- **FR-009**: The system MUST, by default, preserve copyright/authorship information when replacing license content, and MUST manage copyright additively unless explicitly told to replace it.
- **FR-010**: The system MUST allow comment-style associations to be defined in configuration for arbitrary file extensions and exact filenames, and these associations MUST persist across runs without per-file flags.
- **FR-011**: The system MUST read and write headers using the comment style applicable to each file (configured association first, then built-in defaults), and configured filename associations MUST take precedence over extension associations.
- **FR-012**: The system MUST provide a non-writing check operation that returns a clear pass/fail result suitable for gating in commit hooks and CI.
- **FR-012a**: The enforcement gate MUST treat an uncovered file (matched by no rule, no explicit exclusion, and no default) as a failure, so the gate fails until every file is covered by a rule, an explicit exclusion, or the repository default.
- **FR-013**: The system MUST support evaluating only a supplied subset of files (e.g., staged or changed files) rather than always scanning the whole repository.
- **FR-014**: The system MUST produce output conforming to the REUSE specification (SPDX header identifiers, a `LICENSES/` tree of referenced texts, and a recognized mechanism for non-annotatable files) so existing REUSE consumers remain compatible.
- **FR-015**: The system MUST cover non-annotatable/binary files via a REUSE-compatible mechanism so they are still subject to enforcement, without byte-editing the asset. `apply` writes a `<file>.license` sidecar by default, or appends a `REUSE.toml` annotation when configured (`[output] non_annotatable = "reuse-toml"`, or `--non-annotatable`). It MUST NOT write coverage for a file already covered out-of-band, and the REUSE.toml write MUST be idempotent.
- **FR-016**: The system MUST allow paths to be explicitly excluded from coverage, and MUST distinguish explicit exclusions from accidental non-coverage in reporting.
- **FR-017**: The system MUST report referenced-but-missing license texts and MUST be able to supply the missing text. It MUST ship a bundled set of standard SPDX license texts and use them offline by default (no network access required for known identifiers). Fetching a text over the network MUST be limited to identifiers absent from the bundle and MUST require explicit opt-in; custom `LicenseRef-` licenses are scaffolded as placeholders for the maintainer to fill in.
- **FR-018**: The system MUST be able to bootstrap an initial declarative configuration from an existing project's current state (including an existing `REUSE.toml` and existing headers).
- **FR-019**: The system MUST insert headers in a position that respects required-first lines such as shebangs and encoding declarations.
- **FR-020**: The system MUST warn when an operation produces an internally contradictory result (e.g., additive mode leaving two different declared licenses on one file).
- **FR-021**: The system MUST report partial-apply outcomes clearly, identifying which files were changed and which were not when reconciliation cannot complete fully.
- **FR-022**: The system MUST surface rule conflicts (equal-specificity matches) to the maintainer rather than resolving them silently and invisibly.
- **FR-023**: The system MUST key any scan cache on a fingerprint that includes file content, the effective configuration (rules, default, comment-style registry, exclusions), and the tool version, so that a cached classification is never reused when any input that could change that classification has changed. A cache hit MUST be observationally identical to a cold computation.
- **FR-024**: Destructive `apply` MUST refuse to modify files when the working tree has uncommitted changes unless explicitly overridden (`--allow-dirty`), and every file modification MUST be performed atomically (write to a temporary file, then rename over the original) so that an interruption never leaves a source file partially written.
- **FR-025**: The system MUST detect files that are not valid UTF-8 and MUST NOT attempt byte-offset header insertion on them; such files are classified `Unreadable`, reported with a warning, and counted as a gate failure. (Transcoding non-UTF-8 files is out of scope for v1.)
- **FR-026**: When writing or modifying a header the system MUST preserve the file's existing newline convention (LF vs CRLF) and MUST NOT introduce mixed line endings.
- **FR-027**: The file-selection flags (`--staged`, `--changed`, `--files`/`--files-from`/stdin) MUST be mutually exclusive; supplying more than one is a usage error (exit 2).
- **FR-028**: The system MUST report, in `--version` and in the `lint` output, the version of the embedded SPDX license list so operators can audit which license corpus a given binary carries.
- **FR-029**: The system MUST provide a standalone command to materialize referenced-but-missing license texts into the `LICENSES/` tree from the embedded bundle, without modifying any source file or the configuration. The command MUST accept either an explicit set of SPDX identifiers or an "all referenced-but-missing" mode, MUST scaffold `LicenseRef-*` identifiers as empty placeholders, and MUST honor the same offline-by-default / explicit-network-opt-in policy as FR-017 (never reaching the network for a bundled identifier). This is the offline analog of REUSE's `download`: because licet embeds the SPDX corpus, the operation is a copy from the bundle rather than a network fetch. Because it writes only under `LICENSES/`, it does not require a clean working tree.

### Key Entities *(include if data involved)*

- **Licensing Configuration**: The single declarative source of truth. Contains the ordered set of rules, the repository default, comment-style associations, and exclusions.
- **Rule**: A matcher (file type / glob / exact filename) paired with the licensing intent it confers (license expression, copyright-handling policy) and an implied or explicit specificity used for precedence.
- **Comment-Style Association**: A mapping from a file selector (extension or exact filename) to the comment syntax used to read/write that file's header.
- **File Licensing State**: For a given file — its matched rule, declared license/copyright intent, the actually-detected header content, and a drift classification.
- **License Text Inventory**: The set of license identifiers referenced anywhere in the repo and whether each referenced text is present (the `LICENSES/` tree), used for compliance and missing-text reporting.
- **Reconciliation Plan/Report**: The computed set of changes an apply would make (or a check would flag), including per-file before/after and additive-vs-destructive decisions.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A maintainer can express the licensing intent for an entire repository in one configuration and obtain a complete, per-file drift report in a single command, with no per-file manual annotation required to detect drift.
- **SC-002**: After one reconciliation pass, 100% of files covered by a rule match their declared intent, and a subsequent check reports zero drift.
- **SC-003**: The drift check correctly classifies files with zero false "compliant" results — no file that disagrees with declared intent is reported as compliant — across a representative test corpus that includes compliant, wrong-license, missing, uncovered, and excluded files.
- **SC-004**: Destructive reconciliation never destroys copyright/authorship information unless explicitly instructed; in a corpus of files containing both license and copyright lines, 100% of copyright lines survive a license-only replacement.
- **SC-005**: A maintainer can make a previously unrecognized file type (e.g., `*.pkl`) fully managed by adding one configuration entry, after which headers on those files round-trip (write, then recognize) across runs with no per-file flags.
- **SC-006**: The check runs fast enough to be unobtrusive in a commit hook: a changed-file check completes in well under one second; a full-repository scan of a ~10,000-file repository completes in under **1 second warm** (repeat/local, populated cache) and under **3 seconds cold** (empty cache, e.g. a fresh CI checkout) on a 4-core 2020-era runner — and remains comfortably faster than the existing REUSE tool on the same repository and hardware.
- **SC-007**: A repository reconciled by the tool passes a standard REUSE-specification compliance check.
- **SC-008**: An existing REUSE project can be migrated by bootstrapping a configuration from its current state such that projecting that configuration reproduces the project's existing licensing without manual rewriting of every rule. The bootstrap MUST **generalize** rather than enumerate: on the reference REUSE fixture, the generated config covers the repository using substantially fewer rules than files (target: rule count ≤ 25% of covered file count), preferring extension/glob rules over per-file exact-path rules wherever a broader rule reproduces the same projection.
- **SC-009**: The enforcement gate returns an unambiguous pass/fail result so that a drifted change is blocked and a compliant change is not, with output that names the offending files and the declared-vs-actual difference.
- **SC-010**: No source file is ever left partially written or corrupted by `apply`: under an induced failure (process killed mid-run, a file made unwritable, a non-UTF-8 file encountered), every file is either fully reconciled or untouched, and the run reports exactly which files changed and which did not (FR-021, FR-024, FR-025).
- **SC-011**: After any change to the configuration, rule set, comment-style registry, or tool version, a cached run yields the same classification a cold run would — no file is reported `Compliant` on the basis of a stale cache entry (FR-023).

## Assumptions

- **REUSE/SPDX is the interop target, not a constraint to discard**: The user values the REUSE idea and ecosystem, so the tool's persisted output (headers, `LICENSES/` tree, non-annotatable handling) is assumed to conform to the REUSE specification and SPDX identifiers, even though the authoring/enforcement workflow is new.
- **Configuration is the single source of truth**: When declared intent and a file's actual header disagree, the configuration is authoritative; reconciliation moves files toward the configuration, never the reverse.
- **License vs copyright handling differ by default**: License identifiers are managed declaratively and may be replaced; copyright/authorship lines are preserved and accumulated additively unless explicitly overridden, because authorship is not something a path-based rule should silently erase.
- **Scope of the unit of work is a single repository** working tree; the primary interface is a command-line tool suitable for direct use, commit hooks, and CI. Multi-repository orchestration is out of scope for the first version.
- **File selection defaults to tracked files**: The repository's version-controlled file set defines coverage by default; untracked/ignored files are not enforced unless configured.
- **"Fast" means non-blocking in the dev loop**: The concrete bar is "unnoticeable in a commit hook" for changed-file runs and a full scan of a ~10,000-file repository in under 1 second (warm cache), comfortably faster than the current REUSE tool (see SC-006).
- **Existing comment-style coverage is retained**: The tool starts from at least the set of file types the current REUSE tool understands and adds user-defined associations on top, rather than reimplementing fewer.
- **`REUSE.toml`/`dep5` are interop surfaces, not authoring surfaces**: out-of-band REUSE files are read for detection and compatibility (and out-of-band wins over an in-file header when the two disagree), but the maintainer never hand-authors licensing intent there; `license.toml` is the single declarative authoring surface. This reflects the maintainer's view that `REUSE.toml` is not designed for declarative intent.
- **Single-binary distribution embeds a versioned SPDX corpus**: the embedded SPDX license list is a point-in-time snapshot; its version is surfaced (FR-028) and refreshing it is a rebuild/release concern, not a runtime fetch.
