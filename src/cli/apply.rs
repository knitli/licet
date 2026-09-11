//! `apply` — reconcile to declared intent (FR-006..FR-009, FR-021, FR-024; US2).

use std::path::{Path, PathBuf};

use super::{ApplyArgs, Format};
use crate::comment::{CommentResolver, render_sidecar, render_sidecar_multi};
use crate::config::LicensingConfiguration;
use crate::detect;
use crate::domain::{ActualSource, ChangeMode, DriftClass, FileChange, NonAnnotatableStrategy};
use crate::engine::Engine;
use crate::error::{ExitCode, LicetError, Result};
use crate::reconcile::copyrights_to_write;
use crate::report::render::render_human;
use crate::report::{Diagnostic, Report};
use crate::reuse::oob::{self, OutOfBand};
use crate::reuse::{self, inventory};
use crate::walk::{self, Purpose, Selection, Snapshot, discover_root};

pub fn run(args: ApplyArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let (root, _) = discover_root(&cwd)?;
    let selection = args.common.selection()?;
    let config_arg = args.common.config_arg(&cwd, &root);
    let prep = walk::prepare(&cwd, &config_arg, &selection, Purpose::Apply, false)?;
    let config = LicensingConfiguration::from_toml(&prep.config_text)?;

    // `apply --staged` evaluates the staged *path set* but edits working-tree
    // files; the index is never staged into. Say so explicitly.
    if matches!(selection, Selection::Staged) {
        eprintln!(
            "note: apply --staged evaluates staged paths but edits working-tree files; \
             edits are not staged"
        );
    }

    // Dirty-tree guard (FR-024, SC-010) — skip on dry-run. A Git launch/status
    // failure never means "clean", and outside a repository there is no Git
    // undo guarantee, so both require explicit `--allow-dirty`.
    if !args.dry_run && !args.allow_dirty {
        match tree_state(&root)? {
            TreeState::Clean => {}
            TreeState::Dirty => {
                return Err(LicetError::Config(
                    "working tree has uncommitted changes; commit/stash first or pass \
                     --allow-dirty"
                        .to_string(),
                ));
            }
            TreeState::NoGitGuarantee => {
                return Err(LicetError::Config(
                    "not a git repository: no undo guarantee, pass --allow-dirty to proceed"
                        .to_string(),
                ));
            }
        }
    }

    super::check::warn_deprecated_cache_flags(&args.common);
    let snapshot_label = prep.snapshot.source().as_str().to_string();
    let engine = Engine::new(root.clone(), &config, prep.snapshot, true);
    let scan = engine.scan(&prep.paths)?;
    // Freeze the evaluated path set now: post-apply verification re-reads these
    // same paths and must not recompute a set that apply itself changed.
    let frozen = prep.paths.clone();

    let resolver = CommentResolver::new(&config);
    let oob = OutOfBand::load_snapshot(&engine.snapshot)?;
    let mode = if args.additive {
        ChangeMode::Additive
    } else {
        ChangeMode::Destructive
    };
    // CLI flag overrides the `[output] non_annotatable` config (default sidecar).
    let strategy = args
        .non_annotatable
        .map(NonAnnotatableStrategy::from)
        .unwrap_or(config.non_annotatable);

    use crate::domain::{ExecutedWrite, PlannedWrite, WriteKind, WriteStatus};

    /// One planned single-file write plus the bookkeeping to file its outcome.
    struct FileWriteOp {
        change_idx: usize,
        /// File path used in write-failure diagnostics (the covered file, not
        /// the sidecar document actually written for sidecar coverage).
        warn_path: String,
        write: PlannedWrite,
    }
    /// One planned annotation request plus the bookkeeping to file its outcome.
    struct TomlReq {
        change_idx: usize,
        warn_path: String,
        rel: String,
        license: String,
        copyrights: Vec<String>,
    }

    let mut changes: Vec<FileChange> = Vec::new();
    let mut file_ops: Vec<FileWriteOp> = Vec::new();
    let mut toml_reqs: Vec<TomlReq> = Vec::new();
    // Diagnostics produced by the apply pass itself (contradictions, write failures, …).
    // Detection diagnostics are taken from the final re-scan so they reflect on-disk state
    // (a pre-write `encoding_skipped`, say, must not linger after a sidecar fixes it).
    let mut warnings: Vec<Diagnostic> = Vec::new();
    // Operational failures: the tool itself could not do its job (unreadable
    // inputs, refused plans, failed writes). Remaining declaration drift and
    // unfixable entries are violations, not operational failures.
    let mut operational_failure = false;
    // Plan entries that leave drift by design (unfixable, manual fixup):
    // dry-run projects them as gate failures.
    let mut blocked_by_design = false;
    // Gate state before apply ran (summary.before_pass).
    let before_pass =
        crate::report::counts_pass(&crate::report::count_drift(&scan.states, &scan.warnings));

    // Files the snapshot could not be read for are gate failures, not edit targets.
    let unreadable: std::collections::HashSet<&str> = scan
        .warnings
        .iter()
        .filter(|w| w.code == "read_error")
        .filter_map(|w| w.path.as_deref())
        .collect();

    for state in &scan.states {
        // Writable: a license header can be established/fixed, and copyright
        // intent applies even when the license already matches (FR-007).
        // `Unreadable` (a binary asset with no sidecar) is now writable too —
        // it is coverable out-of-band.
        let writable = matches!(
            state.drift,
            DriftClass::MissingHeader
                | DriftClass::WrongLicense { .. }
                | DriftClass::CopyrightMismatch { .. }
                | DriftClass::Unreadable
        );
        if !writable {
            continue;
        }
        let rel_str = state.path.to_string_lossy().replace('\\', "/");
        if unreadable.contains(rel_str.as_str()) {
            continue;
        }
        let intent = match &state.declared_intent {
            Some(i) => i,
            None => continue,
        };
        let abs = root.join(&state.path);

        // Sidecar presence comes from the evaluated snapshot, so a staged sidecar
        // differing from the working copy is honored.
        let sidecar_rel = detect::sidecar_path(&state.path);
        let has_sidecar = match engine.snapshot.read(&sidecar_rel) {
            Ok(b) => b.is_some(),
            Err(e) => {
                operational_failure = true;
                warnings.push(Diagnostic {
                    code: "partial_apply".to_string(),
                    path: Some(rel_str.clone()),
                    message: format!("cannot read sidecar for edit: {e}"),
                });
                continue;
            }
        };
        let is_binary = matches!(state.drift, DriftClass::Unreadable);
        let in_file = !has_sidecar && !is_binary;

        // Annotatable text (a resolvable comment style, no sidecar, readable) gets an
        // in-file header — unless out-of-band metadata already governs the file, in
        // which case the fix belongs to the document: an in-file edit would be
        // suppressed (override), shadowed, or a second record for the same file
        // (FR-003a). Provenance picks the destination before any comment syntax.
        // Everything else is covered out-of-band so the asset is never
        // byte-edited (FR-015).
        let oob_governs = matches!(
            state.actual.detected_source,
            Some(ActualSource::ReuseToml | ActualSource::Dep5)
        ) || state
            .actual
            .out_of_band
            .as_ref()
            .is_some_and(|e| e.suppresses_file);
        if let Some(style) = resolver
            .resolve(&state.path)
            .filter(|_| in_file && !oob_governs)
        {
            // Re-read full content for an accurate edit (engine only read the head).
            // The exact bytes double as the expected-content guard for replacement.
            let old_bytes = match std::fs::read(&abs) {
                Ok(b) => b,
                Err(e) => {
                    operational_failure = true;
                    warnings.push(Diagnostic {
                        code: "partial_apply".to_string(),
                        path: Some(rel_str.clone()),
                        message: format!("cannot read for edit: {e}"),
                    });
                    continue;
                }
            };
            let content = match String::from_utf8(old_bytes.clone()) {
                Ok(c) => c,
                Err(_) => {
                    operational_failure = true;
                    warnings.push(Diagnostic {
                        code: "partial_apply".to_string(),
                        path: Some(rel_str.clone()),
                        message: "file is not valid UTF-8; cannot edit in place".to_string(),
                    });
                    continue;
                }
            };
            // Re-detect against full content so byte ranges are correct for the whole file.
            let actual = detect::detect(&state.path, content.as_bytes(), None, &oob);

            let plan = match crate::reconcile::plan_file(
                &content,
                &actual,
                intent,
                &style,
                mode,
                args.target_header,
            ) {
                Ok(plan) => plan,
                // A refused plan writes nothing: `unfixable` needs a human
                // (the remaining drift is a violation, not a tool failure),
                // an inconsistent target is an operational failure. Either way
                // the file is recorded as not applied (FR-007).
                Err(e) => {
                    let unfixable = matches!(e.kind, crate::reconcile::PlanErrorKind::Unfixable);
                    if unfixable {
                        blocked_by_design = true;
                    } else {
                        operational_failure = true;
                    }
                    warnings.push(Diagnostic {
                        code: if unfixable {
                            "unfixable".to_string()
                        } else {
                            "partial_apply".to_string()
                        },
                        path: Some(rel_str.clone()),
                        message: e.message,
                    });
                    changes.push(FileChange {
                        path: state.path.clone(),
                        mode,
                        target_header: args.target_header,
                        wrote_header: false,
                        preserved_copyrights: 0,
                        applied: false,
                    });
                    continue;
                }
            };

            if plan.contradiction {
                warnings.push(Diagnostic {
                    code: "contradiction".to_string(),
                    path: Some(rel_str.clone()),
                    message: "additive apply left two contradictory licenses".to_string(),
                });
            }

            let change_idx = changes.len();
            changes.push(FileChange {
                path: state.path.clone(),
                mode: plan.mode,
                target_header: plan.target_header,
                wrote_header: plan.wrote_header,
                preserved_copyrights: plan.preserved_copyrights,
                applied: false,
            });
            // The write itself waits for the execute phase below; a no-op
            // plan (already handled) simply records `applied: false`.
            if let Some(new_content) = plan.new_content {
                file_ops.push(FileWriteOp {
                    change_idx,
                    warn_path: rel_str.clone(),
                    write: PlannedWrite {
                        path: state.path.clone(),
                        kind: WriteKind::Source,
                        before: Some(old_bytes),
                        after: Some(new_content.into_bytes()),
                        affected_files: vec![state.path.clone()],
                    },
                });
            }
            continue;
        }

        // --- Out-of-band coverage (non-annotatable / sidecar-managed file) ---

        // Legacy `.reuse/dep5` is read for detection but never rewritten in place (its
        // Debian-paragraph format is deprecated in REUSE 3.x); flag it for manual fixup.
        if matches!(state.actual.detected_source, Some(ActualSource::Dep5)) {
            // Manual fixup: the remaining drift is a violation for the gate,
            // not an operational failure of this run.
            blocked_by_design = true;
            warnings.push(Diagnostic {
                code: "source_override".to_string(),
                path: Some(rel_str.clone()),
                message: "covered by a .reuse/dep5 entry with a conflicting license; \
                          update that entry manually"
                    .to_string(),
            });
            continue;
        }

        let copyrights =
            copyrights_to_write(&state.actual.detected_copyrights, &intent.copyright_policy);
        let preserved = state.actual.detected_copyrights.len();

        // Fix the source that currently *wins* for this file (per precedence), so the fix
        // takes effect regardless of `closest`/`override`. A REUSE.toml annotation is
        // corrected where it lives; in-file headers and sidecars (and a genuinely uncovered
        // file under `--non-annotatable reuse-toml`) are covered file-level via a sidecar
        // unless the configured strategy says otherwise.
        // A suppressing (`override`) entry routes to its document even when
        // it carries no license: any file-level record would be ignored, so a
        // sidecar would be an ineffective second record (FR-003a).
        let via_reuse_toml = match state.actual.detected_source {
            Some(ActualSource::ReuseToml) => true,
            Some(ActualSource::Sidecar | ActualSource::Header | ActualSource::Dep5) => false,
            None => {
                matches!(strategy, NonAnnotatableStrategy::ReuseToml)
                    || state
                        .actual
                        .out_of_band
                        .as_ref()
                        .is_some_and(|e| e.suppresses_file)
            }
        };

        // Never emit a new REUSE document alongside an existing `.reuse/dep5`:
        // the two are mutually exclusive, so a REUSE.toml fix next to dep5 is
        // an unfixable plan entry requiring manual migration, not a write.
        if via_reuse_toml && oob.has_dep5() {
            blocked_by_design = true;
            warnings.push(Diagnostic {
                code: "unfixable".to_string(),
                path: Some(rel_str.clone()),
                message: "a REUSE.toml annotation cannot be added while .reuse/dep5 exists \
                          (the two are mutually exclusive); migrate with `reuse convert-dep5` \
                          and re-run"
                    .to_string(),
            });
            continue;
        }

        // Report path: the asset for REUSE.toml coverage, else the sidecar.
        let change_path = if via_reuse_toml {
            state.path.clone()
        } else {
            detect::sidecar_path(&state.path)
        };

        let change_idx = changes.len();
        changes.push(FileChange {
            path: change_path,
            mode: ChangeMode::Destructive,
            target_header: None,
            wrote_header: true,
            preserved_copyrights: preserved,
            applied: false,
        });
        if via_reuse_toml {
            toml_reqs.push(TomlReq {
                change_idx,
                warn_path: rel_str.clone(),
                rel: rel_str.clone(),
                license: intent.license_expression.clone(),
                copyrights,
            });
        } else {
            // Additive sidecar coverage preserves the old identifiers and
            // notices instead of replacing them (FR-006).
            let body = if mode == ChangeMode::Additive {
                let mut licenses: Vec<String> = state
                    .actual
                    .headers
                    .iter()
                    .flat_map(|h| h.license_ids.clone())
                    .collect();
                if !licenses
                    .iter()
                    .any(|l| crate::spdx::expressions_equal(l, &intent.license_expression))
                {
                    licenses.push(intent.license_expression.clone());
                }
                render_sidecar_multi(&licenses, &copyrights)
            } else {
                render_sidecar(&intent.license_expression, &copyrights)
            };
            // Read the expected bytes now, at plan time: dry-run previews the
            // same record the executor will guard on.
            let sidecar_rel = detect::sidecar_path(&state.path);
            let before = match reuse::read_expected_for_write(&root.join(&sidecar_rel)) {
                Ok(expected) => expected,
                Err(e) => {
                    operational_failure = true;
                    warnings.push(Diagnostic {
                        code: "partial_apply".to_string(),
                        path: Some(rel_str.clone()),
                        message: format!("cannot read sidecar for edit: {e}"),
                    });
                    continue;
                }
            };
            file_ops.push(FileWriteOp {
                change_idx,
                warn_path: rel_str.clone(),
                write: PlannedWrite {
                    path: sidecar_rel,
                    kind: WriteKind::Sidecar,
                    before,
                    after: Some(body.into_bytes()),
                    affected_files: vec![state.path.clone()],
                },
            });
        }
    }

    // Preparation also covers the license-text inventory: missing bundled
    // texts are planned LicenseText writes, missing custom/fetchable texts
    // are explicit blockers. Validation failures here prevent all writes.
    let (text_writes, text_blocked) =
        match inventory::plan_text_writes(&root, &projected_reference_ids(&scan.states, mode)) {
            Ok(plan) => plan,
            Err(e) => {
                operational_failure = true;
                warnings.push(Diagnostic {
                    code: "missing_license_text".to_string(),
                    path: None,
                    message: format!("cannot plan license texts: {e}"),
                });
                (Vec::new(), Vec::new())
            }
        };
    // Group annotation requests by destination document for preview records
    // and failure filing (the batch itself groups identically inside).
    let mut toml_groups: std::collections::BTreeMap<PathBuf, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (k, req) in toml_reqs.iter().enumerate() {
        toml_groups
            .entry(oob::annotation_destination(&oob, &req.rel).doc_rel)
            .or_default()
            .push(k);
    }

    // Every write record, in one place: dry-run previews them all as Planned,
    // real execution replaces them with outcomes below.
    let mut executed: Vec<ExecutedWrite> = Vec::new();
    // Applied-write count for apply_exit (includes durability-committed ones).
    let mut changed: usize = 0;
    // Whether the license-text inventory blocks the gate independent of states.
    let texts_ok = text_blocked.is_empty();
    for b in &text_blocked {
        executed.push(ExecutedWrite::blocked(
            PlannedWrite {
                path: PathBuf::from(format!("LICENSES/{}.txt", b.id)),
                kind: WriteKind::LicenseText,
                before: None,
                after: None,
                affected_files: Vec::new(),
            },
            b.message.clone(),
        ));
        warnings.push(Diagnostic {
            code: "missing_license_text".to_string(),
            path: None,
            message: b.message.clone(),
        });
    }

    if args.dry_run {
        // Nothing is executed or fetched: the same records preview as
        // Planned, and projected_pass predicts the gate.
        for op in &file_ops {
            executed.push(ExecutedWrite::planned(op.write.clone()));
        }
        for (doc_rel, members) in &toml_groups {
            let before = match std::fs::read(root.join(doc_rel)) {
                Ok(b) => Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    operational_failure = true;
                    warnings.push(Diagnostic {
                        code: "partial_apply".to_string(),
                        path: Some(doc_rel.to_string_lossy().replace('\\', "/")),
                        message: format!("cannot preview metadata document: {e}"),
                    });
                    continue;
                }
            };
            executed.push(ExecutedWrite::planned(PlannedWrite {
                path: doc_rel.clone(),
                kind: WriteKind::ReuseToml,
                before,
                after: None,
                affected_files: members
                    .iter()
                    .map(|&k| PathBuf::from(&toml_reqs[k].rel))
                    .collect(),
            }));
        }
        for w in &text_writes {
            executed.push(ExecutedWrite::planned(w.clone()));
        }
    } else {
        // Execution: single-file writes via the library executor in
        // deterministic destination order, then per-document metadata
        // batches, then planned license texts. Reads all settled above.
        let mut single: Vec<PlannedWrite> = Vec::new();
        // (kind, path) -> (change index for file writes, diagnostic path).
        let mut filing: std::collections::HashMap<(String, PathBuf), (Option<usize>, String)> =
            std::collections::HashMap::new();
        for op in &file_ops {
            filing.insert(
                (op.write.kind.as_str().to_string(), op.write.path.clone()),
                (Some(op.change_idx), op.warn_path.clone()),
            );
            single.push(op.write.clone());
        }
        for w in &text_writes {
            filing.insert(
                (w.kind.as_str().to_string(), w.path.clone()),
                (None, w.path.to_string_lossy().replace('\\', "/")),
            );
            single.push(w.clone());
        }
        for outcome in crate::exec::execute_writes(&root, &single) {
            let key = (
                outcome.write.kind.as_str().to_string(),
                outcome.write.path.clone(),
            );
            let (change_idx, warn_path) =
                filing.get(&key).cloned().unwrap_or((None, String::new()));
            match outcome.status {
                WriteStatus::Applied => {
                    changed += 1;
                    if let Some(ci) = change_idx {
                        changes[ci].applied = true;
                    }
                }
                WriteStatus::Failed => {
                    if outcome.replacement_completed {
                        // Committed but unfsynced: a change with a diagnostic.
                        changed += 1;
                        if let Some(ci) = change_idx {
                            changes[ci].applied = true;
                        }
                    } else {
                        operational_failure = true;
                    }
                    warnings.push(Diagnostic {
                        code: "partial_apply".to_string(),
                        path: Some(outcome.write.path.to_string_lossy().replace('\\', "/")),
                        message: format!(
                            "write failed for {}: {}",
                            if warn_path.is_empty() {
                                outcome.write.path.to_string_lossy().replace('\\', "/")
                            } else {
                                warn_path
                            },
                            outcome.message.as_deref().unwrap_or("unknown error")
                        ),
                    });
                }
                WriteStatus::Blocked | WriteStatus::Planned | WriteStatus::Unchanged => {}
            }
            executed.push(outcome);
        }

        if !toml_reqs.is_empty() {
            let requests: Vec<oob::AnnotationRequest<'_>> = toml_reqs
                .iter()
                .map(|req| oob::AnnotationRequest {
                    rel_path: &req.rel,
                    license: &req.license,
                    copyrights: &req.copyrights,
                })
                .collect();
            match oob::write_annotations(&root, &requests, &oob) {
                Ok(outcome) => {
                    for (j, req) in toml_reqs.iter().enumerate() {
                        if outcome.per_request[j].modified() {
                            changed += 1;
                            changes[req.change_idx].applied = true;
                        }
                    }
                    for patch in &outcome.patches {
                        executed.push(ExecutedWrite {
                            write: PlannedWrite {
                                path: patch.doc_rel.clone(),
                                kind: WriteKind::ReuseToml,
                                before: patch.before.clone(),
                                after: Some(patch.after.clone()),
                                affected_files: patch
                                    .requests
                                    .iter()
                                    .map(|&k| PathBuf::from(&toml_reqs[k].rel))
                                    .collect(),
                            },
                            status: WriteStatus::Applied,
                            message: None,
                            replacement_completed: true,
                        });
                    }
                }
                // One shared batch: a failure fails every member request, and
                // each destination document files one Failed record.
                Err(e) => {
                    operational_failure = true;
                    for req in &toml_reqs {
                        warnings.push(Diagnostic {
                            code: "partial_apply".to_string(),
                            path: Some(req.warn_path.clone()),
                            message: format!("write failed: {e}"),
                        });
                    }
                    for (doc_rel, members) in &toml_groups {
                        executed.push(ExecutedWrite {
                            write: PlannedWrite {
                                path: doc_rel.clone(),
                                kind: WriteKind::ReuseToml,
                                before: None,
                                after: None,
                                affected_files: members
                                    .iter()
                                    .map(|&k| PathBuf::from(&toml_reqs[k].rel))
                                    .collect(),
                            },
                            status: WriteStatus::Failed,
                            message: Some(e.to_string()),
                            replacement_completed: false,
                        });
                    }
                }
            }
        }
    }

    // Re-scan post-write so the report and exit code reflect the actual on-disk
    // result. Verification always reads the working tree over the frozen path
    // set — never a recomputed subset, and never the stale index after
    // `apply --staged` edited the worktree. A verification failure retains
    // the attempted write results instead of propagating: exit 3 when any
    // write succeeded, since the outcome is unverified but mutated.
    let mut complete = true;
    let (final_states, scan_warnings, violations) = if args.dry_run {
        // Dry-run predicts: planning clean, nothing blocked by design, and
        // the text inventory complete. (before_pass is reported separately;
        // a failing gate with a complete plan still projects success.)
        let projected_pass = !operational_failure && !blocked_by_design && texts_ok;
        (scan.states, scan.warnings.clone(), !projected_pass)
    } else {
        let verify_engine = Engine::new(
            root.clone(),
            &config,
            Snapshot::Worktree { root: root.clone() },
            true,
        );
        match verify_engine.scan(&frozen) {
            Ok(r) => {
                let bad = !crate::report::counts_pass(&crate::report::count_drift(
                    &r.states,
                    &r.warnings,
                )) || !texts_ok;
                (r.states, r.warnings, bad)
            }
            Err(e) => {
                operational_failure = true;
                complete = false;
                warnings.push(Diagnostic {
                    code: "partial_apply".to_string(),
                    path: None,
                    message: format!("post-write verification scan failed: {e}"),
                });
                // Unverifiable: fail closed, but keep every attempted write.
                (scan.states, scan.warnings.clone(), true)
            }
        }
    };
    // Final report diagnostics: post-write detection diagnostics, then
    // apply-pass diagnostics.
    let mut all_warnings = scan_warnings;
    all_warnings.append(&mut warnings);
    let warnings = all_warnings;

    // One observation triple drives both the summary and the process exit:
    // writes that all succeed but leave drift are violations (exit 1), not
    // partial — partial means the tool itself failed partway.
    let exit = crate::error::apply_exit(changed, operational_failure, violations);
    let partial = matches!(exit, ExitCode::Partial);

    let mut report = Report::build(
        "apply",
        &final_states,
        &changes,
        warnings,
        Some(partial),
        crate::report::ReportMeta {
            snapshot: snapshot_label,
            writes: executed,
            projected_pass: args.dry_run.then_some(!violations),
            before_pass: Some(before_pass),
            complete,
            extra_violations: !texts_ok,
        },
    );
    report.exit_code = Some(exit.code());

    let rendered = match args.common.format {
        Format::Json => format!("{}\n", report.to_json()),
        Format::Human => render_human(&report),
    };
    super::emit_stdout(&rendered)?;
    Ok(exit)
}

