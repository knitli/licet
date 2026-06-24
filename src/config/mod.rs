//! Declarative configuration model + loading/validation (FR-001, FR-010).
//!
//! `license.toml` is the only authoring surface for licensing intent
//! (contracts/config-schema.md). Validation errors map to exit code 2.

pub mod schema;

use std::path::Path;

use crate::domain::{CommentStyle, CopyrightPolicy, LicenseIntent, Selector};
use crate::error::{LicetError, Result};
use crate::spdx;
use schema::{RawConfig, RawIntent, RawRule, RawStyle};

/// A reference to a comment style: a built-in name or an inline definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentStyleRef {
    Named(String),
    Inline(CommentStyle),
}

/// A matcher paired with the intent it confers (data-model §2).
#[derive(Debug, Clone)]
pub struct Rule {
    pub selector: Selector,
    pub intent: LicenseIntent,
    /// Position in the config (breaks specificity ties, FR-002).
    pub source_order: usize,
}

impl Rule {
    /// Stable label for reports/`--explain`.
    pub fn label(&self) -> String {
        self.selector.label()
    }
}

/// A comment-style association overlaid on built-ins (data-model §4).
#[derive(Debug, Clone)]
pub struct CommentStyleAssociation {
    pub selector: Selector,
    pub style: CommentStyleRef,
}

/// The validated declarative configuration (data-model §1).
#[derive(Debug, Clone, Default)]
pub struct LicensingConfiguration {
    pub default: Option<LicenseIntent>,
    pub rules: Vec<Rule>,
    pub comment_styles: Vec<CommentStyleAssociation>,
    pub exclude: Vec<String>,
}

