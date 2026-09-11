# Licet reliability and REUSE 3.3 implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Licet's declarative licensing operations safe, predictable, testable, and conformant with REUSE 3.3, with truthful reports and measured performance.

**Architecture:** Keep the existing walk → detect/rules → classify → reconcile pipeline. Use system Git for repository snapshots, the installed SPDX parser for expressions, the installed TOML parser/serializer for metadata, and one safe file-replacement helper. Distinguish actual REUSE metadata from desired policy, compute a complete write plan before mutation, and remove the unused scan cache.

**Tech Stack:** Rust edition 2024, MSRV 1.89, clap, serde, toml, spdx, globset, ignore, Rayon, tempfile, existing assert_cmd/insta fixtures. No HTTP/TLS/async-runtime dependency; optional downloads remain system curl.

**Spec:** [Audit and evidence](../../2026-09-10-licet-audit.md), [supplied REUSE 3.3 specification](../../REUSE_Specification_v3.3.md), and the existing [behavioral spec](../../../specs/001-declarative-license-headers/spec.md), [CLI contract](../../../specs/001-declarative-license-headers/contracts/cli.md), [configuration contract](../../../specs/001-declarative-license-headers/contracts/config-schema.md), and [report schema](../../../specs/001-declarative-license-headers/contracts/report.schema.json). When older contracts contradict the REUSE target or the decisions below, update them as part of the owning task.

## Global constraints

- Edition 2024; minimum supported Rust version remains **1.89**. Do not raise it to fix local tooling.
- `license.toml` remains the sole declaration of desired policy. Existing metadata describes actual licensing and must retain its provenance.
- Preserve copyright by default. `add:` and explicit `replace:` must work consistently. Never invent a copyright holder or placeholder license text.
- Preserve all nonmetadata bytes and existing file permissions during replacement; preserve LF/CRLF, BOM, and required language preambles.
- Never write through a symlink, escape the project through a metadata-derived path, or delete an existing destination after a failed download.
- Read-only commands and dry runs perform no writes, including caches, temporary project files, license downloads, or metadata updates.
- Network is opt-in through existing curl capability. JSON stdout is exactly one serialized document; progress and prompts go to stderr.
- Keep existing cross-platform CI and binary targets. Platform-specific safety tests may use `#[cfg(unix)]` or Windows equivalents, with explicit coverage limits.
- Do not add a replacement cache, generic filesystem/provider interfaces, plugin systems, a database, or a license-policy DSL.
- Reuse installed dependencies. Promote the already installed `tempfile` to a normal dependency for production atomic writes; remove `gix` once system Git owns discovery. No other new runtime dependency is needed for this plan.
- Existing 0.1.4 source and mise's 0.2.2 installation differ. Do not guess a next package version, tag, publish, or mutate remote releases while implementing. Use the source-built binary.
- Source snapshot has no Git metadata. An executor must locate the actual repository or initialize an isolated development copy before making commits; never fabricate a reviewed commit SHA for this snapshot.

## Behavior decisions — implement these consistently

These are product decisions for this repair, not unresolved questions for the executor.

1. **Command meaning.** `check` validates desired policy over its selected files and requires the license texts referenced by those files. `lint` validates actual REUSE 3.3 metadata over every REUSE Covered File, independently of whether a declaration rule exists. A project can satisfy policy but still lack copyright required by REUSE when its policy is `preserve`; state that distinction plainly. `lint` neither parses auto-discovered policy configuration nor obeys its exclusions. `apply` reconciles declared policy and required texts, then reports the resulting policy status. Users run `lint` for complete REUSE validation.
2. **Coverage.** In a Git repository, default `check`/`apply` coverage is tracked regular files, honoring explicit declaration exclusions but not dropping tracked files because a later ignore rule matches them. `lint` covers tracked plus nonignored untracked files, minus REUSE-specified ignored files. In a nonrepository directory, retain a nonignored filesystem walk; `apply` there requires `--allow-dirty` because no Git undo guarantee exists. Git launch/status errors never mean “clean.”
3. **Snapshots.** `check --staged` evaluates index blobs, including applicable staged metadata/configuration/license texts. `check --changed REV` evaluates current working-tree bytes for paths changed relative to the validated commit. `apply --staged` uses the staged **path set** but edits working-tree files, advertises that fact, obeys the dirty-tree guard, and never stages its edits. Freeze the chosen path set for post-apply verification. Any changed licensing metadata that can affect other files expands a staged/changed check to the relevant snapshot's complete covered set; conservative full expansion is acceptable and must be reported.
4. **Paths.** Explicit file arguments are relative to invocation cwd; declaration selectors and metadata paths are relative to their documented base. Normalize `.`/`..` lexically without changing file identity. Reject paths outside the root and nonregular input paths. Symlinks are ignored consistently, including explicit selection, as required by REUSE; remove the old canonical-target dedup/follow contract. Keep OS paths as OS paths until presentation. Do not replace Unix literal backslashes with separators.
5. **Metadata.** Keep all effective file license expressions and copyright notices, separately from raw editable spans. Multiple associated license expressions apply together: compare their canonical AND-combination with declarative intent. Snippet expressions contribute to license inventory but never satisfy file policy. A malformed tag produces a diagnostic instead of disappearing. Sidecars replace source-file licensing information; malformed sidecars are errors, not permission to fall back silently.
6. **Apply modes.** Destructive mode replaces license values in the selected header scope, preserves other headers and snippets, and reports any remaining policy drift. `--target-header N` is zero-based and must be in range for every affected file; a matching license elsewhere does not defeat an explicit target. Additive mode preserves every existing license expression for in-file, sidecar, and REUSE destinations; if combined actual licensing still differs from intent, exit 1 with a residual-drift diagnostic. Do not call all multi-license combinations intrinsically contradictory. Existing tests expecting any matching line to prove complete compliance must change.
7. **REUSE writes.** Fix the effective source, not merely the source file's syntax. For a REUSE document, retain existing bytes and append serialized exact-path annotations when an update is required; put them in the document whose matching annotation must be superseded. Preserve precedence, unknown keys relevant to the superseded table, and effective copyright. Batch updates to one document. If multiple aggregate/DEP5 sources cannot be safely reconciled without changing sibling meaning, return an actionable unfixable result before writes instead of pretending to fix them.
8. **Outcomes.** Exit 0 means the command's success condition is true. Exit 1 means valid input but remaining policy/compliance violations, unfixable requirements, incomplete validation, or failed writes with no successful changes. A completed additive or targeted edit with residual policy drift is exit 1, even though its requested writes succeeded. Exit 2 means invocation, config, malformed metadata, or snapshot preparation failed before any write. Exit 3 means one or more files were actually changed and an operational error subsequently occurred (including a failed replacement, durability check, or final scan). Once a write commits, later operational errors are partial results rather than unrelated exit 2 errors. Dry-run returns 0 only when fully determined local replacements satisfy policy and text requirements; unverified required downloads or other unresolved requirements yield exit 1, and invalid input yields 2.
9. **Reports.** Introduce report schema **version 2** for independent diagnostics and write records. Do not silently put incompatible shapes in v1. Retain existing human/json format names and command names. Include before and projected/final pass states so dry-run success cannot be confused with current compliance. Include `summary.complete`; unsupported encoding or an unread dependency makes validation incomplete, never a proven REUSE violation or a pass.
10. **Init.** Conservative projection preservation outranks fewer rules. Emit exact rules first; compress only provably uniform groups. Previously unknown files remain unknown. Output defaults to the discovered root's `license.toml`; explicit `--output` overrides `--config`, which is treated as the destination for init. Keep `--from-reuse` as a documented compatibility alias for the existing inspection behavior. Refuse overwrite unless `--force`; explicit output outside the project authorizes only that particular config destination, using its parent as the writer's allowed root.
11. **Encoding and filename compatibility.** The repair guarantees UTF-8 metadata and sources; binary assets remain coverable through readable sidecars/OOB metadata. REUSE only recommends UTF-8, so undecodable non-UTF-8 metadata is reported as `unsupported_encoding`, incomplete validation, exit 1, rather than a normative licensing violation. Do not discard it, rewrite it, or call the project compliant. Document this implementation limit. Existing extensionless license texts remain recognizable to materialization/policy operations, but strict REUSE 3.3 lint reports `missing_license_extension`; the specification requires a filename extension even though the reference tool accepts some extensionless names. Record this deliberate normative/reference difference in differential fixtures.