/// License identifiers the post-apply selected state will need texts for:
/// intents of files apply touches (plus their current licenses in additive
/// mode, which are preserved), and current effective licenses — including
/// snippets — of files it leaves alone. Excluded files are out of scope;
/// stale replaced licenses and unmatched rules never enter.
fn projected_reference_ids(
    states: &[crate::domain::FileLicensingState],
    mode: ChangeMode,
) -> std::collections::BTreeSet<String> {
    use crate::engine::collect_ids;
    let mut out = std::collections::BTreeSet::new();
    for s in states {
        if matches!(s.drift, DriftClass::Excluded) {
            continue;
        }
        let will_touch = matches!(
            s.drift,
            DriftClass::MissingHeader
                | DriftClass::WrongLicense { .. }
                | DriftClass::CopyrightMismatch { .. }
                | DriftClass::Unreadable
        ) && s.declared_intent.is_some();
        if will_touch {
            collect_ids(
                &s.declared_intent
                    .as_ref()
                    .expect("intent checked")
                    .license_expression,
                &mut out,
            );
            if mode == ChangeMode::Additive {
                for c in crate::detect::candidate_licenses(&s.actual) {
                    collect_ids(&c, &mut out);
                }
            }
        } else {
            for c in crate::detect::candidate_licenses(&s.actual) {
                collect_ids(&c, &mut out);
            }
        }
        for snippet in &s.actual.snippet_licenses {
            collect_ids(snippet, &mut out);
        }
    }
    out
}

/// True when Uncovered/Unreadable files remain (apply cannot fix by writing — exit 1).
/// Working-tree state for the dirty-tree guard. A Git launch/status failure is an
/// error (never "clean"), and a non-repository has no undo guarantee.
enum TreeState {
    Clean,
    Dirty,
    NoGitGuarantee,
}

fn tree_state(root: &Path) -> Result<TreeState> {
    use crate::walk::git::{RepoDisposition, discover_repo, git_output};
    use std::ffi::OsStr;
    match discover_repo(root)? {
        RepoDisposition::NonRepo => Ok(TreeState::NoGitGuarantee),
        RepoDisposition::Bare => Err(LicetError::Config(
            "bare git repository has no working tree to modify".to_string(),
        )),
        RepoDisposition::Repo(repo) => {
            let out = git_output(
                &repo.root,
                &[OsStr::new("status"), OsStr::new("--porcelain")],
            )?;
            Ok(if out.is_empty() {
                TreeState::Clean
            } else {
                TreeState::Dirty
            })
        }
    }
}
