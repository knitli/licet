//! `init` — derive a config from current repository state (FR-018; US5).
//!
//! Inspects existing headers and `REUSE.toml`/`.reuse/dep5`, then emits a `licet.toml`
//! whose projection reproduces current licensing. Preservation outranks brevity:
//! every observed file first becomes an exact-path rule, and rules compress to an
//! extension group only when every observed path they would match carries the same
//! license. A `[default]` is emitted only when every covered file is known, with exact
//! exceptions for the rest; previously unknown files remain unknown. The generated
//! config is validated against every observation with the real rule resolver before
//! anything is written. Modifies no source files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{Format, InitArgs};
use crate::config::{
    CONFIG_FILENAME, LicensingConfiguration, has_legacy_only_config, legacy_config_error,
};
use crate::detect;
use crate::error::{ExitCode, LicetError, Result};
use crate::report::{Counts, Diagnostic, Report, Summary, WriteEntry};
use crate::reuse::oob::OutOfBand;
use crate::rules::{Match, RuleSet};
use crate::walk::{self, Purpose, Selection, discover_root};

/// One observed coverable file: repo-relative path plus its effective license
/// (`None` = unknown — unlicensed, unreadable, or unrepresentable here).
struct Observation {
    path: PathBuf,
    license: Option<String>,
}

pub fn run(args: InitArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let (root, _) = discover_root(&cwd)?;

    // Destination: `--output`, else explicit `--config`, else `<root>/licet.toml`.
    // Explicit relative paths resolve from the invocation cwd.
    let explicit = args.output.clone().or(args.config.clone());
    let abs_out = match &explicit {
        Some(p) if p.is_absolute() => p.clone(),
        Some(p) => cwd.join(p),
        None => root.join(CONFIG_FILENAME),
    };
    // Never write a competing default next to a legacy config: the legacy
    // file would be silently ignored from then on. Rename first.
    if explicit.is_none() && has_legacy_only_config(&root) {
        return Err(legacy_config_error(&root));
    }

    // Init observes working-tree state; declaration excludes do not apply to
    // observation (there is no config yet to declare them).
    let prep = walk::prepare(
        &cwd,
        Path::new(CONFIG_FILENAME),
        &Selection::FullTree,
        Purpose::Lint,
        true,
    )?;
    let oob = OutOfBand::load_snapshot(&prep.snapshot)?;

    // The artifact being written is not an observation: with `--force` it
    // exists, but its bytes are the old policy, not licensable content.
    let skip_rel = abs_out
        .strip_prefix(&prep.root)
        .ok()
        .map(|p| p.to_path_buf());

    let mut observations: Vec<Observation> = Vec::new();
    for d in &prep.paths {
        if d.reuse_ignored {
            continue;
        }
        if let Some(skip) = &skip_rel
            && *skip == d.rel_path
        {
            continue;
        }
        let full = match prep.snapshot.read(&d.rel_path) {
            Ok(Some(bytes)) => bytes,
            // Unreadable files cannot be observed; they stay explicitly unknown.
            _ => {
                observations.push(Observation {
                    path: d.rel_path.clone(),
                    license: None,
                });
                continue;
            }
        };
        let sidecar_rel = crate::walk::git::sidecar_for(&d.rel_path);
        let sidecar = prep.snapshot.read(&sidecar_rel).ok().flatten();
        let actual = detect::detect(&d.rel_path, &full, sidecar.as_deref(), &oob);
        observations.push(Observation {
            path: d.rel_path.clone(),
            license: actual.detected_license,
        });
    }
    observations.sort_by(|a, b| a.path.cmp(&b.path));

    let generated = generate(&observations)?;
    let toml = render_config(&generated);

    // Validate the generated config with the real loader and resolver before
    // writing: every known license must project back exactly, and every
    // unknown file must remain uncovered.
    let parsed = LicensingConfiguration::from_toml(&toml)
        .map_err(|e| LicetError::Internal(format!("init generated an invalid config: {e}")))?;
    verify_projection(&parsed, &observations)?;

    // Create-new is the default: an existing destination (file or symlink) is
    // refused unless `--force` replaces exactly the bytes just observed.
    let expected = crate::reuse::read_expected_for_write(&abs_out)
        .map_err(|e| LicetError::Config(format!("cannot read `{}`: {e}", abs_out.display())))?;
    if expected.is_some() && !args.force {
        return Err(LicetError::Config(format!(
            "refusing to overwrite existing `{}` without --force",
            abs_out.display()
        )));
    }

    let display = display_path(&prep.root, &abs_out);
    let (status, message, exit) = match write_config(&abs_out, expected.as_deref(), &toml) {
        Ok(()) => ("applied".to_string(), None, ExitCode::Success),
        Err(e) => (
            "failed".to_string(),
            Some(e.to_string()),
            ExitCode::Violations,
        ),
    };

    let known_paths: Vec<String> = observations
        .iter()
        .filter(|o| o.license.is_some())
        .map(|o| slash(&o.path))
        .collect();
    let unknown_paths: Vec<String> = observations
        .iter()
        .filter(|o| o.license.is_none())
        .map(|o| slash(&o.path))
        .collect();
    let diagnostics: Vec<Diagnostic> = unknown_paths
        .iter()
        .map(|p| Diagnostic {
            code: "missing_license".to_string(),
            path: Some(p.clone()),
            message: "no licensing metadata observed; left uncovered by the generated config"
                .to_string(),
        })
        .collect();
    let report = Report {
        version: 2,
        command: "init".to_string(),
        exit_code: Some(exit.code()),
        snapshot: "worktree".to_string(),
        summary: Summary {
            pass: exit == ExitCode::Success,
            partial: None,
            complete: true,
            before_pass: None,
            projected_pass: None,
            counts: Counts::default(),
        },
        files: Vec::new(),
        diagnostics,
        writes: vec![WriteEntry {
            path: display.clone(),
            kind: "config".to_string(),
            status,
            affected_files: known_paths,
            before_text: None,
            after_text: Some(toml.clone()),
            message,
        }],
        license_texts: None,
    };

    match args.format {
        Format::Json => super::emit_stdout(&format!("{}\n", report.to_json()))?,
        Format::Human => {
            if exit == ExitCode::Success {
                eprintln!(
                    "Wrote {} ({} rule(s) inferred, {} known, {} unknown).",
                    display,
                    generated.rules.len(),
                    observations.len() - unknown_paths.len(),
                    unknown_paths.len(),
                );
                for u in &unknown_paths {
                    eprintln!("unknown: {u}");
                }
            } else {
                eprintln!("init failed: {}", message_of(&report));
                // Never print a config that was not written: stdout stays
                // empty so callers cannot mistake it for the new policy.
                return Ok(exit);
            }
            super::emit_stdout(&toml)?;
        }
    }
    Ok(exit)
}

