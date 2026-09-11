# Licet application audit

Date: 2026-09-10. Scope: the source snapshot in `/opt/coder/licet`, package version **0.1.4**. This is an analysis and implementation handoff; application code was not changed.

Licet has a useful product model and a reasonably small pipeline, but its current success results are not reliable enough for unattended license changes or a REUSE compliance gate. The largest problems are incorrect source selection, lossy metadata handling, unsafe file replacement, and tests that establish internal consistency without exercising the problematic cases. Raw throughput is already adequate on the simple workload measured here. A rewrite, plugin system, database, or new caching architecture would be premature.

Read the [implementation plan](superpowers/plans/2026-09-10-licet-improvements.md) to execute the work. It specifies behavior decisions, file ownership, dependencies, regression cases, and completion gates. The plan deliberately resolves contradictions in the older contracts rather than asking its executor to infer policy from inconsistent comments.

## Evidence and limits

The checkout has no `.git` metadata, so no commit SHA or remote release state can be established from it. Git-dependent experiments used disposable repositories. The build and all reproductions used this checkout's binary, not the separately installed Licet version from mise.

Source-set SHA-256: `88c10c2f1b126b8786d08a48cda98ece192e830bda0bbbd005b846c3f20bae77`. This hashes 67 files: all files under `src`, `tests`, `assets/licenses`, and `.github/workflows`, plus `Cargo.toml`, `Cargo.lock`, and `build.rs`; in sorted relative-path order, feed `path + NUL + contents + NUL` to SHA-256. Documentation added by this audit is outside that set.

