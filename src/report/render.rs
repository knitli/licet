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

    if !report.warnings.is_empty() {
        out.push_str("\nWarnings:\n");
        for w in &report.warnings {
            let path = w
                .path
                .as_ref()
                .map(|p| format!("{p}: "))
                .unwrap_or_default();
            out.push_str(&format!("  [{}] {}{}\n", w.kind, path, w.message));
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

/// Render a single `--explain` line for a path.
pub fn render_explain(path: &str, drift: &DriftClass, matched_rule: &Option<String>) -> String {
    match matched_rule {
        Some(rule) => format!(
            "{path}: winning rule `{rule}` → {} ({})",
            describe(drift),
            drift.as_str()
        ),
        None => format!(
            "{path}: no rule matched; {} ({})",
            describe(drift),
            drift.as_str()
        ),
    }
}

fn describe(drift: &DriftClass) -> String {
    match drift {
        DriftClass::Compliant => "satisfies declared intent".to_string(),
        DriftClass::WrongLicense { declared, actual } => {
            format!("declared `{declared}` but found `{actual}`")
        }
        DriftClass::MissingHeader => "covered but no header present".to_string(),
        DriftClass::Uncovered => "no rule or default covers this path".to_string(),
        DriftClass::Excluded => "explicitly excluded".to_string(),
        DriftClass::Unreadable => "not valid UTF-8; skipped".to_string(),
    }
}
