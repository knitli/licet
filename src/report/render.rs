//! Human-readable rendering of a [`Report`] (shared by check/apply/lint).

use super::Report;
use crate::domain::DriftClass;

/// Render a report as human-readable text for stdout.
pub fn render_human(report: &Report) -> String {
    let mut out = String::new();
    let c = &report.summary.counts;

    // Per-file lines for anything that isn't plainly compliant/excluded.
    for f in &report.files {
        match f.drift.as_str() {
            "compliant" | "excluded" => continue,
            _ => {}
        }
        let detail = match (&f.declared, &f.actual) {
            (Some(d), Some(a)) => format!(" (declared `{d}` vs actual `{a}`)"),
            (Some(d), None) => format!(" (declared `{d}`, no header found)"),
            _ => String::new(),
        };
        let rule = f
            .matched_rule
            .as_ref()
            .map(|r| format!(" [{r}]"))
            .unwrap_or_default();
        out.push_str(&format!("{:<14} {}{}{}\n", f.drift, f.path, detail, rule));
        if let Some(conf) = &f.conflict {
            out.push_str(&format!("               conflict: {}\n", conf.message));
        }
        if let Some(ch) = &f.change {
            let verb = if ch.applied { "applied" } else { "would apply" };
            out.push_str(&format!(
                "               {verb} {} change (copyright preserved: {})\n",
                ch.mode, ch.preserved_copyrights
            ));
        }
    }

    if !report.writes.is_empty() {
        out.push_str("\nWrites:\n");
        for w in &report.writes {
            out.push_str(&format!("  [{}] {} ({})\n", w.status, w.path, w.kind));
            if (w.status == "failed" || w.status == "blocked")
                && let Some(m) = &w.message
            {
                out.push_str(&format!("    {m}\n"));
            }
            if !w.affected_files.is_empty() && w.affected_files.len() > 1 {
                out.push_str(&format!("    covers {} files\n", w.affected_files.len()));
            }
            // Planned writes preview exact bytes (dry-run); applied writes
            // stay compact — the changed path above is the record.
            if w.status == "planned" {
                match &w.before_text {
                    Some(before) => {
                        out.push_str(&format!("    --- before: {}\n", w.path));
                        for line in before.lines() {
                            out.push_str(&format!("    {line}\n"));
                        }
                    }
                    None => out.push_str("    --- before: (new file)\n"),
                }
                match &w.after_text {
                    Some(after) => {
                        out.push_str(&format!("    --- after: {}\n", w.path));
                        for line in after.lines() {
                            out.push_str(&format!("    {line}\n"));
                        }
                    }
                    None => out.push_str("    --- after: (determined at execution)\n"),
                }
            }
        }
    }

    if !report.diagnostics.is_empty() {
        out.push_str("\nWarnings:\n");
        for w in &report.diagnostics {
            let path = w
                .path
                .as_ref()
                .map(|p| format!("{p}: "))
                .unwrap_or_default();
            out.push_str(&format!("  [{}] {}{}\n", w.code, path, w.message));
        }
    }

    out.push_str(&format!(
        "\nSummary: {} compliant, {} wrong-license, {} missing, {} uncovered, {} excluded, {} unreadable",
        c.compliant, c.wrong_license, c.missing_header, c.uncovered, c.excluded, c.unreadable
    ));
    if c.conflicts > 0 {
        out.push_str(&format!(", {} conflicts", c.conflicts));
    }
    if c.contradictions > 0 {
        out.push_str(&format!(", {} contradictions", c.contradictions));
    }
    out.push('\n');
    out.push_str(if report.summary.pass {
        "Result: PASS\n"
    } else {
        "Result: FAIL\n"
    });
    out
}

/// Input for a single-path `--explain` rendering (FR-022): the winning rule
/// (or default), the rules that matched but lost, exclusions, metadata
/// provenance, and the current drift — all resolved directly, without a
/// whole-tree scan.
pub struct ExplainInput<'a> {
    pub path: &'a str,
    pub drift: &'a DriftClass,
    /// 1-based rule number, selector label, and winning intent.
    pub winner: Option<(usize, String, String)>,
    /// Repo-wide default intent, when no rule won.
    pub default_intent: Option<String>,
    /// Equal-specificity rules tied with the winner (FR-022 conflict).
    pub conflict_rules: Vec<(usize, String)>,
    /// Matching rules that lost: 1-based number, selector label, specificity.
    pub losers: Vec<(usize, String, u32)>,
    pub excluded_by_config: bool,
    pub reuse_ignored: bool,
    /// Where the actual license came from, shallowest detail last.
    pub sources: Vec<String>,
    pub snapshot: &'a str,
}

/// Render a `--explain` block for one path.
pub fn render_explain(input: &ExplainInput) -> String {
    let mut out = format!(
        "{}: {} ({})\n",
        input.path,
        describe(input.drift),
        input.drift.as_str()
    );
    if let Some((n, label, intent)) = &input.winner {
        out.push_str(&format!(
            "  winning rule #{n} `{label}`: intent `{intent}`\n"
        ));
    } else if let Some(intent) = &input.default_intent {
        out.push_str(&format!("  no rule matched; default intent `{intent}`\n"));
    } else {
        out.push_str("  no rule matched and no default is set\n");
    }
    for (n, label) in &input.conflict_rules {
        out.push_str(&format!(
            "  tied rule #{n} `{label}` (equal specificity, differing intent)\n"
        ));
    }
    for (n, label, spec) in &input.losers {
        out.push_str(&format!(
            "  losing rule #{n} `{label}` (specificity {spec})\n"
        ));
    }
    if input.excluded_by_config {
        out.push_str("  excluded by a declaration `[exclude]` pattern\n");
    }
    if input.reuse_ignored {
        out.push_str("  REUSE-ignored (license text, sidecar, metadata, or VCS path)\n");
    }
    if !input.excluded_by_config && !input.reuse_ignored {
        out.push_str("  exclusions: none\n");
    }
    if input.sources.is_empty() {
        out.push_str("  metadata sources: none observed\n");
    } else {
        out.push_str("  metadata sources:\n");
        for s in &input.sources {
            out.push_str(&format!("    - {s}\n"));
        }
    }
    out.push_str(&format!("  snapshot: {}\n", input.snapshot));
    out
}

fn describe(drift: &DriftClass) -> String {
    match drift {
        DriftClass::Compliant => "satisfies declared intent".to_string(),
        DriftClass::WrongLicense { declared, actual } => {
            format!("declared `{declared}` but found `{actual}`")
        }
        DriftClass::CopyrightMismatch { declared, actual } => {
            format!("copyright policy requires `{declared}` but found {actual}")
        }
        DriftClass::MissingHeader => "covered but no header present".to_string(),
        DriftClass::Uncovered => "no rule or default covers this path".to_string(),
        DriftClass::Excluded => "explicitly excluded".to_string(),
        DriftClass::Unreadable => "not valid UTF-8; skipped".to_string(),
    }
}