## Execution order and review ownership

There are twelve independently reviewable tasks. Complete tasks 1–9 before calling the app reliable; tasks 10–12 complete the requested performance, testing, and delivery improvements.

```text
1 safe writes ────────────────┐
2 file selection ──┐          │
3 detection/SPDX ──┼─> 4 OOB ─┼─> 5 classification/inventory
                  │          │              │
                  └──────────┴──────────────> 6 reconciliation
                                             │
                                             v
                                         7 outcomes/preview
                                             │
                                         8 init, 9 CLI/config
                                             │
                                         10 performance
                                             │
                                         11 CI evidence
                                             │
                                         12 release/docs
```

Use delegated bounded implementation or review where it is independent. Assign one agent ownership of each touched module at a time; tell workers they are not alone and must not revert others' edits. Tasks 2–5 share domain/engine types: land their agreed shape before parallelizing dependent work. Do not ask several agents to redesign metadata independently while implementation is underway. A separate reviewer should reject a task when acceptance is only demonstrated by the old tests.

For each task: add the specified failing regression, run it to observe the original failure, implement the minimum change, run the focused suite, review the diff, update the relevant contract, and commit in the isolated actual repository. Do not “fix” a regression by weakening its expected behavior. Commands below assume the repository root.

## Task 1 — Contain and preserve every filesystem write

**Findings:** F01, F02, F04, F13. **Depends on:** none.

**Files:** modify `src/reuse/mod.rs`, `src/reuse/inventory.rs`, `src/cli/init.rs`, `src/cli/apply.rs`, `src/reuse/oob.rs`, `Cargo.toml`, `Cargo.lock`; extend `tests/safety.rs`, `tests/us5_add_license.rs`.

**Interface:** replace the text-only writer with one byte-safe helper; all mutation callers must supply their allowed root, relative path, and expected old contents. `None` means the destination must not exist; an empty existing file is `Some(&[])`.

```rust
pub fn atomic_write(
    root: &std::path::Path,
    relative: &std::path::Path,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> Result<(), WriteError>;

#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub struct WriteError {
    pub source: std::io::Error,
    pub replacement_completed: bool,
}
```

- [ ] Add this regression to `tests/safety.rs` (existing `mod common`/`Fixture` setup applies), and run `cargo test --locked --test safety apply_does_not_follow_predictable_temp_symlink`.

```rust
#[cfg(unix)]
#[test]
fn apply_does_not_follow_predictable_temp_symlink() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a() {}\n");
    let outside = tempfile::tempdir().unwrap();
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"KEEP").unwrap();
    symlink(&sentinel, f.path().join(".a.rs.licet.tmp")).unwrap();
    let out = f.licet()
        .args(["apply", "--allow-dirty", "--files", "a.rs"])
        .output().unwrap();
    assert!(out.status.success(), "{:?}", out);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"KEEP");
    assert!(!std::fs::symlink_metadata(f.path().join("a.rs"))
        .unwrap().file_type().is_symlink());
}
```

- [ ] Add table-driven writer tests: preserve modes `0600`, `0700`, `0755`; preserve existing predictable temp file contents; reject `../outside`, absolute relative-path inputs, symlink destination, symlink parent, directory/FIFO destination; a failed replacement keeps the original bytes; expected-content mismatch aborts; two independent destinations do not collide. Guard Unix permission/FIFO cases appropriately and add Windows reparse-path coverage in CI.
- [ ] Implement destination validation **inside** the shared writer. Canonicalize the allowed root, walk the relative path components, reject escape components and symlink/reparse ancestors, and distinguish missing final destination from other metadata errors. Do not use `exists()` to collapse permission/read failures into absence.
- [ ] Create the temporary file with `tempfile::NamedTempFile::new_in(parent)`. Write/flush contents, set existing destination permissions before replacement, sync the temporary file, recheck expected destination bytes/type, and persist. Use no-clobber persistence for creation. Clean up only the owned temp on errors; do not unlink a destination to work around Windows rename behavior. Preserve directory durability where supported and propagate sync errors. Errors before persistence set `replacement_completed=false`; errors after persistence set it true so reporting counts a committed write even if its durability check failed. Add a library-level test seam for a forced post-persist sync failure and assert both resulting disk bytes and this flag; no production CLI test switches.
- [ ] Validate materialization IDs against known SPDX licenses or standard exceptions; allow syntactically valid `LicenseRef-*` only to report a required local text, never for downloading. Canonicalize standard ID spelling, reject separators/absolute paths/control bytes, and reject a compound expression passed as one `add-license` ID. Validate the whole requested list before creating any directory.
- [ ] Change curl to write to owned temporary storage, with `--disable` first (ignore user curl configuration), `--fail --silent --show-error --location`, HTTPS-only initial/redirect protocols, `--connect-timeout 10`, `--max-time 60`, and a 4 MiB maximum text size. Preserve the explicit `--allow-curl`/TTY consent path. After nonempty UTF-8 text validation, install through the shared writer; failure never deletes the final destination. Pin download URLs to the documented corpus release when supplied by the bundle metadata, rather than a floating branch.
- [ ] Put fake curl on PATH in tests. `add-license ../victim --allow-curl` must exit 2 before invoking fake curl and retain `victim.txt`. Bundled MIT must never invoke curl. A valid unbundled standard ID with fake curl success creates one text; exit 22, empty result, oversized result, and launch failure create none and preserve existing files. JSON mode must remain parseable.
- [ ] Replace all `read_to_string(...).unwrap_or_default()` on mutation destinations with a `NotFound`-only absence branch. Route source, sidecar, metadata, init, and license-text writes through the helper. The old cache will be removed in task 10; until then it must never use the unsafe writer.
- [ ] Run `cargo test --locked --test safety --test us5_add_license --test us5_sidecars`, then commit `fix: contain atomic writes and license downloads`.

**Review ceiling:** The portable helper protects against pre-existing malicious paths and detects changes observed between planning and replacement. It is not an OS-level compare-and-swap against a hostile process concurrently replacing ancestors. Do not claim stronger race guarantees. If same-process/multi-process tests expose an actual remaining attack path, use handle-relative platform operations for that path; do not add a speculative filesystem framework.

