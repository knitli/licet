<!--
SYNC IMPACT REPORT
==================
Version change: (unfilled template) → 1.0.0
Rationale: Initial ratification of the licet constitution. MAJOR bump from an
empty template to a populated, governing document.

Modified principles: N/A (initial adoption)
Added principles:
  - I. Spec Is the Source of Truth
  - II. Offline & Hermetic by Default
  - III. Library-First, Data-Driven Engine
  - IV. Safety & Non-Destructive Reconciliation
  - V. REUSE / SPDX Interoperability & Fidelity
  - VI. Zero-Warning, Tested, Cross-Platform Quality
Added sections:
  - Technology & Build Constraints
  - Development Workflow & Quality Gates
Removed sections: none

Templates requiring updates:
  - .specify/templates/plan-template.md ✅ reviewed (Constitution Check gate is
    generic; no stale principle references)
  - .specify/templates/spec-template.md ✅ reviewed (no conflicts)
  - .specify/templates/tasks-template.md ✅ reviewed (optional-tests stance is
    consistent with Principle VI, which gates on CI rather than mandating TDD)
  - .specify/templates/checklist-template.md ✅ reviewed (no conflicts)

Follow-up TODOs: none
-->

# licet Constitution

## Core Principles

### I. Spec Is the Source of Truth

The authoritative behavioral spec, data model, and contracts live under
`specs/001-declarative-license-headers/` (`spec.md`, `data-model.md`,
`contracts/`). Code MUST conform to the spec, not the reverse. Any change that
alters observable behavior MUST update the spec and contracts in the same change
set, and doc comments MUST keep their `FR-0xx` / `data-model §x` reference tags
accurate. `license.toml` is the single authoring surface for intent;
`REUSE.toml`/dep5 are read for interop and detection only and MUST NOT become a
second authoring path.

**Rationale**: This is a spec-driven (Spec Kit) project. Drift between code and
spec silently invalidates every downstream artifact (plans, tasks, reviews), so
the spec and code are held in lockstep by rule, not by habit.

### II. Offline & Hermetic by Default

licet MUST run with zero network access. SPDX license texts are embedded in the
binary at build time from `assets/licenses/*.txt` via `build.rs`. No
network-capable crate (reqwest, hyper, ureq, curl, native-tls, openssl-sys,
rustls, tokio) may enter the dependency tree; the CI offline-guard enforces this
and MUST be treated as a hard invariant. Do not add HTTP, TLS, or async-runtime
dependencies.

**Rationale**: Determinism, reproducibility, and trust in a license-compliance
tool demand that results never depend on a network fetch or a live registry. A
hermetic binary produces identical output everywhere, forever.

### III. Library-First, Data-Driven Engine

All engine logic MUST live in the library (`src/lib.rs` and its modules) so it
stays independently testable; `src/main.rs` is a thin clap shell only. The engine
is language-agnostic: new languages, comment styles, and formats are added as
**pure data** (e.g. a comment-style entry), never as new code paths. The shared
`Engine::scan` pipeline (walk → rules → detect → classify, with `apply` adding
reconcile) is the single path used by both `check` and `apply`.

**Rationale**: A data-driven engine keeps the supported-language matrix growing
without growing complexity, and a library-first split keeps behavior verifiable
without going through the CLI.

### IV. Safety & Non-Destructive Reconciliation

`apply` MUST be safe to run and trivial to undo. Writes MUST be atomic
(temp-file + fsync + rename). Copyright information MUST always be preserved.
`apply` MUST refuse a dirty working tree unless `--allow-dirty` is given, so that
`git checkout` is always a clean undo. `LICENSES/` and `.reuse/` are always
excluded from modification.

**Rationale**: The tool rewrites source files across an entire repository. The
cost of an unsafe write is high and the user's trust is fragile, so safety is a
non-negotiable property of every mutating operation.

### V. REUSE / SPDX Interoperability & Fidelity

