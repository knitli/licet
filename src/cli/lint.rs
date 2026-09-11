//! `lint` — REUSE-compatibility & license-text report (FR-014, FR-017, FR-028; US5).
//!
//! `lint` validates **actual** REUSE 3.3 metadata over every covered file,
//! independently of whether any declaration rule exists: each covered file
//! needs a license expression and a copyright notice, and every referenced
//! license text must exist under `LICENSES/`. It never parses auto-discovered
//! policy configuration — a `license.toml` in the tree is simply another
//! covered file whose own licensing metadata is checked.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::{Format, LintArgs};
use crate::config::LicensingConfiguration;
use crate::domain::DriftClass;
use crate::engine::Engine;
use crate::error::{ExitCode, LicetError, Result};
use crate::report::Diagnostic;
use crate::report::classify::evaluate_reuse;
use crate::reuse::inventory::LicenseTextInventory;
use crate::spdx;
use crate::walk::{Purpose, Selection, discover_root, prepare};

pub fn run(args: LintArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let (root, _) = discover_root(&cwd)?;

    // An explicitly supplied config must exist and parse (usage error 2
    // otherwise); it is accepted with a deprecation notice but never changes
    // REUSE evaluation.
    if let Some(cfg_path) = &args.config {
        let abs = if cfg_path.is_absolute() {
            cfg_path.clone()
        } else {
            cwd.join(cfg_path)
        };
        let text = std::fs::read_to_string(&abs).map_err(|e| {
            LicetError::Config(format!(
                "cannot read explicit lint config `{}`: {e}",
                abs.display()
            ))
        })?;
        LicensingConfiguration::from_toml(&text)?;
        eprintln!(
            "warning: `lint --config` is deprecated and ignored: lint validates actual REUSE metadata, not declared policy"
        );
    }

    // REUSE validation covers tracked plus nonignored untracked files and never
    // applies declaration `[exclude]` rules (a config exclusion cannot hide a
    // file from a whole-project compliance claim). The discovered policy text
    // is deliberately never parsed: it is just another covered file.
    let prep = prepare(
        &cwd,
        Path::new("license.toml"),
        &Selection::FullTree,
        Purpose::Lint,
        true,
    )?;
    let config = LicensingConfiguration::default();

    let engine = Engine::new(root.clone(), &config, prep.snapshot.clone(), false);
    let scan = engine.scan(&prep.paths)?;

    // Snapshot read failures by path, for incomplete-validation diagnostics.
    let read_errors: HashMap<String, &str> = scan
        .warnings
        .iter()
        .filter(|w| w.code == "read_error")
        .filter_map(|w| {
            w.path
                .as_deref()
                .map(|p| (p.to_string(), w.message.as_str()))
        })
        .collect();

    // Policy-only (`rule_conflict`) and superseded (`invalid_license`, restated
    // per file by REUSE evaluation below with the stable code) diagnostics
    // never reach lint output.
    let mut warnings: Vec<Diagnostic> = scan
        .warnings
        .iter()
        .filter(|w| w.code != "rule_conflict" && w.code != "invalid_license")
        .cloned()
        .collect();
    let mut failing_files: HashSet<String> = HashSet::new();
    let mut incomplete = false;
    for state in &scan.states {
        let rel = state.path.to_string_lossy().replace('\\', "/");
        let excluded = matches!(state.drift, DriftClass::Excluded);
        let eval = evaluate_reuse(
            excluded,
            &state.actual,
            read_errors.get(rel.as_str()).copied(),
        );
        if eval.incomplete {
            incomplete = true;
        }
        if !eval.passed {
            failing_files.insert(rel.clone());
        }
        for d in eval.diagnostics {
            warnings.push(Diagnostic {
                code: d.code.to_string(),
                path: Some(rel.clone()),
                message: d.message,
            });
        }
    }
    warnings.sort_by_key(|w| (w.path.clone(), w.code.clone()));

    // License-text inventory over actual references only: declaration rules
    // never contribute to REUSE actuals, and unused config rules stay out.
    let inv = LicenseTextInventory::compute(&prep.snapshot, &scan.actual_referenced_ids)?;
    for id in &inv.missing {
        warnings.push(Diagnostic {
            code: "missing_license_text".to_string(),
            path: None,
            message: format!("license text for `{id}` is missing under LICENSES/"),
        });
    }
    for id in &inv.unused {
        warnings.push(Diagnostic {
            code: "unused_license_text".to_string(),
            path: None,
            message: format!("LICENSES/{id} is never referenced"),
        });
    }
    for u in &inv.unrecognized {
        warnings.push(Diagnostic {
            code: "bad_license_text".to_string(),
            path: Some(u.path.clone()),
            message: format!("unrecognized license text: {}", u.reason),
        });
    }
    for id in &inv.missing_extension {
        warnings.push(Diagnostic {
            code: "missing_license_extension".to_string(),
            path: Some(format!("LICENSES/{id}")),
            message: format!(
                "`{id}` is kept in an extensionless file; REUSE 3.3 requires a filename extension"
            ),
        });
    }
    for u in &inv.unreadable {
        incomplete = true;
        warnings.push(Diagnostic {
            code: "unsupported_encoding".to_string(),
            path: Some(u.path.clone()),
            message: format!("cannot validate license text: {}", u.reason),
        });
    }

    let compliant = failing_files.is_empty() && inv.is_complete() && !incomplete;

    // The evaluated covered set (sorted repo-relative paths, excluding
    // REUSE-ignored files): the differential suite compares this against the
    // reference tool's file list.
    let mut coverage: Vec<String> = scan
        .states
        .iter()
        .filter(|s| !matches!(s.drift, DriftClass::Excluded))
        .map(|s| s.path.to_string_lossy().replace('\\', "/"))
        .collect();
    coverage.sort();

    match args.format {
        Format::Json => {
            let texts = crate::report::LicenseTexts {
                referenced: inv.referenced.iter().cloned().collect(),
                present: inv.present.iter().cloned().collect(),
                missing: inv.missing.iter().cloned().collect(),
                bundled_available: inv.bundled_available.iter().cloned().collect(),
                spdx_list_version: spdx::spdx_list_version().to_string(),
                unused: inv.unused.clone(),
                unrecognized: inv.unrecognized.iter().map(|u| u.path.clone()).collect(),
                missing_extension: inv.missing_extension.clone(),
            };
            let report = serde_json::json!({
                "version": 2,
                "command": "lint",
                "exit_code": if compliant { 0 } else { 1 },
                "summary": { "pass": compliant, "complete": true, "counts": {} },
                "files": [],
                "coverage": coverage,
                "diagnostics": warnings,
                "license_texts": texts,
            });
            // Serializing a `serde_json::Value` cannot fail; the fallback keeps
            // a serialization bug from panicking instead of reporting.
            let body = serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string());
            super::emit_stdout(&format!("{body}\n"))?;
        }
        Format::Human => {
            // Built as one document so stdout goes through the shared
            // emitter (quiet on a closed pipe instead of panicking).
            let mut human = format!(
                "REUSE compliance posture (SPDX list {}):\n",
                spdx::spdx_list_version()
            );
            human.push_str(&format!(
                "  files failing REUSE validation: {}\n",
                failing_files.len()
            ));
            if incomplete {
                human.push_str("  validation incomplete: some files or texts could not be read\n");
            }
            human.push_str(&format!("  LICENSES/ present: {}\n", inv.present.len()));
            if !inv.missing.is_empty() {
                human.push_str("  missing license texts:\n");
                for id in &inv.missing {
                    let hint = if spdx::bundled_text(id).is_some() {
                        "available offline (run `licet add-license`)"
                    } else if spdx::is_license_ref(id) {
                        "custom LicenseRef — supply the text manually"
                    } else if args.allow_network {
                        "absent from bundle — would fetch (network allowed)"
                    } else {
                        "absent from bundle — needs --allow-network"
                    };
                    human.push_str(&format!("    - {id} ({hint})\n"));
                }
            }
            if !inv.unused.is_empty() {
                human.push_str("  unused license texts:\n");
                for id in &inv.unused {
                    human.push_str(&format!("    - {id}\n"));
                }
            }
            if !inv.unrecognized.is_empty() {
                human.push_str("  unrecognized LICENSES/ entries:\n");
                for u in &inv.unrecognized {
                    human.push_str(&format!("    - {} ({})\n", u.path, u.reason));
                }
            }
            if !inv.missing_extension.is_empty() {
                human.push_str("  license texts missing a filename extension:\n");
                for id in &inv.missing_extension {
                    human.push_str(&format!("    - LICENSES/{id}\n"));
                }
            }
            human.push_str(&format!(
                "Result: {}\n",
                if compliant {
                    "COMPLIANT"
                } else {
                    "NON-COMPLIANT"
                }
            ));
            super::emit_stdout(&human)?;
        }
    }

    Ok(if compliant {
        ExitCode::Success
    } else {
        ExitCode::Violations
    })
}