fn message_of(report: &Report) -> String {
    report
        .writes
        .first()
        .and_then(|w| w.message.clone())
        .unwrap_or_default()
}

/// A generated `[[rule]]`: exactly one selector key plus the observed license.
/// Copyright is always `preserve` (never a guessed holder), which is the
/// loader default, so it is not serialized.
struct GeneratedRule {
    ext: Option<String>,
    file: Option<String>,
    license: String,
}

struct GeneratedConfig {
    default: Option<String>,
    rules: Vec<GeneratedRule>,
}

/// Exact-path rules first; compress to extension rules only for provably
/// uniform groups; default only when every covered file is known.
fn generate(observations: &[Observation]) -> Result<GeneratedConfig> {
    let (mut rules, ext_rule_for) = compressed_ext_rules(observations);
    // …exact rules for everything they do not cover.
    for o in observations {
        let Some(lic) = &o.license else { continue };
        let covered_by_ext = o
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| ext_rule_for.contains(&e.to_ascii_lowercase()))
            .unwrap_or(false);
        if covered_by_ext {
            continue;
        }
        rules.push(GeneratedRule {
            ext: None,
            file: Some(exact_value(&o.path)),
            license: lic.clone(),
        });
    }

    // Default only when every covered file is known: the most common license,
    // keeping exact/ext exceptions for the rest.
    let unknowns = observations.iter().filter(|o| o.license.is_none()).count();
    let mut default: Option<String> = None;
    if unknowns == 0 && !observations.is_empty() {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for o in observations {
            *counts
                .entry(o.license.as_deref().unwrap_or(""))
                .or_default() += 1;
        }
        let top = counts
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(l, _)| (*l).to_string());
        if let Some(top_lic) = top {
            rules.retain(|r| !crate::spdx::expressions_equal(&r.license, &top_lic));
            // A default with no exceptions that covers a single file each is
            // still a default; but a default equal to nothing observed is
            // pointless — `top` always names an observed license, so keep it.
            default = Some(top_lic);
        }
    }
    // Deterministic order: ext rules by extension, then exact rules by path.
    rules.sort_by(|a, b| {
        let key = |r: &GeneratedRule| {
            (
                r.ext.is_none(),
                r.ext.clone().unwrap_or_default(),
                r.file.clone().unwrap_or_default(),
            )
        };
        key(a).cmp(&key(b))
    });
    Ok(GeneratedConfig { default, rules })
}

