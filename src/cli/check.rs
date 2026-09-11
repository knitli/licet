//! `check` — non-writing gate (FR-012, FR-012a, FR-013; US1, US4).

use super::{CheckArgs, Format};
use crate::config::LicensingConfiguration;
use crate::engine::Engine;
use crate::error::{ExitCode, LicetError, Result};
use crate::report::Diagnostic;
use crate::report::Report;
use crate::report::render::{ExplainInput, render_explain, render_human};
use crate::reuse::inventory::LicenseTextInventory;
use crate::rules::{Match, RuleSet};
use crate::walk::{self, Purpose, Selection, discover_root};

pub fn run(args: CheckArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let (root, _) = discover_root(&cwd)?;

    // --explain resolves one path directly: no whole-tree content scan and no
    // cache writes. A path outside the selected set (or not on disk) is a
    // usage diagnostic, never success.
    if let Some(target) = &args.explain {
        return explain_one(&args, &cwd, target);
    }

    let selection = args.common.selection()?;
    let config_arg = args.common.config_arg(&cwd, &root);

    // Resolve paths, snapshot, and configuration in one consistent step:
    // `--staged` reads the index (including staged metadata/config/texts).
    let prep = walk::prepare(&cwd, &config_arg, &selection, Purpose::Policy, false)?;
    let config = LicensingConfiguration::from_toml(&prep.config_text)?;

    warn_deprecated_cache_flags(&args.common);
    let engine = Engine::new(root.clone(), &config, prep.snapshot.clone(), true);
    let scan = engine.scan(&prep.paths)?;

    let mut warnings = scan.warnings;
    if let Some(note) = prep.expansion_note {
        warnings.push(Diagnostic {
            code: "selection_expanded".to_string(),
            path: None,
            message: note,
        });
    }

    // `check` requires the license texts referenced by its selected files —
    // actual effective plus declared desired scope (decision 1). Unused and
    // unrecognized project entries never fail a selected-file policy check.
    let mut scope = scan.actual_referenced_ids.clone();
    scope.extend(scan.desired_referenced_ids.iter().cloned());
    let inv = LicenseTextInventory::compute(&prep.snapshot, &scope)?;
    if !inv.missing.is_empty() {
        let missing: Vec<String> = inv.missing.iter().cloned().collect();
        warnings.push(Diagnostic {
            code: "missing_license_text".to_string(),
            path: None,
            message: format!(
                "license texts referenced by the selected files are missing under LICENSES/: {}",
                missing.join(", ")
            ),
        });
    }
    let texts = crate::report::LicenseTexts {
        referenced: inv.referenced.iter().cloned().collect(),
        present: inv.present.iter().cloned().collect(),
        missing: inv.missing.iter().cloned().collect(),
        bundled_available: inv.bundled_available.iter().cloned().collect(),
        spdx_list_version: crate::spdx::spdx_list_version().to_string(),
        unused: Vec::new(),
        unrecognized: Vec::new(),
        missing_extension: Vec::new(),
    };

    let mut report = Report::build(
        "check",
        &scan.states,
        &[],
        warnings,
        None,
        crate::report::ReportMeta {
            snapshot: prep.snapshot.source().as_str().to_string(),
            complete: true,
            ..Default::default()
        },
    );
    if !inv.missing.is_empty() {
        report.summary.pass = false;
    }
    report.license_texts = Some(texts);
    let exit = if report.summary.pass {
        ExitCode::Success
    } else {
        ExitCode::Violations
    };
    report.exit_code = Some(exit.code());

    let rendered = match args.common.format {
        Format::Json => format!("{}\n", report.to_json()),
        Format::Human => render_human(&report),
    };
    super::emit_stdout(&rendered)?;
    Ok(exit)
}

/// `--cache` / `--no-cache` are retained for one compatibility window as
/// documented deprecated no-ops: scans are stateless and never create files.
/// Warns at most with a stderr notice.
pub fn warn_deprecated_cache_flags(common: &super::CommonArgs) {
    if common.no_cache || common.cache.is_some() {
        eprintln!(
            "warning: --cache/--no-cache are deprecated no-ops; licet scans are stateless and never cache"
        );
    }
}

/// Helper reused by apply for selection resolution context.
pub fn resolve_selection(common: &super::CommonArgs) -> Result<Selection> {
    common.selection()
}

