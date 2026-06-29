//! Report / ReconciliationPlan model with JSON (report.schema.json) and human renderers
//! (data-model §8, FR-004, FR-020).

pub mod classify;
pub mod render;

use serde::Serialize;

use crate::domain::{DriftClass, FileChange, FileLicensingState};

/// Top-level machine-readable report (`--format json`).
#[derive(Debug, Serialize)]
pub struct Report {
    pub version: u8,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub summary: Summary,
    pub files: Vec<FileEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_texts: Option<LicenseTexts>,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub pass: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial: Option<bool>,
    pub counts: Counts,
}

#[derive(Debug, Default, Serialize)]
pub struct Counts {
    pub compliant: u32,
    pub wrong_license: u32,
    pub missing_header: u32,
    pub uncovered: u32,
    pub excluded: u32,
    pub unreadable: u32,
    pub conflicts: u32,
    pub contradictions: u32,
}

#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub drift: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<ConflictEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<ChangeEntry>,
}

#[derive(Debug, Serialize)]
pub struct ConflictEntry {
    pub rules: Vec<String>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ChangeEntry {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_header: Option<usize>,
    pub wrote_header: bool,
    pub preserved_copyrights: usize,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Default, Serialize)]
pub struct LicenseTexts {
    pub referenced: Vec<String>,
    pub present: Vec<String>,
    pub missing: Vec<String>,
    pub bundled_available: Vec<String>,
    pub spdx_list_version: String,
}

impl Report {
    /// Assemble a report from classified file states plus any changes/warnings.
    pub fn build(
        command: &str,
        states: &[FileLicensingState],
        changes: &[FileChange],
        warnings: Vec<Warning>,
        partial: Option<bool>,
    ) -> Self {
        let mut counts = Counts::default();
        for s in states {
            match &s.drift {
                DriftClass::Compliant => counts.compliant += 1,
                DriftClass::WrongLicense { .. } => counts.wrong_license += 1,
                DriftClass::MissingHeader => counts.missing_header += 1,
                DriftClass::Uncovered => counts.uncovered += 1,
                DriftClass::Excluded => counts.excluded += 1,
                DriftClass::Unreadable => counts.unreadable += 1,
            }
            if s.conflict.is_some() {
                counts.conflicts += 1;
            }
        }
        for w in &warnings {
            if w.kind == "contradiction" {
                counts.contradictions += 1;
            }
        }

        // A referenced license text that could not be supplied is a compliance failure, not
        // a mere warning: surfacing PASS here while exiting non-zero is exactly the "looks
        // compliant when it isn't" trap we removed placeholders to avoid.
        let missing_text = warnings.iter().any(|w| w.kind == "missing_license_text");
        let pass = counts.wrong_license == 0
            && counts.missing_header == 0
            && counts.uncovered == 0
            && counts.unreadable == 0
            && counts.conflicts == 0
            && !missing_text;

        let change_by_path: std::collections::HashMap<&std::path::Path, &FileChange> =
            changes.iter().map(|c| (c.path.as_path(), c)).collect();

        let files = states
            .iter()
            .map(|s| file_entry(s, change_by_path.get(s.path.as_path()).copied()))
            .collect();

        Report {
            version: 1,
            command: command.to_string(),
            exit_code: None,
            summary: Summary {
                pass,
                partial,
                counts,
            },
            files,
            warnings,
            license_texts: None,
        }
    }

    /// Serialize to pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

fn file_entry(s: &FileLicensingState, change: Option<&FileChange>) -> FileEntry {
    let (declared, actual) = match &s.drift {
        DriftClass::WrongLicense { declared, actual } => {
            (Some(declared.clone()), Some(actual.clone()))
        }
        _ => (
            s.declared_intent
                .as_ref()
                .map(|i| i.license_expression.clone()),
            s.actual.detected_license.clone(),
        ),
    };
    FileEntry {
        path: s.path.to_string_lossy().replace('\\', "/"),
        drift: s.drift.as_str().to_string(),
        declared,
        actual,
        actual_source: s.actual.detected_source.map(|x| x.as_str().to_string()),
        matched_rule: s.matched_rule.clone(),
        conflict: s.conflict.as_ref().map(|c| ConflictEntry {
            rules: c.rules.clone(),
            message: c.message.clone(),
        }),
        change: change.map(|c| ChangeEntry {
            mode: c.mode.as_str().to_string(),
            target_header: c.target_header,
            wrote_header: c.wrote_header,
            preserved_copyrights: c.preserved_copyrights,
            applied: c.applied,
        }),
    }
}