impl LicensingConfiguration {
    /// Load and validate config from a path (default `./license.toml`).
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            LicetError::Config(format!("cannot read config `{}`: {e}", path.display()))
        })?;
        Self::from_toml(&text)
    }

    /// Parse and validate config from a TOML string.
    pub fn from_toml(text: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(text)
            .map_err(|e| LicetError::Config(format!("invalid license.toml: {e}")))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawConfig) -> Result<Self> {
        let default = match raw.default {
            Some(d) => Some(intent_from_raw(&d, "default")?),
            None => None,
        };

        let mut rules = Vec::with_capacity(raw.rules.len());
        for (i, r) in raw.rules.iter().enumerate() {
            rules.push(rule_from_raw(r, i, &default)?);
        }

        let mut comment_styles = Vec::with_capacity(raw.comment_styles.len());
        for cs in &raw.comment_styles {
            let selector = single_style_selector(cs)?;
            let style = match &cs.style {
                RawStyle::Named(name) => CommentStyleRef::Named(name.clone()),
                RawStyle::Inline(inline) => {
                    let style = CommentStyle {
                        line_prefix: inline.line_prefix.clone(),
                        block_start: inline.block_start.clone(),
                        block_end: inline.block_end.clone(),
                        block_line_prefix: inline.block_line_prefix.clone(),
                    };
                    if style.is_empty() {
                        return Err(LicetError::Config(format!(
                            "comment_style for `{}` defines no syntax (need line_prefix or block_start)",
                            selector.label()
                        )));
                    }
                    CommentStyleRef::Inline(style)
                }
            };
            comment_styles.push(CommentStyleAssociation { selector, style });
        }

        let exclude = raw.exclude.map(|e| e.paths).unwrap_or_default();
        // Validate exclude globs are well-formed.
        for p in &exclude {
            globset::Glob::new(p)
                .map_err(|e| LicetError::Config(format!("invalid exclude glob `{p}`: {e}")))?;
        }

        let config = LicensingConfiguration {
            default,
            rules,
            comment_styles,
            exclude,
        };
        config.check_duplicate_selectors()?;
        Ok(config)
    }

    /// Duplicate identical selectors with differing intent are config-level conflicts
    /// (config-schema.md validation rule 5, FR-022).
    fn check_duplicate_selectors(&self) -> Result<()> {
        for (i, a) in self.rules.iter().enumerate() {
            for b in &self.rules[i + 1..] {
                if a.selector == b.selector
                    && !spdx::expressions_equal(
                        &a.intent.license_expression,
                        &b.intent.license_expression,
                    )
                {
                    return Err(LicetError::Config(format!(
                        "duplicate selector `{}` with conflicting licenses `{}` vs `{}`",
                        a.selector.label(),
                        a.intent.license_expression,
                        b.intent.license_expression
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Build a `LicenseIntent` from a raw intent, validating the SPDX expression.
fn intent_from_raw(raw: &RawIntent, ctx: &str) -> Result<LicenseIntent> {
    let license = raw
        .license
        .clone()
        .ok_or_else(|| LicetError::Config(format!("[{ctx}] is missing a `license` field")))?;
    validate_license(&license, ctx)?;
    let copyright_policy = parse_copyright(raw.copyright.as_deref(), ctx)?;
    Ok(LicenseIntent {
        license_expression: license,
        copyright_policy,
    })
}

fn rule_from_raw(raw: &RawRule, order: usize, default: &Option<LicenseIntent>) -> Result<Rule> {
    let selector = single_rule_selector(raw, order)?;
    let license = raw.license.clone().ok_or_else(|| {
        LicetError::Config(format!(
            "rule #{} (`{}`) is missing a `license` field",
            order + 1,
            selector.label()
        ))
    })?;
    validate_license(&license, &format!("rule `{}`", selector.label()))?;
    // A rule may override copyright; otherwise inherit the default's policy.
    let copyright_policy = match raw.copyright.as_deref() {
        Some(c) => parse_copyright(Some(c), &selector.label())?,
        None => default
            .as_ref()
            .map(|d| d.copyright_policy.clone())
            .unwrap_or_default(),
    };
    Ok(Rule {
        selector,
        intent: LicenseIntent {
            license_expression: license,
            copyright_policy,
        },
        source_order: order,
    })
}

/// Exactly one of `ext` / `glob` / `file` must be set (config-schema.md rule 2).
fn single_rule_selector(raw: &RawRule, order: usize) -> Result<Selector> {
    let mut found: Vec<Selector> = Vec::new();
    if let Some(e) = &raw.ext {
        found.push(Selector::Extension(e.trim_start_matches('.').to_string()));
    }
    if let Some(g) = &raw.glob {
        globset::Glob::new(g)
            .map_err(|err| LicetError::Config(format!("invalid glob `{g}`: {err}")))?;
        found.push(Selector::Glob(g.clone()));
    }
    if let Some(f) = &raw.file {
        // A `file` selector with path separators is an exact path; otherwise a filename.
        if f.contains('/') {
            found.push(Selector::ExactPath(f.clone()));
        } else {
            found.push(Selector::Filename(f.clone()));
        }
    }
    match found.len() {
        1 => Ok(found.pop().unwrap()),
        0 => Err(LicetError::Config(format!(
            "rule #{} has no selector (need exactly one of ext/glob/file)",
            order + 1
        ))),
        _ => Err(LicetError::Config(format!(
            "rule #{} has multiple selectors (need exactly one of ext/glob/file)",
            order + 1
        ))),
    }
}

fn single_style_selector(cs: &schema::RawCommentStyle) -> Result<Selector> {
    match (&cs.ext, &cs.file) {
        (Some(e), None) => Ok(Selector::Extension(e.trim_start_matches('.').to_string())),
        (None, Some(f)) => {
            if f.contains('/') {
                Ok(Selector::ExactPath(f.clone()))
            } else {
                Ok(Selector::Filename(f.clone()))
            }
        }
        (None, None) => Err(LicetError::Config(
            "comment_style entry has no selector (need exactly one of ext/file)".to_string(),
        )),
        (Some(_), Some(_)) => Err(LicetError::Config(
            "comment_style entry has both ext and file (need exactly one)".to_string(),
        )),
    }
}

/// Validate that a license is a parseable SPDX expression or a `LicenseRef-*`.
fn validate_license(expr: &str, ctx: &str) -> Result<()> {
    spdx::validate_expression(expr).map_err(|e| LicetError::Config(format!("[{ctx}] {e}")))
}

/// Parse the `copyright` policy string (`preserve` | `add:<text>` | `replace:<text>`).
fn parse_copyright(value: Option<&str>, ctx: &str) -> Result<CopyrightPolicy> {
    match value {
        None => Ok(CopyrightPolicy::Preserve),
        Some(v) => {
            let v = v.trim();
            if v.eq_ignore_ascii_case("preserve") {
                Ok(CopyrightPolicy::Preserve)
            } else if let Some(text) = v.strip_prefix("add:") {
                Ok(CopyrightPolicy::PreserveAndAdd(text.trim().to_string()))
            } else if let Some(text) = v.strip_prefix("replace:") {
                Ok(CopyrightPolicy::Replace(text.trim().to_string()))
            } else {
                Err(LicetError::Config(format!(
                    "[{ctx}] invalid copyright policy `{v}` (expected preserve | add:<text> | replace:<text>)"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARQUE: &str = r#"
[default]
license = "MIT OR Apache-2.0"

[[rule]]
ext = "rs"
license = "LicenseRef-MarqueLicense-1.0"

[[rule]]
glob = "examples/**/*.rs"
license = "MIT OR Apache-2.0"

[[comment_style]]
ext = "pkl"
style = "c"

[[comment_style]]
file = "Jenkinsfile"
style = { line_prefix = "//" }

[exclude]
paths = ["vendor/**", "target/**"]
"#;

    #[test]
    fn parses_marque_config() {
        let cfg = LicensingConfiguration::from_toml(MARQUE).unwrap();
        assert_eq!(cfg.rules.len(), 2);
        assert_eq!(cfg.comment_styles.len(), 2);
        assert_eq!(cfg.exclude.len(), 2);
        assert!(cfg.default.is_some());
    }

    #[test]
    fn rejects_invalid_license() {
        let err =
            LicensingConfiguration::from_toml("[[rule]]\next = \"rs\"\nlicense = \"Not Real\"\n")
                .unwrap_err();
        assert!(matches!(err, LicetError::Config(_)));
    }

    #[test]
    fn rejects_multiple_selectors() {
        let err = LicensingConfiguration::from_toml(
            "[[rule]]\next = \"rs\"\nglob = \"*.rs\"\nlicense = \"MIT\"\n",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("multiple selectors"));
    }

    #[test]
    fn rejects_duplicate_conflicting_selector() {
        let err = LicensingConfiguration::from_toml(
            "[[rule]]\next=\"rs\"\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"Apache-2.0\"\n",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("duplicate selector"));
    }

    #[test]
    fn copyright_policies_parse() {
        let cfg = LicensingConfiguration::from_toml(
            "[default]\nlicense=\"MIT\"\ncopyright=\"add:2026 Acme\"\n",
        )
        .unwrap();
        assert_eq!(
            cfg.default.unwrap().copyright_policy,
            CopyrightPolicy::PreserveAndAdd("2026 Acme".to_string())
        );
    }
}
