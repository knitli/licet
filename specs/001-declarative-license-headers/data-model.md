# Phase 1 Data Model: Declarative License Header Management

Derived from the spec's **Key Entities** and Functional Requirements. These are the
domain types the engine operates on (language-agnostic; the Rust types mirror them).

## Entity overview

```text
LicensingConfiguration
├── default: LicenseIntent
├── rules: [Rule]                       (ordered)
├── comment_styles: [CommentStyleAssociation]
└── exclusions: [PathPattern]

Per-file pipeline:
  File ──walk──▶ matched by Rule(s) ──▶ DeclaredIntent
       ──detect─▶ ActualLicenseState
  (DeclaredIntent vs ActualLicenseState) ──▶ FileLicensingState{drift}
  Σ FileLicensingState ──▶ Report / ReconciliationPlan
  Σ referenced ids ──▶ LicenseTextInventory
```

---

## 1. LicensingConfiguration

The single declarative source of truth (FR-001). Loaded from `license.toml`.

| Field | Type | Notes |
|-------|------|-------|
| `default` | `LicenseIntent` | Repository-wide fallback applied when no rule matches. Optional; absence means "uncovered files are possible." |
| `rules` | ordered list of `Rule` | Declaration order is significant for tie-breaking (FR-002). |
| `comment_styles` | list of `CommentStyleAssociation` | User-defined associations overlaid on built-ins (FR-010). |
| `exclusions` | list of `PathPattern` | Paths explicitly removed from coverage (FR-016). |
| `non_annotatable` | `NonAnnotatableStrategy` | How `apply` covers files that can't carry a header: `Sidecar` (default, writes `<file>.license`) or `ReuseToml` (appends a `REUSE.toml` annotation). From `[output] non_annotatable`; overridable per-run with `--non-annotatable` (FR-015). |

**Validation**: every license expression referenced (in `default`/`rules`) must parse as a
valid SPDX expression or a `LicenseRef-*`; unknown bare identifiers are a config error.
Comment-style references must resolve to a built-in or inline-defined style.

---

## 2. Rule

A matcher paired with the intent it confers (FR-001, FR-002).

| Field | Type | Notes |
|-------|------|-------|
| `selector` | `Selector` | One of: extension, glob, or exact filename/path. |
| `intent` | `LicenseIntent` | License expression + copyright policy applied to matches. |
| `specificity` | derived integer | exact-path/filename > narrow glob > extension > default. Used for precedence. |
| `source_order` | derived integer | Position in config; breaks specificity ties (FR-002). |

**Selector** (one of):
- `Extension(string)` — e.g. `rs`, `pkl`
- `Glob(pattern)` — e.g. `examples/**/*.rs`, `vendor/**`
- `ExactPath(path)` / `Filename(name)` — e.g. `hk.pkl`, `README.md`. A `file`
  value with a leading `./` normalizes to `ExactPath` without the prefix, so
  `./Makefile` pins the root file while bare `Makefile` matches any directory.

**State/derivation rules**:
- Resolution picks the highest `(specificity, then earliest source_order)` match.
- Two matches of **equal specificity** on one file → emit a `RuleConflict` (FR-022) rather
  than silently choosing.

---

## 3. LicenseIntent

The licensing outcome a rule/default confers.

| Field | Type | Notes |
|-------|------|-------|
| `license_expression` | SPDX expression | e.g. `LicenseRef-MarqueLicense-1.0`, `MIT OR Apache-2.0`. |
| `copyright_policy` | enum `{ Preserve, PreserveAndAdd(text), Replace(text) }` | Default `Preserve` — copyright is never erased by a license change (FR-009, SC-004). |

---

## 4. Comment registry & CommentSyntax

Maps file selectors to the comment syntax used to read/write headers (FR-010, FR-011).

**Comment** (one built-in registry row per language family — pure data, so adding a
language is a new row, never new code)

| Field | Type | Notes |
|-------|------|-------|
| `family` | string | Human-facing label, e.g. `"C"`, `"css"`, `"hash"` (diagnostics only — not a lookup key). |
| `extensions` | list of string | Bare extensions (no leading dot) this row covers. The extension tables live in `src/comment/extensions.rs` (pure data). |
| `filenames` | list of string | Exact filenames this row covers, e.g. `Makefile`, `Dockerfile`. |
| `aliases` | list of string | Names usable from config `style = "..."`, e.g. `c`, `hash`, `slashes`. |
| `syntax` | `CommentSyntax` | The comment syntax this family renders/parses. |

