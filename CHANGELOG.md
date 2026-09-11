# Changelog

All notable changes to `licet` are documented here.
## [0.3.0] - 2026-09-11

### Bug Fixes
- REUSE dev tool- Resolve git/curl without consulting the working directory- Reconcile manifest/lock with rebased main, restore serde_json dep- Repair Windows-only build breaks inherited from main- Split platform-divergent resolver test to satisfy Windows -D warnings- Force globset backslash escapes so oob matching is platform-independent- Tolerate Windows path-spelling aliases in explicit file selection

### Documentation
- Attribute vendored REUSE Specification v3.3 (CC-BY-SA-4.0)

### Features
- Implement licet improvement plan (tasks 1-12, F01-F19)- Rename default config license.toml to licet.toml

### Refactor
- Address review complexity, split oob module, docs nits- Split genuinely multi-concern functions flagged by complexity gate

### Diag
- Surface licet stderr in LicenseRef scaling test assertion

## [0.2.2] - 2026-06-28

### Bug Fixes
- Escape generated TOML and reject non-SPDX detected values

## [0.2.0] - 2026-06-27

### Bug Fixes
- Treat .reuse/dep5 as aggregate, not override; vendor REUSE 3.3 spec

### Documentation
- Sync comment registry §4 with expanded families

### Features
- Add .license sidecars and REUSE.toml precedence- Reconcile out-of-band-covered files with wrong licenses- Honor REUSE ignore blocks and SPDX snippets (FR-030)

### Miscellaneous
- Pin cargo:licet tool to 0.1.4

### Performance
- Single-pass Aho-Corasick header scan; widen closer set

### Refactor
- Model comment syntax as a sum type over a data-driven registry- Expand built-in registry and split extension tables

## [0.1.4] - 2026-06-25

### Bug Fixes
- Fix invalid cargo publish commands- Stop .git/ internals leaking into scans on Windows

### Ci
- Publish to crates.io after binary matrix; pin rust-cache- Guard that the pushed tag matches Cargo.toml- Add manual prepare-release workflow

## [0.1.3] - 2026-06-25

### Bug Fixes
- Fix bad table in Cargo.toml and poor matching breaking test in Windows

### Miscellaneous
- Bump to v0.1.2

## [0.1.1] - 2026-06-25

### Bug Fixes
- Correct MSRV in README.md

### Documentation
- Spec for declarative license-header management + Spec Kit setup- Ratify licet constitution v1.0.0 (initial adoption)

### Features
- Implement declarative license-header CLI (licet)- Add `add-license` command for intuitive license addition

### Miscellaneous
- Complete remaining tasks — snapshots, completions, bench, CI/CD- Remove most .specify files, update versions and MSRV- Update versions, pin actions to SHAs- Update Cargo.toml and fix lints- Bump