/// The `file` selector value for an observed path: paths containing `/` are
/// exact already; a root-level file is emitted as `./name` so the loader pins
/// it as an [`ExactPath`](crate::domain::Selector::ExactPath) instead of a
/// directory-spanning filename match.
/// Compress uniform extension groups into ext rules: lowercase ext hands
/// back the emitted rules plus the set of extensions they cover (exact
/// rules skip those). An ext group compresses only when every observed path
/// it would match carries the same license — i.e. no unknown file shares
/// the extension and all known ones agree.
fn compressed_ext_rules(
    observations: &[Observation],
) -> (Vec<GeneratedRule>, std::collections::BTreeSet<String>) {
    // Extension groups: lowercase ext -> (emitted spelling, licenses, paths).
    let mut ext_groups: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    let mut ext_of_unknown: std::collections::BTreeSet<String> = Default::default();
    for o in observations {
        let Some(ext) = o.path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let key = ext.to_ascii_lowercase();
        match &o.license {
            Some(lic) => {
                ext_groups
                    .entry(key)
                    .or_insert_with(|| (ext.to_string(), Vec::new()))
                    .1
                    .push(lic.clone());
            }
            None => {
                ext_of_unknown.insert(key);
            }
        }
    }

    let mut rules: Vec<GeneratedRule> = Vec::new();
    let mut ext_rule_for: std::collections::BTreeSet<String> = Default::default();
    for (key, (spelling, licenses)) in &ext_groups {
        if ext_of_unknown.contains(key) {
            continue;
        }
        let mut iter = licenses.iter();
        let first = iter.next().expect("group is nonempty");
        if iter.all(|l| crate::spdx::expressions_equal(l, first)) {
            rules.push(GeneratedRule {
                ext: Some(spelling.clone()),
                file: None,
                license: first.clone(),
            });
            ext_rule_for.insert(key.clone());
        }
    }
    (rules, ext_rule_for)
}

