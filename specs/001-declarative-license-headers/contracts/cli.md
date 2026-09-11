# CLI Contract

The tool is a single binary named `licet` (Latin "it is permitted" — the root of
"license"). Text I/O contract:
arguments → stdout for results, errors/diagnostics → stderr. Every command accepts
`--format human|json` (default `human`) and `--config <path>` (default `./licet.toml`).

## Exit codes

| Code | Meaning | Used by |
|------|---------|---------|
| `0` | Success / fully compliant — no drift, no uncovered files | `check`, `apply`, `lint`, `add-license` |
| `1` | Drift or violations found (non-compliant) | `check`, `lint`, `add-license` |
| `2` | Usage / configuration error (bad flags, invalid config) | all |
| `3` | Partial apply — some files changed, some failed (FR-021) | `apply` |

`check` maps any of {WrongLicense, MissingHeader, Uncovered, Unreadable, unresolved
RuleConflict, missing license text} to exit `1`. `Uncovered` (FR-012a) and `Unreadable`
(FR-025) **count as failure**. `Excluded` does not.

`apply` exit semantics derive from one observation triple `(changed, operational_failure,
violations)` (FR-021): `3` when writes were applied and the tool itself also failed
(unreadable inputs, refused plan targets, failed writes, failed verification); `1` when
nothing was applied but the run failed operationally, or when drift or blocked
requirements remain — including writes that all succeeded but leave declaration drift
(additive contradictions, unfixable entries, missing required license texts); `0` only
when the gate passes with no operational failure. `Uncovered` (needs a config edit, not
a header) and `Unreadable` (non-UTF-8, never modified) are violations, not operational
failures. Dry-run never writes or fetches: `summary.pass` and `projected_pass` predict
the gate had the plan executed, and the exit mirrors the prediction (`0` iff projected
pass). Every planned, applied, failed, and blocked write is listed in `writes`, each
with its destination, kind, outcome, covered files, and — for text being written — the
exact bytes as text.

`add-license` exit semantics: `0` when every targeted text is present in `LICENSES/`
afterward; `1` when one or more requested/referenced texts could not be supplied (a
`LicenseRef-*` needing a maintainer-supplied text, or an unbundled standard id that was
not fetched), naming the identifier; `2` on flag misuse (neither identifiers nor `--all`
given, or both) and on invalid identifiers (unknown id, path escape, compound
expression) — raised before any directory is created or any download is attempted.

## Global flags

| Flag | Description |
|------|-------------|
| `--config <path>` | Path to declarative config (default `./licet.toml`). A legacy `./license.toml` is not defaulted to (exit 2 names the rename); an explicit path reads any name. |
| `--format human\|json` | Output rendering. `json` conforms to `report.schema.json`. |
| `--files <a> <b> …` / `--files-from <file>` / `-` (stdin) | Restrict evaluation to a supplied subset (FR-013). Paths are relative to the invocation cwd (or absolute), normalized lexically; outside-root and nonregular inputs are usage errors, symlinks are ignored like everywhere else. |
| `--staged` | Restrict to git-staged files (commit-hook mode, FR-013). `check --staged` evaluates **index blobs** — including staged metadata, config, and license texts — so the gate sees the commit as it would land; `apply --staged` uses the staged **path set** but edits working-tree files and never stages its edits. |
| `--changed [<rev>]` | Restrict to files changed vs `<rev>` (default `HEAD`; `<rev>` must resolve to a commit). Evaluates current working-tree bytes for those paths. |
| `--no-cache` / `--cache <path>` | Deprecated no-ops retained for one compatibility window (SC-006): scans are stateless and never create files; at most a stderr notice is printed. |
| `--explain <path>` | Resolve one path directly (no whole-tree scan, no cache writes): winning rule number/selector or default, losing matches with specificity, exclusions, metadata provenance, current drift (FR-002, FR-022). Respects an explicit selected set; a path outside it or not on disk is a usage error (exit `2`). Honors `--format json` with a single-file report. |
| `--version` | Print the tool version **and the embedded SPDX license-list version** (FR-028). |

**Selection flags are mutually exclusive** (FR-027): supplying more than one of
`--staged` / `--changed` / `--files`(/`--files-from`/stdin) is a usage error → exit `2`.

## `check` — non-writing gate (FR-012, FR-012a, FR-013; US1, US4)

```
licet check [--staged | --changed [<rev>] | --files …] [--format …]
```
- Never modifies files.
- Default coverage in a Git repository is **tracked regular files** (a later
  `.gitignore` rule cannot drop a tracked file; untracked files are not covered).
  Outside a repository, the nonignored filesystem walk is used.
- Projects config onto the selected files; classifies each as
  Compliant / WrongLicense / CopyrightMismatch / MissingHeader / Uncovered / Excluded (FR-004).
  License equality joins all effective expressions as one `AND` expression; copyright
  is compared separately against the policy.
- Requires the license texts referenced by the selected files (actual effective plus
  declared desired scope): missing texts fail the gate with a `missing_license_text`
  diagnostic. Texts of unmatched rules never enter the scope.
