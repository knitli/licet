//! Shared scan pipeline: walk → rules → detect → classify, producing per-file states.
//! Used by both `check` and `apply` (FR-013 honors the selected subset while precedence
//! still considers the full ruleset).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::detect;
use crate::domain::{ActualLicenseState, DriftClass, FileLicensingState, Precedence};
use crate::error::Result;
use crate::report::Diagnostic;
use crate::report::classify::{ClassifyInput, classify};
use crate::reuse::oob::OutOfBand;
use crate::rules::{Match, RuleSet};
use crate::walk::{Discovered, Snapshot};
use crate::{config::LicensingConfiguration, spdx};

/// Context for a scan: repository root, validated config, the content snapshot
/// every read observes, and whether declaration `[exclude]` rules apply
/// (`lint` validates REUSE coverage and never applies them). Scans are
/// stateless: every run classifies from current bytes, with no cache.
pub struct Engine<'a> {
    pub root: PathBuf,
    pub config: &'a LicensingConfiguration,
    pub snapshot: Snapshot,
    pub honor_declaration_excludes: bool,
}

/// Result of a scan: classified states plus any warnings collected along the way.
pub struct ScanResult {
    pub states: Vec<FileLicensingState>,
    pub warnings: Vec<Diagnostic>,
    /// Every license identifier referenced by config or detected in files (for inventory).
    pub referenced_ids: BTreeSet<String>,
    /// Identifiers effectively present in evaluated files (file headers,
    /// sidecars, OOB contributions, and snippet expressions) — the REUSE
    /// actual inventory. Suppressed values and unused config rules are
    /// excluded.
    pub actual_referenced_ids: BTreeSet<String>,
    /// Identifiers declared for evaluated files (winning intents only, never
    /// unmatched rules) — the policy desired set.
    pub desired_referenced_ids: BTreeSet<String>,
}

impl<'a> Engine<'a> {
    pub fn new(
        root: PathBuf,
        config: &'a LicensingConfiguration,
        snapshot: Snapshot,
        honor_declaration_excludes: bool,
    ) -> Self {
        Engine {
            root,
            config,
            snapshot,
            honor_declaration_excludes,
        }
    }

