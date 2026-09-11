//! Declarative configuration model + loading/validation (FR-001, FR-010).
//!
//! `license.toml` is the only authoring surface for licensing intent
//! (contracts/config-schema.md). Validation errors map to exit code 2.

pub mod schema;

use std::path::Path;

use crate::domain::{
    BlockStyle, CommentSyntax, CopyrightPolicy, LicenseIntent, LineStyle, NonAnnotatableStrategy,
    Selector,
};
use crate::error::{LicetError, Result};
use crate::spdx;
use schema::{RawConfig, RawIntent, RawRule, RawStyle};
use smol_str::SmolStr;

/// A reference to a comment style: a built-in name or an inline definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentStyleRef {
    Named(String),
    Inline(CommentSyntax),
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
    /// How `apply` covers non-annotatable files (`[output] non_annotatable`).
    pub non_annotatable: NonAnnotatableStrategy,
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
                RawStyle::Named(name) => {
                    // Unknown aliases fail here (exit 2), never as a silent
                    // fallback to a different style at apply time.
                    if crate::comment::by_alias(name).is_none() {
                        return Err(LicetError::Config(format!(
                            "unknown comment style `{name}` for `{}`",
                            selector.label()
                        )));
                    }
                    CommentStyleRef::Named(name.clone())
                }
                RawStyle::Inline(inline) => {
                    let syntax = inline_syntax(inline, &selector)?;
                    CommentStyleRef::Inline(syntax)
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

        let non_annotatable = parse_non_annotatable(raw.output.as_ref())?;

        let config = LicensingConfiguration {
            default,
            rules,
            comment_styles,
            exclude,
            non_annotatable,
        };
        config.check_duplicate_selectors()?;
        Ok(config)
    }

    /// Duplicate identical selectors with differing full intent (license or
    /// copyright policy) are config-level conflicts (config-schema.md
    /// validation rule 5, FR-022).
    fn check_duplicate_selectors(&self) -> Result<()> {
        for (i, a) in self.rules.iter().enumerate() {
            for b in &self.rules[i + 1..] {
                if a.selector == b.selector && !crate::domain::intents_equal(&a.intent, &b.intent) {
                    return Err(LicetError::Config(format!(
                        "duplicate selector `{}` with conflicting intent (`{}` vs `{}`)",
                        a.selector.label(),
                        describe_intent(&a.intent),
                        describe_intent(&b.intent)
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
        found.push(file_selector(f));
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

/// Lower a `file` selector value: a value with path separators is an exact
/// path; a bare name matches that filename in any directory — except a
/// leading `./`, which pins a root-level file as an exact path (`init`
/// emits `./name` so a root file can never govern deeper namesakes).
fn file_selector(f: &str) -> Selector {
    let mut v = f;
    while let Some(rest) = v.strip_prefix("./") {
        v = rest;
    }
    if v.contains('/') || v.len() != f.len() {
        Selector::ExactPath(v.to_string())
    } else {
        Selector::Filename(f.to_string())
    }
}

fn single_style_selector(cs: &schema::RawCommentStyle) -> Result<Selector> {
    match (&cs.ext, &cs.file) {
        (Some(e), None) => Ok(Selector::Extension(e.trim_start_matches('.').to_string())),
        (None, Some(f)) => Ok(file_selector(f)),
        (None, None) => Err(LicetError::Config(
            "comment_style entry has no selector (need exactly one of ext/file)".to_string(),
        )),
        (Some(_), Some(_)) => Err(LicetError::Config(
            "comment_style entry has both ext and file (need exactly one)".to_string(),
        )),
    }
}

/// A comment token must not carry bytes that would break rendering or inject
/// new lines/tags into generated headers: CR, LF, NUL, or any other control
/// character. The interior `block_line_prefix` alone may be blank (existing
/// block styles use `""` or `" * "`), but never control-bearing.
fn validate_comment_token(token: &str, what: &str, selector: &Selector) -> Result<()> {
    if token.trim().is_empty() && what != "block_line_prefix" {
        return Err(LicetError::Config(format!(
            "comment_style for `{}` has a blank {what}",
            selector.label()
        )));
    }
    if let Some(bad) = token.chars().find(|c| c.is_control()) {
        return Err(LicetError::Config(format!(
            "comment_style for `{}` has a control character (U+{:04X}) in {what}",
            selector.label(),
            bad as u32
        )));
    }
    Ok(())
}

/// Lower a raw inline `[[comment_style]]` definition into a [`CommentSyntax`],
/// rejecting empty or half-specified block definitions (data-model §4).
fn inline_syntax(inline: &schema::RawInlineStyle, selector: &Selector) -> Result<CommentSyntax> {
    if let Some(p) = inline.line_prefix.as_deref() {
        validate_comment_token(p, "line_prefix", selector)?;
    }
    if let Some(o) = inline.block_start.as_deref() {
        validate_comment_token(o, "block_start", selector)?;
    }
    if let Some(c) = inline.block_end.as_deref() {
        validate_comment_token(c, "block_end", selector)?;
    }
    if let Some(lp) = inline.block_line_prefix.as_deref() {
        validate_comment_token(lp, "block_line_prefix", selector)?;
    }
    let line = inline.line_prefix.as_deref().map(|p| LineStyle {
        prefix: SmolStr::new(p),
    });

    let block = match (&inline.block_start, &inline.block_end) {
        (Some(open), Some(close)) => Some(BlockStyle {
            open: SmolStr::new(open),
            close: SmolStr::new(close),
            line_prefix: SmolStr::new(inline.block_line_prefix.as_deref().unwrap_or("")),
        }),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            return Err(LicetError::Config(format!(
                "comment_style for `{}` has an incomplete block (need both block_start and block_end)",
                selector.label()
            )));
        }
    };

    match (line, block) {
        (Some(line), Some(block)) => Ok(CommentSyntax::Both { line, block }),
        (Some(line), None) => Ok(CommentSyntax::LineOnly(line)),
        (None, Some(block)) => Ok(CommentSyntax::BlockOnly(block)),
        (None, None) => Err(LicetError::Config(format!(
            "comment_style for `{}` defines no syntax (need line_prefix or block_start)",
            selector.label()
        ))),
    }
}

/// Validate the `[output] non_annotatable` strategy (`sidecar` default | `reuse-toml`).
fn parse_non_annotatable(raw: Option<&schema::RawOutput>) -> Result<NonAnnotatableStrategy> {
    match raw.and_then(|o| o.non_annotatable.as_deref()) {
        None => Ok(NonAnnotatableStrategy::Sidecar),
        Some(v) => parse_non_annotatable_value(v),
    }
}

/// Parse a non-annotatable strategy from a string (shared by config + the CLI flag).
pub fn parse_non_annotatable_value(v: &str) -> Result<NonAnnotatableStrategy> {
    let t = v.trim();
    if t.eq_ignore_ascii_case("sidecar") {
        Ok(NonAnnotatableStrategy::Sidecar)
    } else if t.eq_ignore_ascii_case("reuse-toml") || t.eq_ignore_ascii_case("reuse_toml") {
        Ok(NonAnnotatableStrategy::ReuseToml)
    } else {
        Err(LicetError::Config(format!(
            "[output] invalid non_annotatable `{v}` (expected sidecar | reuse-toml)"
        )))
    }
}

/// Validate that a license is a parseable SPDX expression or a `LicenseRef-*`.
fn validate_license(expr: &str, ctx: &str) -> Result<()> {
    spdx::validate_expression(expr).map_err(|e| LicetError::Config(format!("[{ctx}] {e}")))
}

/// One-line `license` + copyright-policy description for conflict messages.
fn describe_intent(intent: &LicenseIntent) -> String {
    match &intent.copyright_policy {
        CopyrightPolicy::Preserve => intent.license_expression.clone(),
        CopyrightPolicy::PreserveAndAdd(t) => {
            format!("{} + copyright add:{t}", intent.license_expression)
        }
        CopyrightPolicy::Replace(t) => {
            format!("{} + copyright replace:{t}", intent.license_expression)
        }
    }
}

/// Parse the `copyright` policy string (`preserve` | `add:<text>` | `replace:<text>`).
fn parse_copyright(value: Option<&str>, ctx: &str) -> Result<CopyrightPolicy> {
    /// Copyright text becomes a rendered header line: it must be nonempty and
    /// a single line, so it cannot inject additional SPDX tags.
    fn clean(text: &str, ctx: &str) -> Result<String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(LicetError::Config(format!(
                "[{ctx}] copyright text must not be empty"
            )));
        }
        if text.contains(['\r', '\n']) {
            return Err(LicetError::Config(format!(
                "[{ctx}] copyright text must be a single line"
            )));
        }
        Ok(text.to_string())
    }
    match value {
        None => Ok(CopyrightPolicy::Preserve),
        Some(v) => {
            let v = v.trim();
            if v.eq_ignore_ascii_case("preserve") {
                Ok(CopyrightPolicy::Preserve)
            } else if let Some(text) = v.strip_prefix("add:") {
                Ok(CopyrightPolicy::PreserveAndAdd(clean(text, ctx)?))
            } else if let Some(text) = v.strip_prefix("replace:") {
                Ok(CopyrightPolicy::Replace(clean(text, ctx)?))
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
    use crate::domain::Selector;

    #[test]
    fn dot_slash_file_value_is_exact_path() {
        // `init` pins root-level files as `./name`; the loader must not read
        // that as a directory-spanning filename match.
        let cfg = LicensingConfiguration::from_toml(
            "[[rule]]\nfile = \"./Makefile\"\nlicense = \"MIT\"\n\
             [[rule]]\nfile = \"Makefile\"\nlicense = \"Apache-2.0\"\n",
        )
        .unwrap();
        assert_eq!(
            cfg.rules[0].selector,
            Selector::ExactPath("Makefile".to_string())
        );
        assert_eq!(
            cfg.rules[1].selector,
            Selector::Filename("Makefile".to_string())
        );
        let set = crate::rules::RuleSet::new(&cfg);
        assert!(matches!(
            set.resolve(Path::new("sub/Makefile")),
            crate::rules::Match::Rule(r) if r.intent.license_expression == "Apache-2.0"
        ));
        assert!(matches!(
            set.resolve(Path::new("Makefile")),
            crate::rules::Match::Rule(r) if r.intent.license_expression == "MIT"
        ));
    }

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
    fn rejects_duplicate_selector_with_differing_copyright() {
        // Full intent decides duplicates too: same license but different
        // copyright policy still conflicts.
        let err = LicensingConfiguration::from_toml(
            "[[rule]]\next=\"rs\"\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"MIT\"\ncopyright=\"add:2026 Acme\"\n",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("duplicate selector"));
    }

    #[test]
    fn accepts_duplicate_selector_with_identical_full_intent() {
        LicensingConfiguration::from_toml(
            "[[rule]]\next=\"rs\"\nlicense=\"MIT\"\ncopyright=\"add:2026 Acme\"\n[[rule]]\next=\"rs\"\nlicense=\"MIT\"\ncopyright=\"add:2026 Acme\"\n",
        )
        .unwrap();
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

    #[test]
    fn inline_block_only_style_parses() {
        let cfg = LicensingConfiguration::from_toml(
            "[[comment_style]]\next = \"vue\"\nstyle = { block_start = \"<!--\", block_end = \"-->\" }\n",
        )
        .unwrap();
        match &cfg.comment_styles[0].style {
            CommentStyleRef::Inline(CommentSyntax::BlockOnly(b)) => {
                assert_eq!(b.open, "<!--");
                assert_eq!(b.close, "-->");
                assert_eq!(b.line_prefix, "");
            }
            other => panic!("expected block-only inline style, got {other:?}"),
        }
    }

    #[test]
    fn inline_block_requires_both_delimiters() {
        let err = LicensingConfiguration::from_toml(
            "[[comment_style]]\next = \"x\"\nstyle = { block_start = \"/*\" }\n",
        )
        .unwrap_err();
        assert!(
            format!("{err}").contains("incomplete block"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_unknown_comment_style_alias() {
        let err = LicensingConfiguration::from_toml(
            "[[comment_style]]\next = \"x\"\nstyle = \"no-such-style\"\n",
        )
        .unwrap_err();
        assert!(
            format!("{err}").contains("unknown comment style"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_blank_comment_tokens() {
        for style in [
            "style = { line_prefix = \"   \" }",
            "style = { block_start = \"\", block_end = \"*/\" }",
            "style = { block_start = \"/*\", block_end = \"  \" }",
        ] {
            let toml = format!("[[comment_style]]\next = \"x\"\n{style}\n");
            let err = LicensingConfiguration::from_toml(&toml).unwrap_err();
            assert!(format!("{err}").contains("blank"), "got: {err}");
        }
        // The interior block line prefix alone may be empty or whitespace.
        LicensingConfiguration::from_toml(
            "[[comment_style]]\next = \"x\"\nstyle = { block_start = \"/*\", block_end = \"*/\", block_line_prefix = \" * \" }\n",
        )
        .unwrap();
    }

    #[test]
    fn rejects_control_characters_in_comment_tokens() {
        // `\\n` is a TOML escape: the parsed prefix carries a real newline.
        let err = LicensingConfiguration::from_toml(
            "[[comment_style]]\next = \"x\"\nstyle = { line_prefix = \"//\\n\" }\n",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("control character"), "got: {err}");
    }

    #[test]
    fn rejects_empty_and_multiline_copyright_text() {
        for copyright in ["copyright = \"add:\"", "copyright = \"replace:   \""] {
            let err = LicensingConfiguration::from_toml(&format!(
                "[default]\nlicense = \"MIT\"\n{copyright}\n"
            ))
            .unwrap_err();
            assert!(format!("{err}").contains("must not be empty"), "got: {err}");
        }
        let err = LicensingConfiguration::from_toml(
            "[default]\nlicense = \"MIT\"\ncopyright = \"add:2026 A\\nSPDX-License-Identifier: MIT\"\n",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("single line"), "got: {err}");
    }

    #[test]
    fn non_annotatable_defaults_to_sidecar() {
        let cfg = LicensingConfiguration::from_toml("[default]\nlicense=\"MIT\"\n").unwrap();
        assert_eq!(cfg.non_annotatable, NonAnnotatableStrategy::Sidecar);
    }

    #[test]
    fn non_annotatable_reuse_toml_parses() {
        let cfg = LicensingConfiguration::from_toml(
            "[default]\nlicense=\"MIT\"\n[output]\nnon_annotatable=\"reuse-toml\"\n",
        )
        .unwrap();
        assert_eq!(cfg.non_annotatable, NonAnnotatableStrategy::ReuseToml);
    }

    #[test]
    fn non_annotatable_invalid_is_rejected() {
        let err = LicensingConfiguration::from_toml(
            "[default]\nlicense=\"MIT\"\n[output]\nnon_annotatable=\"bogus\"\n",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("invalid non_annotatable"));
    }

    #[test]
    fn inline_empty_style_is_rejected() {
        let err = LicensingConfiguration::from_toml(
            "[[comment_style]]\next = \"x\"\nstyle = { block_line_prefix = \" * \" }\n",
        )
        .unwrap_err();
        assert!(
            format!("{err}").contains("defines no syntax"),
            "unexpected error: {err}"
        );
    }
}
