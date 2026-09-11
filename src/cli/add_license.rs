//! `add-license` (alias `add`) — materialize referenced license texts into `LICENSES/`
//! from the embedded bundle, offline (FR-017, FR-029; US5).
//!
//! This is the offline analog of REUSE's `download`: because the SPDX corpus is embedded,
//! the operation is a copy from the bundle, never a network fetch — unless the caller
//! passes `--allow-network`, which permits downloading valid-but-unbundled standard
//! texts with the system `curl` binary into owned temporary storage. It writes **only**
//! under `LICENSES/` — it never modifies a source file or `licet.toml`, and therefore
//! does not require a clean working tree.
//!
//! Every requested identifier is validated before any directory is created or any
//! subprocess is spawned: unknown ids and path escapes are usage errors (exit 2).
//! Syntactically valid `LicenseRef-*` ids are reported as required local texts —
//! never downloaded, and never scaffolded with placeholder prose.

use std::collections::BTreeSet;

use super::{AddLicenseArgs, Format};
use crate::config::{CONFIG_FILENAME, LicensingConfiguration};
use crate::engine::Engine;
use crate::error::{ExitCode, LicetError, Result};
use crate::reuse::inventory::{self, LicenseTextInventory, ValidatedId, validate_materialize_id};
use crate::reuse::{atomic_write, read_expected_for_write};
use crate::spdx;
use crate::walk::{Purpose, Selection, discover_root, prepare};

pub fn run(args: AddLicenseArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let (root, _) = discover_root(&cwd)?;

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

    // Resolve the target id set: `--all` scans the union of desired and
    // actual references; config errors propagate (exit 2), never swallowed.
    // An explicit list targets exactly those ids (whether or not they are
    // referenced anywhere).
    let raw_targets: BTreeSet<String> = if args.all {
        let config_arg = match &args.config {
            Some(p) if p.is_absolute() => p.clone(),
            Some(p) => cwd.join(p),
            None => root.join(CONFIG_FILENAME),
        };
        let prep = prepare(
            &cwd,
            &config_arg,
            &Selection::FullTree,
            Purpose::Policy,
            true,
        )?;
        let config_text = prep.config_text.clone();
        let config = LicensingConfiguration::from_toml(&config_text)?;
        let engine = Engine::new(root.clone(), &config, prep.snapshot, true);
        engine.scan(&prep.paths)?.referenced_ids
    } else {
        args.ids.iter().cloned().collect()
    };

    // Validate the whole requested list (canonicalizing standard-id spelling) before
    // creating any directory or spawning any subprocess. An invalid id is a usage
    // error even when `--allow-network` is given.
    let mut validated: Vec<ValidatedId> = Vec::with_capacity(raw_targets.len());
    for id in &raw_targets {
        validated.push(validate_materialize_id(id).map_err(|e| LicetError::Config(e.to_string()))?);
    }
    let targets: BTreeSet<String> = validated
        .iter()
        .map(|v| match v {
            ValidatedId::Bundled { canonical } | ValidatedId::Fetchable { canonical } => {
                canonical.clone()
            }
            ValidatedId::CustomRef { id } => id.clone(),
        })
        .collect();

    let worktree = crate::walk::Snapshot::Worktree { root: root.clone() };
    let before = LicenseTextInventory::compute(&worktree, &targets)?;
    let already_present: Vec<String> = targets
        .iter()
        .filter(|id| before.present.contains(*id))
        .cloned()
        .collect();

    let res = inventory::materialize(&root, &targets)?;

    // Optional network fetch for valid-but-unbundled standard ids. LicenseRef-*
    // ids are never fetched: custom texts must be supplied by the maintainer.
    // Failures leave the destination untouched and keep the id missing (exit 1).
    let mut fetched: Vec<String> = Vec::new();
    let mut fetch_failures: Vec<(String, String)> = Vec::new();
    let mut still_missing = res.still_missing;
    if args.allow_network {
        let mut remaining = Vec::new();
        for id in std::mem::take(&mut still_missing) {
            let fetchable = validated
                .iter()
                .any(|v| matches!(v, ValidatedId::Fetchable { canonical } if canonical == &id));
            if !fetchable || before.present.contains(&id) {
                remaining.push(id);
                continue;
            }
            match inventory::fetch_text_via_curl(&id) {
                Ok(bytes) => {
                    let rel = std::path::Path::new("LICENSES").join(format!("{id}.txt"));
                    match read_expected_for_write(&root.join(&rel)) {
                        Ok(None) => match atomic_write(&root, &rel, None, &bytes) {
                            Ok(()) => fetched.push(id),
                            Err(e) => {
                                fetch_failures.push((id, format!("install failed: {e}")));
                                remaining.push(fetch_failures.last().unwrap().0.clone());
                            }
                        },
                        Ok(Some(_)) => {
                            // Appeared concurrently; treat as present.
                        }
                        Err(e) => {
                            fetch_failures.push((id, format!("cannot verify destination: {e}")));
                            remaining.push(fetch_failures.last().unwrap().0.clone());
                        }
                    }
                }
                Err(e) => {
                    fetch_failures.push((id.clone(), format!("download failed: {e}")));
                    remaining.push(id);
                }
            }
        }
        still_missing = remaining;
    }

    let after = LicenseTextInventory::compute(&worktree, &targets)?;
    let mut written = res.written;
    written.extend(fetched.iter().cloned());
    let success = still_missing.is_empty();

    match args.format {
        Format::Json => {
            let texts = crate::report::LicenseTexts {
                referenced: after.referenced.iter().cloned().collect(),
                present: after.present.iter().cloned().collect(),
                missing: after.missing.iter().cloned().collect(),
                bundled_available: after.bundled_available.iter().cloned().collect(),
                spdx_list_version: spdx::spdx_list_version().to_string(),
                unused: after.unused.clone(),
                unrecognized: after.unrecognized.iter().map(|u| u.path.clone()).collect(),
                missing_extension: after.missing_extension.clone(),
            };
            let report = serde_json::json!({
                "version": 2,
                "command": "add-license",
                "exit_code": if success { 0 } else { 1 },
                "summary": { "pass": success, "complete": true, "counts": {} },
                "files": [],
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
                "Materialized {} license text(s) into LICENSES/ (SPDX list {}):\n",
                written.len(),
                spdx::spdx_list_version()
            );
            for id in &written {
                human.push_str(&format!("  + {id}\n"));
            }
            if !already_present.is_empty() {
                human.push_str("Already present (skipped):\n");
                for id in &already_present {
                    human.push_str(&format!("  = {id}\n"));
                }
            }
            if !still_missing.is_empty() {
                human.push_str("Unavailable:\n");
                for id in &still_missing {
                    let why = if spdx::is_license_ref(id) {
                        format!("custom LicenseRef — add LICENSES/{id}.txt manually")
                    } else if let Some((_, err)) = fetch_failures.iter().find(|(fid, _)| fid == id)
                    {
                        err.clone()
                    } else if args.allow_network {
                        format!(
                            "absent from bundle — download failed ({})",
                            inventory::download_url(id)
                        )
                    } else {
                        "absent from bundle — needs --allow-network".to_string()
                    };
                    human.push_str(&format!("  ! {id} ({why})\n"));
                }
            }
            super::emit_stdout(&human)?;
        }
    }

    Ok(if success {
        ExitCode::Success
    } else {
        ExitCode::Violations
    })
}