/// Explain one path: winning rule index/selector (or default), losing
/// matches with specificity, exclusions, metadata provenance, and current
/// drift — evaluated from working-tree bytes for this path alone, with a
/// disabled cache that writes nothing (FR-002, FR-022).
fn explain_one(
    args: &CheckArgs,
    cwd: &std::path::Path,
    target: &std::path::Path,
) -> Result<ExitCode> {
    let resolved = resolve_explain_target(args, cwd, target)?;
    let rel = resolved.rel.clone();
    let config = LicensingConfiguration::from_toml(&resolved.prepared.config_text)?;
    let engine = Engine::new(
        resolved.prepared.root.clone(),
        &config,
        resolved.prepared.snapshot.clone(),
        true,
    );
    let scan = engine.scan(std::slice::from_ref(&resolved.discovered))?;
    let state = scan.states.first().ok_or_else(|| {
        LicetError::Internal(format!("--explain: no state for {}", target.display()))
    })?;

    let ruleset = RuleSet::new(&config);
    let (winner, default_intent, in_conflict) = match ruleset.resolve(&rel) {
        Match::Rule(r) => (
            Some((
                r.source_order + 1,
                r.label(),
                r.intent.license_expression.clone(),
            )),
            None,
            false,
        ),
        Match::Default(d) => (None, d.map(|i| i.license_expression.clone()), false),
        Match::Conflict(_) => (None, None, true),
    };
    // `matching_rules` sorts most specific first with earliest declaration
    // breaking ties — the same order `resolve` picks from — so element zero
    // is the winner whenever there is no conflict.
    let all_matches = crate::rules::matching_rules(&config, &rel);
    let mut conflict_rules: Vec<(usize, String)> = Vec::new();
    let mut losers: Vec<(usize, String, u32)> = Vec::new();
    if in_conflict {
        let top = all_matches.first().map(|(_, spec)| *spec).unwrap_or(0);
        for (rule, spec) in &all_matches {
            let entry = (rule.source_order + 1, rule.label());
            if *spec == top {
                conflict_rules.push(entry);
            } else {
                losers.push((entry.0, entry.1, *spec));
            }
        }
    } else {
        for (rule, spec) in all_matches.iter().skip(1) {
            losers.push((rule.source_order + 1, rule.label(), *spec));
        }
    }

    let excludes = walk::build_excludes(&config.exclude)?;
    let slash = rel.to_string_lossy().replace('\\', "/");
    let sources = explain_sources(state);

    let rel_display = slash.clone();
    match args.common.format {
        Format::Json => {
            let mut report = Report::build(
                "check",
                &scan.states,
                &[],
                scan.warnings,
                None,
                crate::report::ReportMeta {
                    snapshot: resolved.prepared.snapshot.source().as_str().to_string(),
                    complete: true,
                    ..Default::default()
                },
            );
            report.exit_code = Some(ExitCode::Success.code());
            super::emit_stdout(&format!("{}\n", report.to_json()))?;
        }
        Format::Human => {
            super::emit_stdout(&render_explain(&ExplainInput {
                path: &rel_display,
                drift: &state.drift,
                winner,
                default_intent,
                conflict_rules,
                losers,
                excluded_by_config: excludes.is_match(&slash),
                reuse_ignored: resolved.discovered.reuse_ignored,
                sources,
                snapshot: resolved.prepared.snapshot.source().as_str(),
            }))?;
        }
    }
    Ok(ExitCode::Success)
}

/// An `--explain` target resolved against the selection pipeline: the
/// single-file evaluation plan, the discovered entry, and its root-relative
/// path. Outside-root and nonregular inputs fail here as usage errors, as
/// does a target outside an explicitly selected file set.
struct ExplainTarget {
    prepared: walk::Prepared,
    discovered: walk::Discovered,
    rel: std::path::PathBuf,
}

fn resolve_explain_target(
    args: &CheckArgs,
    cwd: &std::path::Path,
    target: &std::path::Path,
) -> Result<ExplainTarget> {
    let (root, _) = discover_root(cwd)?;
    // Normalize through the explicit-file pipeline: outside-root and
    // nonregular inputs fail here as usage errors.
    let prepared = walk::prepare(
        cwd,
        &args.common.config_arg(cwd, &root),
        &Selection::Files(vec![target.to_path_buf()]),
        Purpose::Policy,
        false,
    )?;
    let discovered = prepared.paths.first().cloned().ok_or_else(|| {
        LicetError::Internal(format!(
            "--explain: no evaluable path for {}",
            target.display()
        ))
    })?;
    let rel = discovered.rel_path.clone();
    match std::fs::symlink_metadata(prepared.root.join(&rel)) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LicetError::Config(format!(
                "--explain: no such file {}",
                target.display()
            )));
        }
        Err(e) => {
            return Err(LicetError::Config(format!(
                "--explain: cannot stat {}: {e}",
                target.display()
            )));
        }
    }
    // An explicit selection restricts the answerable set.
    if !matches!(args.common.selection()?, Selection::FullTree) {
        let selected = walk::prepare(
            cwd,
            &args.common.config_arg(cwd, &root),
            &args.common.selection()?,
            Purpose::Policy,
            false,
        )?;
        if !selected.paths.iter().any(|d| d.rel_path == rel) {
            return Err(LicetError::Config(format!(
                "--explain: {} is outside the selected file set",
                target.display()
            )));
        }
    }
    Ok(ExplainTarget {
        prepared,
        discovered,
        rel,
    })
}

/// Human provenance lines for an evaluated file: detection source,
/// out-of-band table origins, and inventoried-but-non-policy findings.
fn explain_sources(state: &crate::domain::FileLicensingState) -> Vec<String> {
    let mut sources = Vec::new();
    if let Some(s) = &state.actual.detected_source {
        sources.push(format!("detected via {}", s.as_str()));
    }
    if let Some(oob) = &state.actual.out_of_band {
        for o in &oob.origins {
            sources.push(format!(
                "{} table {} ({})",
                o.metadata_path.to_string_lossy().replace('\\', "/"),
                o.table_index,
                o.precedence.as_str()
            ));
        }
    }
    if !state.actual.snippet_licenses.is_empty() {
        sources.push(format!(
            "{} snippet license(s) (inventoried, never file policy)",
            state.actual.snippet_licenses.len()
        ));
    }
    if !state.actual.invalid_license_values.is_empty() {
        sources.push(format!(
            "{} rejected license value(s), kept for diagnosis",
            state.actual.invalid_license_values.len()
        ));
    }
    sources
}
