//! Reconciliation engine: destructive (default) and additive license reconciliation,
//! copyright preservation, and targeted-header selection (FR-006..FR-009, FR-020).

pub mod insert;

use crate::comment;
use crate::detect::candidate_licenses;
use crate::domain::{
    ActualLicenseState, ChangeMode, CommentSyntax, CopyrightPolicy, LicenseIntent,
};
use crate::spdx;

/// The computed reconciliation for a single file.
#[derive(Debug, Clone)]
pub struct PlannedChange {
    /// New file content, or `None` when no change is needed.
    pub new_content: Option<String>,
    pub mode: ChangeMode,
    pub target_header: Option<usize>,
    pub wrote_header: bool,
    pub preserved_copyrights: usize,
    /// True when additive apply left two contradictory licenses (FR-020).
    pub contradiction: bool,
}

impl PlannedChange {
    fn noop(mode: ChangeMode) -> Self {
        PlannedChange {
            new_content: None,
            mode,
            target_header: None,
            wrote_header: false,
            preserved_copyrights: 0,
            contradiction: false,
        }
    }
}

/// Plan a file's reconciliation toward `intent` in the given `style`.
pub fn plan_file(
    content: &str,
    actual: &ActualLicenseState,
    intent: &LicenseIntent,
    style: &CommentSyntax,
    mode: ChangeMode,
    target_header: Option<usize>,
) -> PlannedChange {
    // Already compliant → nothing to do.
    let candidates = candidate_licenses(actual);
    let already = candidates
        .iter()
        .any(|c| spdx::expressions_equal(c, &intent.license_expression));
    if already {
        return PlannedChange::noop(mode);
    }

    // No in-file header → insert one.
    if actual.headers.is_empty() {
        let copyrights = copyrights_to_write(&[], &intent.copyright_policy);
        let header = comment::render_header(style, &intent.license_expression, &copyrights);
        let new_content = insert::insert_header(content, &header);
        return PlannedChange {
            new_content: Some(new_content),
            mode,
            target_header: None,
            wrote_header: true,
            preserved_copyrights: copyrights.len(),
            contradiction: false,
        };
    }

    // Edit an existing header block.
    let idx = target_header.unwrap_or(0).min(actual.headers.len() - 1);
    let block = &actual.headers[idx];
    let (start, end) = block.byte_range;
    let block_text = &content[start..end];
    let preserved = block.copyrights.len();

    let new_block = match mode {
        ChangeMode::Destructive => replace_license_in_block(block_text, &intent.license_expression),
        ChangeMode::Additive => add_license_to_block(block_text, &intent.license_expression),
    };

    let contradiction = mode == ChangeMode::Additive
        && block
            .license_ids
            .iter()
            .any(|l| !spdx::expressions_equal(l, &intent.license_expression));

    let mut new_content = String::with_capacity(content.len() + 32);
    new_content.push_str(&content[..start]);
    new_content.push_str(&new_block);
    new_content.push_str(&content[end..]);

    PlannedChange {
        new_content: Some(new_content),
        mode,
        target_header: Some(idx),
        wrote_header: false,
        preserved_copyrights: preserved,
        contradiction,
    }
}

/// Decide which copyright lines to write for a brand-new header given the policy.
pub(crate) fn copyrights_to_write(existing: &[String], policy: &CopyrightPolicy) -> Vec<String> {
    match policy {
        CopyrightPolicy::Preserve => existing.to_vec(),
        CopyrightPolicy::PreserveAndAdd(text) => {
            let mut v = existing.to_vec();
            if !v.iter().any(|c| c == text) {
                v.push(text.clone());
            }
            v
        }
        CopyrightPolicy::Replace(text) => vec![text.clone()],
    }
}

/// Replace the `SPDX-License-Identifier` value(s) in a block, preserving comment prefixes
/// and all copyright lines (destructive default, FR-007 + FR-009).
fn replace_license_in_block(block: &str, new_license: &str) -> String {
    let ending = if block.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing_newline = block.ends_with('\n');
    let mut out_lines: Vec<String> = Vec::new();
    for line in block.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if let Some(pos) = trimmed.find("SPDX-License-Identifier:") {
            let (prefix, rest) = trimmed.split_at(pos + "SPDX-License-Identifier:".len());
            // Preserve any trailing block terminator (e.g. ` -->`, ` */`).
            let terminator = extract_terminator(rest);
            out_lines.push(format!("{prefix} {new_license}{terminator}"));
        } else {
            out_lines.push(trimmed.to_string());
        }
    }
    let mut joined = out_lines.join(ending);
    if trailing_newline {
        joined.push_str(ending);
    }
    joined
}

