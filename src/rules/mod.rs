//! Rule matching with deterministic precedence and conflict surfacing (FR-002, FR-022).
//!
//! Specificity: exact path/filename > glob > extension > default. Among equal-specificity
//! matches on one file, differing intents surface a [`RuleConflict`] rather than silently
//! resolving; identical intents resolve to the earliest declaration order.

use std::path::Path;

use globset::{Glob, GlobMatcher};

use crate::config::{LicensingConfiguration, Rule};
use crate::domain::{LicenseIntent, RuleConflict, Selector};

/// Precompiled rule set for fast repeated matching.
pub struct RuleSet<'a> {
    rules: Vec<CompiledRule<'a>>,
    default: Option<&'a LicenseIntent>,
}

struct CompiledRule<'a> {
    rule: &'a Rule,
    glob: Option<GlobMatcher>,
}

/// Outcome of resolving a path against the rule set.
pub enum Match<'a> {
    /// A single winning rule.
    Rule(&'a Rule),
    /// No rule matched; the repo-wide default applies (if any).
    Default(Option<&'a LicenseIntent>),
    /// Equal-specificity rules with differing intent (FR-022).
    Conflict(RuleConflict),
}

impl<'a> RuleSet<'a> {
    /// Compile the rule set, precompiling glob matchers.
    pub fn new(config: &'a LicensingConfiguration) -> Self {
        let rules = config
            .rules
            .iter()
            .map(|rule| {
                let glob = match &rule.selector {
                    Selector::Glob(pat) => Glob::new(pat).ok().map(|g| g.compile_matcher()),
                    _ => None,
                };
                CompiledRule { rule, glob }
            })
            .collect();
        RuleSet {
            rules,
            default: config.default.as_ref(),
        }
    }

    /// Resolve which rule (or the default) governs `rel_path` (repo-relative).
    pub fn resolve(&self, rel_path: &Path) -> Match<'a> {
        let mut matches: Vec<&'a Rule> = self
            .rules
            .iter()
            .filter(|c| selector_matches(&c.rule.selector, c.glob.as_ref(), rel_path))
            .map(|c| c.rule)
            .collect();

        if matches.is_empty() {
            return Match::Default(self.default);
        }

        // Highest specificity wins.
        let top = matches
            .iter()
            .map(|r| r.selector.specificity())
            .max()
            .unwrap();
        matches.retain(|r| r.selector.specificity() == top);
        // Deterministic order by declaration position.
        matches.sort_by_key(|r| r.source_order);

        // Among equal-specificity matches, differing full intent (license or
        // copyright policy) is a conflict; identical intent resolves to the
        // earliest declaration order.
        let first = matches[0];
        let differing = matches
            .iter()
            .any(|r| !crate::domain::intents_equal(&r.intent, &first.intent));
        if matches.len() > 1 && differing {
            let labels: Vec<String> = matches.iter().map(|r| r.label()).collect();
            return Match::Conflict(RuleConflict {
                message: format!(
                    "{} rules of equal specificity match with differing intent: {}",
                    matches.len(),
                    labels.join(", ")
                ),
                rules: labels,
            });
        }

        Match::Rule(first)
    }
}