| Verification | Observed result |
|---|---|
| `cargo test --locked` | 152 tests passed: 94 unit tests and 58 integration tests. Zero doctests. |
| `cargo fmt --all --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed. |
| `cargo +1.89 check --locked --offline --all-targets` | Passed; the initial default-target check also passed. |
| `cargo bench --locked --bench scan -- --quick` | Completed; measurements below. |
| Existing REUSE comparison, initial run | Silently skipped because installed `reuse --version` failed with `NoEncodingModuleError`. |
| Existing comparison after isolated tool repair | `reuse_conformance_after_apply` passed with REUSE 6.2.0 and an encoding extra. |
| Five adversarial REUSE fixtures | All five disagreements confirmed against REUSE 6.2.0, which reports specification 3.3. |

The primary toolchain was Rust 1.100.0-nightly on macOS arm64. Cargo initially needed missing locked dependencies; after their download the exact test command succeeded. One subsequent fixture run encountered inherited Git commit signing and was rerun with signing disabled **only in the test process**. Neither failure was treated as an application bug. The test helper's dependence on global Git configuration is an improvement opportunity.

The REUSE comparison used `reuse[charset-normalizer]==6.2.0` in `/private/tmp/licet-reuse-comparator-venv`, without modifying the global installation. The supplied [REUSE 3.3 document](REUSE_Specification_v3.3.md) was checked against the [official specification](https://reuse.software/spec-3.3/). Workflow findings are source reviews, not claims about a live GitHub run, registry trust policy, or released binary. No dependency advisory audit or cross-platform runtime certification was performed.

## Priority and implementation mapping

**P1** means fix before using Licet for unattended mutation or authoritative gating. **P2** means required for predictable supported behavior and the promised REUSE 3.3 scope. **P3** means maintenance or delivery improvement. These are engineering priorities, not CVSS scores; filesystem attacks require malicious local repository content, a supplied argument, or another process able to place paths.

| ID | Priority | Finding | Plan task |
|---|---|---|---|
| F01 | P1 | Predictable temporary file and permission loss | 1 |
| F02 | P1 | License downloads/materialization escape their destination | 1 |
| F03 | P1 | Apply exits successfully with residual failures | 7 |
| F04 | P1 | REUSE metadata writing can destroy or invalidate existing coverage | 1, 6 |
| F05 | P1 | Header editing can break source syntax; additive mode is inconsistent | 6 |
| F06 | P1 | Staged checks inspect unstaged bytes | 2 |
| F07 | P1 | File selection is inconsistent and corrupts Git filenames | 2 |
| F08 | P1 | Lint produces false REUSE compliance results | 3, 4, 5 |
| F09 | P1 | Valid REUSE metadata is ignored or matches the wrong files | 4 |
| F10 | P1 | Init can infer unintended relicensing and overwrite authoring intent | 8 |
| F11 | P2 | Copyright policies are not reconciled | 5, 6 |
| F12 | P2 | SPDX normalization reparses valid expressions incorrectly and expensively | 3 |
| F13 | P2 | Invalid configuration and I/O errors disappear | 1–5, 9 |
| F14 | P2 | Comment-style configuration can affect the wrong files or emit invalid code | 6, 9 |
| F15 | P2 | Reports and dry runs omit important changes and break machine output | 7, 9 |
| F16 | P2 | Cache costs work but never avoids it | 10 |
| F17 | P2 | Tests and benchmarks omit the important contracts | All tasks, 10, 11 |
| F18 | P2 | Release workflow has authentication and sequencing defects | 12 |
| F19 | P3 | Toolchain, bundle metadata, and comments obscure actual guarantees | 10–12 |

## Detailed findings

### F01 — File replacement can overwrite another file and change access permissions

`src/reuse/mod.rs:18–25` derives a fixed `.<filename>.licet.tmp` path and opens it with `File::create`, then renames it over the source. A pre-existing regular file at that name is truncated. A symlink at that name is followed while writing, then moved over the source.

**CLI reproduction:** `.a.rs.licet.tmp` pointed to a sentinel outside the repository. Applying a header to `a.rs` overwrote the sentinel, replaced `a.rs` with that symlink, and exited 0. A separate executable `a.sh` changed from mode `0700` to `0644`. This both removes executable permission and broadens access under the tested umask.

Use an exclusively created sibling temporary file, preserve source permissions, and reject symlink destinations and unsafe ancestors. A failed write must clean up only the temporary file that this invocation created. Preflight existing metadata reads; a non-UTF-8 or inaccessible file is not an absent file. Concurrent source modifications must be detected before replacement. Atomic replacement is per file; do not promise a repository-wide transaction.

### F02 — License-text operations do not enforce the LICENSES boundary

`src/cli/add_license.rs:41–50` accepts arbitrary identifiers. `src/reuse/inventory.rs:152–175` embeds them into a destination path, passes that path to curl, and deletes it on curl failure. With an existing `victim.txt`, `add-license ../victim --allow-curl` and a fake curl that merely exited 22 deleted the victim. No successful network response was necessary.

Separately, `present_texts` ignores symlink entries (`inventory.rs:53`), while bundled materialization follows them using `std::fs::write` (`:106`). `LICENSES/MIT.txt` pointing outside the repository caused `add-license MIT` to overwrite the external target. Symlinked parent directories have the same containment problem.

Validate individual license/exception IDs and filename components before any filesystem mutation. Download into owned temporary storage with bounded execution, validate a nonempty text result, and install it using the same safe writer. Never remove or truncate the final destination during failed retrieval. Keep network consent explicit and test subprocess behavior with fake curl.

### F03 — Exit status and reported outcome disagree

`src/cli/apply.rs:257–279` fails only on selected drift categories through `has_unfixable`; remaining `MissingHeader`, `WrongLicense`, and rule conflicts do not necessarily affect the exit code. `:237–240` ignores license materialization errors.

Confirmed examples:

- A copyright-only first header remains without a license; apply exits **0** with JSON `summary.pass = false` and `missing_header = 1`.
- An annotatable file covered by an overriding Apache-2.0 REUSE annotation, with declared MIT, remains wrong; apply exits **0** with `wrong_license = 1`.
- `LICENSES` exists as an ordinary file. A source header is inserted, license-text creation fails, and apply exits **0** with `summary.pass = true`.

One result calculation must drive JSON, human output, and process exit status. Plan failures, write failures, residual policy drift, and missing texts must all participate. Partial status must account for license-text and metadata writes as well as source writes.

### F04 — REUSE output can destroy unrelated metadata

`src/reuse/oob.rs:224–225` converts every read error to empty content. Existing invalid UTF-8 `REUSE.toml` containing `KEEP\xff` was replaced entirely when a binary asset needed an annotation; exit 0.

`oob.rs:265–267` writes the copyright key repeatedly rather than once as an array. A valid broad binary annotation with holders Alice and Bob, followed by apply that creates an exact-path exception, produced invalid TOML with duplicate keys. The entire metadata file then stopped parsing, affecting sibling coverage. `:287–362` is a second, incomplete TOML parser that misses literal strings, multiline values, escapes, and table variants. Exact updates also normalize all line endings and ignore copyright changes.

Delete the line-oriented TOML interpretation. Parse with the installed TOML library, serialize generated tables with it, and validate the complete proposed document before writing. A minimal comment-preserving strategy is to append a correctly serialized exact annotation to the document whose matching annotation must change, retaining existing bytes. Preserve newline style, last-match semantics, precedence, and sibling coverage; group updates to each document and write it once.

### F05 — Header edits are not safe across supported syntax or modes

`src/reconcile/mod.rs:171–178` preserves only `-->` and `*/`, although the detector recognizes many other block closers. An OCaml header `(* SPDX-License-Identifier: MIT *)` became an unterminated comment after replacement with Apache-2.0; exit 0. Additive editing of a one-line block can also mirror the wrong prefix or place another terminator outside the original block.

`src/reconcile/insert.rs:45–56` does not implement PHP opening-tag placement despite `detect::leading_position` recognizing it. Applying to `<?php ...` inserts `//` text outside PHP, changing program output. BOM/shebang combinations, XML declaration plus content on one line, and copyright/license lines separated by a blank line also need direct preservation tests.

