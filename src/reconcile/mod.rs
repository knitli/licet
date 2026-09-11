//! Reconciliation engine: destructive (default) and additive license reconciliation,
//! copyright preservation, and targeted-header selection (FR-006..FR-009, FR-020).

pub mod insert;

use crate::comment;
use crate::detect::candidate_licenses;
use crate::domain::{
    ActualLicenseState, ChangeMode, CommentSyntax, CopyrightPolicy, HeaderBlock, LicenseIntent,
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

/// How [`plan_file`] can fail instead of producing an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanErrorKind {
    /// A human must intervene: a tag sits in program text rather than a
    /// comment, or the target block carries conflicting licenses. The caller
    /// reports an actionable `unfixable` diagnostic and writes nothing.
    Unfixable,
    /// The plan target is internally inconsistent (stale spans, a range outside
    /// the content). The caller reports a write failure rather than guessing.
    InvalidTarget,
}

/// A refused plan: kind, actionable message, and the 1-based line of the
/// offending tag when known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    pub kind: PlanErrorKind,
    pub message: String,
    pub line: Option<usize>,
}

impl PlanError {
    fn unfixable(message: impl Into<String>, line: Option<usize>) -> Self {
        PlanError {
            kind: PlanErrorKind::Unfixable,
            message: message.into(),
            line,
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        PlanError {
            kind: PlanErrorKind::InvalidTarget,
            message: message.into(),
            line: None,
        }
    }
}

/// Plan a file's reconciliation toward `intent` in the given `style`.
///
/// Edits are span-based: license values are replaced at the exact byte spans
/// carried from detection, and appended lines are anchored to existing tag
/// lines with closers derived from `style` — never from re-parsed trailing
/// code. Copyright lines are never touched (FR-009). Returns [`PlanError`]
/// instead of guessing when the target block cannot be edited safely.
pub fn plan_file(
    content: &str,
    actual: &ActualLicenseState,
    intent: &LicenseIntent,
    style: &CommentSyntax,
    mode: ChangeMode,
    target_header: Option<usize>,
) -> Result<PlannedChange, PlanError> {
    let candidates = candidate_licenses(actual);
    let license_ok = candidates
        .iter()
        .any(|c| spdx::expressions_equal(c, &intent.license_expression));

    // No in-file header → insert one carrying the full intent.
    if actual.headers.is_empty() {
        if license_ok {
            // Covered out-of-band: nothing in-file to do.
            return Ok(PlannedChange::noop(mode));
        }
        let copyrights = copyrights_to_write(&[], &intent.copyright_policy);
        let header = comment::render_header(style, &intent.license_expression, &copyrights);
        let new_content = insert::insert_header(content, &header);
        return Ok(PlannedChange {
            new_content: Some(new_content),
            mode,
            target_header: None,
            wrote_header: true,
            preserved_copyrights: copyrights.len(),
            contradiction: false,
        });
    }

    // Edit an existing header block. An explicit index is validated, never
    // clamped: silently editing a different block than requested corrupts.
    let idx = match target_header {
        Some(t) if t >= actual.headers.len() => {
            return Err(PlanError::invalid(format!(
                "target header #{t} is out of range (file has {} header block(s))",
                actual.headers.len()
            )));
        }
        Some(t) => t,
        None => 0,
    };
    let block = &actual.headers[idx];
    let (start, end) = block.byte_range;
    if end > content.len() || start > end {
        return Err(PlanError::invalid(format!(
            "header #{idx} byte range ({start}, {end}) is outside the file; re-run"
        )));
    }
    if block.license_ids.len() != block.license_spans.len()
        || block.copyrights.len() != block.copyright_spans.len()
    {
        return Err(PlanError::invalid(format!(
            "header #{idx} has {} license / {} copyright values but {} / {} recorded spans",
            block.license_ids.len(),
            block.copyrights.len(),
            block.license_spans.len(),
            block.copyright_spans.len()
        )));
    }
    let preserved = block.copyrights.len();

    // Copyright intent applies even when the license already matches (FR-007):
    // a missing requested notice is a real change, not a no-op.
    let desired_cprs = copyrights_to_write(&block.copyrights, &intent.copyright_policy);
    let cpr_edit = plan_copyright_edit(block, idx, &desired_cprs, content)?;

    // Every span must still address the value it was recorded for.
    for (id, span) in block.license_ids.iter().zip(&block.license_spans) {
        check_span(content, idx, start, end, "license", id, *span)?;
    }
    if let CprEdit::Swap(i) = cpr_edit {
        check_span(
            content,
            idx,
            start,
            end,
            "copyright",
            &block.copyrights[i],
            block.copyright_spans[i],
        )?;
    }

    // Collect every span replacement (license values, plus a single-copyright
    // swap) for one descending pass so offsets never interfere.
    let mut reps: Vec<((usize, usize), String)> = Vec::new();
    // How a still-missing license line is added, if at all.
    enum LicenseInsert {
        None,
        /// After the block's last license line (additive over license drift).
        AfterLicense,
        /// After the block's last line (a license-less block gains its record).
        AtEnd,
    }
    let mut license_insert_kind = LicenseInsert::None;
    if !license_ok {
        if block.license_ids.is_empty() {
            // An in-file record would duplicate out-of-band coverage, so a
            // license-less block gains its line only when the license is
            // unsatisfied (FR-003a).
            license_insert_kind = LicenseInsert::AtEnd;
        } else if mode == ChangeMode::Destructive {
            // Replacing several *distinct* licenses would silently resolve a
            // conflict the classifier surfaced — refuse instead.
            if block
                .license_ids
                .iter()
                .skip(1)
                .any(|l| !spdx::expressions_equal(l, &block.license_ids[0]))
            {
                return Err(PlanError::unfixable(
                    format!(
                        "header #{idx} declares conflicting licenses ({})",
                        block.license_ids.join(", ")
                    ),
                    block
                        .license_spans
                        .first()
                        .map(|sp| line_for_offset(content, sp.0)),
                ));
            }
            for span in &block.license_spans {
                check_editable(content, span.0, style)?;
                reps.push((*span, intent.license_expression.clone()));
            }
        } else {
            license_insert_kind = LicenseInsert::AfterLicense;
        }
    }
    if let CprEdit::Swap(i) = cpr_edit {
        check_editable(content, block.copyright_spans[i].0, style)?;
        reps.push((block.copyright_spans[i], desired_cprs[0].clone()));
    }

    let mut out = content.to_string();
    apply_spans(&mut out, &reps);
    // Net length change at or before a point, for re-anchoring appends. All
    // replacements lie inside the block by validation.
    let shift = |at: usize| -> usize {
        (at as i64
            + reps
                .iter()
                .filter(|(sp, _)| sp.1 <= at)
                .map(|(sp, text)| text.len() as i64 - (sp.1 - sp.0) as i64)
                .sum::<i64>()) as usize
    };

    // Anchored inserts, computed against the post-replacement string. The
    // license insert is pushed before copyright inserts so that a shared point
    // renders copyright lines first per REUSE convention (stable order: the
    // license text is spliced first, then copyright lines land before it).
    let mut pending: Vec<(usize, String)> = Vec::new();
    let license_tag = format!("SPDX-License-Identifier: {}", intent.license_expression);
    match license_insert_kind {
        LicenseInsert::AfterLicense => {
            // The new record goes after the block's last license line, so a
            // same-line closer is handled with a style-derived suffix and a
            // later-line closer keeps the insert before it. Existing lines are
            // only read for anchoring, never rewritten.
            let last = block.license_spans[block.license_spans.len() - 1];
            let anchor = license_anchor(&out, (shift(last.0), shift(last.0)), style)?;
            pending.push((
                anchor.insert_at,
                anchor_tag_line(&anchor, style, &license_tag),
            ));
        }
        LicenseInsert::AtEnd => {
            let (anchor, tag_at) = block_anchor(&out, start, shift(end), style);
            check_editable(&out, tag_at, style)?;
            pending.push((
                anchor.insert_at,
                anchor_tag_line(&anchor, style, &license_tag),
            ));
        }
        LicenseInsert::None => {}
    }
    if matches!(cpr_edit, CprEdit::Append) {
        let (anchor, tag_at) = block_anchor(&out, start, shift(end), style);
        check_editable(&out, tag_at, style)?;
        let mut text = String::new();
        for notice in missing_notices(&block.copyrights, &desired_cprs) {
            text.push_str(&anchor_tag_line(
                &anchor,
                style,
                &format!("SPDX-FileCopyrightText: {notice}"),
            ));
        }
        pending.push((anchor.insert_at, text));
    }
    pending.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (at, text) in pending {
        out = splice_insert(&out, at, &text);
    }

    if license_ok && matches!(cpr_edit, CprEdit::None) {
        return Ok(PlannedChange::noop(mode));
    }

    let contradiction = mode == ChangeMode::Additive
        && block
            .license_ids
            .iter()
            .any(|l| !spdx::expressions_equal(l, &intent.license_expression));

    Ok(PlannedChange {
        new_content: Some(out),
        mode,
        target_header: Some(idx),
        wrote_header: false,
        preserved_copyrights: preserved,
        contradiction,
    })
}

/// The copyright half of a block plan: nothing, a single value swap (by index
/// into the block's copyrights), or appended notice lines (FR-007, FR-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CprEdit {
    None,
    Swap(usize),
    Append,
}

