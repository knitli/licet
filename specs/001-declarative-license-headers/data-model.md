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
- `ExactPath(path)` / `Filename(name)` — e.g. `hk.pkl`, `README.md`

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

## 4. CommentStyleAssociation & CommentStyle

Maps file selectors to the comment syntax used to read/write headers (FR-010, FR-011).

**CommentStyleAssociation**

| Field | Type | Notes |
|-------|------|-------|
| `selector` | `Selector` | Filename selector takes precedence over extension (FR-011). |
| `style` | `CommentStyleRef` | Name of a built-in style or an inline `CommentStyle`. |

**CommentStyle** (primitive model, so new languages are pure data)

| Field | Type | Notes |
|-------|------|-------|
| `line_prefix` | optional string | e.g. `//`, `#`, `;`. |
| `block_start` / `block_end` | optional strings | e.g. `/*` … `*/`, `<!--` … `-->`. |
| `block_line_prefix` | optional string | e.g. ` * ` for inside C block comments. |

**Resolution precedence** (FR-011): exact filename association → extension association →
built-in default for the type. Built-ins seed at least the REUSE-known set.

---

## 5. ActualLicenseState

What is really present for a file, gathered by detection (FR-003a).

| Field | Type | Notes |
|-------|------|-------|
| `headers` | list of `HeaderBlock` | Every parsed in-file SPDX header occurrence (not just the first — FR-008). |
| `out_of_band` | optional `OutOfBandEntry` | License/copyright from `REUSE.toml` or `.reuse/dep5` covering this path. Read for interop/detection only — never an authoring surface. |
| `detected_license` | optional SPDX expression | Canonicalized from headers and/or out-of-band; either source satisfies intent. **Precedence on disagreement**: when in-file header and out-of-band disagree, the out-of-band value is authoritative and a non-failing `source_override` diagnostic is recorded (FR-003a). |
| `detected_copyrights` | list of string | All `SPDX-FileCopyrightText` lines found. |
| `encoding_ok` | bool | False when the file is not valid UTF-8; drives the `Unreadable` classification (FR-025). |

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
`Compliant` | `WrongLicense{declared, actual}` | `MissingHeader` | `Uncovered` | `Excluded` | `Unreadable`

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

Tracks referenced identifiers vs present texts in `LICENSES/` (FR-014, FR-017).

| Field | Type | Notes |
|-------|------|-------|
| `referenced` | set of SPDX id | Every identifier used anywhere in the repo/config. |
| `present` | set of SPDX id | Texts found under `LICENSES/`. |
| `missing` | derived set | `referenced − present` → reported; standard ids materializable from the embedded bundle offline; `LicenseRef-*` scaffolded as placeholders. |
| `bundled` | set of SPDX id | Identifiers whose text is embedded in the binary. |

---

## 8. ReconciliationPlan / Report

The computed result of a `check` (read-only) or `apply` (writing) run.

| Field | Type | Notes |
|-------|------|-------|
| `files` | list of `FileLicensingState` | Per-file classification. |
| `changes` | list of `FileChange` | For `apply`: before/after per file; for `check`: would-be changes. |
| `warnings` | list of `Warning` | Contradictions (FR-020), rule conflicts (FR-022), missing texts, `source_override` (FR-003a), encoding skips (FR-025). |
| `summary` | `{ pass, partial, counts }` | `counts` holds per-`DriftClass` totals **plus** `conflicts` and `contradictions`; `partial` (FR-021) and `pass` (FR-012a, SC-009) live here too. Drives the exit code. This is the canonical shape; `report.schema.json` matches it. |

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
| ActualLicenseState, HeaderBlock | FR-003a, FR-005, FR-008, FR-019, FR-025, FR-026 |
| FileLicensingState, DriftClass | FR-003, FR-004, FR-012a, FR-025 |
| LicenseTextInventory | FR-014, FR-015, FR-017, FR-028 |
| ReconciliationPlan/Report, FileChange | FR-006, FR-007, FR-012, FR-013, FR-020, FR-021, FR-024, SC-009, SC-010 |
| Scan cache (fingerprint key) | FR-023, SC-011 |
