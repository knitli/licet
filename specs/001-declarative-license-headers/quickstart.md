# Quickstart & Validation Guide

This guide proves the feature works end-to-end. Each scenario maps to a user story in
[spec.md](./spec.md) and the contracts in [contracts/](./contracts/). Commands use the
working binary name `licet` and the config schema in
[contracts/config-schema.md](./contracts/config-schema.md).

## Prerequisites

- Rust stable toolchain (1.83+) — `rustup show`
- A git repository to operate on (default coverage = tracked files)
- Build the tool: `cargo build --release` → `./target/release/licet`

## Setup: declare intent once (US1, FR-001)

Create `license.toml` at the repo root reproducing the user's Marque rule:

```toml
[default]
license = "MIT OR Apache-2.0"

[[rule]]
ext = "rs"
license = "LicenseRef-MarqueLicense-1.0"

[[rule]]
glob = "examples/**/*.rs"
license = "MIT OR Apache-2.0"

[[rule]]
glob = "specs/**"
license = "LicenseRef-MarqueLicense-1.0"

[[comment_style]]
ext = "pkl"
style = "c"

[exclude]
paths = ["vendor/**", "target/**"]
```

## Scenario 1 — See drift (US1 / SC-001, SC-003)

```bash
licet check
```

**Expect**: exit `1` with a per-file report. A Rust file under `examples/` carrying
`LicenseRef-MarqueLicense-1.0` is listed as `wrong_license` with declared
`MIT OR Apache-2.0` vs actual shown; a file with no header is `missing_header`; a file no
rule/default covers is `uncovered`; excluded paths are `excluded`, not flagged.

Machine form (validates against [report.schema.json](./contracts/report.schema.json)):

```bash
licet check --format json | jq '.summary.counts'
```

## Scenario 2 — Reconcile (US2 / SC-002, SC-004)

```bash
git add license.toml && git commit -m "Add license config"   # apply refuses on a dirty tree
licet apply            # destructive on license id by default; copyright preserved; atomic writes
licet check            # now exits 0
```

> `apply` refuses to modify a dirty working tree so `git diff`/`git checkout` is always a
> clean undo (FR-024). Use `licet apply --allow-dirty` to override (e.g. for a first run
> where you haven't committed `license.toml` yet).

**Expect**: every covered file matches declared intent; the re-run reports zero drift
(exit `0`). On a file that had both a license line and `SPDX-FileCopyrightText`, the
copyright line is **unchanged** (SC-004). Compare additive vs destructive on a
conflicting file:

```bash
licet apply --additive   # keeps the old license line AND adds declared → warns contradiction (FR-020)
licet apply --target-header 1   # replace the 2nd header block, not the first (FR-008)
```

## Scenario 3 — Unknown file type (US3 / SC-005)

With the `[[comment_style]] ext = "pkl"` entry already in config:

```bash
licet apply --files hk.pkl
licet check --files hk.pkl     # header round-trips; recognized, no per-file flag
```

**Expect**: `hk.pkl` gets a C-style-commented SPDX header on apply, and the subsequent
check recognizes it (compliant). No `-s style` flag needed on either run.

## Scenario 4 — Enforce in a hook / CI (US4 / SC-006, SC-009)

Commit-hook (staged subset) mode:

```bash
licet check --staged       # fast; blocks the commit on drift, names offending files
```

Full CI scan + performance gate:

```bash
time licet check           # full repo, single authoritative pass/fail
```

**Expect**: staged check is unobtrusive. Performance has **two** bars (SC-006): a repeat/
local run with a populated cache completes in **< 1s warm**; a fresh CI checkout (empty
cache — the case that actually governs CI) completes in **< 3s cold** on a 4-core runner.
Exit code is the gate: `0` pass, `1` fail. Drift output names files, the declared-vs-actual
difference, and — for a contributor who didn't author the rules — the remediation command.
A non-UTF-8 file is reported as `unreadable` and fails the gate (never silently skipped).

## Scenario 5 — REUSE compatibility & migration (US5 / SC-007, SC-008)

Bootstrap config from an existing REUSE project:

```bash
licet init --from-reuse    # derives license.toml from existing headers + REUSE.toml
```

Materialize any referenced-but-missing license texts into `LICENSES/` offline, without
touching source files (FR-029, the offline analog of `reuse download`):

```bash
licet add-license --all     # copy every missing referenced text from the embedded bundle
licet add MIT               # or materialize specific identifiers (alias `add`)
```

Confirm REUSE-spec compliance of a reconciled repo (offline by default, FR-017):

```bash
licet lint                 # LICENSES/ completeness, out-of-band coverage, missing texts
reuse lint                  # the upstream REUSE tool still reports the repo compliant
```

**Expect**: `init` produces a config whose projection reproduces current licensing
(SC-008); `add-license` populates `LICENSES/` from the offline bundle without modifying
sources (FR-029); `lint` resolves missing standard texts from the offline bundle; the
upstream `reuse lint` passes (SC-007).

## Validation checklist (maps scenarios → success criteria)

| Scenario | Proves | Success Criteria |
|----------|--------|------------------|
| 1 | Declarative drift report, no false "compliant" | SC-001, SC-003 |
| 2 | Reconcile to zero drift; copyright preserved | SC-002, SC-004 |
| 3 | New comment style persists & round-trips | SC-005 |
| 4 | Fast gate, clear pass/fail | SC-006, SC-009 |
| 5 | REUSE compliance + migration | SC-007, SC-008 |

Implementation-level test suites (unit, integration per story, conformance, perf) live in
`tests/` per [plan.md](./plan.md); this guide is the manual/CI end-to-end proof, not the
full test code.