## Task 2 — Make file selection and snapshot reads correct

**Findings:** F06, F07, F13. **Depends on:** task 1 containment policy.

**Files:** modify `src/walk/mod.rs`, `src/engine.rs`, `src/cli/mod.rs`, `src/cli/check.rs`, `src/cli/apply.rs`, `src/cli/lint.rs`, `src/domain.rs`; add `src/walk/git.rs` only to hold the concrete Git command/snapshot code; extend `tests/us4_enforce.rs`, `tests/safety.rs`, `tests/common/mod.rs`.

**Interfaces:** use a small content-source enum, not a provider trait. Carry the same source through all dependent reads. `Discovered` retains OS-native relative paths. Store index entries by path and object ID once per scan.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSource { Worktree, Index }

// In walk/git.rs; return checked raw output and retain stderr in errors.
pub fn git_output(root: &std::path::Path, args: &[&std::ffi::OsStr])
    -> crate::Result<Vec<u8>>;
```

- [ ] Extend `Fixture::new()` to disable inherited commit signing and repository hooks locally. Set `commit.gpgsign=false`, `tag.gpgsign=false`, and `core.hooksPath` to a newly created empty fixture directory; keep that directory out of fixture commits. This prevents the audit's signing-agent failure from recurring. Add a helper for checked Git commands and use it in new fixtures.
- [ ] Add staged-snapshot regression in `tests/us4_enforce.rs`:

```rust
#[test]
fn staged_check_reads_index_bytes() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"license.toml\"]\n")
        .write("f.rs", "// SPDX-License-Identifier: MIT\n")
        .commit("base");
    f.write("f.rs", "// SPDX-License-Identifier: Apache-2.0\n").stage_all();
    f.write("f.rs", "// SPDX-License-Identifier: MIT\n");
    let out = f.licet().args(["check", "--staged", "--format", "json"])
        .output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(report["files"].as_array().unwrap().iter().any(|entry|
        entry["path"] == "f.rs" && entry["actual"] == "Apache-2.0"));
}
```

- [ ] Add reverse staged/worktree content case, staged deletion/rename/addition on unborn HEAD, unresolved merge index, staged sidecar with different working-tree sidecar, metadata-only staged change affecting unchanged source, and staged license-text deletion. Assert no index mutation. Explicit custom `--config` for index checking must exist in that same snapshot inside the root; otherwise exit 2 instead of silently reading the working copy.
- [ ] Use checked system Git commands: root with `rev-parse --show-toplevel`, Git metadata directory with `rev-parse --absolute-git-dir`, tracked entries with `ls-files --stage -z`, subsets with `diff --name-only -z --diff-filter=d`, and validate REV as a commit before passing its object ID to diff. Use `--`/end-of-options as appropriate; do not interpolate user revisions into shell text. Distinguish a nonrepository from Git missing or failing; no arbitrary Git error may authorize mutation.
- [ ] Decode NUL-delimited filenames losslessly. On Unix use `OsStringExt::from_vec`; preserve bytes through lookup and writes. Use escaped display plus an optional byte-array path field in JSON for non-UTF-8 Unix paths. On a platform unable to represent a path, produce an explicit failure instead of a lossy replacement. Spaces, quotes, Unicode, tab, newline, and literal Unix backslash must work.
- [ ] Read index blobs with one `git cat-file --batch` process addressed by object IDs from the index, parsing the declared blob length exactly. Do not send filenames through newline framing and do not spawn once per file. Read file, sidecar, all applicable metadata, config, and LICENSES through the same selected snapshot. Gitlinks and symlink index modes are excluded from covered content.
- [ ] Implement a common normalized path pipeline; test `src/x.rs`, `./src/x.rs`, and `src/../src/x.rs` from root and subdirectory. Reject outside-root file arguments before scanning. Skip symlinks identically regardless of order in an explicit list; do not canonical-dedup different ordinary paths.
- [ ] Implement the REUSE ignore matrix: root LICENSES text files, allowed COPYING/LICENSE/LICENCE variants at any depth, VCS internals, VCS-ignored untracked content, submodules, Meson subprojects as separate projects, all applicable REUSE.toml files, root `.reuse`, symlinks, zero-byte files, and SPDX document formats. Nonempty `data.empty` is not exempt by suffix. Nested unrelated `LICENSES` directories are not automatically a project-wide exemption. Test `sub/LICENSE-MIT`, `sub/COPYING.GPL`, and `sub/LICENCE.md` as ignored, but `sub/LICENSES/a.rs` as covered. Sidecars are checked through their companion; diagnose orphan/malformed sidecars according to the reference fixture behavior.
- [ ] Use tracked entries for default policy operations and tracked plus `ls-files --others --exclude-standard -z` for Git-backed lint. In the nonrepository fallback, retain `ignore` traversal and surface traversal/read errors. Do not apply arbitrary declaration exclusions to lint. Test a compliant repo with no license.toml and an unlicensed file hidden only by a declaration exclusion.
- [ ] Run `cargo test --locked --test us4_enforce --test safety --test determinism`. Review linked-worktree, metadata-only subset, and non-UTF-8 cases independently; commit `fix: evaluate licensing against consistent Git snapshots`.

## Task 3 — Use parsed SPDX expressions and complete metadata scans

**Findings:** F08, F12, F13. **Depends on:** task 2 read-source interface.

**Files:** modify `src/spdx/mod.rs`, `src/detect/mod.rs`, `src/domain.rs`, `src/engine.rs`; extend `tests/us5_snippets.rs`, `tests/us1_check_drift.rs`, and existing unit tests.

**Interfaces:** retain `parse_canonical` and `expressions_equal` for callers, but require parse errors to remain diagnostics. Add `expression_ids(expr: &str) -> Result<BTreeSet<String>, String>` using the dependency requirements. Extend `ActualLicenseState` with `effective_licenses: Vec<String>` and `diagnostics`; keep raw `headers` for later surgical edits. Keep `detected_license` only as the canonical combined presentation value, not the inventory source.

- [ ] Add and run this unit regression:

```rust
#[test]
fn canonicalization_uses_parsed_structure() {
    for expr in [
        "MIT OR Apache-2.0",
        "MIT\tOR\tApache-2.0",
        "MIT\nOR\nApache-2.0",
        "(((MIT OR Apache-2.0)))",
        "Apache-2.0 OR MIT",
    ] {
        assert!(expressions_equal(expr, "MIT OR Apache-2.0"), "{expr:?}");
    }
    assert!(!expressions_equal("MIT AND Apache-2.0", "MIT OR Apache-2.0"));
}
```

- [ ] Replace string reparsing with the already available postfix AST. Keep the existing `Node` rendering initially; populate it as follows, then delete `parse_node`, `split_top_level`, `strip_outer_parens`, `normalize_leaf`, and `split_with` when their callers disappear:

```rust
use spdx::expression::{ExprNode, Operator};
fn canonicalize(expr: &spdx::Expression) -> String {
    let mut stack = Vec::new();
    for item in expr.iter() {
        match item {
            ExprNode::Req(req) => stack.push(Node::Leaf(req.req.to_string())),
            ExprNode::Op(op) => {
                let right = stack.pop().expect("validated expression: right operand");
                let left = stack.pop().expect("validated expression: left operand");
                stack.push(match op {
                    Operator::And => Node::And(vec![left, right]),
                    Operator::Or => Node::Or(vec![left, right]),
                });
            }
        }
    }
    render(&stack.pop().expect("validated nonempty expression"))
}
```

- [ ] Check case/canonical ID policy against the existing public tests. Use the dependency's lax parsing/canonical IDs only where needed for the promised standard-ID case tolerance; preserve custom LicenseRef case. Do not retain an invalid-raw-string equality fallback that can turn malformed metadata into success. REUSE 3.3 rejects nonstandard exception identifiers even if a newer SPDX parser accepts custom additions. Add explicit tests for WITH, standard exceptions, `+`, LicenseRef, malformed values, and AND/OR grouping. Use `requirements()` for every license and exception reference, with reference fixtures for legacy `GPL-2.0+` filename normalization.
- [ ] Remove the 8 KiB semantic cutoff. Scan text metadata to EOF using `BufRead::read_until(b'\n', ...)`, preserving parser state across chunks and absolute byte offsets. Keep memory bounded to active input and retained metadata; a binary containing non-UTF-8 bytes is not an I/O failure. If sidecar or overriding metadata makes source bytes irrelevant, avoid unnecessarily decoding the binary asset. Apply still loads a target's full bytes when preparing an edit.
- [ ] Refactor the existing marker logic into a reusable per-line parser state, shared by streaming reads and parsing in-memory replacement content. Track ignored regions, snippet begin/end, file metadata, and byte spans. Tags within snippets do not become file headers. Diagnose invalid license values, unclosed/nested/mismatched snippets, unsupported required metadata encoding, and incomplete reads. Treat unclosed ignore blocks according to the supplied spec/reference fixture; never quietly discard an unrelated I/O error. Add Latin-1 source/sidecar/license-text fixtures containing a non-ASCII holder: required undecodable metadata produces `unsupported_encoding` and `complete=false`; a non-UTF-8 asset with complete UTF-8 sidecar/OOB coverage can still validate. Do not assert that Latin-1 is prohibited by REUSE.
- [ ] Recognize tagged and conventional `Copyright`/`©` notices; retain original raw text and location. Validate nonblank holder information without requiring a year or email. Do not mistake a source-code string example for an editable comment; detection may find a tag anywhere for REUSE parity, but write spans need a validated comment context. Ignore-block examples must be tested.
- [ ] Add CLI fixtures: valid header and late snippet at 10 KiB and 1 MiB; marker crossing the old 8 KiB boundary; UTF-8 sequence crossing a chunk boundary; sidecar >8 KiB; binary with valid sidecar; malformed sidecar over valid source; invalid tag alongside valid tag; copyright-only header followed by blank line and license; a 16 KiB LicenseRef processed without the old quadratic delay. No hard microsecond assertion in ordinary unit tests.
- [ ] Run `cargo test --locked spdx::` and `cargo test --locked --test us5_snippets --test us1_check_drift`; commit `fix: parse complete licensing metadata with the SPDX AST`.

## Task 4 — Resolve complete REUSE metadata with provenance

**Findings:** F08, F09, F13. **Depends on:** tasks 2 and 3.

**Files:** modify `src/reuse/oob.rs`, `src/domain.rs`, `src/detect/mod.rs`, `src/engine.rs`; extend `tests/us5_reuse.rs`, `tests/safety.rs`, `tests/us5_sidecars.rs`.

**Interfaces:** make `OutOfBand::load` fallible and supply the current content source. Replace single-value `OutOfBandEntry.license` with a list. Retain each selected table's metadata path, directory base, table index, precedence, licenses, and copyrights. Keep a compact concrete `MetadataOrigin` record in `domain.rs`; no virtual-source hierarchy.

```rust
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
enum StringList { One(String), Many(Vec<String>) }