/// Decide the copyright edit from the policy-desired notices versus the
/// block's own. Reducing several notices to one exact notice is refused: no
/// single span covers the surplus lines, and deleting comment lines the tool
/// did not render risks trailing code.
fn plan_copyright_edit(
    block: &HeaderBlock,
    idx: usize,
    desired: &[String],
    content: &str,
) -> Result<CprEdit, PlanError> {
    if *desired == *block.copyrights {
        return Ok(CprEdit::None);
    }
    if block.copyrights.len() == 1 && desired.len() == 1 {
        return Ok(CprEdit::Swap(0));
    }
    if desired.len() <= block.copyrights.len() {
        return Err(PlanError::unfixable(
            format!(
                "header #{idx} carries {} copyright notices but policy requires exactly `{}`; \
                 reduce to one notice manually",
                block.copyrights.len(),
                desired.join(", "),
            ),
            block
                .copyright_spans
                .first()
                .map(|sp| line_for_offset(content, sp.0)),
        ));
    }
    Ok(CprEdit::Append)
}

/// Notices in `desired` absent from `existing`, in order.
fn missing_notices(existing: &[String], desired: &[String]) -> Vec<String> {
    desired
        .iter()
        .filter(|d| !existing.iter().any(|e| e == *d))
        .cloned()
        .collect()
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

/// Where a new license line goes, and what it mirrors.
struct Anchor {
    /// Byte offset (at a line boundary) where the new line is inserted.
    insert_at: usize,
    /// Comment prefix mirrored from the anchor line (its own validated marker).
    prefix: String,
    /// Whether the anchor line ends with the style's block closer, in which
    /// case the new line carries the same closer and stays self-contained.
    same_line_closer: bool,
    /// Newline convention of the anchor line.
    ending: &'static str,
}

/// Refuse tags that sit in ordinary program text rather than a comment: the
/// text between the line start and the tag must be blank or carry one of the
/// style's comment markers (FR-007). Line openers and block openers match
/// anywhere in the prefix (`code(); /* tag */`, `<?php // tag`); a block
/// interior marker only matches at the prefix start, where interior lines put
/// it (` * tag` — a `*` elsewhere is an operator, not a comment).
fn check_editable(content: &str, value_at: usize, style: &CommentSyntax) -> Result<(), PlanError> {
    if editable_context(content, value_at, style) {
        return Ok(());
    }
    let line = line_for_offset(content, value_at);
    Err(PlanError::unfixable(
        format!(
            "SPDX tag on line {line} sits in program text, not a {} comment; move it into a \
             comment or cover the file out-of-band",
            style_family(style),
        ),
        Some(line),
    ))
}

fn editable_context(content: &str, value_at: usize, style: &CommentSyntax) -> bool {
    let line_start = content[..value_at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let infix = content[line_start..value_at].trim();
    if infix.is_empty() {
        // A bare tag line can only be a block-comment interior (which needs no
        // per-line marker); under a line-only style it is program text — a
        // Makefile target, an assignment — and must not be rewritten.
        return style.block().is_some();
    }
    if style
        .line()
        .is_some_and(|l| infix.contains(l.prefix.as_str()))
    {
        return true;
    }
    if let Some(b) = style.block() {
        if infix.contains(b.open.as_str()) {
            return true;
        }
        let interior = b.line_prefix.trim();
        if !interior.is_empty() && infix.trim_start().starts_with(interior) {
            return true;
        }
    }
    false
}

/// Anchor an additive append after the last license line: the insert lands
/// before any later-line closing delimiter, and a same-line closer is replayed
/// from the style so the new line is self-contained (FR-006).
fn license_anchor(
    content: &str,
    value_span: (usize, usize),
    style: &CommentSyntax,
) -> Result<Anchor, PlanError> {
    check_editable(content, value_span.0, style)?;
    let line_start = content[..value_span.0]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let line_end = content[value_span.0..]
        .find('\n')
        .map(|i| value_span.0 + i + 1)
        .unwrap_or(content.len());
    let line = &content[line_start..line_end];
    let stripped = line.trim_end_matches(['\n', '\r']);
    // The value span sits after the license marker, so the prefix is everything
    // before the marker on this line.
    let marker_at = stripped.find("SPDX-License-Identifier:").unwrap_or(0);
    let prefix = stripped[..marker_at].to_string();
    let same_line_closer = style
        .block()
        .is_some_and(|b| stripped.trim_end().ends_with(b.close.as_str()));
    let ending = if line.contains("\r\n") { "\r\n" } else { "\n" };
    Ok(Anchor {
        insert_at: line_end,
        prefix,
        same_line_closer,
        ending,
    })
}

/// A recorded value span must still address the value it was recorded for;
/// otherwise the plan target is stale and the caller must re-run, not guess.
fn check_span(
    content: &str,
    idx: usize,
    start: usize,
    end: usize,
    kind: &str,
    value: &str,
    span: (usize, usize),
) -> Result<(), PlanError> {
    let (s, e) = span;
    if e > content.len() || s > e || s < start || e > end || &content[s..e] != value {
        return Err(PlanError::invalid(format!(
            "header #{idx} {kind} span ({s}, {e}) no longer holds `{value}`; re-run"
        )));
    }
    Ok(())
}

/// Apply disjoint span replacements in descending offset order so earlier
/// offsets stay valid throughout.
fn apply_spans(out: &mut String, reps: &[((usize, usize), String)]) {
    let mut order: Vec<usize> = (0..reps.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(reps[i].0.0));
    for i in order {
        let ((s, e), text) = &reps[i];
        out.replace_range(*s..*e, text);
    }
}

/// Anchor for appending after a block's last line: mirror that line's prefix.
/// Returns the anchor plus the tag offset used for the editability check.
fn block_anchor(content: &str, start: usize, end: usize, style: &CommentSyntax) -> (Anchor, usize) {
    let text = &content[start..end];
    let last_line = text.lines().next_back().unwrap_or("");
    let marker_at = last_line
        .find("SPDX-License-Identifier:")
        .or_else(|| last_line.find("SPDX-FileCopyrightText:"))
        .unwrap_or(0);
    let prefix = last_line[..marker_at].to_string();
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let tag_at = start + text.len() - last_line.len() + marker_at;
    // The block range ends at the last tag line's final byte (detection
    // excludes the terminator): the insert goes after that terminator so the
    // new line does not fuse with the anchor line.
    let mut insert_at = end;
    if content[insert_at..].starts_with("\r\n") {
        insert_at += 2;
    } else if content[insert_at..].starts_with('\n') {
        insert_at += 1;
    }
    // A self-closed anchor line (`/* … */`) replays the style's closer on the
    // new line; without it the rest of the file would become a comment.
    let same_line_closer = style
        .block()
        .is_some_and(|b| last_line.trim_end().ends_with(b.close.as_str()));
    (
        Anchor {
            insert_at,
            prefix,
            same_line_closer,
            ending,
        },
        tag_at,
    )
}

/// Render an appended tag line from the anchor: mirrored prefix plus the
/// style-derived closer when the anchor line carries one.
fn anchor_tag_line(anchor: &Anchor, style: &CommentSyntax, tag: &str) -> String {
    let mut line = format!("{}{tag}", anchor.prefix);
    if anchor.same_line_closer
        && let Some(b) = style.block()
    {
        line.push(' ');
        line.push_str(b.close.as_str());
    }
    line.push_str(anchor.ending);
    line
}

/// Splice `insert` into `content` at `at` (a line boundary).
fn splice_insert(content: &str, at: usize, insert: &str) -> String {
    let mut out = String::with_capacity(content.len() + insert.len());
    out.push_str(&content[..at]);
    out.push_str(insert);
    out.push_str(&content[at..]);
    out
}

/// 1-based line number of a byte offset.
fn line_for_offset(content: &str, at: usize) -> usize {
    content[..at.min(content.len())].matches('\n').count() + 1
}

/// Short human label for the expected comment family (diagnostics only).
fn style_family(style: &CommentSyntax) -> &'static str {
    match style {
        CommentSyntax::LineOnly(_) => "line",
        CommentSyntax::BlockOnly(_) => "block",
        CommentSyntax::Both { .. } => "line or block",
    }
}

// REUSE-IgnoreStart — SPDX tags in the tests below are fixtures, not this file's licensing.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ActualLicenseState;
    use crate::reuse::oob::OutOfBand;
    use std::path::Path;

    fn intent(expr: &str) -> LicenseIntent {
        LicenseIntent {
            license_expression: expr.to_string(),
            copyright_policy: CopyrightPolicy::Preserve,
        }
    }

    fn intent_cpr(expr: &str, policy: CopyrightPolicy) -> LicenseIntent {
        LicenseIntent {
            license_expression: expr.to_string(),
            copyright_policy: policy,
        }
    }

    /// Real detection state, so spans match the content exactly as in production.
    fn detected(content: &str) -> ActualLicenseState {
        let oob = OutOfBand::default();
        crate::detect::detect(Path::new("t.rs"), content.as_bytes(), None, &oob)
    }

    fn plan(
        content: &str,
        actual: &ActualLicenseState,
        license: &str,
        style: &CommentSyntax,
        mode: ChangeMode,
    ) -> Result<PlannedChange, PlanError> {
        plan_file(content, actual, &intent(license), style, mode, None)
    }

    #[test]
    fn destructive_replaces_license_preserves_copyright() {
        let content = "// SPDX-FileCopyrightText: 2026 Acme\n// SPDX-License-Identifier: Apache-2.0\n\ncode\n";
        let plan = plan(
            content,
            &detected(content),
            "MIT",
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
        )
        .unwrap();
        let new = plan.new_content.unwrap();
        assert!(new.contains("SPDX-License-Identifier: MIT"));
        assert!(new.contains("SPDX-FileCopyrightText: 2026 Acme"));
        assert!(!new.contains("Apache-2.0"));
        assert_eq!(plan.preserved_copyrights, 1);
    }

    #[test]
    fn destructive_span_edit_preserves_surrounding_code() {
        // Only the value bytes change: the code prefix and the same-line
        // closer survive byte-for-byte.
        let content = "code(); /* SPDX-License-Identifier: MIT */\n";
        let plan = plan(
            content,
            &detected(content),
            "Apache-2.0",
            &CommentSyntax::both("//", "/*", "*/", " * "),
            ChangeMode::Destructive,
        )
        .unwrap();
        assert_eq!(
            plan.new_content.unwrap(),
            "code(); /* SPDX-License-Identifier: Apache-2.0 */\n"
        );
    }

    #[test]
    fn destructive_program_text_tag_is_unfixable() {
        let content = "FOO=SPDX-License-Identifier: Apache-2.0\n";
        let err = plan(
            content,
            &detected(content),
            "MIT",
            &CommentSyntax::both("//", "/*", "*/", " * "),
            ChangeMode::Destructive,
        )
        .unwrap_err();
        assert_eq!(err.kind, PlanErrorKind::Unfixable);
        assert_eq!(err.line, Some(1));
    }

    #[test]
    fn destructive_bare_tag_under_line_style_is_unfixable() {
        // A bare tag line is a Makefile target, not a comment.
        let content = "SPDX-License-Identifier: Apache-2.0\n";
        let err = plan(
            content,
            &detected(content),
            "MIT",
            &CommentSyntax::line_only("#"),
            ChangeMode::Destructive,
        )
        .unwrap_err();
        assert_eq!(err.kind, PlanErrorKind::Unfixable);
    }

    #[test]
    fn destructive_conflicting_licenses_are_unfixable() {
        let content =
            "// SPDX-License-Identifier: MIT\n// SPDX-License-Identifier: Apache-2.0\n\ncode\n";
        let err = plan(
            content,
            &detected(content),
            "CC0-1.0",
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
        )
        .unwrap_err();
        assert_eq!(err.kind, PlanErrorKind::Unfixable);
        assert_eq!(err.line, Some(1));
    }

    #[test]
    fn additive_keeps_both_and_flags_contradiction() {
        let content = "// SPDX-License-Identifier: Apache-2.0\n\ncode\n";
        let plan = plan(
            content,
            &detected(content),
            "MIT",
            &CommentSyntax::line_only("//"),
            ChangeMode::Additive,
        )
        .unwrap();
        let new = plan.new_content.unwrap();
        assert!(new.contains("Apache-2.0"));
        assert!(new.contains("MIT"));
        assert!(plan.contradiction);
    }

    #[test]
    fn additive_block_style_appends_before_closer() {
        let content = "/*\n * SPDX-License-Identifier: MIT\n */\ncode\n";
        let plan = plan(
            content,
            &detected(content),
            "Apache-2.0",
            &CommentSyntax::both("//", "/*", "*/", " * "),
            ChangeMode::Additive,
        )
        .unwrap();
        assert_eq!(
            plan.new_content.unwrap(),
            "/*\n * SPDX-License-Identifier: MIT\n * SPDX-License-Identifier: Apache-2.0\n */\ncode\n"
        );
        assert!(plan.contradiction);
    }

    #[test]
    fn additive_same_line_closer_replays_style_closer() {
        let content = "<!-- SPDX-License-Identifier: MIT -->\n<root/>\n";
        let plan = plan(
            content,
            &detected(content),
            "Apache-2.0",
            &CommentSyntax::block_only("<!--", "-->", ""),
            ChangeMode::Additive,
        )
        .unwrap();
        assert_eq!(
            plan.new_content.unwrap(),
            "<!-- SPDX-License-Identifier: MIT -->\n<!-- SPDX-License-Identifier: Apache-2.0 -->\n<root/>\n"
        );
    }

    #[test]
    fn copyright_only_block_gains_license_line() {
        let content = "# SPDX-FileCopyrightText: 2026 Acme\n\ncode\n";
        let plan = plan(
            content,
            &detected(content),
            "MIT",
            &CommentSyntax::line_only("#"),
            ChangeMode::Destructive,
        )
        .unwrap();
        assert_eq!(
            plan.new_content.unwrap(),
            "# SPDX-FileCopyrightText: 2026 Acme\n# SPDX-License-Identifier: MIT\n\ncode\n"
        );
    }

    #[test]
    fn missing_header_inserts() {
        let content = "code\n";
        let actual = ActualLicenseState {
            encoding_ok: true,
            ..Default::default()
        };
        let plan = plan(
            content,
            &actual,
            "MIT",
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
        )
        .unwrap();
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
        let plan = plan(
            content,
            &detected(content),
            "MIT",
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
        )
        .unwrap();
        assert!(plan.new_content.is_none());
    }

    #[test]
    fn copyright_add_appends_notice_when_license_matches() {
        // Copyright intent applies even when the license already matches: a
        // missing requested notice is a real change, not a no-op.
        let content = "// SPDX-License-Identifier: MIT\n\ncode\n";
        let plan = plan_file(
            content,
            &detected(content),
            &intent_cpr(
                "MIT",
                CopyrightPolicy::PreserveAndAdd("2026 Acme".to_string()),
            ),
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.new_content.unwrap(),
            "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\n\ncode\n"
        );
    }

    #[test]
    fn copyright_replace_swaps_single_notice() {
        let content =
            "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2025 Acme\n\ncode\n";
        let plan = plan_file(
            content,
            &detected(content),
            &intent_cpr("MIT", CopyrightPolicy::Replace("2026 Acme".to_string())),
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.new_content.unwrap(),
            "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\n\ncode\n"
        );
    }

    #[test]
    fn copyright_replace_multi_is_unfixable() {
        let content = "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: A\n// SPDX-FileCopyrightText: B\n\ncode\n";
        let err = plan_file(
            content,
            &detected(content),
            &intent_cpr("MIT", CopyrightPolicy::Replace("2026 Acme".to_string())),
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
            None,
        )
        .unwrap_err();
        assert_eq!(err.kind, PlanErrorKind::Unfixable);
        assert_eq!(err.line, Some(2));
    }

    #[test]
    fn license_drift_and_copyright_add_combine() {
        let content = "// SPDX-License-Identifier: Apache-2.0\n\ncode\n";
        let plan = plan_file(
            content,
            &detected(content),
            &intent_cpr(
                "MIT",
                CopyrightPolicy::PreserveAndAdd("2026 Acme".to_string()),
            ),
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.new_content.unwrap(),
            "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\n\ncode\n"
        );
    }

    #[test]
    fn explicit_target_index_out_of_range_is_invalid() {
        let content = "// SPDX-License-Identifier: Apache-2.0\n\ncode\n";
        let err = plan_file(
            content,
            &detected(content),
            &intent("MIT"),
            &CommentSyntax::line_only("//"),
            ChangeMode::Destructive,
            Some(3),
        )
        .unwrap_err();
        assert_eq!(err.kind, PlanErrorKind::InvalidTarget);
    }
}
// REUSE-IgnoreEnd
