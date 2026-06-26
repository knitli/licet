# CLI Contract

The tool is a single binary named `licet` (Latin "it is permitted" — the root of
"license"). Text I/O contract:
arguments → stdout for results, errors/diagnostics → stderr. Every command accepts
`--format human|json` (default `human`) and `--config <path>` (default `./license.toml`).

## Exit codes

| Code | Meaning | Used by |
|------|---------|---------|
| `0` | Success / fully compliant — no drift, no uncovered files | `check`, `apply`, `lint`, `add-license` |
| `1` | Drift or violations found (non-compliant) | `check`, `lint`, `add-license` |
| `2` | Usage / configuration error (bad flags, invalid `license.toml`) | all |
| `3` | Partial apply — some files changed, some failed (FR-021) | `apply` |

`check` maps any of {WrongLicense, MissingHeader, Uncovered, Unreadable, unresolved
RuleConflict, missing license text} to exit `1`. `Uncovered` (FR-012a) and `Unreadable`
(FR-025) **count as failure**. `Excluded` does not.

`apply` exit semantics: `0` when the run leaves every selected file Compliant or Excluded;
`1` when files remain non-compliant that `apply` cannot fix by writing — specifically
**Uncovered** (needs a config edit, not a header) and **Unreadable** (non-UTF-8, never
modified); `3` when some writes succeeded and others failed (FR-021).

`add-license` exit semantics: `0` when every targeted text is present in `LICENSES/`
afterward; `1` when one or more requested/referenced texts could not be supplied (absent
from the bundle and not a `LicenseRef-*`, with `--allow-network` not given), naming the
identifier; `2` on flag misuse (neither identifiers nor `--all` given, or both).

## Global flags

| Flag | Description |
|------|-------------|
| `--config <path>` | Path to declarative config (default `./license.toml`). |
| `--format human\|json` | Output rendering. `json` conforms to `report.schema.json`. |
| `--files <a> <b> …` / `--files-from <file>` / `-` (stdin) | Restrict evaluation to a supplied subset (FR-013). |
| `--staged` | Restrict to git-staged files (commit-hook mode, FR-013). |
| `--changed [<rev>]` | Restrict to files changed vs `<rev>` (default `HEAD`). |
| `--no-cache` / `--cache <path>` | Disable or relocate the scan cache (SC-006). The cache key folds in file content + effective config + tool version, so it is never stale (FR-023, SC-011). |
| `--explain <path>` | Print which rule matched `<path>` and why (FR-002, FR-022). |
| `--version` | Print the tool version **and the embedded SPDX license-list version** (FR-028). |

**Selection flags are mutually exclusive** (FR-027): supplying more than one of
`--staged` / `--changed` / `--files`(/`--files-from`/stdin) is a usage error → exit `2`.

## `check` — non-writing gate (FR-012, FR-012a, FR-013; US1, US4)

```
licet check [--staged | --changed [<rev>] | --files …] [--format …]
```
- Never modifies files.
- Projects config onto the selected files; classifies each as
  Compliant / WrongLicense / MissingHeader / Uncovered / Excluded (FR-004).
- Output names each offending file with **declared vs actual** identifiers (SC-009).
- Exit `0` only if every selected file is Compliant or Excluded.

**Acceptance (from spec)**: drifted staged file → exit `1` with the offending file named;
compliant staged set → exit `0` quickly; full CI scan → single authoritative pass/fail.

## `apply` — reconcile to intent (FR-006, FR-007, FR-008, FR-009; US2)

```
licet apply [--additive] [--target-header <index>] [--allow-dirty]
            [--non-annotatable <sidecar|reuse-toml>] [selection flags] [--dry-run]
```
- **Default (no mode flag)**: **destructive on the license identifier** — replaces
  `SPDX-License-Identifier` to match config; **always preserves** copyright/authorship
  (FR-007, FR-009, SC-004).
- **Safety (FR-024, SC-010)**: refuses to modify files when the working tree has
  uncommitted changes unless `--allow-dirty` is passed (exit `2` on refusal). Every write
  is **atomic** (temp file + rename) so an interruption never leaves a file half-written.
- **Encoding (FR-025)**: non-UTF-8 files are never byte-edited. An uncovered one is reported
  as `Unreadable` and contributes to a non-zero exit; one already covered by a sidecar or
  REUSE.toml annotation is read through that coverage. Existing newline conventions (LF/CRLF)
  are preserved on write (FR-026).
- **Non-annotatable files (FR-015)**: files with no resolvable comment style (binaries,
  JSON, etc.) are covered out-of-band rather than byte-edited. By default `apply` writes a
  `<file>.license` sidecar (bare SPDX lines); `--non-annotatable reuse-toml` (or
  `[output] non_annotatable = "reuse-toml"`) appends an idempotent `REUSE.toml` annotation
  instead. A file already covered out-of-band is left untouched. The flag overrides config.
