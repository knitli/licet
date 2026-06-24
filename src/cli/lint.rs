//! `lint` — REUSE-compatibility & license-text report (FR-014, FR-017, FR-028; US5).

use super::{Format, LintArgs};
use crate::config::LicensingConfiguration;
use crate::engine::Engine;
use crate::error::{ExitCode, Result};
use crate::reuse::inventory::LicenseTextInventory;
use crate::spdx;
use crate::walk::cache::ScanCache;
use crate::walk::{discover_root, Selection};

pub fn run(args: LintArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = discover_root(&cwd);
    let config_text = std::fs::read_to_string(&args.config).unwrap_or_default();
    let config = LicensingConfiguration::from_toml(&config_text).unwrap_or_default();

    let mut cache = ScanCache::disabled();
    let engine = Engine::new(root.clone(), &config, &config_text);
    let scan = engine.scan(&Selection::FullTree, &mut cache)?;

    let inv = LicenseTextInventory::compute(&root, &scan.referenced_ids);

    // REUSE posture: every covered file has a license, and every referenced text is present.
    let header_failures = scan.states.iter().filter(|s| s.drift.is_failure()).count();
    let compliant = header_failures == 0 && inv.is_complete();

    match args.format {
        Format::Json => {
            let texts = crate::report::LicenseTexts {
                referenced: inv.referenced.iter().cloned().collect(),
                present: inv.present.iter().cloned().collect(),
                missing: inv.missing.iter().cloned().collect(),
                bundled_available: inv.bundled_available.iter().cloned().collect(),
                spdx_list_version: spdx::spdx_list_version().to_string(),
            };
            let report = serde_json::json!({
                "version": 1,
                "command": "lint",
                "exit_code": if compliant { 0 } else { 1 },
                "summary": { "pass": compliant, "counts": {} },
                "files": [],
                "license_texts": texts,
            });
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }
        Format::Human => {
            println!(
                "REUSE compliance posture (SPDX list {}):",
                spdx::spdx_list_version()
            );
            println!("  files failing header coverage: {header_failures}");
            println!("  LICENSES/ present: {}", inv.present.len());
            if !inv.missing.is_empty() {
                println!("  missing license texts:");
                for id in &inv.missing {
                    let hint = if spdx::bundled_text(id).is_some() {
                        "available offline (run `licet apply`)"
                    } else if spdx::is_license_ref(id) {
                        "custom LicenseRef — scaffold a placeholder"
                    } else if args.allow_network {
                        "absent from bundle — would fetch (network allowed)"
                    } else {
                        "absent from bundle — needs --allow-network"
                    };
                    println!("    - {id} ({hint})");
                }
            }
            println!(
                "Result: {}",
                if compliant {
                    "COMPLIANT"
                } else {
                    "NON-COMPLIANT"
                }
            );
        }
    }

    Ok(if compliant {
        ExitCode::Success
    } else {
        ExitCode::Violations
    })
}