fn exact_value(rel: &Path) -> String {
    let s = slash(rel);
    if s.contains('/') { s } else { format!("./{s}") }
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Re-resolve every observation through the real rule set: known licenses
/// must come back equal, unknown files must stay uncovered.
fn verify_projection(config: &LicensingConfiguration, observations: &[Observation]) -> Result<()> {
    let set = RuleSet::new(config);
    for o in observations {
        match (&o.license, set.resolve(&o.path)) {
            (Some(expected), Match::Rule(r)) => {
                if !crate::spdx::expressions_equal(&r.intent.license_expression, expected) {
                    return Err(LicetError::Internal(format!(
                        "init projection mismatch for `{}`: observed `{expected}` but generated config resolves `{}`",
                        slash(&o.path),
                        r.intent.license_expression
                    )));
                }
            }
            (Some(expected), Match::Default(Some(intent))) => {
                if !crate::spdx::expressions_equal(&intent.license_expression, expected) {
                    return Err(LicetError::Internal(format!(
                        "init projection mismatch for `{}`: observed `{expected}` but generated default resolves `{}`",
                        slash(&o.path),
                        intent.license_expression
                    )));
                }
            }
            (None, Match::Default(None)) => {}
            (Some(_), Match::Default(None)) => {
                return Err(LicetError::Internal(format!(
                    "init left known file `{}` uncovered",
                    slash(&o.path)
                )));
            }
            (Some(_), Match::Conflict(c)) => {
                return Err(LicetError::Internal(format!(
                    "init generated conflicting rules for `{}`: {}",
                    slash(&o.path),
                    c.message
                )));
            }
            (None, Match::Rule(r)) => {
                return Err(LicetError::Internal(format!(
                    "init covers previously unknown file `{}` via `{}`",
                    slash(&o.path),
                    r.label()
                )));
            }
            (None, Match::Default(Some(intent))) => {
                return Err(LicetError::Internal(format!(
                    "init default covers previously unknown file `{}` (`{}`)",
                    slash(&o.path),
                    intent.license_expression
                )));
            }
            (None, Match::Conflict(c)) => {
                return Err(LicetError::Internal(format!(
                    "init generated conflicting rules for unknown file `{}`: {}",
                    slash(&o.path),
                    c.message
                )));
            }
        }
    }
    Ok(())
}

/// Write through the shared safe writer: the destination's parent is the
/// allowed root, so an explicit `--output` elsewhere authorizes only that
/// particular config destination.
fn write_config(
    abs_out: &Path,
    expected: Option<&[u8]>,
    toml: &str,
) -> std::result::Result<(), crate::reuse::WriteError> {
    let bad = |msg: String| crate::reuse::WriteError {
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, msg),
        replacement_completed: false,
    };
    let parent = abs_out.parent().ok_or_else(|| {
        bad(format!(
            "cannot determine parent of output {}",
            abs_out.display()
        ))
    })?;
    let file_name = abs_out
        .file_name()
        .ok_or_else(|| bad(format!("output {} names no file", abs_out.display())))?;
    crate::reuse::atomic_write(parent, Path::new(file_name), expected, toml.as_bytes())
}

fn display_path(root: &Path, abs: &Path) -> String {
    match abs.strip_prefix(root) {
        Ok(rel) => slash(rel),
        Err(_) => abs.to_string_lossy().into_owned(),
    }
}

/// Serializable shape of the generated `licet.toml`. Emitted via the `toml` crate so any
/// license id, extension, or path is correctly quoted/escaped — hand-rolled `"{d}"`
/// interpolation previously produced invalid TOML for values containing quotes,
/// backslashes, or newlines.
#[derive(serde::Serialize)]
struct RenderedConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<DefaultSection>,
    #[serde(rename = "rule", skip_serializing_if = "Vec::is_empty")]
    rules: Vec<RuleSection>,
}

#[derive(serde::Serialize)]
struct DefaultSection {
    license: String,
}

#[derive(serde::Serialize)]
struct RuleSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    ext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    license: String,
}

