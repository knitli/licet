# licet

> Latin *licet* — "it is permitted"; the root of *license*.

A single-binary CLI that manages SPDX/REUSE-compatible license and copyright metadata
for an entire repository from **one declarative configuration**. You declare intent once
in `license.toml`; `licet` projects that intent onto the working tree, reports drift, and
reconciles files to match — destructive on the license identifier by default, additive
opt-in, copyright always preserved.

It is offline and hermetic by default (SPDX license texts are embedded in the binary),
fast enough for commit hooks and CI, and its output stays compatible with the upstream
[REUSE](https://reuse.software) tool.

## Install

```bash
cargo build --release    # → target/release/licet
```

Requirements: Rust 1.83+. A git repository (default coverage = tracked files).

## Configure: declare intent once

`license.toml` at the repo root is the **only** authoring surface for licensing intent.
Rules are ordered; specificity is `file` > `glob` > `ext`, with declaration order breaking
ties. `REUSE.toml`/`.reuse/dep5` are read for interop/detection only.

```toml
[default]
license = "MIT OR Apache-2.0"        # SPDX expression or LicenseRef-*
copyright = "preserve"               # preserve (default) | add:<text> | replace:<text>

[[rule]]
ext = "rs"
license = "LicenseRef-MarqueLicense-1.0"

[[rule]]
glob = "examples/**/*.rs"            # more specific than a bare ext
license = "MIT OR Apache-2.0"

[[comment_style]]
ext = "pkl"
style = "c"                          # built-in style name, or an inline table

[exclude]
paths = ["vendor/**", "target/**"]
```

See [`contracts/config-schema.md`](specs/001-declarative-license-headers/contracts/config-schema.md)
for the full schema.

## Commands

| Command | Purpose |
|---------|---------|
| `licet check` | Non-writing gate: classify drift, exit pass/fail. |
| `licet apply` | Reconcile files to declared intent (destructive by default). |
| `licet init`  | Derive a `license.toml` from current repository state. |
| `licet lint`  | Report REUSE-compatibility posture & license-text completeness. |

### `check` — the gate

```bash
licet check                 # full repo
licet check --staged        # commit-hook mode (git-staged subset)
licet check --changed HEAD  # only files changed vs a rev
licet check --files a.rs b.rs
licet check --format json   # machine output (report.schema.json)
licet check --explain examples/demo.rs   # which rule won, and why
```

Exit codes: `0` compliant · `1` drift/violations · `2` usage/config error · `3` partial apply.
`uncovered` and `unreadable` (non-UTF-8) count as failures; `excluded` does not.

### `apply` — reconcile

```bash
licet apply                 # destructive on the license id; copyright preserved; atomic writes
licet apply --additive      # keep existing license line AND add the declared one (warns on contradiction)
licet apply --target-header 1   # replace the 2nd header block, not the first
licet apply --dry-run       # print the full plan without writing
```

`apply` refuses to modify a dirty working tree unless `--allow-dirty`, so `git checkout`
is always a clean undo. Writes are atomic (temp-file + rename). Missing standard license
texts are materialized into `LICENSES/` from the offline bundle; `LicenseRef-*` texts are
scaffolded as placeholders.

### `init` / `lint`

```bash
licet init --from-reuse     # bootstrap license.toml from existing headers + REUSE.toml
licet lint                  # LICENSES/ completeness, missing texts, SPDX list version
licet --version             # tool version + embedded SPDX license-list version
```

## Behavior guarantees

- **Deterministic**: same inputs → same classification, ordering, and exit code.
- **Offline by default**: no network unless `--allow-network` is passed to `lint`.
- **Copyright-safe**: copyright/authorship preserved across a license-only replace.
- **Detection precedence**: when an in-file header and out-of-band metadata disagree, the
  out-of-band value wins and a non-failing `source_override` warning is emitted.
- **Cache fidelity**: the scan cache key folds in file content + effective config + tool
  version, so a hit is observationally identical to a cold run.
- **Line endings**: LF/CRLF preserved on write; non-UTF-8 files are never byte-edited.

## Development

```bash
cargo test            # unit + integration + conformance + perf
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The engine is a library (`src/lib.rs`) decomposed by pipeline stage — `walk` →
`detect` + `rules` → `report` (classify) → `reconcile` — with `config`, `comment`,
`spdx`, and `reuse` as cross-cutting support modules; the binary (`src/main.rs`) is a thin
shell. Design docs live under
[`specs/001-declarative-license-headers/`](specs/001-declarative-license-headers/).

## License

`MIT OR Apache-2.0`.
