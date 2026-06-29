---
description: "Task list for Declarative License Header Management"
---

# Tasks: Declarative License Header Management

**Input**: Design documents from `/specs/001-declarative-license-headers/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/ (cli.md, config-schema.md, report.schema.json), quickstart.md

**Tests**: INCLUDED. The plan's Testing section mandates per-story integration suites, REUSE conformance (SC-007), a 10k-file perf gate (SC-006), and `insta` snapshots — so test tasks are first-class here.

**Organization**: Tasks are grouped by user story (P1–P5) so each story is independently implementable and testable.

**Binary/working names**: binary `licet`, config `license.toml` (per contracts).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: US1–US5; Setup/Foundational/Polish carry no story label
- File paths are relative to the repository root

## Path Conventions

Single Rust project: library core in `src/`, integration/conformance/perf suites in `tests/`, embedded license corpus in `assets/licenses/`, build script `build.rs` at root (per plan.md "Source Code").

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization and basic structure

- [X] T001 Create the Cargo project skeleton: `Cargo.toml` (crate `licet`, `[lib]` + `[[bin]]`), `src/lib.rs`, `src/main.rs`, and empty module dirs `src/{cli,config,rules,walk,detect,comment,spdx,reconcile,report,reuse}/` each with a `mod.rs`, per plan.md "Source Code"
- [X] T002 Declare dependencies in `Cargo.toml`: `clap` (derive), `ignore`, `rayon`, `spdx`, `serde`, `toml`, `globset`, `gix`, `memchr`, `bstr`, `anyhow`, `thiserror`, and dev-deps `insta`, `assert_cmd`, `predicates`, `tempfile`, `criterion`
- [X] T003 [P] Configure `rustfmt.toml`, `clippy` lints (deny warnings) via `Cargo.toml`/`.cargo/config.toml`, and an `mise.toml`/CI task running `cargo fmt --check && cargo clippy && cargo test`
- [~] T004 [P] ~~Create test fixture scaffolding under `tests/fixtures/`~~ **Superseded**: fixtures are built programmatically per-test by `tests/common/mod.rs` (`Fixture` spins up an isolated git repo with the exact files/config each scenario needs). Static on-disk sample repos would duplicate this with worse isolation, so they were intentionally not added.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types, config, SPDX engine, CLI shell, and reporting infrastructure that every user story builds on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [X] T005 [P] Define core domain types in `src/domain.rs` (re-exported from `src/lib.rs`): `Selector` (Extension/Glob/ExactPath/Filename), `LicenseIntent` (+ `CopyrightPolicy` enum Preserve/PreserveAndAdd/Replace), `CommentStyle` primitives, `DriftClass`, `FileLicensingState`, `RuleConflict` — mirroring data-model.md §1–§6
- [X] T006 [P] Implement error types and exit-code mapping in `src/error.rs` and `src/main.rs`: library uses `thiserror`, binary uses `anyhow`; map results to exit codes 0/1/2/3 per contracts/cli.md
- [X] T007 Generate the embedded SPDX license-text table in `build.rs` from `assets/licenses/` (populate the corpus), exposing a compile-time lookup `id → text` per FR-017 / research.md §8
- [X] T008 Implement the SPDX expression engine in `src/spdx/mod.rs`: parse expressions via the `spdx` crate, canonicalize, and compare semantically (`A OR B` ≡ `B OR A`); expose `is_known_id`, `bundled_ids()` over the embedded table (FR-005, research.md §2)
- [X] T009 Implement config model + TOML deserialization in `src/config/schema.rs` and `src/config/mod.rs`: `LicensingConfiguration` (`default`, `rules`, `comment_styles`, `exclude`) per contracts/config-schema.md, with validation (exactly-one selector key, valid SPDX/`LicenseRef-*`, well-formed globs, duplicate-selector conflict surfacing) returning exit-2 config errors (FR-001, FR-010)
- [X] T010 Build the CLI skeleton in `src/cli/mod.rs` and `src/main.rs`: `clap` subcommands `check`/`apply`/`init`/`lint` and global flags `--config`, `--format human|json`, `--files`/`--files-from`/`-`, `--staged`, `--changed`, `--no-cache`/`--cache`, `--explain` (parse only; dispatch stubs) per contracts/cli.md
- [X] T011 Define the report model and renderers in `src/report/mod.rs` and `src/report/render.rs`: `Report`/`ReconciliationPlan`, `FileChange`, `Warning`, `summary.counts`, serde `--format json` conforming to contracts/report.schema.json, plus a human renderer (shared by check/apply/lint)

**Checkpoint**: Foundation ready — config parses, SPDX compares, CLI parses, reports render. User stories can now begin.

---

## Phase 3: User Story 1 - Declare licensing rules and see drift (Priority: P1) 🎯 MVP

**Goal**: A read-only `check` that projects declared intent onto every tracked file and reports per-file drift (compliant / wrong-license / missing / uncovered / excluded).

**Independent Test**: Author a config with a default + override rule, run `licet check` against a repo of compliant and drifted files, and confirm each file is classified correctly and exit code reflects pass/fail.

### Tests for User Story 1

- [X] T012 [P] [US1] Integration test in `tests/integration/us1_drift.rs` (using `assert_cmd` + `tests/fixtures/`): asserts each acceptance scenario — examples/*.rs drift shown declared-vs-actual, missing header → `missing_header`, no-rule file → `uncovered`, compliant repo → exit 0 (spec US1 scenarios 1–4); plus two equal-specificity rules matching one file surface a `RuleConflict` (warning kind `rule_conflict`, exit 1) rather than silently resolving (FR-022)
- [X] T013 [P] [US1] JSON contract test in `tests/integration/us1_report_schema.rs`: validates `licet check --format json` output against contracts/report.schema.json (structure, `drift` enum, `summary.counts`)
- [X] T014 [P] [US1] Snapshot tests in `tests/us1_snapshots.rs` using `insta` for the human-readable drift report rendering (drift / uncovered / compliant renderings pinned; snapshots under `tests/snapshots/`)

### Implementation for User Story 1

- [X] T015 [P] [US1] Implement rule matching, derived specificity (file > glob > ext > default) and declaration-order tie-break in `src/rules/mod.rs`, surfacing equal-specificity matches as `RuleConflict` (FR-002, FR-022)
- [X] T016 [P] [US1] Implement full-tree file enumeration in `src/walk/mod.rs` using `ignore` (gitignore-aware, parallel) for default tracked-file coverage (FR-013 full-scan path; subset flags added in US4)
- [X] T017 [P] [US1] Implement the built-in comment-style registry (read side) in `src/comment/mod.rs`, seeded to at least the REUSE-known extension/filename set, for recognizing existing headers (FR-011)
- [X] T018 [US1] Implement actual-license detection in `src/detect/mod.rs`: parse in-file `SPDX-License-Identifier`/`SPDX-FileCopyrightText` header blocks (with byte ranges + position-after context) and read out-of-band `REUSE.toml`/`.reuse/dep5`; either source satisfies intent (FR-003a, data-model §5) — depends on T017
- [X] T019 [US1] Implement the drift classification engine in `src/report/classify.rs`: join declared intent (T015) vs detected actual (T018) into `FileLicensingState` with `DriftClass`, comparing licenses semantically via `src/spdx` (FR-003, FR-004, FR-005) — depends on T015, T018
- [X] T020 [US1] Wire the `check` command in `src/cli/check.rs`: orchestrate walk → rules → detect → classify → report; honor `--explain <path>` (print winning rule + why); map results to exit 0/1 where Uncovered counts as failure and Excluded does not (FR-012, FR-012a) — depends on T016, T019

**Checkpoint**: `licet check` produces an authoritative per-file drift report with correct exit codes — MVP usable as a read-only audit.

---

## Phase 4: User Story 2 - Reconcile files to declared intent (Priority: P2)

**Goal**: An `apply` that writes missing headers and corrects wrong ones — destructive on the license id by default, additive opt-in, copyright always preserved, with targeted-header selection.

**Independent Test**: Take the drifted repo from US1, run `licet apply`, confirm every covered file matches intent and a re-run `check` reports zero drift; verify additive vs destructive differ on a conflicting file and copyright lines survive.

### Tests for User Story 2

- [X] T021 [P] [US2] Integration test in `tests/integration/us2_apply.rs`: destructive replaces wrong license then `check` → exit 0; additive keeps old + adds declared + contradiction warning; copyright lines unchanged after license-only replace; `--target-header 1` replaces the 2nd block (spec US2 scenarios 1–5, SC-002, SC-004); a file reachable via a symlink is annotated exactly once (T027, Edge Cases)
- [X] T022 [P] [US2] Integration test in `tests/integration/us2_partial.rs`: a write failure (e.g. read-only file) yields exit 3 and a report naming changed vs unchanged files (FR-021)

### Implementation for User Story 2

- [X] T023 [P] [US2] Implement the comment-style header writer (write side) in `src/comment/mod.rs`: render an SPDX header block in a given `CommentStyle` (line-prefix and block forms) (FR-011)
- [X] T024 [P] [US2] Implement the first-line-aware inserter in `src/reconcile/insert.rs`: detect/skip shebang, encoding/XML decl, BOM and insert the header immediately after (FR-019, research.md §6)
- [X] T025 [US2] Implement the reconcile engine in `src/reconcile/mod.rs`: destructive license-id replacement (default), additive append with contradiction detection, copyright preservation by default, and targeted-header selection among multiple blocks (FR-006, FR-007, FR-008, FR-009, FR-020) — depends on T023, T024
- [X] T026 [US2] Implement offline license-text materialization in `src/reuse/inventory.rs`: write referenced-but-missing standard texts into `LICENSES/` from the embedded bundle; never scaffold placeholders — surface unbundled/`LicenseRef-*` ids as still-missing with guidance, recognize `.txt`/`.md`/extension-less `LICENSES/` files, and offer opt-in `curl` fetch for standard ids (FR-017) — depends on T007
- [X] T027 [US2] Add symlink-safety dedup in `src/walk/mod.rs` so a file reached via symlink is annotated only once (Edge Cases)
- [X] T028 [US2] Wire the `apply` command in `src/cli/apply.rs`: `--additive`, `--target-header <index>`, `--dry-run` (emit ReconciliationPlan without writing); partial-apply reporting with exit 3; populate `change`/`warnings` in the JSON report (FR-007, FR-008, FR-021) — depends on T025, T026

**Checkpoint**: `licet apply` reconciles drift to zero, preserves copyright, and supports additive/destructive/targeted modes.

---

## Phase 5: User Story 3 - Define comment styles for unknown file types (Priority: P3)

**Goal**: Persisted, config-driven comment-style associations (e.g. C-style for `*.pkl`) that round-trip on read and write with no per-file flags, filename overriding extension.

**Independent Test**: Add a `[[comment_style]]` association for an unknown extension, `apply` a file of that type, confirm the header uses the configured syntax and is recognized on the next `check`.

### Tests for User Story 3

- [X] T029 [P] [US3] Integration test in `tests/integration/us3_comment_styles.rs`: `ext = "pkl"` → `apply` writes C-style header on `hk.pkl`, next `check` recognizes it (round-trip); a `file =` association overrides an `ext =` one; known types keep built-in styles (spec US3 scenarios 1–4, SC-005)

### Implementation for User Story 3

- [X] T030 [P] [US3] Extend `src/comment/mod.rs` resolution to overlay user-defined associations from config onto built-ins with precedence exact-filename → extension → built-in (FR-010, FR-011)
- [X] T031 [P] [US3] Support inline custom `CommentStyle` (`line_prefix`, `block_start`, `block_end`, `block_line_prefix`) and built-in style references by name in `src/config/schema.rs` + `src/comment/mod.rs` (config-schema.md `[[comment_style]]`)
- [X] T032 [US3] Route both detection (`src/detect`) and reconciliation (`src/reconcile`) through the association-aware resolver so configured styles persist across runs with no per-file flag — depends on T030, T031

**Checkpoint**: Previously unknown file types become fully managed via one config entry and round-trip across runs.

---

## Phase 6: User Story 4 - Enforce in pre-commit and CI without slowing the workflow (Priority: P4)

**Goal**: Fast, subset-aware enforcement: staged/changed/file-list modes, a warm-scan cache, and a clean pass/fail exit contract meeting the <1s/10k-file budget.

**Independent Test**: Run `check` over a representative repo and over a small staged subset; confirm correct pass/fail and that a 10k-file fixture completes under 1s warm.

### Tests for User Story 4

- [X] T033 [P] [US4] Integration test in `tests/integration/us4_subset.rs`: `--staged`, `--changed [<rev>]`, and `--files`/`--files-from`/`-` each evaluate only the supplied subset; drifted staged file → exit 1 naming the file; compliant staged set → exit 0 (spec US4 scenarios 1–4, FR-013, SC-009)
- [X] T034 [P] [US4] Perf gate in `tests/perf/scan_10k.rs` (release timing, e.g. `criterion` or a guarded `--release` assertion): a generated ~10,000-file fixture scans in <1s warm (SC-006)

### Implementation for User Story 4

- [X] T035 [P] [US4] Implement git subset selection in `src/walk/mod.rs` using `gix`: tracked set, staged (index vs HEAD), and `--changed [<rev>]` diffs, plus `--files`/`--files-from`/stdin lists (FR-013, research.md §3)
- [X] T036 [P] [US4] Implement the warm-scan cache in `src/walk/cache.rs`: content-hash → classification persistence honoring `--no-cache`/`--cache <path>` (SC-006, research.md §10)
- [X] T037 [US4] Optimize header detection in `src/detect/mod.rs` to read only the file head via `memchr`/`bstr` (no full UTF-8 decode) for throughput (SC-006, research.md §10)
- [X] T038 [US4] Wire selection + cache flags into `src/cli/check.rs` and `src/cli/apply.rs` so both honor subset and cache modes while rule precedence still considers the full ruleset (FR-013, cli.md cross-command guarantees) — depends on T035, T036

**Checkpoint**: Enforcement is fast, subset-aware, and unobtrusive in hooks/CI with an unambiguous exit-code gate.

---

## Phase 7: User Story 5 - Stay REUSE/SPDX compatible and migrate (Priority: P5)

**Goal**: REUSE-spec-conformant output (headers, `LICENSES/` tree, out-of-band for non-annotatable files), a `lint` compatibility report, and `init --from-reuse` bootstrap from existing state.

**Independent Test**: Run on an existing REUSE repo, confirm reconciled output passes upstream `reuse lint`, and that `init --from-reuse` derives a config whose projection reproduces current licensing.

### Tests for User Story 5

- [X] T039 [P] [US5] Conformance test in `tests/conformance/reuse_compat.rs`: reconcile a fixture then assert the upstream `reuse lint` (or its spec checks) reports it compliant; gate-skip with a clear message if `reuse` is unavailable (SC-007)
- [X] T040 [P] [US5] Integration test in `tests/integration/us5_init.rs`: `init --from-reuse` against a fixture with `REUSE.toml` + headers produces a `license.toml` whose projection matches current licensing (spec US5 scenarios 1–3, SC-008)
- [X] T051 [P] [US5] Integration suite in `tests/us5_add_license.rs` for the `add-license` command: explicit-id and `--all` materialization, idempotent re-run, `LicenseRef` never-scaffolded exit 1 with path guidance, compound-expression split, `.md` text recognition, unknown-id exit 1, flag-misuse exit 2, dirty-tree tolerance + source/config untouched, and JSON shape (FR-029)

### Implementation for User Story 5

- [X] T041 [P] [US5] Implement `LicenseTextInventory` reporting in `src/reuse/inventory.rs`: compute referenced/present/missing/bundled-available over `LICENSES/` and config (FR-014, FR-017) — extends T026
- [X] T042 [P] [US5] Implement out-of-band metadata writing for non-annotatable/binary files via `REUSE.toml` in `src/reuse/oob.rs`, reading `.reuse/dep5` for backward compatibility (FR-015, research.md §7)
- [X] T043 [US5] Implement the `init`/bootstrap command in `src/cli/init.rs`: inspect existing headers + `REUSE.toml`/`.reuse/dep5` and emit `license.toml` (`--output`), modifying no source files (FR-018) — depends on T018, T009
- [X] T044 [US5] Implement the `lint` command in `src/cli/lint.rs`: report REUSE posture (header presence, `LICENSES/` completeness, out-of-band coverage), list missing texts with actionable guidance (bundle/add-license, SPDX URL, or `LICENSES/<id>.txt` path), read-only/no fetch, exit 1 if non-compliant (FR-014, FR-017) — depends on T041, T042
- [X] T052 [US5] Implement the `add-license` command (alias `add`) in `src/cli/add_license.rs`: materialize referenced license texts into `LICENSES/` from the offline bundle standalone — explicit ids or `--all` (mutually exclusive), never scaffolding placeholders, opt-in `--allow-curl`/interactive fetch for unbundled standard ids, never touching source files/config or requiring a clean tree; exit 0/1/2 per cli.md. Wire into `src/cli/mod.rs`; repoint the `lint` missing-text hint from `apply` to `add-license` (FR-017, FR-029) — depends on T041

**Checkpoint**: Output is REUSE-compliant, `lint` reports posture offline, and existing projects migrate via bootstrap.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Improvements spanning multiple stories

- [X] T045 [P] Add `spdx`-canonicalization unit tests in `src/spdx/mod.rs` covering parenthesization, `WITH` exceptions, `+`, and operator reordering (SC-003 edge cases)
- [X] T046 [P] Generate shell completions (clap) and document the binary/config naming in `README.md` (`licet completions <shell>` via `clap_complete`; bash/zsh/fish/powershell/elvish; README "Naming" section added)
- [X] T047 [P] Write `README.md` usage docs and run the quickstart.md scenarios end-to-end as a documented manual/CI validation pass
- [X] T048 [P] Add a `criterion` benchmark harness under `benches/` for scan throughput, tracking the SC-006 budget over time (`benches/scan.rs`, cold + warm cache groups; ~120k files/s observed)
- [X] T049 Cross-platform release wiring (Linux/macOS/Windows, x86-64 + arm64) producing a single self-contained binary (`.github/workflows/release.yml` matrix via `taiki-e/upload-rust-binary-action`; Linux static musl; release notes via `git-cliff`/`cliff.toml`; versioning via `cargo-release`/`release.toml`)
- [X] T050 Final determinism + offline-by-default audit: confirm identical inputs → identical classification/ordering/exit code and that the default paths reach no network (FR-002, FR-017) (`tests/determinism.rs`: byte-identical reports, sorted ordering, `lint` is offline & deterministic; verified zero network-capable crates in the dependency tree — the only network access is opt-in `--allow-curl`, which shells out to the user's own `curl`)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **User Stories (Phase 3–7)**: All depend on Foundational
  - US1 (P1) is the MVP and is a prerequisite in practice for US2 (apply reconciles US1's classifications) and US4 (gates the check)
  - US3 extends US1/US2's comment handling; US5 extends US2's materialization
  - Stories are individually testable; recommended order is priority order P1 → P5
- **Polish (Phase 8)**: Depends on the targeted stories being complete

### User Story Dependencies

- **US1 (P1)**: Foundational only — fully independent (read-only audit)
- **US2 (P2)**: Builds on US1's detect/classify; independently testable via the drifted fixture
- **US3 (P3)**: Builds on US1 detect + US2 write path; independently testable on an unknown extension
- **US4 (P4)**: Builds on US1's `check`; adds subset/cache/perf; independently testable
- **US5 (P5)**: Builds on US2's materialization + US1 detect; independently testable for REUSE compat + migration

### Within Each User Story

- Tests are written first and expected to FAIL before implementation
- `rules`/`walk`/`comment` (read) precede `detect`; `detect` precedes `classify`; `classify` precedes the `check` wiring
- `comment` (write) + `insert` precede the `reconcile` engine, which precedes `apply` wiring
- Story complete before moving to the next priority

### Parallel Opportunities

- Setup: T003, T004 in parallel
- Foundational: T005, T006 in parallel; T007 before T008; T009/T010/T011 largely parallel after types exist
- US1: tests T012–T014 in parallel; impl T015/T016/T017 in parallel, then T018 → T019 → T020
- US2: tests T021/T022 in parallel; impl T023/T024 in parallel, then T025; T026/T027 parallel; then T028
- US4: T035/T036 in parallel; T033/T034 in parallel
- Across teams: once Foundational lands, US1→US5 can be staffed in parallel given the dependency notes above

---

## Parallel Example: User Story 1

```bash
# Tests for User Story 1 together:
Task: "Integration test in tests/integration/us1_drift.rs"
Task: "JSON contract test in tests/integration/us1_report_schema.rs"
Task: "Snapshot tests in tests/integration/us1_snapshots.rs"

# Independent implementation modules together:
Task: "Rule matching + precedence in src/rules/mod.rs"
Task: "File enumeration in src/walk/mod.rs"
Task: "Built-in comment-style registry (read) in src/comment/mod.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1 (`licet check`)
4. **STOP and VALIDATE**: drift report classifies the fixture correctly with the right exit codes
5. Ship as a read-only audit MVP

### Incremental Delivery

1. Setup + Foundational → foundation ready
2. US1 → drift audit (MVP)
3. US2 → reconciliation/enforcement teeth
4. US3 → extensible comment styles
5. US4 → fast pre-commit/CI gate
6. US5 → REUSE compatibility + migration

### Parallel Team Strategy

After Foundational: Dev A on US1, then US2/US4 layer on the check/detect core; Dev B on US3 comment extensibility; Dev C on US5 REUSE/migration — integrating per the dependency notes.

---

## Notes

- [P] = different files, no dependency on incomplete tasks
- [Story] label maps each task to its user story for traceability
- Verify tests fail before implementing
- Commit after each task or logical group
- Stop at any checkpoint to validate a story independently