    /// Scan the prepared files, classifying each from current bytes.
    pub fn scan(&self, paths: &[Discovered]) -> Result<ScanResult> {
        let rules = RuleSet::new(self.config);
        let oob = OutOfBand::load_snapshot(&self.snapshot)?;
        let excludes = if self.honor_declaration_excludes {
            Some(crate::walk::build_excludes(&self.config.exclude)?)
        } else {
            None
        };

        // Parallel detect + classify over the selected files only.
        let mut results: Vec<(FileLicensingState, Option<Diagnostic>)> = paths
            .par_iter()
            .map(|d| self.classify_one(d, &rules, &oob, excludes.as_ref()))
            .collect();

        // Orphan sidecars (a `.license` file whose companion is absent from the
        // snapshot) are diagnosed, never silently evaluated or ignored.
        let mut warnings = Vec::new();
        for d in paths {
            if !d
                .rel_path
                .extension()
                .map(|e| e == "license")
                .unwrap_or(false)
            {
                continue;
            }
            let companion = companion_of(&d.rel_path);
            let absent = self
                .snapshot
                .read(&companion)
                .map(|b| b.is_none())
                .unwrap_or(false);
            if absent {
                warnings.push(Diagnostic {
                    code: "orphan_sidecar".to_string(),
                    path: Some(d.rel_path.to_string_lossy().replace('\\', "/")),
                    message: format!(
                        "sidecar has no companion file {} in the evaluated snapshot",
                        companion.display()
                    ),
                });
            }
        }

        // Collect warnings and referenced ids.
        let mut referenced_ids = self.config_referenced_ids();
        let mut actual_referenced_ids = BTreeSet::new();
        let mut desired_referenced_ids = BTreeSet::new();
        let mut states = Vec::with_capacity(results.len());
        for (state, read_warning) in results.drain(..) {
            if let Some(w) = read_warning {
                warnings.push(w);
            }
            // A copyright mismatch next to license drift stays visible as its
            // own diagnostic instead of disappearing into `wrong_license`.
            if let Some(cd) = &state.copyright_drift
                && matches!(state.drift, DriftClass::WrongLicense { .. })
            {
                warnings.push(Diagnostic {
                    code: "copyright_mismatch".to_string(),
                    path: Some(state.path.to_string_lossy().replace('\\', "/")),
                    message: format!(
                        "copyright policy requires `{}` but found {}",
                        cd.declared, cd.actual
                    ),
                });
            }
            // Conflict / source-override warnings.
            if let Some(conf) = &state.conflict {
                warnings.push(Diagnostic {
                    code: "rule_conflict".to_string(),
                    path: Some(state.path.to_string_lossy().replace('\\', "/")),
                    message: conf.message.clone(),
                });
            }
            // An override-precedence annotation is the only case where the in-file header
            // is actually suppressed; only then is the disagreement a "source override"
            // (FR-003a). Under `closest`/`aggregate` the header is not overridden.
            if state
                .actual
                .out_of_band
                .as_ref()
                .is_some_and(|o| o.precedence == Precedence::Override)
                && has_header_disagreement(&state.actual)
            {
                warnings.push(Diagnostic {
                    code: "source_override".to_string(),
                    path: Some(state.path.to_string_lossy().replace('\\', "/")),
                    message: "in-file header disagrees with out-of-band metadata; out-of-band entry has precedence = override".to_string(),
                });
            }
            if matches!(state.drift, DriftClass::Unreadable) {
                warnings.push(Diagnostic {
                    code: "encoding_skipped".to_string(),
                    path: Some(state.path.to_string_lossy().replace('\\', "/")),
                    message: "file is not valid UTF-8; skipped and counted as failure".to_string(),
                });
            }
            // Every effective file expression counts for text inventory —
            // including aggregate/fallback OOB contributions, not just the
            // primary presentation value (FR-003a, FR-030).
            for l in crate::detect::candidate_licenses(&state.actual) {
                collect_ids(&l, &mut referenced_ids);
                collect_ids(&l, &mut actual_referenced_ids);
            }
            // Declared intent of evaluated files only: unmatched rules never
            // inflate the desired set, and excluded files are out of scope.
            if !matches!(state.drift, DriftClass::Excluded)
                && let Some(intent) = &state.declared_intent
            {
                collect_ids(&intent.license_expression, &mut referenced_ids);
                collect_ids(&intent.license_expression, &mut desired_referenced_ids);
            }
            // SPDX-snippet licenses are not the file's license, but their texts must still
            // exist under LICENSES/ for REUSE compliance (FR-030).
            for s in &state.actual.snippet_licenses {
                collect_ids(s, &mut referenced_ids);
                collect_ids(s, &mut actual_referenced_ids);
            }
            states.push(state);
        }

        Ok(ScanResult {
            states,
            warnings,
            referenced_ids,
            actual_referenced_ids,
            desired_referenced_ids,
        })
    }