- Output names each offending file with **declared vs actual** identifiers (SC-009).
- When a staged/changed subset contains licensing metadata (config, `REUSE.toml`,
  `.reuse/dep5`, sidecars, `LICENSES/` texts) that can affect other files, the check
  conservatively expands to full tracked coverage and reports the expansion.
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
  uncommitted changes unless `--allow-dirty` is passed (exit `2` on refusal). A Git
  launch/status failure never means "clean" (exit `2`); outside a repository there is
  no Git undo guarantee, so `--allow-dirty` is required there too. Every write goes
  through one contained atomic writer (expected-bytes guard, no symlink traversal,
  permission preservation, fsync) so an interruption never leaves a file half-written.
- **Encoding (FR-025)**: non-UTF-8 files are never byte-edited. An uncovered one is reported
  as `Unreadable` and contributes to a non-zero exit; one already covered by a sidecar or
  REUSE.toml annotation is read through that coverage. Existing newline conventions (LF/CRLF)
  are preserved on write (FR-026).
- **Non-annotatable files (FR-015)**: files with no resolvable comment style (binaries,
  JSON, etc.) are covered out-of-band rather than byte-edited. By default `apply` writes a
  `<file>.license` sidecar (bare SPDX lines); `--non-annotatable reuse-toml` (or
  `[output] non_annotatable = "reuse-toml"`) appends an idempotent `REUSE.toml` annotation
  instead. A file already *correctly* covered out-of-band is left untouched; one covered but
  with the wrong license is reconciled where the coverage lives — a superseding exact-path
  `REUSE.toml` annotation is appended so it wins by last match (REUSE 3.3), carrying
  `precedence = "override"` when the file sits behind an `override` barrier. The existing
  document is never rewritten in place, so comments and unrelated stanzas survive
  byte-for-byte and a rerun converges to a no-op. Legacy `.reuse/dep5` coverage is flagged
  for manual fixup rather than rewritten. The flag overrides config.
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
  `LICENSES/` from the offline bundle; `LicenseRef-*` texts are never invented — the
  maintainer supplies them (FR-017).
- On partial failure: exit `3`, report changed vs unchanged files (FR-021).

**Acceptance**: destructive replaces a wrong `LicenseRef-MarqueLicense-1.0` under
`examples/` with `MIT OR Apache-2.0`; additive keeps both and warns; copyright lines
survive a license-only replace; a specific header can be targeted.

## `init` / `bootstrap` — derive config from current state (FR-018; US5)

```
licet init [--from-reuse] [--output <path>] [--config <path>] [--force] [--format …]
```
- Inspects existing headers and any `REUSE.toml`/`.reuse/dep5`, then generates an initial
  config whose projection reproduces the repository's current licensing (SC-008).
  Preservation outranks brevity: every observed file first becomes an exact-path
  rule (a root-level file is emitted as `file = "./name"` so it cannot govern
  deeper namesakes); rules compress to an extension group only when every
  observed path they would match carries the same license; a `[default]` is
  emitted only when every covered file is known, with exact exceptions for the
  rest. Previously unknown files remain unknown. Copyright is always `preserve`;
  no holder is ever guessed. The generated config is validated against every
  observation with the real rule resolver before anything is written, and a
  validation failure writes nothing.
- Does not modify source files; writes only the config. Destination: `--output`,
  else explicit `--config`, else `<root>/licet.toml` (explicit relative paths
  resolve from the invocation cwd; an explicit destination outside the project
  authorizes only that config file). Create-new is the default: an existing
  destination (file or symlink) is refused with exit `2` unless `--force`
  replaces exactly the bytes just observed. `--from-reuse` is a documented
  compatibility alias — inspection already covers REUSE state.
- `--format json` joins the report envelope (`command: "init"`, `summary.pass`
  = config written and projection verified, one `config` write record carrying
  the generated TOML, unknown paths as `missing_license` diagnostics).

## `lint` — REUSE-compatibility & license-text report (FR-014, FR-017; US5)

```
licet lint [--allow-network] [--config <path>]
```

- Reports REUSE conformance posture over **actual** metadata, independently of any
  declared policy: every covered file needs a license expression and a copyright
  notice (`missing_license` / `missing_copyright` diagnostics), malformed values
  are `invalid_license`, and unreadable inputs make validation incomplete
  (`read_error` / `unsupported_encoding`) rather than a proven violation or a pass.
