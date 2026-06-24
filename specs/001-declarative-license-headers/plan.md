# Implementation Plan: Declarative License Header Management

**Branch**: `001-declarative-license-headers` | **Date**: 2026-06-24 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-declarative-license-headers/spec.md`

## Summary

Build a single-binary CLI tool that manages SPDX/REUSE-style license and copyright
metadata for every file in a repository from **one declarative configuration**. The
config expresses intent as ordered rules (by extension, glob, exact filename) plus a
repo-wide default; the tool **projects** that intent onto the working tree, **reports
drift** (compliant / wrong-license / missing / uncovered / excluded), and **reconciles**
files to match — destructive on the license identifier by default, additive opt-in,
copyright always preserved. Output stays REUSE-spec compatible (SPDX headers, a
`LICENSES/` tree, out-of-band metadata for binaries) so existing REUSE consumers keep
working.

Technical approach: a **Rust** CLI. Rust is chosen to meet the spec's hard
non-functional bars that the existing Python REUSE tool misses — a full scan of a
~10,000-file repo in **under 1 second warm** (SC-006), **offline/hermetic by default**
with **bundled SPDX license texts embedded in the binary** (FR-017), and a single
static executable that drops cleanly into commit hooks and CI. Parallel, gitignore-aware
file walking (`ignore` + `rayon`), semantic SPDX expression comparison (`spdx` crate),
and a persisted comment-style registry address the five named REUSE grievances:
declarativeness, enforcement, speed, extensible comment styles, and precise
additive-vs-destructive control.

## Technical Context

**Language/Version**: Rust 1.83+ (2021 edition; stable toolchain)

**Primary Dependencies**:
- `clap` (v4, derive) — CLI parsing, subcommands, shell-completion
- `ignore` — parallel, `.gitignore`-aware file walking (powers ripgrep; gives the tracked-by-default file set and fast traversal)
- `rayon` — data-parallel per-file classification across cores
- `spdx` — SPDX license-expression parsing and **semantic** equivalence (FR-005)
- `serde` + `toml` — declarative config + REUSE.toml parsing
- `globset` — glob/extension/filename rule matching with precedence
- `gix` (gitoxide) or `git2` — resolve the tracked file set and staged/changed subsets (FR-013); `gix` preferred (pure-Rust, no libgit2 link, faster, hermetic)
- `memchr` / `bstr` — fast header scanning of file heads without full UTF-8 decode
- `anyhow` + `thiserror` — error context (libs use `thiserror`, the binary uses `anyhow`)
- `insta` (dev) — snapshot tests for rendered headers and reports
- SPDX license-text corpus embedded at build time via `include_dir!` / generated `build.rs` table

**Storage**: Filesystem only. Inputs/outputs are repository files. Configuration is a
single TOML file at repo root (working name `license.toml`); referenced license texts
live in `LICENSES/` per the REUSE spec. No database. An optional on-disk scan cache
(content-hash → classification) backs the "warm cache" performance target.

**Testing**: `cargo test` (unit + integration), `insta` snapshots for header/report
rendering, fixture repositories under `tests/fixtures/`, and a benchmark harness
(`criterion` or a `cargo test --release` timing gate) asserting **both** the 10k-file
**<1s warm** and **<3s cold** budgets (SC-006). A conformance test runs the real `reuse`
tool (or its spec checks) against reconciled fixtures to prove REUSE compatibility (SC-007).

Additional targeted suites mandated by the review:
- **Fixture corpus matrix (SC-003)** — `tests/fixtures/` MUST cover the cross-product of:
  drift classes {compliant, wrong-license, missing, uncovered, excluded, **unreadable**} ×
  comment styles {line `//`/`#`/`;`, block `/* */`/`<!-- -->`, configured `.pkl`} ×
  first-line cases {plain, shebang, BOM, XML/encoding decl} × encodings {UTF-8, **UTF-16**,
  Latin-1} × {regular file, symlink, duplicate-content}. The matrix is the evidence for
  "zero false-compliant."
- **Cache-fidelity test (SC-011)** — run, mutate the config, re-run; assert the classification
  matches a `--no-cache` cold run (no stale `Compliant`).
- **Apply-safety / fault-injection test (SC-010)** — kill mid-apply and make a file
  unwritable; assert no file is half-written, copyright survives, and changed-vs-unchanged is
  reported. Assert `apply` refuses on a dirty tree without `--allow-dirty`.
- **Bootstrap-generalization test (SC-008)** — assert the generated config's rule count is
  ≤ 25% of covered files and prefers ext/glob over per-file rules.
- **Source-precedence test (FR-003a)** — header vs out-of-band disagreement → out-of-band
  wins, `source_override` warning emitted, gate still passes.

**Target Platform**: Cross-platform CLI — Linux, macOS, Windows (x86-64 + arm64),
distributed as a single self-contained binary. Runs in developer shells, `hk`/pre-commit
hooks, and CI runners.

**Project Type**: Single-project CLI tool with a reusable library core (`lib` + thin
`main`), so the engine is testable and embeddable independently of the CLI.

**Performance Goals**: Full scan of a ~10,000-file repository **< 1s warm** and **< 3s cold**
(empty cache, e.g. fresh CI checkout) on a 4-core 2020-era runner (SC-006); changed-file/
staged checks **well under 1s** and unobtrusive in a commit hook; throughput bounded by
parallel IO + header parsing, not by per-file process startup. The cold bar is the one that
governs CI.

