//! Raw `serde` deserialization shapes for `license.toml` (contracts/config-schema.md).
//! Validated into the domain [`super::LicensingConfiguration`] by `super::mod`.

use serde::Deserialize;

/// Top-level `license.toml` document.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    pub default: Option<RawIntent>,
    #[serde(default, rename = "rule")]
    pub rules: Vec<RawRule>,
    #[serde(default, rename = "comment_style")]
    pub comment_styles: Vec<RawCommentStyle>,
    pub exclude: Option<RawExclude>,
}

/// `[default]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawIntent {
    pub license: Option<String>,
    pub copyright: Option<String>,
}

/// A `[[rule]]` entry: exactly one selector key plus intent.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRule {
    pub ext: Option<String>,
    pub glob: Option<String>,
    pub file: Option<String>,
    pub license: Option<String>,
    pub copyright: Option<String>,
}

/// A `[[comment_style]]` entry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCommentStyle {
    pub ext: Option<String>,
    pub file: Option<String>,
    pub style: RawStyle,
}

/// `style = "c"` (named) or `style = { line_prefix = "//" }` (inline).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RawStyle {
    Named(String),
    Inline(RawInlineStyle),
}

/// Inline custom comment style (primitive model, data-model §4).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawInlineStyle {
    pub line_prefix: Option<String>,
    pub block_start: Option<String>,
    pub block_end: Option<String>,
    pub block_line_prefix: Option<String>,
}

/// `[exclude]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawExclude {
    #[serde(default)]
    pub paths: Vec<String>,
}