#[derive(Debug, serde::Deserialize)]
struct ReuseToml {
    version: u32,
    #[serde(default)]
    annotations: Vec<ReuseAnnotation>,
}
// path is required StringList; licensing and copyright fields are optional StringList.
// Preserve/allow unknown REUSE keys; the REUSE schema explicitly permits extension keys.
```

- [ ] Add a valid license-array fixture with both real license texts and a metadata-covered binary. Assert all expressions appear in effective metadata and inventory; a valid array in one table must not discard other tables. Make malformed document, version other than 1, missing path, invalid precedence, invalid expression, and REUSE-plus-DEP5 coexistence fail before writes with document path and parse location.
- [ ] Discover applicable nonignored REUSE files at every directory depth using the same snapshot. A nested file's paths are relative to that metadata file, may not escape its directory, and can cover only its descendants. Retain last-match selection **within each document** before hierarchy resolution.
- [ ] Implement a separate REUSE pattern compiler: `*` cannot cross `/`; `**` and `**/` can; only the specified asterisk/backslash escapes are special. Quote `?`, brackets, and braces before feeding globset, or translate the small REUSE grammar into globset's escaped syntax; use `GlobBuilder::literal_separator(true)`. Do not reuse it unchanged for DEP5. Add literal special-character filenames and escape cases.
- [ ] Resolve copyright and license independently. This matrix is mandatory, including cases where a table provides only one field:

| Root/parent metadata | Child metadata | File/sidecar | Effective field |
|---|---|---|---|
| closest A | absent | B | B |
| closest A | closest B | absent | B |
| closest A | override B | C | B |
| aggregate A | closest B | C | A plus C |
| aggregate A | closest B | absent | A plus B |
| override A | override B | C | A |
| closest A | aggregate B | C | B plus C |
| absent | absent | sidecar B, source C | B |

- [ ] Determine the rootmost matching override barrier first: it suppresses all closer tables and file/sidecar licensing information, even if the override table omits one field. Then resolve license and copyright independently over the allowed contributors; an absent field contributes nothing, rather than reopening suppressed sources. Add root copyright-only override plus complete file (override copyright, missing license), root license-only override plus complete file (override license, missing copyright), parent aggregate plus child override (parent aggregate retained), and parent override plus child aggregate (child suppressed). Keep raw suppressed notices available for preservation but exclude them from effective values and inventory. Include all contributing source records in JSON explanation.
- [ ] Implement DEP5 paragraph continuation/unfolding for Files, Copyright, and License; aggregate it with file-level metadata. Preserve unsupported extra fields without interpreting them. Never emit a new REUSE document alongside existing DEP5. Existing DEP5 mutation remains manual with precise affected paths and a nonzero result.
- [ ] Run the matrix through the pinned reference comparator as well as internal assertions; `licet lint` and `reuse lint --json` must agree on coverage/reference sets. Run `cargo test --locked --test us5_reuse --test us5_sidecars --test safety`; commit `fix: resolve REUSE annotations and precedence without losing values`.

## Task 5 — Separate policy drift, REUSE validation, and license inventory

**Findings:** F08, F11, F13. **Depends on:** tasks 3 and 4.

**Files:** modify `src/report/classify.rs`, `src/domain.rs`, `src/reuse/inventory.rs`, `src/engine.rs`, `src/cli/check.rs`, `src/cli/lint.rs`, `src/cli/add_license.rs`, `src/rules/mod.rs`; extend relevant unit tests and `tests/us5_reuse.rs`, `tests/us5_add_license.rs`.

**Interfaces:** keep separate `actual_referenced_ids` and `desired_referenced_ids` in `ScanResult`. Inventory computation becomes fallible and accepts actual reference IDs explicitly. File diagnostics carry stable codes such as `missing_copyright`, `invalid_license`, `read_error`, `unclosed_snippet`; do not encode them as fabricated `<conflict>` licenses. Add `CopyrightMismatch` policy drift with its own count to the v2 schema; if license and copyright both differ, use `WrongLicense` as the primary drift and retain the copyright mismatch as a diagnostic.

- [ ] Add the missing-copyright false-pass regression below to `tests/us5_reuse.rs`:

```rust
#[test]
fn lint_requires_copyright_independently_of_license_intent() {
    let f = Fixture::new();
    f.config("# SPDX-License-Identifier: MIT\n# SPDX-FileCopyrightText: 2026 Test\n[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\n")
        .write("LICENSES/MIT.txt", licet::spdx::bundled_text("MIT").unwrap());
    let out = f.licet().args(["lint", "--format", "json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["summary"]["pass"], false);
    assert!(out.stdout.windows(b"missing_copyright".len())
        .any(|bytes| bytes == b"missing_copyright"));
}
```

- [ ] Policy equality uses all effective license expressions, joined as an AND expression with necessary parentheses, parsed/canonicalized once. A matching MIT candidate must not hide an additional Apache license. Preserve syntactically valid multiple expressions as valid REUSE metadata while distinguishing declaration mismatch. Update additive/target-header tests to the explicit mode policy above.
- [ ] Compare copyright policy separately: preserve accepts existing notices without creating one; add requires existing notices plus the requested normalized notice; replace requires the explicit requested notice in the chosen effective scope. Compare duplicate rules using the entire normalized intent, including copyright policy. Equal specificity and different intent is a conflict; identical intent resolves to earliest declaration order. Document the rule instead of simultaneously promising last-order overrides and conflicts.
- [ ] `lint` succeeds without `license.toml` when actual metadata is complete. It does not parse auto-discovered policy content; that file is simply another Covered File whose licensing metadata is checked. Retain lint's explicit `--config` flag for compatibility: explicitly supplied missing/malformed config is usage error 2, and an existing valid explicit config is accepted with a deprecation notice but does not change REUSE evaluation. Change this flag to `Option<PathBuf>` so omitted and explicit inputs can be distinguished. Test correctly annotated but malformed auto-discovered policy TOML as REUSE-compliant. `check` continues to require its config. `add-license --all` uses the union of desired and actual references and must not swallow config errors.
- [ ] Inventory **every effective file expression and snippet expression**, including exceptions. Exclude suppressed values and unused config rules from REUSE actual inventory. In policy `check`, require texts for the selected actual/desired scope as documented. `apply` supplies required texts for its projected selected state, not stale replaced licenses or unrelated unused rules. Never delete unused texts automatically.
- [ ] Inventory validates regular contained text files, nonempty readable text, recognized SPDX/LicenseRef naming, duplicate IDs under multiple filenames, and unused/unrecognized entries. Project-wide unused/unrecognized findings affect only lint's full-reference evaluation, never a selected-file policy check. Test MIT `a.rs` and Apache `b.rs` with both texts: `check --files a.rs` and lint pass; deleting `b.rs` makes lint alone flag the unused Apache text. Use the pinned tool to characterize plaintext suffixes and legacy `+` naming. Support `.txt`/`.md`; recognize extensionless IDs for existing materialization compatibility, but lint reports their missing extension per decision 11. Explicitly test `LicenseRef-Acme-1.0.txt` versus `LicenseRef-Acme-1.0`, preserving the custom identifier's dotted version. Report unsupported encoding as incomplete validation, separately from empty/missing text or malformed names. Do not claim to prove legal correctness of arbitrary file contents.
- [ ] Test multiple header expressions, aggregate expressions, excluded snippet text, late snippets, missing exception, empty/invalid/binary license text, unused text, unknown file in LICENSES, no-config valid project, unlicensed file excluded only in declaration config, and unreachable desired rule. Check result codes **and** precise reference sets.
- [ ] Run `cargo test --locked --test us5_reuse --test us5_add_license --test us1_check_drift`; commit `fix: distinguish declared policy from REUSE compliance`.

## Task 6 — Reconcile metadata without corrupting sources or changing the wrong scope

**Findings:** F04, F05, F11, F14. **Depends on:** tasks 1–5.

**Files:** modify `src/reconcile/mod.rs`, `src/reconcile/insert.rs`, `src/comment/mod.rs`, `src/comment/comment_style.rs`, `src/detect/mod.rs`, `src/reuse/oob.rs`, `src/cli/apply.rs`; extend `tests/us2_apply.rs`, `tests/us3_comment_style.rs`, `tests/us5_sidecars.rs`.

**Interfaces:** `plan_file` becomes fallible. Carry comment context and exact SPDX value byte spans from detection, so replacement does not reparse arbitrary trailing code. Return planned bytes plus explicit mode/target/change reason. Sidecars and REUSE edits consume the same computed desired license/copyright values.

- [ ] Add table-driven preservation cases using this complete assertion pattern in reconciliation unit tests:

```rust
#[test]
fn ocaml_license_replacement_preserves_block_closer() {
    let old = "(* SPDX-License-Identifier: MIT *)\nlet x = 1\n";
    let actual = crate::detect::detect(
        std::path::Path::new("a.ml"), old.as_bytes(), None,
        &crate::reuse::oob::OutOfBand::default(),
    );
    let intent = crate::domain::LicenseIntent {
        license_expression: "Apache-2.0".into(),
        copyright_policy: crate::domain::CopyrightPolicy::Preserve,
    };
    let plan = plan_file(old, &actual, &intent,
        &crate::domain::CommentSyntax::block_only("(*", "*)", ""),
        crate::domain::ChangeMode::Destructive, None).unwrap();
    assert_eq!(plan.new_content.as_deref(),
        Some("(* SPDX-License-Identifier: Apache-2.0 *)\nlet x = 1\n"));
}
```

- [ ] Replace only validated metadata spans, applying edits in descending byte-offset order. Preserve opener/closer/trailing source exactly. Use the registry for both reading editable context and rendering; delete the independent two-terminator list. Additive mode inserts a valid separate comment/metadata line, never duplicates a terminator into executable text. A tag detected in ordinary program text without a safe comment span must not be rewritten; report an actionable unfixable location or use an explicitly chosen sidecar strategy.
- [ ] Distinguish copyright-only and license-bearing blocks. Without `--target-header`, target the first file-level license block; if none exists, insert a license in a safe header context while preserving existing notices. With an explicit index, validate it without clamping. Apply copyright intent even when the license already matches. Never edit snippets as file-level licenses. Preserve unrelated headers and report residual differences.
- [ ] Consolidate insertion-position logic. Support BOM immediately followed by shebang, shebang plus Python encoding declaration, XML declaration on its own line or before same-line body, PHP opening tag, Cabal required first line, and the existing TeX/BibTeX metadata preambles. For BOM-only prefix, do not insert a newline between BOM and a required shebang. Add exact-byte tests for LF, CRLF, no trailing newline, Unicode, multiline and single-line block comments. Where a language interpreter/compiler is installed, smoke-check representative generated files; deterministic byte assertions remain mandatory.
- [ ] Choose the write destination from effective metadata provenance **before** choosing source comment syntax. A root overriding REUSE annotation remains authoritative even when the source is Rust. Existing sidecars remain the target; sidecar additive mode preserves old identifiers and notices. Unsupported conflicting DEP5/aggregate layouts yield an unfixable plan entry instead of ineffective source edits.
- [ ] Delete `parse_blocks`, `quoted_values`, and handwritten `toml_string` in `reuse/oob.rs`. Parse existing metadata once, derive required exact exceptions, serialize an append-only patch with `toml::to_string`, and parse the complete proposed result before committing. Serialize copyrights as one string/list field, never repeated keys. Escape a literal path according to REUSE pattern grammar before TOML serialization so filenames containing `*` or backslashes do not become new globs. Retain original unknown fields where needed to preserve annotation meaning. Reuse the original newline convention for appended content.
- [ ] Append to the winning document at its own relative base, retaining/setting the precedence needed to supersede only the chosen path. Group all planned exceptions by destination so 1,000 assets produce one metadata write. Repeated apply with identical intent must add no annotation and perform no writes. Changing the license/copyright can append one replacement exact exception; no compaction/reformatting of hand-authored tables is required in this repair.
- [ ] Test two-holder broad annotation → exact exception (TOML remains valid, both holders preserved, sibling unchanged), exact annotation add/replace policy, literal-string/multiline/unknown-key documents, nested root override, malformed existing document byte preservation, and additive sidecar/REUSE behavior. Test a matching license with missing requested copyright as a real change.
- [ ] Run `cargo test --locked --test us2_apply --test us3_comment_style --test us5_sidecars --test safety`; commit `fix: preserve source syntax and effective metadata during apply`.

## Task 7 — Build a complete preview and derive one truthful outcome

**Findings:** F03, F15. **Depends on:** tasks 1–6.

**Files:** modify `src/cli/apply.rs`, `src/report/mod.rs`, `src/report/render.rs`, `src/domain.rs`, `src/error.rs`, `src/cli/check.rs`, `src/cli/lint.rs`, `src/cli/add_license.rs`, `specs/001-declarative-license-headers/contracts/report.schema.json`; extend `tests/us2_partial.rs`, `tests/us1_snapshots.rs`, `tests/us2_apply.rs`.

**Interfaces:** add concrete write records independent of file states. Use these same records for dry-run and execution. Byte buffers remain internal; JSON serializes text/diffs only for text metadata being written.

```rust
pub enum WriteKind { Source, Sidecar, ReuseToml, Config, LicenseText }
pub struct PlannedWrite {
    pub path: std::path::PathBuf,
    pub kind: WriteKind,
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
    pub affected_files: Vec<std::path::PathBuf>,
}
// Serialized outcome values: planned, applied, unchanged, failed, blocked.
// Reports own an independent writes array; do not join sidecar paths to asset states.
```

- [ ] Reproduce `LICENSES` as an ordinary file and copyright-only/no-license source. Assert apply cannot exit 0 with a failed summary or missing required license text. Add an unresolved equal-specificity rule conflict and an ineffective override fixture; all must be nonzero with precise paths/reasons.
- [ ] Split apply into preparation, execution, and final verification. Preparation scans once, derives all source/sidecar/REUSE writes, calculates projected required texts, validates all generated bytes and destinations, and identifies unfixable requirements before mutation. Known invalid config/metadata/path failures prevent all writes. Never fetch during dry-run. Missing bundled text is a planned license write; missing custom text is an explicit blocker; an unbundled permitted fetch is reported as a conditional planned operation, with its URL/destination. A required fetch that has not occurred makes dry-run `projected_pass=false` and exit 1 even with `--allow-curl`; explain that the network result is unverified. Human prompt-only preview also exits 1 without prompting. Test both cases and verify zero fake-curl invocations.
- [ ] Before execution, show/retain the complete plan and apply the dirty-tree guard as a fallible operation. Evaluate expected current bytes before every atomic write. Process independent writes in deterministic destination order; batch metadata-document updates. A runtime failure may allow independent safe writes to finish, but dependent writes must be blocked. Record every attempt and failure.
- [ ] Compute final inventory after text writes/downloads and rescan the frozen selected paths. Do not recompute a changed path set that expands because apply changed files. On post-write read/scan failure, retain attempted write results and produce exit 3 when any write succeeded.
- [ ] Centralize result derivation. The equivalent logic must drive both summary and exit:

```rust
fn apply_exit(changed: usize, operational_failure: bool, violations: bool)
    -> crate::ExitCode
{
    match (changed > 0, operational_failure, violations) {
        (true, true, _) => crate::ExitCode::Partial,
        (_, true, _) | (_, false, true) => crate::ExitCode::Violations,
        (_, false, false) => crate::ExitCode::Success,
    }
}
```

- [ ] Report schema v2 includes `version`, `command`, `exit_code`, snapshot/scope metadata, `summary` with `pass`, `complete`, `before_pass`, `partial`, counts, actual `files`, structured `diagnostics`, independent `writes`, and license inventory. Dry-run also includes `projected_pass`; its `summary.pass` means projected success and must be labeled as such in human output. `init` joins the same envelope in task 8. Keep JSON field ordering deterministic and diagnostics sorted by path/code. A committed replacement followed by durability failure is a changed write with a diagnostic and partial result. Additive writes that all succeed but leave declaration drift produce exit 1, not partial; test that case explicitly.
- [ ] Human output shows changed and failed destinations even if final file state is compliant. Use distinct words for planned, applied, failed, and blocked. Dry-run shows exact before/after text or a unified diff for each text write, plus license-text destinations and unavailable requirements. Ordinary successful apply shows compact counts and changed paths. Use existing `similar` only if promoted for a justified unified diff; the dependency-free alternative is clearly delimited before/after text, which satisfies this plan.
- [ ] Partial tests must assert status 3, failed path, failed bytes unchanged, successful bytes changed, and independent write statuses. Use deterministic failure (destination turns into a directory between plan/execution via the library-level executor test) rather than permissions alone; do not add production test flags. Keep the Unix permissions integration case as extra coverage.
- [ ] Hash/list the complete fixture tree before/after dry-run, including Git metadata and license texts; assert zero changes. Run real apply on a copy and compare actual destinations and replacement bytes with the preview. Re-run apply and assert an empty changed set and identical bytes.
- [ ] Run `cargo test --locked --test us2_partial --test us2_apply --test us1_snapshots --test us5_sidecars`. Review snapshot changes instead of blindly accepting them; commit `fix: report complete plans and consistent command outcomes`.

## Task 8 — Make init preserve observed licensing

**Finding:** F10. **Depends on:** tasks 2–7.

**Files:** modify `src/cli/init.rs`, `src/cli/mod.rs`, `src/config/mod.rs`, `src/report/mod.rs`; extend `tests/us5_reuse.rs` and the init portion of the CLI contract.

- [ ] Add a real round-trip regression: two MIT `.rs` files, one Apache-2.0 `.rs`, an exceptional extensionless file, a sidecar-covered binary, nested REUSE coverage, and one unlicensed file. Run init to a fresh output; load its config and resolve each observed path. Every known effective license must be equal before/after; the unknown file must remain uncovered. Do not use substring presence as the projection assertion.
- [ ] Generate exact-path rules for observed effective licensing first. A root filename must be emitted as `./filename` (then normalized as an ExactPath selector) so it cannot accidentally govern the same filename in other directories. Avoid a default unless every covered file is known and a verified exception set preserves all observations. Emit no guessed holder; use preserve by default.
- [ ] Compress only groups with identical licensing for **all** observed paths they would match. Uniform extension groups may become ext rules; heterogeneous groups keep exact exceptions. Directory/glob inference is unnecessary for this repair. Validate generated config and its projection against every observation using the real rule resolver before writing.
- [ ] Add `--force` to init; create-new is the default, and existing files/symlinks are not followed or truncated. Define output as `--output` if provided, otherwise `--config` if explicit, otherwise root/license.toml. Explain `--from-reuse` compatibility behavior in help; it is not a hidden alternate parsing mode. Use shared safe writes and v2 serialization, including paths with quotes/newlines.
- [ ] Include generated rules, unknown paths, and inference counts in the report. `--force` authorizes replacing only the chosen config and still checks expected bytes. An init failure changes no source or existing config.
- [ ] Run `cargo test --locked --test us5_reuse` plus init unit tests; commit `fix: preserve licensing exceptions when generating configuration`.

## Task 9 — Tighten config validation and CLI ergonomics

**Findings:** F13–F15. **Depends on:** tasks 2–8.

**Files:** modify `src/config/mod.rs`, `src/config/schema.rs`, `src/comment/mod.rs`, `src/rules/mod.rs`, `src/cli/mod.rs`, command modules, `src/main.rs`, contracts and README; extend `tests/us3_comment_style.rs`, `tests/us4_enforce.rs`, `tests/completions.rs`.

- [ ] Validate named comment aliases immediately with `comment::by_alias`; unknown alias is config error 2. A selected line-comment form requires a nonblank prefix; a block form requires both nonblank delimiters. The optional interior `block_line_prefix` may be empty or whitespace, as existing valid block styles require. Reject embedded CR/LF/NUL/control characters in tokens, empty add/replace copyright text, and copyright newlines that could inject additional tags. Invalid config causes no writes.
- [ ] Match comment associations by full normalized ExactPath before Filename before Extension, using the same path semantics as rules. Test `a/foo` and `b/foo` with distinct styles, unknown aliases, empty prefixes, inline block-only syntax, and duplicate selectors with differing copyright intent. Keep `license.toml` glob behavior documented separately from REUSE pattern grammar rather than accidentally changing all globs while fixing OOB matching.
- [ ] Resolve default config from the discovered root for every relevant command. Explicit config/file-list paths remain relative to cwd. All commands distinguish missing optional default, explicit missing config, malformed config, and read error. Keep existing flags/aliases/completions consistent; use clap conflicts where practical instead of delayed bespoke validation.
- [ ] Make `check --explain PATH` resolve the target and matching rules directly. Report winning rule index/selector or default, losing matching rules and specificity, exclusions, metadata sources, and current drift. Respect an explicit selected set; a path outside it/nonexistent returns a usage diagnostic, not success. Honor JSON format for explanations. Do not scan or write caches for the whole tree merely to explain one path.
- [ ] Route all JSON through serde. Move fetch progress and prompts to stderr. In JSON mode, disable implicit interactive prompting unless explicit `--allow-curl` was supplied, so automation never stalls; human TTY mode retains the documented y/N consent. A broken stdout pipe should terminate quietly instead of panicking; return other output errors normally.
- [ ] Add process tests that parse the entire stdout for check/apply/dry-run/lint/init/add-license success and error cases, including quote/newline paths and fake curl. Assert no progress prefix/suffix is present. Generate all supported completions after flag changes.
- [ ] Update examples: check policy vs lint conformance; staged index vs staged-path apply; root/cwd path bases; dirty nonrepo behavior; additive residual drift; safe init overwrite; actual dry-run output. Run `cargo test --locked --test us3_comment_style --test us4_enforce --test completions --test us5_add_license`; commit `fix: validate configuration and make CLI behavior predictable`.

## Task 10 — Delete inert caching and measure the corrected pipeline

**Findings:** F12, F16, F17, F19. **Depends on:** tasks 2–9.

**Files:** modify `src/engine.rs`, `src/walk/mod.rs`, `src/cli/check.rs`, `src/cli/apply.rs`, `src/cli/mod.rs`, `Cargo.toml`, `Cargo.lock`, `benches/scan.rs`, `tests/perf.rs`, `tests/determinism.rs`, README/contracts; delete `src/walk/cache.rs` and unused cache-only plumbing.

- [ ] Before removing cache, record release-binary wall time for the audit's full 10,000-file and one-file subset workloads on this machine. Record versions, number of selected files, actual byte count, repetitions, and whether filesystem caches are hot; keep errors/status checks in the timing harness.
- [ ] Remove `ScanCache`, content hashing performed only for it, configuration fingerprints, cache flushing, drift-label tuple fields, unused engine config-text storage, and the unused comment resolver inside scan. Retain `--cache`/`--no-cache` for one compatibility window as documented deprecated no-ops, with at most a stderr notice; they must not create files. Update tests to check stateless deterministic behavior rather than fake hit behavior.
- [ ] Remove `gix` now that task 2 owns root/snapshot discovery through system Git. Search every direct dependency use before removing cache-only/unused crates (including sha2 if no other use remains). Use `cargo tree --locked --edges normal` before/after to record the reduction; do not churn unrelated lock versions.
- [ ] Keep precompiled rule/OOB matchers and the existing shared marker automaton. Precompute canonical declared expressions once per configuration. Batch REUSE writes per document and Git object reads per snapshot as already required; do not parallelize writes. Avoid additional rule indexes or worker-pool tuning unless profiling finds a real regression.
- [ ] Repair Criterion: use a real initialized/staged fixture matching policy coverage; do not include a cache artifact; benchmark equivalent file sets and assert count/status equality outside timed loops. Label engine-only timing separately from end-to-end CLI timing. Sizes: 500, 2,000, 10,000 files. Workloads: tiny files, 8 KiB and 1 MiB text with late snippets, binary sidecars, nested metadata, dozens of rules, and one-file staged checks after a full run.
- [ ] Measure wall time and peak RSS for release binaries. Use at least ten repetitions for stable small-file medians; report the initial filesystem-cold-ish sample separately. Keep timing thresholds out of ordinary correctness tests except a generous hang guard. On a named reference runner, retain the original 10,000-small-file targets (<3 s initial scan, <1 s subsequent scan) and report failures honestly. No claim of parity with the old head-only scanner on large files is required; complete metadata scanning does more work.
- [ ] Re-run the long-LicenseRef scaling check after AST normalization; use increasing sizes with timeouts and ensure no quadratic suffix-copying. Profile only if the new whole-file reader or rule matching actually dominates.
- [ ] Run `cargo test --locked --test determinism --test perf` and `cargo bench --locked --bench scan -- --quick`; commit `perf: remove unused scan cache and redundant Git machinery`.

## Task 11 — Turn conformance and safety evidence into required CI

**Finding:** F17. **Depends on:** tasks 1–10.

**Files:** modify `.github/workflows/ci.yml`, `tests/common/mod.rs`, `tests/us5_reuse.rs`, `tests/determinism.rs`, relevant regression suites; add one `tests/reuse_differential.rs` to hold the compact cross-tool fixture matrix. Do not introduce a second fixture framework.

- [ ] Install the exact known-good comparator in a dedicated Linux CI job:

```yaml
- name: Install the pinned reference tool in an isolated environment
  run: |
    python3 -m venv "$RUNNER_TEMP/licet-reuse"
    "$RUNNER_TEMP/licet-reuse/bin/python" -m pip install 'reuse[charset-normalizer]==6.2.0'
    "$RUNNER_TEMP/licet-reuse/bin/reuse" --version
    echo "$RUNNER_TEMP/licet-reuse/bin" >> "$GITHUB_PATH"
- run: cargo test --locked --test reuse_differential --test us5_reuse
  env:
    LICET_REQUIRE_REUSE: '1'
```

Use the Linux runner's Python 3.10 or newer to create the isolated environment; verify its version before installation. This avoids adding another setup action. Include encoding dependencies so `--version` failure cannot silently disable conformance.

- [ ] Make comparator absence/failure fatal under `LICET_REQUIRE_REUSE=1`; locally, a missing comparator may print an explicit skip. Capture its version and specification version in the test output. Compare semantic fields rather than human output strings. Record mismatches for review rather than blessing Licet output automatically.
- [ ] Required differential fixtures: the five audited disagreements; nested precedence matrix; copyright-only/standard notices; sidecar-over-header; multiple expressions and exceptions; late/unclosed snippets; excluded/zero-byte/symlink/SPDX-document cases; ignored and untracked files; deprecated DEP5; valid custom-license text; missing/empty/unused license files. Compare covered paths, effective references, and pass/fail. When a reference tool limitation differs from normative text, document the exact fixture and normative decision; no silent exclusion.
- [ ] Keep existing fast unit/integration suites and add the table-driven adversarial cases from owning tasks. Full-byte preservation, outside sentinels, idempotent second apply, dry-run equality, exact partial outcomes, non-UTF-8 path identity, and JSON parsing are mandatory. Fuzzing infrastructure is optional; deterministic cases derived from actual bugs come first. A small seeded generator for newline/comment/expression variants is sufficient if extra breadth is needed.
- [ ] Replace the “offline” output-equality test with fake-curl call assertions. Preserve deterministic-report tests separately. In the dependency guard, first run `cargo tree --locked --edges normal --prefix none > dependency-tree.txt` successfully; only then check the denylist. A failed cargo command is a failed check. The denylist is dependency hygiene, not proof that subprocesses cannot access the network.
- [ ] Use `--locked` in all CI compilation/check/test commands. Run tests on Linux/macOS/Windows and MSRV checks with **all targets**. Keep Criterion performance runs in their own job; use `cargo test --lib --bins --tests` and `cargo test --doc` for ordinary correctness rather than accidentally running Criterion's harness during every all-target test job.
- [ ] Execute locally with the comparator required, then run full final checks from the last section. Commit `test: require REUSE conformance and adversarial safety checks`.

## Task 12 — Repair release gating and align the public contract

**Findings:** F18, F19. **Depends on:** tasks 10 and 11.

**Files:** modify `.github/workflows/release.yml`, `.github/workflows/prepare-release.yml`, `build.rs`, `mise.toml`, README, `CLAUDE.md`, `CHANGELOG.md` unreleased notes, affected spec/contracts. Do not bump/publish a package version without current remote-state review.

- [ ] Add job-local OIDC permissions to `publish-crate`:

```yaml
permissions:
  contents: read
  id-token: write
```

- [ ] Gate publishing on the exact tagged commit passing locked tests, conformance, and version validation. Prefer making the existing CI workflow reusable through `workflow_call`; have tag release depend on that job. Produce a draft GitHub release, build/upload every expected artifact and checksum, publish the crate only after verification, and mark the release public last. Finalization must explicitly accept either successful stable crate publication or intentionally skipped prerelease crate publication; ordinary `needs: publish-crate` skip propagation is insufficient. Require successful validation and all artifact jobs in either case. Validate this condition matrix: stable+all success → public; prerelease+all artifacts success+crate intentionally skipped → public prerelease; any validation/artifact/publish failure → draft; cancelled upstream job → draft. An error leaves a clearly incomplete draft, not a public success-shaped release.
- [ ] Add an operable manual tagged-ref release path or explicitly dispatch the release workflow from prepare-release when using GITHUB_TOKEN. Validate a supplied tag to an existing immutable commit and check manifest version before mutations. If retaining PAT-trigger-only behavior, fail before commit/tag/push when the credential is absent; do not retain documentation for a nonexistent manual path. Preserve recovery by rerunning only failed jobs; no automatic deletion/recreation of tags or published versions.
- [ ] Validate build assets: missing/empty directory or unreadable entry fails the build; all selected text files must be nonempty valid text with known IDs. Register `cargo:rerun-if-env-changed=LICET_SPDX_LIST_VERSION` while it exists. Separate reported SPDX recognition-list version from text-bundle provenance/version and bundle count. There are currently 14 texts; document that subset honestly instead of claiming the whole corpus is embedded.
- [ ] Align mise with the stable/MSRV policy and remove the unrelated installed Licet pin from the development path or replace it with a `cargo run --` task. Do not require nightly-only behavior. Keep a reproducible pinned comparator for conformance and document its encoding extra.
- [ ] Update CLI/config/report contracts and examples to the final implementation, including intentional compatibility changes listed earlier. Remove obsolete cache promises, gix subset claims, blanket OOB override wording, and unsupported “always preserves” claims where explicit copyright replace is available. Keep a short migration note for v2 JSON, staged snapshots, additive residual drift, symlink exclusions, and init overwrite protection.
- [ ] Validate workflow syntax and job dependency/permission structure locally; run `cargo package --locked` and inspect packaged assets in an actual repository, with release publication disabled. Document any missing live registry/environment verification. Commit `ci: gate releases on verified artifacts and conformance`.

## Final acceptance and handoff

- [ ] Every F01–F19 finding maps to a completed task and at least one concrete check or documented source/workflow validation. Do not close a reproduced behavioral finding using only a comment change.
- [ ] Run the final suite with test Git signing/hooks isolated and the reference tool required:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
LICET_REQUIRE_REUSE=1 cargo test --locked --lib --bins --tests
cargo test --locked --doc
cargo +1.89 check --locked --all-targets
cargo bench --locked --bench scan -- --quick
cargo package --locked
```

- [ ] Re-run the five audited conformance counterexamples and the source-corruption/symlink/permission/curl cases against the final release binary, independently of unit-test helpers.
- [ ] Confirm dry-run performs no writes and predicts actual destinations/text; second apply is byte-identical and records no applied writes; JSON pass/exit/partial fields agree; no missing copyright, invalid metadata, or unreadable selected dependency can produce a false REUSE pass.
- [ ] Have a fresh reviewer trace selection → snapshot → effective metadata → inventory → edit plan → write → final report for staged files, nested overrides, binary sidecars, and partial failures. Review tests against supplied REUSE text and reference results, not against agent consensus.
- [ ] Report final source commit/diff, executed checks and actual counts, platform coverage, reference-tool version, performance measurements with workload limits, and remaining issues. Do not state that release delivery is tested live unless it was separately authorized and verified. Leave a reviewable implementation and updated documentation; no tag or registry publication is part of this plan.