- `--additive`: adds the declared header without removing existing license lines; warns on
  resulting contradiction (FR-020).
- `--target-header <index>`: when a file has multiple headers, choose which block is
  replaced rather than only the first (FR-008). Default targets the primary header.
- `--dry-run`: compute and print the full `ReconciliationPlan` (every would-be write)
  without modifying any file. The two preview surfaces are distinct, not redundant:
  **`check`** answers "does this pass the gate?" (classification + exit code), while
  **`apply --dry-run`** answers "exactly what would `apply` change?" (per-file before/after).
- Writes missing headers in the file's resolved comment style (FR-011), respecting
  shebang/encoding first-lines (FR-019). Materializes missing standard license texts into
  `LICENSES/` from the offline bundle; scaffolds `LicenseRef-*` placeholders (FR-017).
- On partial failure: exit `3`, report changed vs unchanged files (FR-021).

**Acceptance**: destructive replaces a wrong `LicenseRef-MarqueLicense-1.0` under
`examples/` with `MIT OR Apache-2.0`; additive keeps both and warns; copyright lines
survive a license-only replace; a specific header can be targeted.

## `init` / `bootstrap` — derive config from current state (FR-018; US5)

```
licet init [--from-reuse] [--output <path>]
```
- Inspects existing headers and any `REUSE.toml`/`.reuse/dep5`, then generates an initial
  `license.toml` whose projection reproduces the repository's current licensing (SC-008).
- Does not modify source files; writes only the config (and reports what it inferred).

## `lint` — REUSE-compatibility & license-text report (FR-014, FR-017; US5)

```
licet lint [--allow-network]
```
- Reports REUSE conformance posture: SPDX headers present, `LICENSES/` completeness,
  out-of-band coverage for non-annotatable files.
- Lists referenced-but-missing license texts. Resolves known ids from the **offline
  bundle**; `--allow-network` permits fetching only ids absent from the bundle (FR-017).
- Reports the embedded **SPDX license-list version** so the compliance posture is auditable
  (FR-028).
- Exit `1` if the repository would not pass a REUSE-spec compliance check.

## `add-license` (alias `add`) — materialize license texts offline (FR-017, FR-029; US5)

```
licet add-license [<SPDX-ID> …] [--all] [--allow-network] [--format …]
licet add <SPDX-ID> …
```
- Copies referenced license texts into `LICENSES/` from the **embedded bundle** — the
  offline analog of REUSE's `download`. Because the SPDX corpus is embedded, this is a copy
  from the bundle, never a network fetch for a bundled identifier (offline-guard invariant).
- `<SPDX-ID> …`: materialize exactly these identifiers. `--all`: materialize every
  identifier referenced by the config and existing headers that is **missing** from
  `LICENSES/` (the parallel of `reuse download --all`). Supplying neither — and not
  `--all` — is a usage error → exit `2`; supplying both is also exit `2`.
- `LicenseRef-*` identifiers are scaffolded as empty placeholder texts for the maintainer to
  fill in (FR-017); they are never fetched.
- `--allow-network`: permits fetching identifiers absent from the bundle; without it, an
  unbundled non-`LicenseRef` identifier cannot be supplied and the run exits `1`, naming it
  (consistent with `lint`).
- Writes **only** under `LICENSES/`: it never modifies source files or `license.toml`, and
  therefore — unlike `apply` — does **not** require a clean working tree. (`apply` still
  materializes texts as a side-effect of annotating; `add-license` exposes that
  materialization standalone, without touching headers.)
- `--format json` emits a report listing the referenced/present/missing/materialized texts
  and the embedded SPDX list version, reusing the `license_texts` shape of `lint`.

## Behavior guarantees (cross-command)

- **Deterministic**: same inputs → same classification, ordering, and exit code (FR-002).
- **Offline by default**: no network unless `--allow-network` is passed (FR-017).
- **Subset honoring**: when a selection flag is given, only those files are evaluated
  (FR-013), but rule precedence still considers the full ruleset. Selection flags are
  mutually exclusive (FR-027).
- **Symlink safety**: a file reached via symlink is annotated once (Edge Cases).
- **Atomic & non-destructive to copyright**: writes are temp-file-plus-rename (FR-024);
  copyright/authorship is preserved by default (FR-009, SC-004).
- **Detection precedence**: when an in-file header and an out-of-band entry disagree, the
  out-of-band value is authoritative for the detected license and a non-failing
  `source_override` warning is emitted (FR-003a).
- **Cache fidelity**: a cache hit is observationally identical to a cold run; config, rule,
  comment-style, or version changes invalidate affected entries (FR-023, SC-011).
- **Version transparency**: `--version` and `lint` report the embedded SPDX list version
  (FR-028).
