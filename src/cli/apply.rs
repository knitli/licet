//! `apply` — reconcile to declared intent (FR-006..FR-009, FR-021, FR-024; US2).

use std::path::Path;

use super::{ApplyArgs, Format};
use crate::comment::{CommentResolver, render_sidecar};
use crate::config::LicensingConfiguration;
use crate::detect;
use crate::domain::{ActualSource, ChangeMode, DriftClass, FileChange, NonAnnotatableStrategy};
use crate::engine::Engine;
use crate::error::{ExitCode, LicetError, Result};
use crate::reconcile::copyrights_to_write;
use crate::report::render::render_human;
use crate::report::{Report, Warning};
use crate::reuse::oob::{self, OutOfBand};
use crate::reuse::{self, inventory};
use crate::walk::{Selection, discover_root};

pub fn run(args: ApplyArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = discover_root(&cwd);
    let config_text = std::fs::read_to_string(&args.common.config)
        .map_err(|e| LicetError::Config(format!("cannot read config: {e}")))?;
    let config = LicensingConfiguration::from_toml(&config_text)?;
    let selection = args.common.selection()?;

    // Dirty-tree guard (FR-024, SC-010) — skip on dry-run.
    if !args.dry_run && !args.allow_dirty && is_dirty(&root) {
        return Err(LicetError::Config(
            "working tree has uncommitted changes; commit/stash first or pass --allow-dirty"
                .to_string(),
        ));
    }

    let mut cache = super::check::open_cache(&args.common, &root, &config_text);
    let engine = Engine::new(root.clone(), &config, &config_text);
    let scan = engine.scan(&selection, &mut cache)?;

    let resolver = CommentResolver::new(&config);
    let oob = OutOfBand::load(&root);
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

    let mut changes: Vec<FileChange> = Vec::new();
    // Warnings produced by the apply pass itself (contradictions, write failures, …).
    // Detection warnings are taken from the final re-scan so they reflect on-disk state
    // (a pre-write `encoding_skipped`, say, must not linger after a sidecar fixes it).
    let mut warnings: Vec<Warning> = Vec::new();
    let mut any_failure = false;
    let mut any_success = false;

    for state in &scan.states {
        // Writable: a license header can be established/fixed. `Unreadable` (a binary
        // asset with no sidecar) is now writable too — it is coverable out-of-band.
        let writable = matches!(
            state.drift,
            DriftClass::MissingHeader | DriftClass::WrongLicense { .. } | DriftClass::Unreadable
        );
        if !writable {
            continue;
        }
        let intent = match &state.declared_intent {
            Some(i) => i,
            None => continue,
        };
        let abs = root.join(&state.path);
        let rel_str = state.path.to_string_lossy().replace('\\', "/");

        let has_sidecar = detect::sidecar_path(&abs).exists();
        let is_binary = matches!(state.drift, DriftClass::Unreadable);
        let in_file = !has_sidecar && !is_binary;

        // Annotatable text (a resolvable comment style, no sidecar, readable) gets an
        // in-file header; everything else is covered out-of-band so the asset is never
        // byte-edited (FR-015).
        if let Some(style) = resolver.resolve(&state.path).filter(|_| in_file) {
            // Re-read full content for an accurate edit (engine only read the head).
            let content = match std::fs::read_to_string(&abs) {
                Ok(c) => c,
                Err(_) => {
                    any_failure = true;
                    continue;
                }
            };
            // Re-detect against full content so byte ranges are correct for the whole file.
            let actual = detect::detect(&state.path, content.as_bytes(), None, &oob);

            let plan = crate::reconcile::plan_file(
                &content,
                &actual,
                intent,
                &style,
                mode,
                args.target_header,
            );

            if plan.contradiction {
                warnings.push(Warning {
                    kind: "contradiction".to_string(),
                    path: Some(rel_str.clone()),
                    message: "additive apply left two contradictory licenses".to_string(),
                });
            }

            let applied = if args.dry_run {
                false
            } else if let Some(new_content) = &plan.new_content {
                match reuse::atomic_write(&abs, new_content) {
                    Ok(()) => {
                        any_success = true;
                        true
                    }
                    Err(e) => {
                        any_failure = true;
                        warnings.push(Warning {
                            kind: "partial_apply".to_string(),
                            path: Some(rel_str.clone()),
                            message: format!("write failed: {e}"),
                        });
                        false
                    }
                }
            } else {
                false
            };

            changes.push(FileChange {
                path: state.path.clone(),
                mode: plan.mode,
                target_header: plan.target_header,
                wrote_header: plan.wrote_header,
                preserved_copyrights: plan.preserved_copyrights,
                applied,
            });
            continue;
        }

        // --- Out-of-band coverage (non-annotatable / sidecar-managed file) ---

        // Legacy `.reuse/dep5` is read for detection but never rewritten in place (its
        // Debian-paragraph format is deprecated in REUSE 3.x); flag it for manual fixup.
        if matches!(state.actual.detected_source, Some(ActualSource::Dep5)) {
            warnings.push(Warning {
                kind: "source_override".to_string(),
                path: Some(rel_str.clone()),
                message: "covered by a .reuse/dep5 entry with a conflicting license; \
                          update that entry manually"
                    .to_string(),
            });
            any_failure = true;
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
        let via_reuse_toml = match state.actual.detected_source {
            Some(ActualSource::ReuseToml) => true,
            Some(ActualSource::Sidecar | ActualSource::Header | ActualSource::Dep5) => false,
            None => matches!(strategy, NonAnnotatableStrategy::ReuseToml),
        };

        // Report path: the asset for REUSE.toml coverage, else the sidecar.
        let change_path = if via_reuse_toml {
            state.path.clone()
        } else {
            detect::sidecar_path(&state.path)
        };

        let applied = if args.dry_run {
            false
        } else {
            let result = if via_reuse_toml {
                oob::write_annotation(&root, &rel_str, &intent.license_expression, &copyrights)
                    .map(|w| w.modified())
            } else {
                let body = render_sidecar(&intent.license_expression, &copyrights);
                reuse::atomic_write(&detect::sidecar_path(&abs), &body).map(|()| true)
            };
            match result {
                Ok(changed) => {
                    if changed {
                        any_success = true;
                    }
                    changed
                }
                Err(e) => {
                    any_failure = true;
                    warnings.push(Warning {
                        kind: "partial_apply".to_string(),
                        path: Some(rel_str.clone()),
                        message: format!("write failed: {e}"),
                    });
                    false
                }
            }
        };

        changes.push(FileChange {
            path: change_path,
            mode: ChangeMode::Destructive,
            target_header: None,
            wrote_header: true,
            preserved_copyrights: preserved,
            applied,
        });
    }

    // Re-scan post-write so the report, exit code, and license-text inventory reflect the
    // actual on-disk result (dry-run keeps the pre-apply states, since nothing was written).
    let partial = any_failure && any_success;
    let (final_states, scan_warnings, post_referenced) = if args.dry_run {
        (scan.states, scan.warnings.clone(), scan.referenced_ids)
    } else {
        let mut fresh = super::check::open_cache(&args.common, &root, &config_text);
        let r = engine.scan(&selection, &mut fresh)?;
        (r.states, r.warnings, r.referenced_ids)
    };

    // Materialize referenced-but-missing license texts (offline) unless dry-run, against the
    // *post-apply* references so texts for licenses we just replaced are not sought. A text we
    // cannot produce is a hard failure (FR-017): silently stubbing it would let a non-compliant
    // repo read as compliant. Standard SPDX ids may be fetched via `curl` when the user opts in
    // (`--allow-curl` or an interactive y/N); the rest fail with guidance.
    let mut missing_texts: Vec<String> = Vec::new();
    if !args.dry_run
        && let Ok(res) = inventory::materialize(&root, &post_referenced)
    {
        missing_texts = super::resolve_missing_texts(&root, &res.still_missing, args.allow_curl);
        for id in &missing_texts {
            warnings.push(Warning {
                kind: "missing_license_text".to_string(),
                path: None,
                message: inventory::missing_text_guidance(id),
            });
        }
    }
    // Final report warnings: post-write detection warnings, then apply-pass warnings.
    let mut all_warnings = scan_warnings;
    all_warnings.append(&mut warnings);
    let warnings = all_warnings;

    let exit = if partial {
        ExitCode::Partial
    } else if has_unfixable(&final_states) || any_failure || !missing_texts.is_empty() {
        ExitCode::Violations
    } else {
        ExitCode::Success
    };

    let mut report = Report::build("apply", &final_states, &changes, warnings, Some(partial));
    report.exit_code = Some(exit.code());

    match args.common.format {
        Format::Json => println!("{}", report.to_json()),
        Format::Human => print!("{}", render_human(&report)),
    }
    Ok(exit)
}

/// True when Uncovered/Unreadable files remain (apply cannot fix by writing — exit 1).
fn has_unfixable(states: &[crate::domain::FileLicensingState]) -> bool {
    states
        .iter()
        .any(|s| matches!(s.drift, DriftClass::Uncovered | DriftClass::Unreadable))
}

/// Detect an uncommitted working tree via `git status --porcelain`.
fn is_dirty(root: &Path) -> bool {
    match std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
    {
        Ok(out) if out.status.success() => !out.stdout.is_empty(),
        // Not a git repo (or git missing) → treat as clean so apply still works.
        _ => false,
    }
}

/// Exposed for selection used by the dry-run preview surface.
#[allow(dead_code)]
fn _selection_doc(_: &Selection) {}