fn render_config(generated: &GeneratedConfig) -> String {
    let config = RenderedConfig {
        default: generated
            .default
            .as_ref()
            .map(|d| DefaultSection { license: d.clone() }),
        rules: generated
            .rules
            .iter()
            .map(|r| RuleSection {
                ext: r.ext.clone(),
                file: r.file.clone(),
                license: r.license.clone(),
            })
            .collect(),
    };
    // This flat, string-only shape always serializes; fall back to an empty body rather
    // than panicking if that ever changes.
    let body = toml::to_string(&config).unwrap_or_default();
    format!("# Generated by `licet init` from existing repository state.\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_compression_needs_uniform_known_licenses() {
        // Uniform known licenses compress to one ext rule; a disagreeing
        // license or an unknown file sharing the extension blocks it (those
        // observations fall through to exact rules).
        let obs = |path: &str, license: Option<&str>| Observation {
            path: PathBuf::from(path),
            license: license.map(str::to_string),
        };
        let observations = vec![
            obs("a.rs", Some("MIT")),
            obs("sub/b.rs", Some("MIT")),
            obs("c.py", Some("MIT")),
            obs("d.py", Some("Apache-2.0")),
            obs("e.md", None),
            obs("f.md", Some("MIT")),
            obs("Makefile", Some("MIT")),
        ];
        let (rules, covered) = compressed_ext_rules(&observations);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].ext.as_deref(), Some("rs"));
        assert_eq!(rules[0].license, "MIT");
        assert_eq!(
            covered,
            std::collections::BTreeSet::from(["rs".to_string()])
        );
    }

    #[test]
    fn generated_toml_is_well_formed_for_adversarial_values() {
        // Adversarial values (quotes, backslashes, newlines) must be escaped, not
        // interpolated raw — the original `format!("license = \"{d}\"")` emitted invalid
        // TOML so `init` crashed with a *TOML parse error* validating its own output.
        // Escaping reduces that to, at worst, a clear semantic SPDX error from the loader.
        let toml = render_config(&GeneratedConfig {
            default: Some("MIT\nx=1\n\")".to_string()),
            rules: vec![GeneratedRule {
                ext: Some("r\"s".to_string()),
                file: None,
                license: "Apache-2.0 OR \"GPL\"".to_string(),
            }],
        });
        toml::from_str::<toml::Value>(&toml)
            .expect("generated output must be syntactically valid TOML");
    }

    #[test]
    fn render_is_well_formed_for_ordinary_input() {
        let toml = render_config(&GeneratedConfig {
            default: Some("MIT".to_string()),
            rules: vec![GeneratedRule {
                ext: Some("rs".to_string()),
                file: None,
                license: "Apache-2.0".to_string(),
            }],
        });
        assert!(
            toml.contains("[default]") && toml.contains("license = \"MIT\""),
            "{toml}"
        );
        assert!(
            toml.contains("[[rule]]") && toml.contains("ext = \"rs\""),
            "{toml}"
        );
        LicensingConfiguration::from_toml(&toml).expect("ordinary config must parse");
    }

    #[test]
    fn root_file_exact_value_is_dot_prefixed() {
        assert_eq!(exact_value(Path::new("Makefile")), "./Makefile");
        assert_eq!(exact_value(Path::new("sub/f.rs")), "sub/f.rs");
    }

    #[test]
    fn heterogeneous_ext_group_keeps_exact_rules_and_no_default() {
        let obs = vec![
            Observation {
                path: PathBuf::from("a.rs"),
                license: Some("MIT".to_string()),
            },
            Observation {
                path: PathBuf::from("b.rs"),
                license: Some("Apache-2.0".to_string()),
            },
            Observation {
                path: PathBuf::from("notes.txt"),
                license: None,
            },
        ];
        let gen_cfg = generate(&obs).unwrap();
        assert!(gen_cfg.default.is_none(), "unknowns present → no default");
        assert!(
            gen_cfg.rules.iter().all(|r| r.ext.is_none()),
            "heterogeneous ext group must not compress: {:?}",
            gen_cfg.rules.iter().map(|r| &r.license).collect::<Vec<_>>()
        );
        assert_eq!(gen_cfg.rules.len(), 2);
    }

    #[test]
    fn uniform_ext_group_compresses_and_unknown_ext_blocks() {
        let obs = vec![
            Observation {
                path: PathBuf::from("a.py"),
                license: Some("MIT".to_string()),
            },
            Observation {
                path: PathBuf::from("b.py"),
                license: Some("MIT".to_string()),
            },
            Observation {
                path: PathBuf::from("c.js"),
                license: None,
            },
            Observation {
                path: PathBuf::from("d.js"),
                license: Some("MIT".to_string()),
            },
        ];
        let gen_cfg = generate(&obs).unwrap();
        assert!(gen_cfg.default.is_none());
        assert!(
            gen_cfg
                .rules
                .iter()
                .any(|r| r.ext.as_deref() == Some("py") && r.license == "MIT"),
            "uniform py group compresses"
        );
        assert!(
            gen_cfg
                .rules
                .iter()
                .any(|r| r.file.as_deref() == Some("./d.js")),
            "js group has an unknown sibling → d.js stays exact"
        );
    }

    #[test]
    fn all_known_single_license_becomes_default() {
        let obs = vec![
            Observation {
                path: PathBuf::from("a.py"),
                license: Some("MIT".to_string()),
            },
            Observation {
                path: PathBuf::from("b.py"),
                license: Some("MIT".to_string()),
            },
        ];
        let gen_cfg = generate(&obs).unwrap();
        assert_eq!(gen_cfg.default.as_deref(), Some("MIT"));
        assert!(gen_cfg.rules.is_empty());
    }
}
