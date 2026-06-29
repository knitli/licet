# Phase 0 Research: Declarative License Header Management

This document resolves the technical unknowns implied by the spec's Technical Context.
Each decision lists rationale and the alternatives rejected.

## 1. Implementation language & runtime

**Decision**: Rust (stable, 2021 edition), shipped as a single self-contained binary.

**Rationale**:
- **Speed (SC-006, grievance #3)**: The spec requires a full scan of a ~10,000-file repo
  in **< 1s warm**. The current Python `reuse` tool is "a bump on the dev process";
  beating it decisively needs a compiled language with cheap startup (no interpreter
  warm-up) and easy data parallelism. Rust + `rayon` over an `ignore`-based walker is a
  proven pattern (ripgrep scans far larger trees in well under a second).
- **Hermetic/offline (FR-017)**: Rust can **embed the entire SPDX license-text corpus**
  into the binary at build time (`include_dir!`/generated table), so known identifiers
  need no network and CI stays hermetic. No runtime package manager or interpreter.
- **Single-binary distribution**: Drops into `hk`/pre-commit hooks and CI without a
  language runtime, virtualenv, or dependency resolution step.
- **Ecosystem fit**: First-class crates exist for every hard part — `spdx` (semantic
  license-expression handling), `ignore` (gitignore-aware parallel walk), `gix` (pure-Rust
  git), `globset` (matching), `serde`/`toml` (config).
- **User fit**: The user already works in Rust (the Marque example is a Rust project), so
  a Rust tool fits their environment and build conventions.

**Alternatives considered**:
- **Python** (like REUSE itself): rejected — interpreter startup + GIL make the <1s/10k
  target and "no bump in the dev loop" hard; distribution needs a runtime.
- **Go**: viable (fast, single binary, easy concurrency) but the SPDX-expression and
  gitignore-walk ecosystem is weaker/less precise than Rust's `spdx`/`ignore`, and
  embedding + semantic license comparison are less turnkey.
- **Node/TypeScript**: rejected — startup cost and runtime dependency; weakest fit for the
  performance bar.

## 2. SPDX license-expression comparison (FR-005)

**Decision**: Use the `spdx` crate to parse expressions into an AST and compare
**semantically** (normalize operator structure; treat `A OR B` ≡ `B OR A`), not by raw
string.

**Rationale**: The spec demands `MIT OR Apache-2.0` equal its reordered/whitespace
variants (Edge Cases; SC-003). The `spdx` crate validates against the official SPDX
license list, parses compound expressions (`AND`/`OR`/`WITH`/`+`), and exposes the parsed
form for canonicalization. Bundling its license list also feeds the embedded-text
inventory.

**Scope (matches FR-005)**: comparison is canonical-form — equal up to commutativity,
associativity, whitespace, parenthesization, and case-insensitive identifiers. Full
logical/distributive equivalence (`A AND (B OR C)` ≡ `(A AND B) OR (A AND C)`) is an
explicit **non-goal** for v1; FR-005 is worded to claim only what canonical form delivers.

**Alternatives**: Hand-rolled string normalization (rejected — fragile against
parenthesization and `WITH` exceptions); full SAT-style license equivalence (rejected as
over-engineered for v1 — canonical-form comparison covers the spec's examples).

## 3. File enumeration & changed-file subset (FR-013, "tracked files" assumption)

**Decision**: Default coverage = git-tracked files, enumerated via `gix`; traversal and
ignore handling via the `ignore` crate. A `--files`/stdin list and a `--staged`/changed
mode restrict evaluation to a supplied subset.

**Rationale**: The spec's assumption is "tracked files define coverage by default;
untracked/ignored not enforced unless configured." `gix` is pure-Rust (no libgit2 link →
hermetic, faster, simpler cross-compilation) and gives the index/HEAD diff for staged and
changed sets. `ignore` provides parallel, `.gitignore`-respecting walking for the
non-git or full-tree paths and is the fastest available walker.

**Alternatives**: `git2` (libgit2) — works but adds a C dependency and link complexity;
shelling out to `git` — rejected (process overhead per call, fragile parsing, breaks the
single-binary/hermetic story). Keep `git2` as a fallback only if a `gix` capability gap
appears.

## 4. Comment-style registry & persistence (FR-010, FR-011, grievance #4)

**Decision**: A built-in table of comment styles keyed by extension/filename (seeded to at
least REUSE's known set), overlaid by **user-defined associations stored in the config**
(`license.toml`). Resolution precedence: exact filename → extension → built-in default.
Styles are expressed as a small set of primitives (line-prefix, block start/end/optional
line-prefix) so new languages are declared by data, not code.

**Rationale**: Directly fixes the `hk.pkl`/pkl-lang gap — "use C-style comments for
`.pkl`" becomes a config entry that **persists** across runs (unlike REUSE's transient
`annotate -s`). Filename-specific precedence satisfies acceptance scenario 4 of Story 3.
A primitive-based style model means associations are reusable (point `.pkl` at the
built-in `c` style, or define a new one inline).

**Alternatives**: Hard-coded style table only (rejected — reproduces REUSE's
non-extensibility); per-file flags (rejected — the user explicitly dislikes the
no-persistence behavior).

## 5. Additive vs destructive reconciliation & targeted headers (FR-007, FR-008, FR-009, grievance #5)

**Decision**: Model a file's header region as a structured set of SPDX tag lines
(`SPDX-License-Identifier`, `SPDX-FileCopyrightText`, plus contributor lines). `apply`
operates on this model:
- **Destructive (default for the license id)**: replace the `SPDX-License-Identifier`
  value to match config; **always preserve** `SPDX-FileCopyrightText`/authorship lines.
- **Additive (opt-in)**: append the declared identifier without removing existing ones;
  emit a contradiction warning when two differing licenses coexist (FR-020).
- **Targeting (FR-008)**: when multiple header blocks exist, allow selecting which block's
  license is replaced rather than only the first — enabled because the structured model
  indexes every header occurrence, not just the leading one.

**Rationale**: Resolves the user's "destructive vs additive is unclear and only hits the
first header" complaint. Because intent comes from the declarative config, the default can
safely be destructive-on-license while still never erasing copyright (SC-004) — the
distinction the user found murky in REUSE becomes an explicit, defaulted mode.

**Alternatives**: Whole-header text replace (rejected — would clobber copyright, violating
SC-004); first-header-only (rejected — that's the REUSE limitation being fixed).

## 6. Header insertion position (FR-019)

**Decision**: A first-line-aware inserter: detect and skip shebang (`#!...`), encoding/XML
declarations, BOM, and other required-first lines, inserting the header immediately after
them using the file's resolved comment style.

**Rationale**: Edge Cases require headers not to break shebangs/encoding directives. This
is a well-understood placement problem; encode the known "must stay first" patterns per
comment style.

**Encoding & line endings (FR-025, FR-026)**: Before any write, validate the file is UTF-8.
Non-UTF-8 files (UTF-16, Latin-1, …) are **never byte-edited** — byte-offset insertion
would corrupt them — so they are detected, skipped, classified `Unreadable`, warned, and
counted as a gate failure; transcoding is deferred beyond v1. On files that are written,
the existing newline convention (LF vs CRLF) is detected and preserved, and no mixed
endings are introduced, so reconciliation produces a minimal diff.

**Alternatives**: Always insert at byte 0 (rejected — breaks executables and encoding
sniffers). Best-effort transcode-and-reencode non-UTF-8 files (rejected for v1 — adds
encoding-detection risk for a rare case; skip-and-fail is the safe default).

## 7. REUSE compatibility & out-of-band metadata (FR-014, FR-015, FR-003a)

**Decision**: Conform to the **REUSE Specification 3.x**: in-file `SPDX-License-Identifier`
+ `SPDX-FileCopyrightText` headers, a `LICENSES/` directory holding each referenced
license text, and **`REUSE.toml`** (current spec) — with `.reuse/dep5` read for backward
compatibility — for files that cannot carry inline headers (binaries/images). Drift
detection reads the actual license from **either** in-file headers **or** these out-of-band
sources (FR-003a).

**Rationale**: Protects the user's REUSE investment (SC-007) and handles non-annotatable
files (FR-015) the same way REUSE does. Reading both header and out-of-band sources means
a file licensed via `REUSE.toml` is not falsely reported as "missing."

**Precedence on disagreement (FR-003a)**: when an in-file header and an out-of-band entry
disagree on a file's license, the **out-of-band entry is authoritative** for the detected
actual license, and a non-failing `source_override` diagnostic is emitted. `REUSE.toml`/
`dep5` are treated as **interop/detection surfaces only**, never an authoring surface —
declarative intent is authored solely in `license.toml` (the maintainer's position is that
`REUSE.toml` is not designed for declarative entries).

**Alternatives**: Invent a proprietary sidecar format (rejected — breaks REUSE
compatibility, the user's stated value). Flag header/out-of-band disagreement as a
gate-failing conflict (rejected — the maintainer chose out-of-band-wins with a non-failing
diagnostic). Let the in-file header win (rejected — same decision).

## 8. Bundled license texts & acquisition (FR-017)

**Decision**: Embed the full SPDX license-text set in the binary at build time. On
`apply`/`add-license`, materialize any referenced-but-missing standard text into `LICENSES/`
from the bundle **offline**. A text that cannot be produced is **never** stubbed with a
placeholder — a stub would falsely pass REUSE's text-existence check and read as compliant.
Instead the run errors with guidance: standard ids give the exact SPDX download URL,
`LicenseRef-*` ids name the `LICENSES/<id>.txt` path to create. Fetching is opt-in and only
for standard ids absent from the bundle: the binary ships no network capability (offline-guard
invariant), so `--allow-curl` (or an interactive y/N) shells out to the user's own `curl`.

**Rationale**: Implements the clarified offline-by-default posture (hermetic CI) while
still supporting unusual/custom identifiers. Embedding avoids any runtime download for the
common case.

**Versioning (FR-028)**: the embedded corpus is a point-in-time SPDX-list snapshot. The
list version is recorded at build time and surfaced via `--version` and `lint` so operators
can audit which corpus a binary carries; refreshing it is a rebuild/release step, not a
runtime fetch. Placeholder `LicenseRef-*` files satisfy REUSE's text-existence check but are
flagged by `lint` until filled — so SC-007 holds for standard ids while custom refs remain
visibly incomplete until the maintainer supplies text.

**Alternatives**: Always fetch from the web (rejected — breaks hermetic CI, the clarified
requirement); ship texts as loose files alongside the binary (rejected — defeats
single-binary distribution).

## 9. Rule precedence & conflict surfacing (FR-002, FR-022)

**Decision**: Rules carry a computed **specificity** (exact filename > narrower glob >
extension > default); ties broken by **declaration order**, and **equal-specificity
matches on the same file are surfaced as a reported conflict**, not silently resolved.
An `--explain <path>` mode prints which rule won and why.

**Rationale**: Determinism (FR-002) plus the explicit "surface conflicts" requirement
(FR-022) and the Edge Case on equal-specificity rules. `--explain` makes projection
auditable.

**Alternatives**: Last-match-wins with no reporting (rejected — hides conflicts the spec
requires surfacing).

## 10. Performance approach & cache correctness (SC-006, FR-023, SC-011)

**Decision**: Parallel pipeline — `ignore`/`gix` enumerate files; `rayon` classifies them
concurrently; only each file's **head** (first few KB) is read for header detection via
`memchr`/`bstr` (with a UTF-8 validity check — see §6). An on-disk classification **cache**
delivers the "warm" path. Apply writes are batched, atomic, and parallel-safe (see §12).

**Two performance bars (SC-006)**:
- **Warm** (repeat/local, populated cache): full 10k-file scan **< 1s**.
- **Cold** (empty cache, e.g. fresh CI checkout): full 10k-file scan **< 3s** on a 4-core
  2020-era runner. This is the bar that actually governs CI, where the cache never exists.

**Cache key (FR-023, SC-011)** — the cache is **not** keyed on file content alone. The key
folds in: file content hash **+** a fingerprint of the effective configuration (rules,
default, comment-style registry, exclusions) **+** the tool version. Any change that could
alter a file's classification invalidates the affected entries, so a cache hit is
observationally identical to a cold computation and can never produce a stale
false-`Compliant`. Cache location/format remains an implementation detail (deferred below).

**Rationale**: Header detection needs only the file head, so IO and CPU stay minimal; the
warm cache makes repeat runs near-instant while the cold bar keeps CI honest. Mirrors how
ripgrep sustains this throughput. A content-only cache key was the obvious trap (config
edits would silently reuse old verdicts) — folding config+version in closes it.

**Alternatives**: Sequential whole-file reads (rejected — won't meet the budget on 10k
files); content-only cache key (rejected — stale after config changes, violates SC-011);
no cold bar (rejected — leaves the primary CI path ungated).

## 12. Apply safety: atomicity & working-tree guard (FR-024, SC-010)

**Decision**: Destructive `apply` refuses to run when the git working tree has uncommitted
changes unless `--allow-dirty` is passed (so the user always has `git diff`/`git checkout`
as an undo). Each file modification is **atomic**: write the new content to a sibling temp
file, fsync, then rename over the original. A run that fails partway reports exactly which
files changed and which did not (FR-021), and never leaves a source file half-written.

**Rationale**: A default-destructive mass rewrite needs a safety net. Git cleanliness + VCS
diff is the undo mechanism; atomic rename is the crash-safety mechanism. Together they make
the dangerous default acceptable (SC-010).

**Alternatives**: In-place truncate-and-write (rejected — a crash mid-write corrupts
source); a bespoke backup directory (rejected — git already is the backup; `--allow-dirty`
covers non-git or intentional cases); making additive the default (rejected — the clarified
decision is destructive-by-default, FR-007).

## 11. Configuration format (FR-001, FR-010)

**Decision**: A single TOML file at repo root (working name `license.toml`) holding the
ordered rules, the repo default, comment-style associations, and exclusions.

**Rationale**: TOML matches the REUSE ecosystem (`REUSE.toml`) the user already uses,
serializes cleanly with `serde`, and is human-editable for the "declare intent once"
workflow. Concrete schema is defined in `contracts/config-schema.md`.

**Alternatives**: YAML (rejected — whitespace fragility, less aligned with REUSE.toml);
a bespoke DSL (rejected — needless parser + learning cost for v1).

## Open items deferred to implementation

- Final crate selection between `gix` and `git2` pending a spike on staged-diff ergonomics
  (default `gix`).
- Exact on-disk cache format/location (e.g., under `.git/` vs a dotfile) — an
  implementation detail that does not affect external contracts. (The cache **key**,
  however, is a contract: it must fold in config + tool version per FR-023/§10.)
- Whether the binary name and config filename are user-overridable (cosmetic; default
  `license.toml`).
