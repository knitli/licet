# Configuration Contract: `license.toml`

The single declarative source of truth (FR-001). Parsed by `src/config` via `serde`/`toml`
into the `LicensingConfiguration` entity (see `data-model.md`). Lives at repo root; path
overridable with `--config`.

`license.toml` is the **only authoring surface** for licensing intent. `REUSE.toml`/
`.reuse/dep5` are read for interop and actual-license detection only (their REUSE 3.3
`precedence` decides detection when they disagree with an in-file header — `closest` by
default, FR-003a); they are never hand-authored as the declarative config. This reflects the maintainer's view that `REUSE.toml`, while TOML, is
not designed for declarative intent.

## Top-level structure

```toml
# Repository-wide default applied to any covered file no rule matches.
[default]
license = "MIT OR Apache-2.0"        # SPDX expression or LicenseRef-*
copyright = "preserve"               # "preserve" (default) | "add:<text>" | "replace:<text>"

# Ordered rules. Declaration order breaks specificity ties (FR-002).
# Each rule has exactly one selector key: ext | glob | file.
[[rule]]
ext = "rs"                           # extension selector
license = "LicenseRef-MarqueLicense-1.0"

[[rule]]
glob = "examples/**/*.rs"            # glob selector (more specific than bare ext)
license = "MIT OR Apache-2.0"

[[rule]]
glob = "specs/**"
license = "LicenseRef-MarqueLicense-1.0"

[[rule]]
file = "hk.pkl"                      # exact-filename selector (most specific)
license = "LicenseRef-MarqueLicense-1.0"

# Comment-style associations (FR-010, FR-011). Persisted — no per-file flags.
[[comment_style]]
ext = "pkl"
style = "c"                          # reference a built-in style by name

# Inline custom style (primitive model from data-model.md §4).
[[comment_style]]
file = "Jenkinsfile"
style = { line_prefix = "//" }

# Paths excluded from coverage (FR-016). Distinct from "uncovered".
[exclude]
paths = ["vendor/**", "target/**", "*.lock"]

# How `apply` covers files that can't carry an in-file header (FR-015).
[output]
non_annotatable = "sidecar"          # "sidecar" (default) | "reuse-toml"
```

## Field reference

### `[default]`
| Key | Type | Required | Notes |
|-----|------|----------|-------|
| `license` | SPDX expression string | recommended | Omit to allow `Uncovered` classification (which fails the gate, FR-012a). |
| `copyright` | string | no | `preserve` (default) \| `add:<text>` \| `replace:<text>` (FR-009). |

### `[[rule]]`
Exactly one selector key, plus intent:
| Key | Type | Notes |
|-----|------|-------|
| `ext` \| `glob` \| `file` | string | The selector. Specificity: `file` > `glob` > `ext` (FR-002). |
| `license` | SPDX expression | Required. Validated against SPDX list / `LicenseRef-*`. |
| `copyright` | string | Optional per-rule override of the default copyright policy. |

Equal-specificity selectors matching one file are surfaced as a conflict (FR-022), not
silently resolved.

### `[[comment_style]]`
| Key | Type | Notes |
|-----|------|-------|
| `ext` \| `file` | string | Selector; `file` takes precedence over `ext` (FR-011). |
| `style` | string \| inline table | Built-in style **alias** (e.g. `c`, `hash`, `slashes`), or an inline table that lowers into a `CommentSyntax` (data-model §4). |

Inline-table keys: `line_prefix`, `block_start`, `block_end`, `block_line_prefix` (the
internal block alignment prefix). The result is classified by which keys are present:
`line_prefix` only → line-only; `block_start` + `block_end` (+ optional
`block_line_prefix`) → block-only; both → supports both forms.

### `[exclude]`
| Key | Type | Notes |
|-----|------|-------|
| `paths` | list of glob | Explicitly excluded; reported as `Excluded`, never `Uncovered` (FR-016). |

### `[output]`
| Key | Type | Notes |
|-----|------|-------|
| `non_annotatable` | string | How `apply` covers files that can't carry an in-file header: `sidecar` (default — writes `<file>.license`) or `reuse-toml` (appends a `REUSE.toml` annotation). Overridable per-run with `--non-annotatable` (FR-015). |

## Validation rules (config errors → exit `2`)

1. Every `license` parses as a valid SPDX expression or `LicenseRef-*` (FR-005).
2. Each `[[rule]]` / `[[comment_style]]` has **exactly one** selector key.
3. Every `style` reference resolves to a built-in alias or inline-defined style. An inline
   style must define at least one form (`line_prefix` or a block), and a block must give
   **both** `block_start` and `block_end` — a half-specified block is a config error.
4. Glob patterns are well-formed.
5. Duplicate identical selectors with differing intent are reported as conflicts (FR-022).

## Worked example → projection

Given the config above, the Marque rule set projects as:
- `src/lib.rs` → `LicenseRef-MarqueLicense-1.0` (rule `ext=rs`)
- `examples/demo.rs` → `MIT OR Apache-2.0` (rule `glob=examples/**/*.rs` wins, more specific)
- `specs/plan.md` → `LicenseRef-MarqueLicense-1.0` (rule `glob=specs/**`)
- `hk.pkl` → `LicenseRef-MarqueLicense-1.0`, written with C-style comments
- `README.md` (no rule) → `MIT OR Apache-2.0` (default)
- `vendor/x.rs` → `Excluded`

This reproduces the user's stated Marque intent ("source/specs/planning =
`LicenseRef-MarqueLicense-1.0`; everything else = `MIT OR Apache-2.0`") from one file.

> This is the same configuration exercised by spec.md US1/US2 scenario 1 and
> quickstart.md Scenario 1.
