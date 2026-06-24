# Specification Quality Checklist: Declarative License Header Management

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-06-24
**Last revalidated**: 2026-06-24 (after specification review panel — covers FR-001..FR-028, SC-001..SC-011)
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details (languages, frameworks, APIs)
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria are technology-agnostic (no implementation details)
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] No implementation details leak into specification

## Notes

- Items marked incomplete require spec updates before `/speckit-clarify` or `/speckit-plan`.
- Resolved by informed assumption rather than blocking clarification (documented in spec **Assumptions**):
  - **Copyright vs license handling**: license identifiers are declaratively replaceable; copyright/authorship is preserved/accumulated additively by default.
  - **Reconciliation direction**: configuration is the single source of truth; files move toward config, never the reverse.
  - **REUSE/SPDX compatibility** is an interop target the output must conform to; `REUSE.toml`/`dep5` are detection/interop surfaces only, never the declarative authoring surface (`license.toml` is the sole authoring surface).
- **Performance is now numerically specified** (supersedes the earlier "deferred to planning" note): full 10k-file scan **< 1s warm** and **< 3s cold** on a 4-core 2020-era runner, changed-file checks well under 1s (SC-006). Stated at the outcome level with a named reference machine; the concrete crate/algorithm choices live in plan.md/research.md, keeping the spec itself technology-agnostic.

### Revalidation after specification review panel (2026-06-24)

The panel pass added six functional requirements (FR-023..FR-028) and three success criteria (SC-010, SC-011, plus the cold-scan bar in SC-006). Each checklist item was re-checked against the expanded spec and still holds:

- **Requirements testable & unambiguous** — the new clarification session records explicit decisions (cold-scan bar, source-disagreement precedence, non-UTF-8 handling, selection-flag exclusivity, apply safety) so none introduce ambiguity. New `Unreadable` drift class is exhaustive/mutually-exclusive with the others.
- **Success criteria measurable** — SC-008 was tightened to a measurable generalization bound (rules ≤ 25% of covered files); SC-010 (no partial/corrupt writes under induced failure) and SC-011 (cache hit == cold run) are observable pass/fail outcomes.
- **Edge cases identified** — added: in-file vs out-of-band disagreement, non-UTF-8 encoding, line-ending preservation, destructive-apply safety, stale-cache-after-config-change.
- **Dependencies & assumptions identified** — added assumptions on `REUSE.toml` as interop-only and the embedded SPDX corpus being a versioned snapshot (FR-028).
- **User scenarios cover primary flows** — US4 now also covers the *contributor* (non-author) blocked by the gate, with a remediation-command expectation.
- **No implementation leakage** — the new FRs stay behavioral (e.g. "writes must be atomic", "skip non-UTF-8 and fail the gate"); the *how* (temp-file+rename, cache key composition, SPDX crate) remains in plan.md/research.md.
