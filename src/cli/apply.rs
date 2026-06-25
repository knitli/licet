//! `apply` — reconcile to declared intent (FR-006..FR-009, FR-021, FR-024; US2).

use std::path::Path;

use super::{ApplyArgs, Format};
use crate::comment::CommentResolver;
use crate::config::LicensingConfiguration;
use crate::detect;
use crate::domain::{ChangeMode, DriftClass, FileChange};
use crate::engine::Engine;
use crate::error::{ExitCode, LicetError, Result};
use crate::report::render::render_human;
use crate::report::{Report, Warning};
use crate::reuse::oob::OutOfBand;
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

    let mut changes: Vec<FileChange> = Vec::new();
    let mut warnings: Vec<Warning> = scan.warnings.clone();
    let mut any_failure = false;
    let mut any_success = false;

    for state in &scan.states {
        // Only Missing/WrongLicense are writable; Uncovered/Unreadable/Excluded are not.
        let writable = matches!(
            state.drift,
            DriftClass::MissingHeader | DriftClass::WrongLicense { .. }
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

        let style = match resolver.resolve(&state.path) {
            Some(s) => s,
            None => {
                warnings.push(Warning {
                    kind: "missing_license_text".to_string(),
                    path: Some(rel_str.clone()),
                    message: "no comment style resolved for this file type; cannot write header"
                        .to_string(),
                });
                any_failure = true;
                continue;
            }
        };

        // Re-read full content for an accurate edit (engine only read the head).
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(_) => {
                any_failure = true;
                continue;
            }
        };
        // Re-detect against full content so byte ranges are correct for the whole file.
        let actual = detect::detect(&state.path, content.as_bytes(), &oob);

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
    }

    // Materialize referenced-but-missing license texts (offline) unless dry-run.
    if !args.dry_run
        && let Ok(res) = inventory::materialize(&root, &scan.referenced_ids)
    {
        for id in res.still_missing {
            warnings.push(Warning {
                kind: "missing_license_text".to_string(),
                path: None,
                message: format!("no offline text for `{id}` (use lint --allow-network)"),
            });
        }
    }

    // Re-scan post-write so the report and exit code reflect the actual on-disk result
    // (dry-run keeps the pre-apply states, since nothing was written).
    let partial = any_failure && any_success;
    let final_states = if args.dry_run {
        scan.states
    } else {
        let mut fresh = super::check::open_cache(&args.common, &root, &config_text);
        engine.scan(&selection, &mut fresh)?.states
    };

    let exit = if partial {
        ExitCode::Partial
    } else if has_unfixable(&final_states) || any_failure {
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