`src/reconcile/mod.rs:51–82` short-circuits on any matching candidate, then silently clamps invalid header indexes to the last block. A copyright-only block is selected as if it contained a replaceable license. `src/cli/apply.rs:193–195,221` renders a replacement sidecar regardless of additive mode: **confirmed `apply --additive` deletes the existing MIT license from an MIT sidecar when intent is Apache-2.0**.

Use recorded tag-value spans and the comment registry to edit only metadata values. Treat insertion, replacement, targeting, and additive behavior consistently for every destination. Reject invalid targets with a useful error; preserve original bytes outside intentional edits.

### F06 — A staged check can approve the wrong commit

`src/walk/mod.rs:91` gets changed index filenames, but `src/engine.rs:157` reads working-tree contents. Staging an Apache-2.0 header and then changing only the working copy to MIT resulted in `check --staged` exit 0, actual MIT. The commit still contains Apache-2.0.

A staged check must use an index snapshot for source files, sidecars, REUSE metadata, configuration, and license texts. Changes to metadata can affect files whose source blobs did not change. Index conflicts and missing staged dependencies must be visible. Batch object reads; one Git subprocess per file would fix correctness at avoidable cost. Preserve a separately documented working-tree write mode for `apply --staged`, which never stages edits automatically.

### F07 — Scope and path identity vary with spelling and repository shape

The documented default is tracked files, but `src/walk/mod.rs:144–159` walks nonignored working-tree files. It omitted a tracked file newly matched by `.gitignore` and included an untracked unlicensed file. In a clean linked worktree, the `.git` control file was scanned as source and failed coverage. Implicit exclusions also omit some REUSE-required ignore cases and incorrectly exempt nonempty `*.empty` files by name.

Git output lacks `-z`, and `:205–208` treats it as newline-delimited UTF-8. A compliant staged `café.rs` became the nonexistent reported path `"caf/303/251.rs"`; tab/newline filenames are similarly corrupted. Explicit `src/../src/x.rs` bypassed the exact rule that correctly covered `src/x.rs`.

Explicit symlinks are canonically deduplicated but written through the lexical path. Selecting `alias.rs -> real.rs` before `real.rs` replaced the symlink with an ordinary annotated file and left the real file unchanged. Explicit outside-root paths are not rejected. The REUSE target requires ignoring symlinks, so the plan replaces the old symlink-following contract with a consistent exclusion policy.

Use system Git for tracked enumeration and NUL-safe subsets, a single lexical normalization/containment path, and the REUSE exclusion rules. Separate declarative selection from the broader REUSE covered-file universe used by lint.

### F08 — Lint is a policy-drift check masquerading as REUSE validation

`src/cli/lint.rs:24–26` uses declaration drift plus missing texts to decide REUSE compliance. That omits copyright completeness and depends on having declaration rules even when actual metadata is valid. `src/engine.rs:97–102` inventories only the first effective expression; `:176–184` adds every configured rule ID even when the rule matches no file. `detect::HEAD_BYTES` limits metadata scans to 8 KiB. `inventory::is_complete` ignores unused/unrecognized license files and accepts empty text files by name.

These independent fixtures were confirmed against the reference tool:

