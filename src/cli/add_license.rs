//! `add-license` (alias `add`) — materialize referenced license texts into `LICENSES/`
//! from the embedded bundle, offline (FR-017, FR-029; US5).
//!
//! This is the offline analog of REUSE's `download`: because the SPDX corpus is embedded,
//! the operation is a copy from the bundle (or a `LicenseRef-*` placeholder scaffold),
//! never a network fetch. It writes **only** under `LICENSES/` — it never modifies a source
//! file or `license.toml`, and therefore does not require a clean working tree.

use std::collections::BTreeSet;

use super::{AddLicenseArgs, Format};
use crate::config::LicensingConfiguration;
use crate::engine::Engine;
use crate::error::{ExitCode, LicetError, Result};
use crate::reuse::inventory::{self, LicenseTextInventory};
use crate::spdx;
use crate::walk::cache::ScanCache;
use crate::walk::{Selection, discover_root};

pub fn run(args: AddLicenseArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = discover_root(&cwd);

    // Flag contract (FR-029): exactly one of <ids> or --all.
    if args.ids.is_empty() && !args.all {
        return Err(LicetError::Config(
            "specify one or more SPDX identifiers, or --all to materialize every missing text"
                .to_string(),
        ));
    }
    if !args.ids.is_empty() && args.all {
        return Err(LicetError::Config(
            "pass either explicit identifiers or --all, not both".to_string(),
        ));
    }

    // Resolve the target id set: `--all` scans config + headers for referenced ids; an
    // explicit list targets exactly those ids (whether or not they are referenced anywhere).
    let targets: BTreeSet<String> = if args.all {
        let config_text = std::fs::read_to_string(&args.config).unwrap_or_default();
        let config = LicensingConfiguration::from_toml(&config_text).unwrap_or_default();
        let mut cache = ScanCache::disabled();
        let engine = Engine::new(root.clone(), &config, &config_text);
        engine
            .scan(&Selection::FullTree, &mut cache)?
            .referenced_ids
    } else {
        args.ids.iter().cloned().collect()
    };

    let before = LicenseTextInventory::compute(&root, &targets);
    let already_present: Vec<String> = targets
        .iter()
        .filter(|id| before.present.contains(*id))
        .cloned()
        .collect();

    let res = inventory::materialize(&root, &targets)?;
    let after = LicenseTextInventory::compute(&root, &targets);
    let success = res.still_missing.is_empty();

    match args.format {
        Format::Json => {
            let texts = crate::report::LicenseTexts {
                referenced: after.referenced.iter().cloned().collect(),
                present: after.present.iter().cloned().collect(),
                missing: after.missing.iter().cloned().collect(),
                bundled_available: after.bundled_available.iter().cloned().collect(),
                spdx_list_version: spdx::spdx_list_version().to_string(),
            };
            let report = serde_json::json!({
                "version": 1,
                "command": "add-license",
                "exit_code": if success { 0 } else { 1 },
                "summary": { "pass": success, "counts": {} },
                "files": [],
                "license_texts": texts,
            });
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }
        Format::Human => {
            println!(
                "Materialized {} license text(s) into LICENSES/ (SPDX list {}):",
                res.written.len(),
                spdx::spdx_list_version()
            );
            for id in &res.written {
                println!("  + {id}");
            }
            if !already_present.is_empty() {
                println!("Already present (skipped):");
                for id in &already_present {
                    println!("  = {id}");
                }
            }
            if !res.still_missing.is_empty() {
                println!("Unavailable offline:");
                for id in &res.still_missing {
                    let why = if args.allow_network {
                        "absent from bundle — network fetch is unavailable in this hermetic build"
                    } else {
                        "absent from bundle — needs --allow-network"
                    };
                    println!("  ! {id} ({why})");
                }
            }
        }
    }

    Ok(if success {
        ExitCode::Success
    } else {
        ExitCode::Violations
    })
}
