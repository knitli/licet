# Changelog

All notable changes to `licet` are documented here.
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