| Fixture | Licet lint | REUSE 6.2.0 / spec 3.3 |
|---|---|---|
| MIT header, no copyright, real MIT text | Exit 0 | Exit 1: missing copyright |
| MIT and Apache-2.0 header lines, only MIT text | Exit 0, references only MIT | Exit 1: missing Apache-2.0 text |
| Apache-2.0 snippet after 10 KiB, only MIT text | Exit 0, references only MIT | Exit 1: missing Apache-2.0 text |
| `src/*` annotation and headerless `src/deep/a.rs` | Exit 0 | Exit 1: missing copyright and license |
| Valid license array and both license texts | Exit 1: missing header | Exit 0 |

All fixture configs were themselves correctly annotated, and real bundled license texts were used. Reference runs had no read errors or invalid-license diagnostics. These disagreements therefore do not rely on artificial licensing of the fixture infrastructure.

Additional source-confirmed gaps: ordinary `Copyright`/`©` notices are not recognized; malformed license tags are discarded instead of diagnosed; unclosed snippets do not cause a finding. A `license.toml` exclusion must not allow a covered file to disappear from a report claiming whole-project REUSE compliance. Inventory should validate useful structural properties of license files, but should not claim to establish legal equivalence of arbitrary prose to a license.

### F09 — REUSE annotations are incomplete and precedence is lossy

`src/reuse/oob.rs:36–37` accepts a license string but not the valid list form. Deserialization failure silently discards the entire file (`:88–92`). Required version/path and valid precedence are not enforced. Only root metadata is loaded (`:74–85`); nested REUSE files and their directory-relative precedence cannot work. Both REUSE.toml and deprecated DEP5 are accepted together despite their mutual-exclusion requirement.

`Glob::new` in `oob.rs:179–183` lets `*` cross separators by default and interprets syntax not defined by REUSE. DEP5 and REUSE require distinct matching behavior. DEP5 continuation lines are lost. `src/detect/mod.rs:112–116` always combines copyright notices, even when precedence should suppress one source; copyright and license fallback need independent evaluation.

Preserve a list of effective values and their originating documents, resolve the last matching table within each document, then resolve the directory hierarchy. Keep raw notices separately from effective values so preservation does not accidentally change meaning. The detailed parent/child matrix is in the plan.

### F10 — Init invents a majority-based policy that can relicense exceptions

`src/cli/init.rs:48–63` selects a majority default and majority license per extension, losing minority files with the same extension and exceptional extensionless files. With two MIT `.rs` files and one Apache-2.0 `.rs` file, init generated only default MIT. Following the suggested workflow would change the Apache file on the next apply.

The generated configuration is only parsed for validity, never compared with observations. Existing `license.toml` is overwritten unconditionally at `:74`; `--config` is unused and `--from-reuse` changes reporting rather than behavior. No source mutation is necessary for init itself to lose valuable intent.

Generate conservative exact rules first, compress only where every observed covered file keeps identical intent, and validate the generated projection. Do not assign licensing to previously unknown files through a guessed default. Create new output exclusively; require an explicit `--force` for replacing an existing configuration.

### F11 — Copyright policies are parsed but not applied consistently

`src/report/classify.rs:65–86` ignores copyright intent. `src/reconcile/mod.rs:51–58` returns a no-op when the license already matches; its existing-header branch never applies copyright policy. Exact REUSE rewrites also ignore the supplied copyright list.

Confirmed: configured `copyright="add:2026 Bob"` plus an existing MIT/Alice header leaves Alice alone, reports compliance, and exits 0. Existing exact binary REUSE annotations retain only Alice under both `add:Bob` and `replace:Bob`, even when the license itself changes.

Compare license intent and copyright intent independently. Preserve is the default; add is idempotent; replace changes only copyright metadata in the explicitly selected scope. Missing copyright remains a separate REUSE completeness problem when preserve has no existing notice to preserve. Never invent an author.

### F12 — SPDX parsing validates with one parser and normalizes with another

`src/spdx/mod.rs:77` discards the dependency AST and reparses `Expression::as_ref()`, which is the original input. `split_top_level` recognizes only literal space-surrounded operators. Configured tabs or newlines around OR validate successfully but compare unequal to ordinary-space headers. Three or more redundant surrounding parentheses also caused false drift in tested expressions.

`spdx/mod.rs:159` uppercases a complete remaining suffix at every byte offset, giving quadratic work on a long valid LicenseRef. A one-file debug check took about 34 ms for a 1,000-character suffix, 424 ms for 4,000, and exceeded a four-second timeout for 16,000. This is a bounded diagnostic experiment, not a release benchmark.