- Covers tracked **plus** nonignored untracked files in a repository (the policy
  gate's tracked-only default does not apply), and never applies declaration
  `[exclude]` rules — a config exclusion cannot hide a file from whole-project
  REUSE validation. Needs no configuration: an auto-discovered `licet.toml` is
  just another covered file. An explicit `--config <path>` must exist and parse
  (exit `2` otherwise); it is accepted with a deprecation notice and never changes
  evaluation.
- Lists referenced-but-missing license texts, plus project-wide findings: unused
  texts, unrecognized `LICENSES/` entries, missing filename extensions, and
  undecodable texts. Duplicate ids under several filenames are usage error (exit
  `2`). Resolves known ids from the **offline bundle**; `--allow-network` permits
  fetching only ids absent from the bundle (FR-017).
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
  identifier in the union of desired (declared) and actual (detected) references
  that is **missing** from `LICENSES/` (the parallel of `reuse download --all`).
  A malformed policy config is usage error → exit `2`, never swallowed.
  Supplying neither — and not `--all` — is a usage error → exit `2`; supplying
  both is also exit `2`.
- Every requested identifier is validated before any directory is created or any
  download is attempted: unknown ids, path escapes (`../x`, absolute paths, separators),
  control characters, and compound expressions passed as one id are usage errors → exit
  `2`. Standard-id spelling is canonicalized (`mit` → `MIT`).
- `LicenseRef-*` identifiers are reported as required local texts for the maintainer to
  supply at `LICENSES/<id>.txt` (FR-017); they are never fetched and no placeholder prose
  is ever invented. An unsupplied custom text exits `1`, naming it.
- `--allow-network`: permits fetching valid-but-unbundled standard ids with the system
  `curl` binary into owned temporary storage (HTTPS-only, bounded execution, 4 MiB cap,
  nonempty UTF-8 validated) before installing through the safe writer; a failed download
  never deletes or truncates the destination. Without it, an unbundled non-`LicenseRef`
  identifier cannot be supplied and the run exits `1`, naming it (consistent with `lint`).
  Bundled texts never invoke the network.
- Writes **only** under `LICENSES/`: it never modifies source files or `licet.toml`, and
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
- **Symlink safety**: symlinks are never written through — a symlink destination or
  symlink ancestor aborts the write, and symlinked `LICENSES/` entries do not count as
  present texts. Callers must use the real path.
- **Contained atomic writes & non-destructive to copyright**: every mutation goes through
  one helper that confines the destination to its allowed root, requires the caller to
  state the expected current bytes (`None` = must not exist), preserves file permissions,
  fsyncs content plus the directory, and re-verifies the destination before renaming
  (FR-024); copyright/authorship is preserved by default (FR-009, SC-004).
- **Ignore blocks & snippets**: SPDX tags between `REUSE-IgnoreStart`/`REUSE-IgnoreEnd` are
  ignored during detection (unclosed → to end of input); SPDX-snippet licenses
  (`SPDX-SnippetBegin`..`SPDX-SnippetEnd`) never count as the file's license but are still
  referenced for `LICENSES/` completeness. Ignore blocks outrank snippet markers (FR-030).
- **Detection precedence**: when an in-file header (or `.license` sidecar) and a `REUSE.toml`
  annotation disagree, the annotation's REUSE 3.3 `precedence` decides — `closest` (default)
  keeps file-level info, `override` lets the annotation win (emitting a non-failing
  `source_override` warning), `aggregate` accepts both. Overlapping annotations resolve by
  last match. Legacy `.reuse/dep5` is aggregated with file-level information (FR-003a).
- **Cache fidelity**: a cache hit is observationally identical to a cold run; config, rule,
  comment-style, or version changes invalidate affected entries (FR-023, SC-011).
- **Version transparency**: `--version` and `lint` report the embedded SPDX list version
  (FR-028).
- **Path bases**: explicit file arguments are relative to the invocation cwd;
  declaration selectors and metadata paths are relative to their documented
  base. An omitted `--config` resolves from the discovered root
  (`<root>/licet.toml`); an explicit relative `--config` stays cwd-relative.
- **Output streams**: `--format json` prints exactly one serialized document to
  stdout; progress, prompts, and human diagnostics go to stderr. The tool never
  prompts implicitly — automation never stalls; network happens only with an
  explicit `--allow-network`. A closed stdout pipe terminates quietly (exit 0)
  instead of panicking; other output errors are reported normally.

## Examples

- Policy vs conformance: `licet check` gates declared policy over selected
  files (needs their license texts); `licet lint` validates actual REUSE 3.3
  metadata over the whole project, no config needed. A `preserve` project can
  pass `check` yet fail `lint` for missing copyright — run both.
- Snapshots: `licet check --staged` evaluates index bytes (what would land);
  `licet apply --staged` edits the working tree for the staged path set and
  never stages its edits (dirty-tree guard still applies).
- Scopes: from `pkg/`, `licet check` uses the root config by default, while
  `licet check --config ./local.toml` reads `./local.toml` under `pkg/`.
- Dirty non-repo: outside Git there is no undo guarantee, so `apply` requires
  `--allow-dirty` there just like for a dirty tree.
- Additive drift: `licet apply --additive` keeps old identifiers; when the
  combination still differs from intent it exits `1` with a residual-drift
  diagnostic — success of the writes, failure of the gate.
- Safe init: `licet init` refuses to overwrite `licet.toml`; re-run with
  `licet init --force`. With no `--output`/`--config` and only a legacy
  `license.toml` present, `init` refuses (exit `2`) instead of writing a
  competing default — rename first.
- Preview: `licet apply --dry-run` lists every planned/applied write with
  before/after text and exits nonzero when the projected gate fails — running
  the real `apply` afterwards produces those exact bytes.