/// Add a new license line to a block without removing existing ones (additive, FR-006).
fn add_license_to_block(block: &str, new_license: &str) -> String {
    let ending = if block.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing_newline = block.ends_with('\n');
    let mut out_lines: Vec<String> = Vec::new();
    let mut inserted = false;
    for line in block.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        out_lines.push(trimmed.to_string());
        if !inserted && let Some(pos) = trimmed.find("SPDX-License-Identifier:") {
            // Mirror the existing line's comment prefix.
            let prefix = &trimmed[..pos];
            let terminator = extract_terminator(&trimmed[pos + "SPDX-License-Identifier:".len()..]);
            out_lines.push(format!(
                "{prefix}SPDX-License-Identifier: {new_license}{terminator}"
            ));
            inserted = true;
        }
    }
    let mut joined = out_lines.join(ending);
    if trailing_newline {
        joined.push_str(ending);
    }
    joined
}

/// Extract a trailing block-comment terminator (` -->`, ` */`) from a license value, if any.
fn extract_terminator(value: &str) -> String {
    let v = value.trim_end();
    for term in ["-->", "*/"] {
        if v.ends_with(term) {
            return format!(" {term}");
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ActualSource, HeaderBlock, PositionAfter};

    fn intent(expr: &str) -> LicenseIntent {
        LicenseIntent {
            license_expression: expr.to_string(),
            copyright_policy: CopyrightPolicy::Preserve,
        }
    }

    fn state_with_header(
        content: &str,
        license: &str,
        copyrights: Vec<String>,
    ) -> ActualLicenseState {
        // byte range spanning the two header lines.
        let end = content.find("\n\n").map(|i| i + 1).unwrap_or(content.len());
        ActualLicenseState {
            headers: vec![HeaderBlock {
                byte_range: (0, end),
                license_ids: vec![license.to_string()],
                copyrights,
                position_after: PositionAfter::FileStart,
            }],
            out_of_band: None,
            detected_license: Some(license.to_string()),
            detected_source: Some(ActualSource::Header),
            detected_copyrights: vec![],
            encoding_ok: true,
        }
    }

    #[test]
    fn destructive_replaces_license_preserves_copyright() {
        let content = "// SPDX-FileCopyrightText: 2026 Acme\n// SPDX-License-Identifier: Apache-2.0\n\ncode\n";
        let actual = state_with_header(content, "Apache-2.0", vec!["2026 Acme".to_string()]);
        let plan = plan_file(
            content,
            &actual,
            &intent("MIT"),
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
            None,
        );
        let new = plan.new_content.unwrap();
        assert!(new.contains("SPDX-License-Identifier: MIT"));
        assert!(new.contains("SPDX-FileCopyrightText: 2026 Acme"));
        assert!(!new.contains("Apache-2.0"));
        assert_eq!(plan.preserved_copyrights, 1);
    }

    #[test]
    fn additive_keeps_both_and_flags_contradiction() {
        let content = "// SPDX-License-Identifier: Apache-2.0\n\ncode\n";
        let actual = state_with_header(content, "Apache-2.0", vec![]);
        let plan = plan_file(
            content,
            &actual,
            &intent("MIT"),
            &CommentSyntax::line_only("//"),
            ChangeMode::Additive,
            None,
        );
        let new = plan.new_content.unwrap();
        assert!(new.contains("Apache-2.0"));
        assert!(new.contains("MIT"));
        assert!(plan.contradiction);
    }

    #[test]
    fn missing_header_inserts() {
        let content = "code\n";
        let actual = ActualLicenseState {
            encoding_ok: true,
            ..Default::default()
        };
        let plan = plan_file(
            content,
            &actual,
            &intent("MIT"),
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
            None,
        );
        assert!(plan.wrote_header);
        assert!(
            plan.new_content
                .unwrap()
                .starts_with("// SPDX-License-Identifier: MIT")
        );
    }

    #[test]
    fn compliant_is_noop() {
        let content = "// SPDX-License-Identifier: MIT\n\ncode\n";
        let actual = state_with_header(content, "MIT", vec![]);
        let plan = plan_file(
            content,
            &actual,
            &intent("MIT"),
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
            None,
        );
        assert!(plan.new_content.is_none());
    }
}