Use `spdx::Expression::iter()` and its `ExprNode::{Req, Op}` postfix representation to build the existing normalized representation. Use parsed requirements for inventory too. Delete handwritten whitespace, parenthesis, and WITH splitting. Preserve associative/commutative equality, exception attachment, and the distinction between AND and OR; full Boolean theorem proving remains unnecessary.

### F13 — Errors are converted into apparently valid inputs

Examples include file read errors becoming empty contents (`engine.rs:157`), walk errors being skipped (`walk/mod.rs:154`), malformed config becoming defaults (`cli/lint.rs:15–16`, `cli/add_license.rs:42–43`), invalid sidecars falling back to source content (`detect/mod.rs:100–106`), and failed Git status being treated as clean (`cli/apply.rs:284–294`).

The implementation needs a consistent distinction among absence, invalid content, and I/O failure. Optional discovery may accept `NotFound`; an explicitly named missing configuration must be a usage error. An unreadable selected file must identify the path and reason, not become missing-header drift or successful coverage from an unexamined source. A failed clean-tree check must not authorize writes.

### F14 — Comment-style mistakes are not rejected early

`config/mod.rs:95–96` stores arbitrary named styles without resolving them. An unknown alias on `.rs` silently falls back to built-in Rust syntax. `inline_syntax` accepts an empty `line_prefix`; confirmed output was a bare SPDX line inside Rust with apply exit 0. Newlines/control characters in configurable tokens and copyright text also need validation before rendering.

`comment/mod.rs:109` matches an `ExactPath` association by basename alone, so configuration for `a/foo` also applies to `b/foo`. Resolve exact paths before filenames and extensions, with consistent declaration-order rules. Reuse the existing registry; no new language plugin mechanism is warranted.

### F15 — The preview and machine interfaces hide useful information

Successful human apply reports skip compliant final files before showing their changes (`report/render.rs:15–16`). Sidecar changes are keyed by sidecar path while file states use asset path (`cli/apply.rs:178–182`, `report/mod.rs:135–141`), so their changes disappear from file entries. License-text writes are absent from the preview, as are actual before/after contents. A failed write can be rendered as “would apply,” conflating failure with dry-run.

`check --explain` scans the full selected tree and returns 0 even when the path is absent; defaults are described as no rule matched. `init --format json --output 'a"b.toml'` emits invalid JSON because its path is interpolated directly (`cli/init.rs:77–81`). Curl prompts and progress use stdout even for JSON output (`cli/mod.rs:270–294`). Lint reports empty file lists and empty counts despite reporting file failures.

A shared serializable report should expose file diagnostics and an independent list of planned/attempted writes, including source, sidecar, REUSE, config, and license-text destinations. Send diagnostics/progress to stderr; stdout in JSON mode must contain exactly one valid document. Keep ordinary output compact and make dry-run contain the proposed text/diff and unresolvable requirements.

### F16 — Cache and unnecessary dependency work can be deleted

`engine.rs:160` discards every cache lookup. Detection, classification, and content hashing still run, including under `--no-cache`. `walk/cache.rs:115` marks unchanged entries dirty, then flush rewrites the entire historical cache. Removed paths are not pruned. The existing keys omit sidecars and external metadata, so trusting the cached drift label would introduce new correctness bugs.

Remove the inactive cache implementation; retain old CLI flags temporarily as documented deprecated no-ops if needed. Do not introduce a replacement until end-to-end measurements justify it. `gix` is used only for root discovery, although broad expensive features are enabled; system Git already supplies subset and dirty-tree operations. Consolidating Git access removes this dependency instead of duplicating engines. Remove unused direct dependencies only after searching their actual uses and running locked checks.

### F17 — Green tests miss the scenarios that matter

The test suite is a useful foundation: disposable Git fixtures, byte-stable reports, ordinary destructive/additive edits, sidecars, comment registry round trips, and ordinary precedence are already represented. Keep those assets.

Weak points include optional external conformance even in CI (`tests/us5_reuse.rs:67`), substring-only init assertions, an offline test that only compares two outputs, and a partial-write fixture based on directory permissions that behaves differently under root. CRLF tests do not prove that no bare LF was introduced or that nonmetadata bytes survived. Existing target-header tests do not test invalid indexes, copyright-only blocks, or comments separated by blank lines.