Output MUST remain compatible with the upstream REUSE tool and SPDX conventions.
SPDX expressions MUST be parsed, canonicalized, and compared semantically — never
by naive string equality. When in-file headers and out-of-band metadata
(`REUSE.toml` / `.reuse/dep5`) disagree, out-of-band metadata is authoritative.

**Rationale**: licet earns its place by interoperating with an existing
ecosystem. Diverging from REUSE/SPDX semantics would make its output wrong in the
eyes of the very tools and humans it serves.

### VI. Zero-Warning, Tested, Cross-Platform Quality

The build MUST stay warning-free: CI sets `RUSTFLAGS: -D warnings` and gates on
`cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings`.
Behavior MUST be covered by tests — unit, doctests, and integration suites
grouped by user story (`us1_`…`us5_`) plus cross-cutting suites (`safety`,
`determinism`, `perf`, `completions`) — and MUST pass on the Linux/macOS/Windows
matrix at MSRV. Human-rendered output is pinned with insta snapshots; intentional
changes are accepted via `cargo insta review`, never blindly.

**Rationale**: A compliance tool must itself be compliant with its own quality
bar. Warnings, untested paths, and platform-specific breakage erode the
correctness guarantees the tool exists to provide.

## Technology & Build Constraints

- **Language**: Rust, edition 2024, **MSRV 1.89.0**; rustfmt with `max_width 100`.
- **Errors**: library errors are `thiserror` (`LicetError`); the binary maps them
  to the `ExitCode` contract in `src/error.rs` (`0` compliant · `1`
  drift/violations · `2` usage/config error · `3` partial apply). `uncovered` and
  `unreadable` count as gate failures; `excluded` and `compliant` pass.
- **Determinism**: output (JSON `Report` per `contracts/report.schema.json`, human
  renderer, and file writes) MUST be deterministic and reproducible.
- **Bundled corpus**: add a license by dropping `<SPDX-ID>.txt` into
  `assets/licenses/`; `build.rs` regenerates `BUNDLED_LICENSES` and
  `SPDX_LIST_VERSION` automatically. Do not hand-edit generated output.
- **Caching**: scan caches live under `.git/` (e.g. `.git/licet-cache`) and MUST
  never dirty the working tree.

## Development Workflow & Quality Gates

- Every module begins with a `//!` doc comment stating its role and the FR/spec
  tags it implements; preserve this when editing.
- A change is not complete until: `cargo fmt --all --check`, `cargo clippy
  --all-targets -- -D warnings`, `cargo test --all-targets`, and `cargo test
  --doc` all pass, and the MSRV and offline-guard checks would pass.
- Behavior changes update `spec.md` / `data-model.md` / `contracts/` and the
  `FR-`/`§` doc-comment references in the same change set.
- New languages/formats are contributed as comment-style data with accompanying
  tests, not as new branches in the engine.
- Snapshot changes are reviewed and explicitly accepted, never auto-committed.

## Governance

This constitution supersedes other conventions when they conflict. The CLAUDE.md
and README are operational guidance and MUST stay consistent with these
principles; on conflict, the constitution wins and the guidance is corrected.

Amendments MUST be made by editing this file with: a clear description of the
change, a version bump per the policy below, and propagation to any dependent
templates and guidance documents. Versioning follows semantic rules:

- **MAJOR**: removing or redefining a principle, or other backward-incompatible
  governance changes.
- **MINOR**: adding a new principle or section, or materially expanding guidance.
- **PATCH**: clarifications, wording, and non-semantic refinements.

Compliance is verified at review time: every change MUST be checkable against
these principles, and any deliberate deviation MUST be justified in writing
(e.g. a plan's Complexity Tracking table) or the change MUST be revised. The CI
gates in `.github/workflows/ci.yml` are the automated enforcement arm of
Principles II and VI and MUST NOT be weakened to make a change pass.

**Version**: 1.0.0 | **Ratified**: 2026-06-24 | **Last Amended**: 2026-06-24