    /// Detect + classify a single discovered file. Snapshot read failures become
    /// an `Unreadable` state with a `read_error` warning carrying path + reason
    /// (F13) — never silent absence, never missing-header drift.
    fn classify_one(
        &self,
        d: &Discovered,
        rules: &RuleSet<'a>,
        oob: &OutOfBand,
        excludes: Option<&globset::GlobSet>,
    ) -> (FileLicensingState, Option<Diagnostic>) {
        let rel = &d.rel_path;

        // Resolve the rule (cheap, no IO) — done for every file so excluded files still
        // carry their matched rule for `--explain`.
        let resolved = rules.resolve(rel);
        let (matched_rule, declared, conflict) = match resolved {
            Match::Rule(r) => (Some(r.label()), Some(r.intent.clone()), None),
            Match::Default(d) => (None, d.cloned(), None),
            Match::Conflict(c) => (None, None, Some(c)),
        };

        // Declaration exclusions apply to policy scans only; REUSE ignores always apply.
        let declaration_excluded = excludes
            .map(|set| set.is_match(rel.to_string_lossy().replace('\\', "/")))
            .unwrap_or(false);
        if d.reuse_ignored || declaration_excluded {
            let state = classify(ClassifyInput {
                path: rel.clone(),
                matched_rule,
                declared: declared.as_ref(),
                actual: ActualLicenseState::default(),
                conflict,
                excluded: true,
            });
            return (state, None);
        }

        // Read through the snapshot (head + any `.license` sidecar) and detect.
        // Any read failure becomes Unreadable with a path-attached reason.
        let unreadable = |message: String| {
            let warning = Diagnostic {
                code: "read_error".to_string(),
                path: Some(rel.to_string_lossy().replace('\\', "/")),
                message,
            };
            let state = classify(ClassifyInput {
                path: rel.clone(),
                matched_rule: matched_rule.clone(),
                declared: declared.as_ref(),
                actual: ActualLicenseState {
                    encoding_ok: false,
                    ..Default::default()
                },
                conflict: conflict.clone(),
                excluded: false,
            });
            (state, Some(warning))
        };
        let bytes = match self.snapshot.read(rel) {
            Ok(b) => b,
            Err(e) => return unreadable(format!("cannot read file for evaluation: {e}")),
        };
        let sidecar_rel = crate::walk::git::sidecar_for(rel);
        let sidecar_bytes = match self.snapshot.read(&sidecar_rel) {
            Ok(b) => b,
            Err(e) => {
                return unreadable(format!(
                    "cannot read sidecar {}: {e}",
                    sidecar_rel.display()
                ));
            }
        };
        // Complete content is scanned (no head cutoff — task 3).
        let full = bytes.as_deref().unwrap_or_default();
        let sidecar_opt = sidecar_bytes.as_deref();
        let actual = detect::detect(rel, full, sidecar_opt, oob);

        // Rejected license values are diagnosed in-band with line + value.
        let invalid_warning = if actual.invalid_license_values.is_empty() {
            None
        } else {
            let details = actual
                .invalid_license_values
                .iter()
                .map(|v| format!("line {}: `{}` ({})", v.line, v.value, v.reason))
                .collect::<Vec<_>>()
                .join("; ");
            Some(Diagnostic {
                code: "invalid_license".to_string(),
                path: Some(rel.to_string_lossy().replace('\\', "/")),
                message: format!("invalid SPDX-License-Identifier value: {details}"),
            })
        };
        let state = classify(ClassifyInput {
            path: rel.clone(),
            matched_rule: matched_rule.clone(),
            declared: declared.as_ref(),
            actual,
            conflict: conflict.clone(),
            excluded: false,
        });
        (state, invalid_warning)
    }

    /// Identifiers referenced by config (`default` + rules).
    fn config_referenced_ids(&self) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        if let Some(d) = &self.config.default {
            collect_ids(&d.license_expression, &mut ids);
        }
        for r in &self.config.rules {
            collect_ids(&r.intent.license_expression, &mut ids);
        }
        ids
    }
}

/// True when in-file headers and suppressing out-of-band metadata disagree on
/// the license (an `override` barrier makes the header ineffective).
fn has_header_disagreement(actual: &ActualLicenseState) -> bool {
    let Some(o) = actual.out_of_band.as_ref() else {
        return false;
    };
    if !o.suppresses_file {
        return false;
    }
    let header_lics: Vec<String> = actual
        .headers
        .iter()
        .flat_map(|h| h.license_ids.clone())
        .collect();
    match (
        crate::domain::combine_licenses(&header_lics),
        crate::domain::combine_licenses(&o.licenses),
    ) {
        (Some(h), Some(ol)) => !spdx::expressions_equal(&ol, &h),
        _ => false,
    }
}

/// Split an SPDX expression into its constituent identifiers and collect them
/// (AST-based, so tabs and casing variants split correctly).
pub(crate) fn collect_ids(expr: &str, out: &mut BTreeSet<String>) {
    for id in crate::spdx::expression_ids(expr) {
        out.insert(id);
    }
}

/// Strip a trailing `.license` sidecar suffix to get the companion asset path.
fn companion_of(sidecar: &Path) -> PathBuf {
    let name = sidecar
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    match name.and_then(|n| n.strip_suffix(".license").map(str::to_string)) {
        Some(base) => sidecar.with_file_name(base),
        None => sidecar.to_path_buf(),
    }
}