Add regressions at the actual public boundary for each finding, a compact table of REUSE differential fixtures, and full-buffer byte comparisons around edits. Assert command status, exact affected paths, unchanged failed-file contents, absence of outside-root writes, and post-apply behavior. Make the reference comparator mandatory in a dedicated CI job; failure to launch it must fail that job. A second independent review should assess semantic fixtures against the specification, not merely the implementation's outputs.

### F18 — Publishing has source-visible blockers and premature side effects

`.github/workflows/release.yml:12` grants contents write but not the `id-token: write` permission needed by its crates.io OIDC authentication action (`:121`). This permission requirement is documented by [GitHub](https://docs.github.com/en/actions/reference/security/oidc#workflow-permissions-for-the-requesting-the-oidc-token) and the [crates.io authentication action](https://github.com/rust-lang/crates-io-auth-action). Registry trust configuration remains unverified.

The workflow publishes a public GitHub release before binary builds complete, and no tests gate tag releases. `prepare-release.yml` documents a GITHUB_TOKEN fallback that cannot trigger the tag workflow, then recommends manual release execution although `release.yml` has no manual trigger.

Grant OIDC only to the publish job, validate/test the exact tagged commit, keep the release draft until artifacts succeed, and make the fallback operable or fail before creating a tag. Do not fix this by retagging or publishing during the improvement task. Re-query remote tags and versions before any future release; the mise/source version mismatch makes guessing a next version unsafe.

### F19 — Metadata and documentation need narrower, truthful claims

There are 14 embedded license texts, not the whole SPDX corpus. `build.rs:17–20` tolerates a missing bundle directory and skips directory-entry errors; `:43` reads a version environment variable without registering `rerun-if-env-changed`. The hardcoded text-bundle version does not necessarily describe the recognition list embedded by the spdx dependency.

Mise selects nightly and installs Licet 0.2.2 while this checkout is 0.1.4. CI builds without `--locked`. Several comments still say gix handles subsets, every cache hit avoids work, all out-of-band values override headers, or new language support is purely data despite special insertion rules. Dead symmetry plumbing and hand-built success fallbacks make the implementation harder to assess.

Document separate tool, recognition-list, and bundle versions; validate bundle inputs; align development checks with the MSRV/stable policy. Update contracts alongside behavior and remove false claims rather than preserving them in comments.

## Performance measurements and recommendations

The optimized existing Criterion benchmark completed with these point estimates:

| Files | Existing “cold” engine benchmark | Existing “warm” engine benchmark |
|---:|---:|---:|
| 500 | 10.415 ms | 11.450 ms |
| 2,000 | 40.193 ms | 40.853 ms |

These labels are not trustworthy comparisons of a real cold/warm CLI: “cold” disables cache; “warm” includes its own `.licet-bench-cache` file in selection and omits cache flush. They establish that the benchmark runs, not that its workloads are equivalent.

A separate release-binary experiment used 10,000 committed tiny Rust files in 100 directories, default MIT, excluded config, and captured JSON output. Every full run asserted 10,000 compliant files; every staged run asserted one selected file. Five samples per mode gave:

| End-to-end command mode | Median elapsed |
|---|---:|
| Full scan, `--no-cache` | 202.09 ms |
| Full scan, cache file removed before each run | 194.79 ms |
| Full scan, populated cache | 204.02 ms |
| One staged file after full scan, `--no-cache` | 14.77 ms |
| One staged file after full scan, populated cache | 17.19 ms |

These are local wall-clock samples with a hot OS filesystem cache, not reference-runner guarantees or peak-memory measurements. The first no-cache sample took 1.196 seconds; the other full-scan samples ranged about 184–259 ms. Files were tiny and had no mixed OOB metadata. The cache shows no benefit on this workload, consistent with the traced code. Whole-file metadata scanning will change the workload; remeasure after correctness fixes.

Priorities: delete inert caching and duplicate parsing, batch Git object reads and REUSE document updates, then measure representative small/large files and many annotations. Avoid an indexed rule engine or parallel walker until profiles show a meaningful bottleneck. Existing Rayon and the installed SPDX/TOML libraries are enough for the first repair pass.

## Completion criteria

The app is ready for review when every confirmed reproduction is a regression test, REUSE differential cases agree or have an explicitly documented policy-only difference, dry-run matches actual planned destinations, failure status is consistent across all output modes, and outside-root sentinels and nonmetadata bytes survive adversarial writes. Run all required checks on the final source and report platform coverage honestly. The plan ends at a reviewable implementation; it does not authorize a registry publication or tag mutation.
