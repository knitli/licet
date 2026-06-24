//! `check` — non-writing gate (FR-012, FR-012a, FR-013; US1, US4).

use std::path::Path;

use super::{CheckArgs, Format};
use crate::config::LicensingConfiguration;
use crate::engine::{default_cache_path, Engine};
use crate::error::{ExitCode, LicetError, Result};
use crate::report::render::{render_explain, render_human};
use crate::report::Report;
use crate::walk::cache::{config_fingerprint, ScanCache};
use crate::walk::{discover_root, Selection};

pub fn run(args: CheckArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let root = discover_root(&cwd);
    let config_text = read_config(&args.common.config)?;
    let config = LicensingConfiguration::from_toml(&config_text)?;
    let selection = args.common.selection()?;

    let mut cache = open_cache(&args.common, &root, &config_text);
    let engine = Engine::new(root.clone(), &config, &config_text);
    let scan = engine.scan(&selection, &mut cache)?;
    cache.flush().ok();

    // --explain: print the winning rule for a single path and exit 0.
    if let Some(path) = &args.explain {
        let rel = path.strip_prefix(&root).unwrap_or(path);
        if let Some(state) = scan
            .states
            .iter()
            .find(|s| s.path == *rel || s.path == *path)
        {
            println!(
                "{}",
                render_explain(
                    &state.path.to_string_lossy(),
                    &state.drift,
                    &state.matched_rule
                )
            );
        } else {
            println!("{}: not found in the selected file set", path.display());
        }
        return Ok(ExitCode::Success);
    }

    let mut report = Report::build("check", &scan.states, &[], scan.warnings, None);
    let exit = if report.summary.pass {
        ExitCode::Success
    } else {
        ExitCode::Violations
    };
    report.exit_code = Some(exit.code());

    match args.common.format {
        Format::Json => println!("{}", report.to_json()),
        Format::Human => print!("{}", render_human(&report)),
    }
    Ok(exit)
}

fn read_config(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| LicetError::Config(format!("cannot read config `{}`: {e}", path.display())))
}

/// Open the scan cache honoring `--no-cache` / `--cache <path>` (SC-006).
pub fn open_cache(common: &super::CommonArgs, root: &Path, config_text: &str) -> ScanCache {
    if common.no_cache {
        return ScanCache::disabled();
    }
    let path = common
        .cache
        .clone()
        .unwrap_or_else(|| default_cache_path(root));
    ScanCache::open(&path, &config_fingerprint(config_text))
}

/// Helper reused by apply for selection resolution context.
pub fn resolve_selection(common: &super::CommonArgs) -> Result<Selection> {
    common.selection()
}