/// Every rule whose selector matches `rel_path`, most specific first (ties by
/// earliest declaration). Powers `--explain` losing-rule reporting; the
/// winning pick itself stays in [`RuleSet::resolve`].
pub fn matching_rules<'a>(
    config: &'a LicensingConfiguration,
    rel_path: &Path,
) -> Vec<(&'a Rule, u32)> {
    let mut out: Vec<(&'a Rule, u32)> = config
        .rules
        .iter()
        .filter(|rule| {
            let glob = match &rule.selector {
                Selector::Glob(pat) => Glob::new(pat).ok().map(|g| g.compile_matcher()),
                _ => None,
            };
            selector_matches(&rule.selector, glob.as_ref(), rel_path)
        })
        .map(|rule| (rule, rule.selector.specificity()))
        .collect();
    out.sort_by_key(|(rule, spec)| (u32::MAX - spec, rule.source_order));
    out
}

/// Does a single selector match a repo-relative path?
fn selector_matches(selector: &Selector, glob: Option<&GlobMatcher>, rel_path: &Path) -> bool {
    match selector {
        Selector::Extension(ext) => rel_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case(ext))
            .unwrap_or(false),
        Selector::Glob(_) => glob.map(|g| g.is_match(rel_path)).unwrap_or(false),
        Selector::Filename(name) => rel_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == name)
            .unwrap_or(false),
        Selector::ExactPath(p) => {
            let norm = rel_path.to_string_lossy().replace('\\', "/");
            norm == *p
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LicensingConfiguration;
    use std::path::PathBuf;

    fn cfg(toml: &str) -> LicensingConfiguration {
        LicensingConfiguration::from_toml(toml).unwrap()
    }

    #[test]
    fn glob_beats_extension() {
        let c = cfg("[[rule]]\next=\"rs\"\nlicense=\"LicenseRef-Marque-1.0\"\n\
             [[rule]]\nglob=\"examples/**/*.rs\"\nlicense=\"MIT OR Apache-2.0\"\n");
        let rs = RuleSet::new(&c);
        match rs.resolve(&PathBuf::from("examples/demo.rs")) {
            Match::Rule(r) => assert_eq!(r.intent.license_expression, "MIT OR Apache-2.0"),
            _ => panic!("expected glob rule to win"),
        }
    }

    #[test]
    fn extension_matches_when_no_glob() {
        let c = cfg("[[rule]]\next=\"rs\"\nlicense=\"LicenseRef-Marque-1.0\"\n");
        let rs = RuleSet::new(&c);
        match rs.resolve(&PathBuf::from("src/lib.rs")) {
            Match::Rule(r) => assert_eq!(r.intent.license_expression, "LicenseRef-Marque-1.0"),
            _ => panic!("expected ext rule"),
        }
    }

    #[test]
    fn no_match_falls_to_default() {
        let c = cfg("[default]\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"MIT\"\n");
        let rs = RuleSet::new(&c);
        match rs.resolve(&PathBuf::from("README.md")) {
            Match::Default(Some(i)) => assert_eq!(i.license_expression, "MIT"),
            _ => panic!("expected default"),
        }
    }

    #[test]
    fn equal_specificity_differing_is_conflict() {
        let c = cfg("[[rule]]\nglob=\"examples/**\"\nlicense=\"MIT\"\n\
             [[rule]]\nglob=\"**/*.rs\"\nlicense=\"Apache-2.0\"\n");
        let rs = RuleSet::new(&c);
        match rs.resolve(&PathBuf::from("examples/demo.rs")) {
            Match::Conflict(c) => assert_eq!(c.rules.len(), 2),
            _ => panic!("expected conflict"),
        }
    }

    #[test]
    fn equal_specificity_same_license_resolves() {
        let c = cfg("[[rule]]\nglob=\"examples/**\"\nlicense=\"MIT\"\n\
             [[rule]]\nglob=\"**/*.rs\"\nlicense=\"MIT\"\n");
        let rs = RuleSet::new(&c);
        assert!(matches!(
            rs.resolve(&PathBuf::from("examples/demo.rs")),
            Match::Rule(_)
        ));
    }

    #[test]
    fn equal_specificity_same_license_different_copyright_conflicts() {
        // Full intent (license + copyright policy) decides: identical
        // licenses with different copyright handling still conflict.
        let c = cfg(
            "[[rule]]\nglob=\"examples/**\"\nlicense=\"MIT\"\ncopyright=\"preserve\"\n\
             [[rule]]\nglob=\"**/*.rs\"\nlicense=\"MIT\"\ncopyright=\"add:2026 Acme\"\n",
        );
        let rs = RuleSet::new(&c);
        assert!(matches!(
            rs.resolve(&PathBuf::from("examples/demo.rs")),
            Match::Conflict(_)
        ));
    }

    #[test]
    fn equal_specificity_identical_full_intent_resolves() {
        let c = cfg(
            "[[rule]]\nglob=\"examples/**\"\nlicense=\"MIT\"\ncopyright=\"add:2026 Acme\"\n\
             [[rule]]\nglob=\"**/*.rs\"\nlicense=\"MIT\"\ncopyright=\"add:2026 Acme\"\n",
        );
        let rs = RuleSet::new(&c);
        assert!(matches!(
            rs.resolve(&PathBuf::from("examples/demo.rs")),
            Match::Rule(_)
        ));
    }
}