The three lookup namespaces (filename, extension, alias) are indexed independently — a
config `style = "c"` (alias) and a `.c` file (extension) are resolved through separate
indices, even when (as here) they map to the same family.

Block-only languages whose only portable form is `/* … */` (CSS/LESS/SASS/SCSS, GAS
assembly, linker scripts) live in the `css` family; C/C++ and the curly-brace family that
support both `//` and `/* … */` live in `C`. A language is placed by the forms it actually
supports, not by file-name resemblance (e.g. `.scss` is block-only even though it accepts
`//`, matching the conservative render choice).

**CommentSyntax** — a sum type so a language's support is exhaustive and illegal states
(neither form, or a half-specified block) are unrepresentable:

| Variant | Carries | Example |
|---------|---------|---------|
| `LineOnly` | `LineStyle` | `#` (Python, YAML) |
| `BlockOnly` | `BlockStyle` | `<!-- … -->` (HTML, Markdown) |
| `Both` | `LineStyle` + `BlockStyle` | `//` + `/* … */` (C, Rust, JS) |

- **LineStyle** — `prefix` (e.g. `//`, `#`, `;`).
- **BlockStyle** — `open` / `close` delimiters (e.g. `/*` … `*/`) and `line_prefix`, the
  **internal alignment** prefix applied to each content line (e.g. ` * ` for a C block;
  empty when the block carries no per-line decoration).

**Render policy** (FR-011): when a style supports both forms, the **line** form is
preferred (REUSE convention is a single `# SPDX-License-Identifier:` line); the block
form is used only when it is the sole option.

**Parse/render invariant**: detection parses a **superset** of what rendering emits, so a
header `licet` writes is always recognized again on the next scan (never re-flagged as
missing/wrong). Enforced by a round-trip test over every built-in style.

**CommentStyleAssociation** — user override overlaid on built-ins:

| Field | Type | Notes |
|-------|------|-------|
| `selector` | `Selector` | Filename selector takes precedence over extension (FR-011). |
| `style` | `CommentStyleRef` | A built-in alias (`Named`) or an inline `CommentSyntax` (`Inline`). |

**Resolution precedence** (FR-011): exact filename association → extension association →
built-in filename → built-in extension. Built-ins seed at least the REUSE-known set.

---

## 5. ActualLicenseState

What is really present for a file, gathered by detection (FR-003a).

| Field | Type | Notes |
|-------|------|-------|
| `headers` | list of `HeaderBlock` | File-level SPDX header occurrences. Parsed from the complete file content (no head cutoff); when a `<file>.license` **sidecar** exists, its headers are used instead (the REUSE spec treats sidecar content as "inside the file"), so a binary asset can be covered without byte access. Tags inside `REUSE-IgnoreStart`/`REUSE-IgnoreEnd` and inside SPDX snippets are excluded from this list (FR-030). Rejected `SPDX-License-Identifier` values are kept with line + reason for diagnosis, never dropped silently. |
| `snippet_licenses` | list of SPDX expression | Licenses declared inside `SPDX-SnippetBegin`..`SPDX-SnippetEnd` regions. These describe snippets, not the file, so they never affect drift — but they are added to the referenced license-text set for `LICENSES/` completeness (FR-030). |
| `out_of_band` | optional `OutOfBandEntry` | Hierarchy-resolved `REUSE.toml`/`.reuse/dep5` coverage for this path (unconditional values + per-field `closest` fallbacks + contributing-table provenance). Read for interop/detection only — never an authoring surface. |
| `detected_license` | optional SPDX expression | The primary resolved license (first file-level, else first unconditional OOB, else first fallback). |
| `detected_source` | optional `ActualSource` | One of `Header`, `Sidecar` (`license_file`), `ReuseToml`, `Dep5`. |
| `detected_copyrights` | list of string | Effective copyrights: file-level plus unconditional OOB notices, plus the `closest` fallback only when the file carries none. Raw suppressed notices stay in `headers` for preservation. |
| `encoding_ok` | bool | False only when the asset is not valid UTF-8 **and** has no sidecar/out-of-band coverage; drives `Unreadable` (FR-025). A non-UTF8 binary covered by a sidecar or annotation is readable. |

**Precedence (FR-003a)** — `REUSE.toml` documents are discovered at every directory
depth and consulted root-first; each document contributes exclusively its last matching
table, and consultation stops after the rootmost `override` table (verified against the
reference REUSE 6.2.0 tool). License and copyright resolve independently:

- The rootmost `override` table (the *barrier*) suppresses file/sidecar info and every
  deeper table — even for a field the table omits (an omitted field stays missing; it
  never reopens suppressed sources). Shallower `aggregate` tables are still retained,
  and shallower `closest` tables still serve as the per-field fallback. Emits
  `source_override` on disagreement with the suppressed header.
- Every consulted `aggregate` table always contributes its licenses and copyrights.
- `closest` tables are a per-field fallback: the nearest-outward table supplying a
  license (resp. copyright) fills that field only when file-level info — or, under a
  barrier, nothing — provides none. A file carrying exactly one field still takes the
  other's fallback.
- `.reuse/dep5` paragraphs are `aggregate` contributors; the last matching paragraph
  governs a path. `REUSE.toml` and `.reuse/dep5` are mutually exclusive — their
  coexistence, like any malformed document (bad TOML, `version` other than 1, missing
  `path`, invalid `precedence`, invalid license expression), fails the scan before any
  write, naming the document.

`candidate_licenses()` is the single precedence-aware resolver both `classify` and
`reconcile` consult, so the rule is applied in exactly one place. `REUSE.toml` path
patterns use the REUSE grammar (`*` never crosses `/`, `**` does, only `\`-escapes are
special, `?[]{} ` are literal); dep5 `Files:` patterns are shell-style (`*` crosses `/`).

**HeaderBlock**

| Field | Type | Notes |
|-------|------|-------|
| `byte_range` | span | Location in the file (enables targeted replacement — FR-008). |
| `comment_style` | `CommentStyleRef` | Style the block was written in. |
| `license_ids` | list of SPDX expression | Usually one; multiple ⇒ potential contradiction (FR-020). |
| `copyrights` | list of string | Preserved across destructive license replacement. |
| `position_after` | enum `{ FileStart, Shebang, EncodingDecl, Bom }` | First-line context for safe insertion (FR-019). |

---

## 6. FileLicensingState

The per-file join of declared vs actual, with classification (FR-004).

| Field | Type | Notes |
|-------|------|-------|
| `path` | path | |
| `matched_rule` | optional `Rule` ref | None ⇒ default applied, or uncovered. |
| `declared_intent` | optional `LicenseIntent` | From rule or default. |
| `actual` | `ActualLicenseState` | |
| `drift` | enum `DriftClass` | See below. |
| `conflict` | optional `RuleConflict` | Set when equal-specificity rules matched (FR-022). |

**DriftClass** (FR-004) — exhaustive, mutually exclusive:
`Compliant` | `WrongLicense{declared, actual}` | `CopyrightMismatch{declared, actual}` | `MissingHeader` | `Uncovered` | `Excluded` | `Unreadable`

License equality joins **all** effective expressions as one `AND` expression and
compares once: a matching candidate never hides an additional license. Copyright is
compared separately against the policy (`preserve` imposes nothing; `add` needs
existing notices plus the requested normalized notice; `replace` needs exactly it):
a license match with an unsatisfied copyright policy is `CopyrightMismatch`, while a
license mismatch keeps the copyright mismatch as a `copyright_mismatch` diagnostic
next to `WrongLicense`. An unresolved rule conflict has no single declared intent;
its `declared` names the tied expressions descriptively (never a fabricated token)
and the conflict stays structured in `conflict` plus a `rule_conflict` warning.

`Unreadable` (FR-025) covers files that cannot be safely parsed or written (e.g. non-UTF-8); the tool never byte-edits them.

**State transitions** (a file's drift after operations):
```
MissingHeader  ──apply(write)────────▶ Compliant
WrongLicense   ──apply(destructive)──▶ Compliant
WrongLicense   ──apply(additive)─────▶ Compliant + ContradictionWarning (FR-020)
Uncovered      ──(edit config: add rule/exclusion/default)──▶ Compliant | Excluded
Unreadable     ──apply──▶ Unreadable (skipped, never modified; reported as failure)
Compliant      ──(file edited off-intent)──▶ WrongLicense | MissingHeader
```
`Uncovered` and `Unreadable` are **gate failures** (FR-012a, FR-025); `Excluded` is not.

---

## 7. LicenseTextInventory

Tracks referenced identifiers vs present texts in `LICENSES/` (FR-014, FR-017, FR-029).

| Field | Type | Notes |
|-------|------|-------|
| `referenced` | set of SPDX id | Every identifier used anywhere in the repo/config. |
| `present` | set of SPDX id | Recognized texts under root `LICENSES/` (`.txt`/`.md` suffixed, or a bare known id — the bare form satisfies presence but strict lint reports its missing extension). Symlinks never count. |
| `missing` | derived set | `referenced − present` → reported; standard ids materializable from the embedded bundle offline; `LicenseRef-*` scaffolded as placeholders. The `add-license` command (FR-029) materializes this set (or an explicit subset) into `LICENSES/`. |
| `bundled` | set of SPDX id | Identifiers whose text is embedded in the binary. |
| `unused` | list of SPDX id | Present but never referenced — a project-wide lint finding only, never a selected-file policy failure. |
| `unrecognized` | list of path+reason | `LICENSES/` entries naming no recognizable license (bad suffix, unknown id, legacy `+` spelling) — lint finding. |
| `missing_extension` | list of SPDX id | Recognized ids in extensionless files — lint finding; still satisfy presence for policy and materialization. |
| `unreadable` | list of path+reason | Recognized texts that are not readable UTF-8 — validation is incomplete for them, never a pass and never a proven violation. |

Duplicate ids under several filenames make the inventory ambiguous and fail before
use (as in the reference tool). The scan keeps two selected-scope reference sets:
`actual_referenced_ids` (effective file + snippet expressions, excluding suppressed
values and unused config rules) feeds REUSE inventory; `desired_referenced_ids`
(winning intents of evaluated files only) joins it for the policy `check` text
scope. `apply` materializes its projected post-apply state only — never stale
replaced licenses or unmatched rules — and never deletes texts.

---

## 8. ReconciliationPlan / Report

The computed result of a `check` (read-only) or `apply` (writing) run.

| Field | Type | Notes |
|-------|------|-------|
| `files` | list of `FileLicensingState` | Per-file classification. |
| `changes` | list of `FileChange` | For `apply`: before/after per file; for `check`: would-be changes. |
| `diagnostics` | list of `Diagnostic` | Contradictions (FR-020), rule conflicts (FR-022), missing texts, `source_override` (FR-003a), encoding skips (FR-025). Sorted by path, then code. |
| `writes` | list of write records | Independent of file states: one record per planned/applied/failed/blocked write with destination, kind, outcome, covered files, and exact text for text writes (FR-021). |
| `summary` | `{ pass, partial, complete, before_pass, projected_pass, counts }` | `counts` holds per-`DriftClass` totals **plus** `conflicts` and `contradictions`; `partial` (FR-021) and `pass` (FR-012a, SC-009) live here too. `complete` is false when final verification did not run; `before_pass` is the pre-apply gate; dry-run `pass` means `projected_pass`. Drives the exit code. This is the canonical shape; `report.schema.json` matches it. |

**PlannedWrite / write record**

| Field | Type | Notes |
|-------|------|-------|
| `path` | path | Destination written (the document itself for metadata patches). |
| `kind` | enum `{ source, sidecar, reuse_toml, license_text, config }` | What the write mutates (FR-021). |
| `status` | enum `{ planned, applied, failed, blocked }` | Dry-run previews as `planned`; converged runs emit no record (`unchanged` is absence). |
| `affected_files` | list of paths | Selected files the write covers (the assets behind a metadata patch). |
| `before_text` / `after_text` | optional text | Exact bytes as text for text being written; absent for blocked writes with no known result. |

**FileChange**

| Field | Type | Notes |
|-------|------|-------|
| `path` | path | |
| `mode` | enum `{ Additive, Destructive }` | Destructive is the default for the license id (FR-007). |
| `before` / `after` | header snapshot | Copyright lines identical pre/post unless policy says otherwise (SC-004). |
| `target_header` | optional index | Which `HeaderBlock` was replaced when multiple exist (FR-008). |

---

## Cross-references to requirements

| Entity | Requirements |
|--------|-------------|
| LicensingConfiguration, Rule, Selector | FR-001, FR-002, FR-016, FR-022 |
| LicenseIntent, copyright_policy | FR-009, SC-004 |
| CommentStyle(Association) | FR-010, FR-011 |
| ActualLicenseState, HeaderBlock | FR-003a, FR-005, FR-008, FR-019, FR-025, FR-026, FR-030 |
| FileLicensingState, DriftClass | FR-003, FR-004, FR-012a, FR-025 |
| LicenseTextInventory | FR-014, FR-015, FR-017, FR-028, FR-029 |
| ReconciliationPlan/Report, FileChange | FR-006, FR-007, FR-012, FR-013, FR-020, FR-021, FR-024, SC-009, SC-010 |
| Stateless scan (no cache) | FR-023, SC-011 |