**Constraints**: **Offline by default / hermetic CI** — no network for known SPDX IDs;
network fetch is opt-in and limited to identifiers absent from the bundle (FR-017).
Deterministic rule precedence and reporting (FR-002, FR-022). Non-destructive to
copyright/authorship by default (FR-009, SC-004). **Apply safety**: refuse on a dirty tree
unless `--allow-dirty`, atomic temp-file+rename writes (FR-024, SC-010). **Cache fidelity**:
key folds in config + tool version so a hit equals a cold run (FR-023, SC-011). **Encoding**:
non-UTF-8 files are skipped, classified `Unreadable`, and fail the gate; LF/CRLF preserved on
write (FR-025, FR-026). Must not double-annotate symlinked duplicates; must respect
shebang/encoding first-lines (FR-019). Out-of-band wins over in-file header on disagreement
(FR-003a). Selection flags are mutually exclusive (FR-027).

**Scale/Scope**: Single repository working tree per invocation; tens of thousands of
files; dozens of rules and comment-style associations; the full SPDX license list
(~600 identifiers) bundled. Multi-repo orchestration is out of scope for v1.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

The project constitution (`.specify/memory/constitution.md`) is an unfilled template with
no ratified principles, so there are **no project-specific gates to enforce**. Default
engineering gates are applied and all pass:

| Gate | Status | Notes |
|------|--------|-------|
| Library-first / testable core | PASS | Engine is a library; CLI is a thin shell over it. |
| CLI text I/O contract | PASS | stdin/args → stdout, errors → stderr; human + machine (JSON) output (see contracts). |
| Test-first feasible | PASS | Each user story has an independent test; fixtures + snapshots planned. |
| Simplicity / YAGNI | PASS | Single binary, single config, no service/DB; multi-repo deferred. |
| Observability | PASS | Deterministic, explainable drift reports; `--explain` traces which rule matched. |
| Data safety (destructive default) | PASS | Dirty-tree guard + atomic writes + copyright preservation make destructive-by-default recoverable (FR-024, SC-004, SC-010). |
| Cache correctness | PASS | Cache key folds in config + version; a hit equals a cold run (FR-023, SC-011). |

No violations → Complexity Tracking left empty.

## Project Structure

### Documentation (this feature)

```text
specs/001-declarative-license-headers/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
│   ├── cli.md           # Command-line contract (subcommands, flags, exit codes)
│   ├── config-schema.md # Declarative config (license.toml) schema
│   └── report.schema.json # Machine-readable drift/apply report schema
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
Cargo.toml               # workspace / crate manifest
build.rs                 # generates the embedded SPDX license-text table

src/
├── main.rs              # binary entry: parse CLI, dispatch, map result → exit code
├── lib.rs               # library root re-exporting the engine
├── cli/                 # clap command/flag definitions and output rendering
│   ├── mod.rs
│   ├── check.rs         # `check` (non-writing gate) — FR-012, FR-012a, FR-013
│   ├── apply.rs         # `apply` (reconcile) — FR-006..FR-009
│   ├── init.rs          # `init`/`bootstrap` from existing state — FR-018
│   └── lint.rs          # REUSE-compat compliance summary — FR-014, FR-017
├── config/             # declarative config model + TOML (de)serialization — FR-001, FR-010
│   ├── mod.rs
│   └── schema.rs
├── rules/              # rule matching + deterministic precedence/conflict surfacing — FR-002, FR-022
├── walk/               # tracked-file enumeration, staged/changed subset, parallel traversal — FR-013
├── detect/             # actual-license detection: in-file SPDX + REUSE.toml/.reuse/dep5, out-of-band precedence, UTF-8 validation — FR-003a, FR-025
├── comment/            # comment-style registry (built-in + configured associations) — FR-010, FR-011
├── spdx/               # canonical-form expression compare + embedded license-text inventory + list version — FR-005, FR-017, FR-028
├── reconcile/          # additive/destructive apply, targeted replacement, first-line safety, atomic writes, dirty-tree guard, line-ending preservation — FR-007, FR-008, FR-019, FR-021, FR-024, FR-026
├── cache/              # classification cache keyed on content + config fingerprint + tool version — FR-023, SC-011
├── report/             # drift classification (incl. Unreadable) + human and JSON renderers — FR-004, FR-020
└── reuse/              # REUSE-compatible output, LICENSES/ tree, dep5/out-of-band, bootstrap import (generalizing) — FR-014..FR-016, FR-018, SC-008

assets/
└── licenses/           # bundled SPDX license texts (embedded at build time) — FR-017

tests/
├── integration/        # one suite per user story (P1..P5), exit-code + report assertions
├── conformance/        # reconciled fixtures must pass a real REUSE spec check — SC-007
├── perf/               # 10k-file fixture timing gate — SC-006
└── fixtures/           # sample repos: compliant / wrong-license / missing / uncovered / excluded
```

**Structure Decision**: Single Rust project with a reusable library core (`src/lib.rs`)
and a thin binary (`src/main.rs`). The engine is decomposed by pipeline stage — `walk` →
`detect` + `rules` → `report` → `reconcile` — with `config`, `comment`, `spdx`, and
`reuse` as cross-cutting support modules. This keeps each spec capability in an
independently testable unit, lets commit hooks and CI link the same code path the CLI
uses, and isolates the REUSE-compatibility surface (`src/reuse/`) so interop changes
don't ripple into the core engine.

## Complexity Tracking

> No constitution violations. Section intentionally empty.
