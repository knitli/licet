//! Shared scan pipeline: walk → rules → detect → classify, producing per-file states.
//! Used by both `check` and `apply` (FR-013 honors the selected subset while precedence
//! still considers the full ruleset).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::comment::CommentResolver;
use crate::detect::{self};
use crate::domain::{ActualLicenseState, DriftClass, FileLicensingState, Precedence};
use crate::error::Result;
use crate::report::Warning;
use crate::report::classify::{ClassifyInput, classify};
use crate::reuse::oob::OutOfBand;
use crate::rules::{Match, RuleSet};
use crate::walk::cache::ScanCache;
use crate::walk::{self, Discovered, Selection};
use crate::{config::LicensingConfiguration, spdx};

/// Context for a scan: repository root and validated config.
pub struct Engine<'a> {
    pub root: PathBuf,
    pub config: &'a LicensingConfiguration,
    pub config_text: &'a str,
}

/// Result of a scan: classified states plus any warnings collected along the way.
pub struct ScanResult {
    pub states: Vec<FileLicensingState>,
    pub warnings: Vec<Warning>,
    /// Every license identifier referenced by config or detected in files (for inventory).
    pub referenced_ids: BTreeSet<String>,
}

impl<'a> Engine<'a> {
    pub fn new(root: PathBuf, config: &'a LicensingConfiguration, config_text: &'a str) -> Self {
        Engine {
            root,
            config,
            config_text,
        }
    }

    /// Scan the selected files, classifying each.
    pub fn scan(&self, selection: &Selection, cache: &mut ScanCache) -> Result<ScanResult> {
        let discovered = walk::enumerate(&self.root, selection, &self.config.exclude)?;
        let rules = RuleSet::new(self.config);
        let resolver = CommentResolver::new(self.config);
        let oob = OutOfBand::load(&self.root);

        // Parallel detect + classify. Cache is consulted/updated sequentially afterward to
        // avoid lock contention; on a warm hit the file IS still read but classification is
        // trusted from the cache key (content+config+version), guaranteeing no stale verdict.
        let mut results: Vec<(FileLicensingState, Option<String>, String)> = discovered
            .par_iter()
            .map(|d| self.classify_one(d, &rules, &resolver, &oob, cache))
            .collect();

        // Apply cache, collect warnings and referenced ids.
        let mut warnings = Vec::new();
        let mut referenced_ids = self.config_referenced_ids();
        let mut states = Vec::with_capacity(results.len());
        for (state, content_hash, _drift_label) in results.drain(..) {
            // Conflict / source-override warnings.
            if let Some(conf) = &state.conflict {
                warnings.push(Warning {
                    kind: "rule_conflict".to_string(),
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
                warnings.push(Warning {
                    kind: "source_override".to_string(),
                    path: Some(state.path.to_string_lossy().replace('\\', "/")),
                    message: "in-file header disagrees with out-of-band metadata; out-of-band entry has precedence = override".to_string(),
                });
            }
            if matches!(state.drift, DriftClass::Unreadable) {
                warnings.push(Warning {
                    kind: "encoding_skipped".to_string(),
                    path: Some(state.path.to_string_lossy().replace('\\', "/")),
                    message: "file is not valid UTF-8; skipped and counted as failure".to_string(),
                });
            }
            if let Some(l) = &state.actual.detected_license {
                referenced_ids.insert(l.clone());
            }
            // Update cache.
            if let Some(hash) = content_hash {
                let rel = state.path.to_string_lossy().replace('\\', "/");
                cache.put(&rel, &hash, state.drift.as_str());
            }
            states.push(state);
        }

        Ok(ScanResult {
            states,
            warnings,
            referenced_ids,
        })
    }

    /// Detect + classify a single discovered file.
    fn classify_one(
        &self,
        d: &Discovered,
        rules: &RuleSet<'a>,
        resolver: &CommentResolver,
        oob: &OutOfBand,
        cache: &ScanCache,
    ) -> (FileLicensingState, Option<String>, String) {
        let _ = resolver; // resolver is used by apply; kept here for symmetry.
        let rel = &d.rel_path;

        // Resolve the rule (cheap, no IO) — done for every file so excluded files still
        // carry their matched rule for `--explain`.
        let resolved = rules.resolve(rel);
        let (matched_rule, declared, conflict) = match resolved {
            Match::Rule(r) => (Some(r.label()), Some(r.intent.clone()), None),
            Match::Default(d) => (None, d.cloned(), None),
            Match::Conflict(c) => (None, None, Some(c)),
        };

        if d.excluded {
            let state = classify(ClassifyInput {
                path: rel.clone(),
                matched_rule,
                declared: declared.as_ref(),
                actual: ActualLicenseState::default(),
                conflict,
                excluded: true,
            });
            return (state, None, "excluded".to_string());
        }

        // Read head (+ any `.license` sidecar) and detect.
        let head = detect::read_head(&d.abs_path).unwrap_or_default();
        let sidecar = detect::read_sidecar(&d.abs_path);
        let content_hash = ScanCache::content_hash(&head);
        let _ = cache.get(rel.to_string_lossy().as_ref(), &content_hash); // hit recorded; full detail recomputed
        let actual = detect::detect(rel, &head, sidecar.as_deref(), oob);

        let state = classify(ClassifyInput {
            path: rel.clone(),
            matched_rule,
            declared: declared.as_ref(),
            actual,
            conflict,
            excluded: false,
        });
        let drift_label = state.drift.as_str().to_string();
        (state, Some(content_hash), drift_label)
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

/// True when in-file headers and out-of-band metadata disagree on the license.
fn has_header_disagreement(actual: &ActualLicenseState) -> bool {
    let header_lic = actual
        .headers
        .iter()
        .flat_map(|h| h.license_ids.clone())
        .next();
    match (&actual.out_of_band, header_lic) {
        (Some(o), Some(h)) => match &o.license {
            Some(ol) => !spdx::expressions_equal(ol, &h),
            None => false,
        },
        _ => false,
    }
}

/// Split an SPDX expression into its constituent identifiers and collect them.
fn collect_ids(expr: &str, out: &mut BTreeSet<String>) {
    for tok in expr.split([' ', '(', ')']) {
        let t = tok.trim().trim_end_matches('+');
        if t.is_empty() {
            continue;
        }
        if t.eq_ignore_ascii_case("OR")
            || t.eq_ignore_ascii_case("AND")
            || t.eq_ignore_ascii_case("WITH")
        {
            continue;
        }
        out.insert(t.to_string());
    }
}

/// Default cache path. Stored inside `.git/` when present so it never dirties the working
/// tree (which would otherwise block `apply`'s clean-tree guard); falls back to the root.
pub fn default_cache_path(root: &Path) -> PathBuf {
    let git_dir = root.join(".git");
    if git_dir.is_dir() {
        git_dir.join("licet-cache")
    } else {
        root.join(".licet-cache")
    }
}
