# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`licet` is a single-binary Rust CLI that manages SPDX/REUSE-compatible license/copyright
headers across a repository from one declarative config (`license.toml`). You declare
intent once; `licet` projects it onto the working tree, classifies drift, and reconciles
files to match. It is **offline and hermetic by default** — SPDX license texts are
embedded in the binary at build time — and output stays compatible with the upstream
[REUSE](https://reuse.software) tool.

The library (`src/lib.rs`) exposes the full engine; the binary (`src/main.rs`) is a thin
clap shell. Keep new logic in the library so it stays independently testable.

## Commands

```bash
cargo build --release            # → target/release/licet
cargo test --all-targets         # all unit + integration tests
cargo test --doc                 # doctests (run separately in CI)
cargo test --test us2_apply      # one integration test file (tests/us2_apply.rs)
cargo test --test us1_check_drift drift_wrong_license   # one test by name
cargo lint                       # alias for: clippy --all-targets -- -D warnings
cargo fmt --all                  # rustfmt (edition 2024, max_width 100)
cargo bench                      # scan-throughput benchmark → target/criterion/
```

CI (`.github/workflows/ci.yml`) gates on: `cargo fmt --all --check`, `cargo clippy
--all-targets -- -D warnings`, the test matrix (Linux/macOS/Windows), `cargo test --doc`,
MSRV check at **1.89.0**, and an **offline-guard** that fails if any network-capable crate
(reqwest, hyper, ureq, curl, native-tls, openssl-sys, rustls, tokio) enters the dependency
tree. Treat that guard as a hard invariant: do not add HTTP/TLS/async-runtime deps.

`RUSTFLAGS: -D warnings` is set in CI, so any warning is a build failure.

## Snapshot tests (insta)

`tests/us1_snapshots.rs` uses `insta::assert_snapshot!`; snapshots live in
`tests/snapshots/*.snap`. When intentionally changing human-rendered output, review and
accept new snapshots with `cargo insta review` (or `cargo insta accept`).

## Architecture

The core abstraction is a **language-agnostic engine** in `src/domain.rs` — `Selector`,
`LicenseIntent`, `CommentStyle`, `DriftClass`, `FileLicensingState`, etc. mirror
`specs/001-declarative-license-headers/data-model.md`. New languages are added as **pure
data** (a comment style), never code.

The scan pipeline (`src/engine.rs`, `Engine::scan`) is shared by both `check` and `apply`
and runs in this order, parallelized per-file with rayon:

1. **walk** (`src/walk/`) — enumerate files. `Selection` is `FullTree` (gitignore-aware
   walk via the `ignore` crate) or a git subset (`Staged` / `Changed(rev)` / explicit
   `Files`) discovered via `gix`. Selection flags are mutually exclusive. `LICENSES/` and
   `.reuse/` are always excluded.
2. **rules** (`src/rules/`) — resolve each path to a winning rule. Precedence by
   specificity: `ExactPath` > `Filename` > `Glob` > `Extension` > `default`, with config
   declaration order breaking ties. Equal-specificity rules with **differing** intent
   surface a `RuleConflict` rather than silently resolving (the conflict becomes a warning,
   not an arbitrary pick).
3. **detect** (`src/detect/`) — read only the file **head** (8 KB) and parse in-file SPDX
   header blocks; also read out-of-band `REUSE.toml`/`.reuse/dep5` (`src/reuse/oob.rs`).
   On disagreement, **out-of-band metadata is authoritative** over the in-file header.
4. **classify** (`src/report/classify.rs`) — join declared intent vs detected actual into
   one of the mutually-exclusive `DriftClass` variants (compliant, wrong_license,
   missing_header, uncovered, excluded, unreadable).

`apply` additionally runs **reconcile** (`src/reconcile/`): `plan_file` computes new
content (destructive on the license id by default; `--additive` keeps both), copyright is
**always preserved**, and writes are **atomic** (temp-file + fsync + rename, see
`src/reuse/atomic_write`). `apply` refuses a dirty working tree unless `--allow-dirty`, so
`git checkout` is always a clean undo.

Supporting modules:
- `src/config/` — `license.toml` loading/validation (`schema.rs` is the raw serde model;
  `mod.rs` validates into the domain types). This is the **only** authoring surface for
  intent; `REUSE.toml`/dep5 are read for interop/detection only.
- `src/comment/` — built-in comment-style registry (seeded to the REUSE-known set),
  overlaid by user associations; render + parse sides of headers.
- `src/spdx/` — SPDX expression parsing/canonicalization/semantic comparison, plus the
  embedded license corpus. `build.rs` generates `BUNDLED_LICENSES` and `SPDX_LIST_VERSION`
  from `assets/licenses/*.txt` into `OUT_DIR/licet_licenses.rs`, `include!`d here.
- `src/report/` — `Report` JSON model (matches `contracts/report.schema.json`) and the
  human renderer.
- `src/walk/cache.rs` — scan cache keyed by content+config+SPDX-version, stored in
  `.git/licet-cache` so it never dirties the working tree.

### Adding a bundled license

Drop `<SPDX-ID>.txt` into `assets/licenses/`; `build.rs` picks it up automatically
(`cargo:rerun-if-changed` is wired). The SPDX list version can be overridden via the
`LICET_SPDX_LIST_VERSION` env var at build time.

## Exit codes

Defined in `src/error.rs` (`ExitCode`): `0` compliant · `1` drift/violations · `2`
usage/config error · `3` partial apply. `uncovered` and `unreadable` (non-UTF-8) count as
gate **failures**; `excluded` and `compliant` pass (`DriftClass::is_failure`).

## Spec is the source of truth

This is a spec-driven (Spec Kit) project. The authoritative behavioral spec, data model,
and contracts live under `specs/001-declarative-license-headers/`:
- `spec.md`, `data-model.md` — requirements and domain model (code comments cite `FR-0xx`
  / `data-model §x` tags that map back here).
- `contracts/cli.md` — CLI surface and exit-code contract.
- `contracts/config-schema.md` — full `license.toml` schema.
- `contracts/report.schema.json` — JSON output schema.

When changing behavior, update the spec/contracts alongside the code, and keep the
`FR-`/`§` references in doc comments accurate.

## Conventions

- Edition 2024, MSRV 1.89. Every module starts with a `//!` doc comment stating its role
  and the FR/spec tags it implements — preserve this when editing.
- Library errors are `thiserror` (`LicetError`); the binary maps them to `ExitCode`.
- Integration tests build throwaway git repos via the `Fixture` helper in
  `tests/common/mod.rs` and invoke the binary with `assert_cmd`. Test files are grouped by
  user story (`us1_`…`us5_`) plus cross-cutting suites (`safety`, `determinism`, `perf`,
  `completions`).
