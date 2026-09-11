//! Report / ReconciliationPlan model with JSON (report.schema.json) and human renderers
//! (data-model §8, FR-004, FR-020).

pub mod classify;
pub mod render;

use serde::Serialize;

use crate::domain::{DriftClass, FileChange, FileLicensingState};

/// Top-level machine-readable report (`--format json`, contract v2).
#[derive(Debug, Serialize)]
pub struct Report {
    pub version: u8,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// How the evaluated content was sourced (`worktree`, `index`).
    pub snapshot: String,
    pub summary: Summary,
    pub files: Vec<FileEntry>,
    /// Structured diagnostics, sorted by path then code (v2 name for warnings).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
    /// Independent write records: the same plan/execution log, never joined
    /// to asset states (a metadata patch covers many files).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<WriteEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_texts: Option<LicenseTexts>,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub pass: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial: Option<bool>,
    /// Final verification ran to completion (false when the post-write
    /// re-scan itself failed and the report rests on planned data).
    pub complete: bool,
    /// Gate state before apply ran (apply only; dry-run reports the same
    /// pre-apply gate, since nothing was written).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_pass: Option<bool>,
    /// Dry-run only: whether executing the planned writes is projected to
    /// pass the gate. `summary.pass` mirrors it, labeled as projected in
    /// human output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_pass: Option<bool>,
    pub counts: Counts,
}

#[derive(Debug, Default, Serialize)]
pub struct Counts {
    pub compliant: u32,
    pub wrong_license: u32,
    pub copyright_mismatch: u32,
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
    /// Contributing out-of-band tables, shallowest document first (omitted
    /// when the file has no OOB coverage).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata_origins: Vec<MetadataOriginEntry>,
}

/// Provenance of one metadata table behind a file's effective licensing.
#[derive(Debug, Serialize)]
pub struct MetadataOriginEntry {
    pub metadata: String,
    pub table: usize,
    pub precedence: String,
    pub licenses: Vec<String>,
    pub copyrights: Vec<String>,
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

/// One structured diagnostic: a stable machine-readable code plus a human
/// message (report contract v2; formerly `warnings`/`kind`).
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}

/// One write record for the report: destination, kind, outcome, the selected
/// files it covers, and — for text being written — the exact bytes as text.
/// Byte buffers stay in [`crate::domain::ExecutedWrite`]; only text crosses
/// into JSON, never binary.
#[derive(Debug, Clone, Serialize)]
pub struct WriteEntry {
    pub path: String,
    pub kind: String,
    pub status: String,
    pub affected_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl WriteEntry {
    pub fn from_executed(exec: &crate::domain::ExecutedWrite) -> Self {
        let text = |bytes: Option<&Vec<u8>>| bytes.map(|b| String::from_utf8_lossy(b).into_owned());
        WriteEntry {
            path: exec.write.path.to_string_lossy().replace('\\', "/"),
            kind: exec.write.kind.as_str().to_string(),
            status: exec.status.as_str().to_string(),
            affected_files: exec
                .write
                .affected_files
                .iter()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .collect(),
            before_text: text(exec.write.before.as_ref()),
            after_text: text(exec.write.after.as_ref()),
            message: exec.message.clone(),
        }
    }
}

/// Extra report inputs beyond states/changes/diagnostics.
#[derive(Debug, Default)]
pub struct ReportMeta {
    /// Content source label (`worktree`, `index`).
    pub snapshot: String,
    /// Independent write records (apply: plan dry-run, outcomes real run).
    pub writes: Vec<crate::domain::ExecutedWrite>,
    /// Dry-run only: projected gate outcome.
    pub projected_pass: Option<bool>,
    /// Apply only: gate state before apply ran.
    pub before_pass: Option<bool>,
    /// False when final verification did not complete.
    pub complete: bool,
    /// Violations beyond state drift (blocked license texts): force the gate
    /// shut even when every file state is compliant.
    pub extra_violations: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct LicenseTexts {
    pub referenced: Vec<String>,
    pub present: Vec<String>,
    pub missing: Vec<String>,
    pub bundled_available: Vec<String>,
    pub spdx_list_version: String,
    /// Present but never referenced (project-wide lint finding only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unused: Vec<String>,
    /// `LICENSES/` entries naming no recognizable license (lint finding).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unrecognized: Vec<String>,
    /// Recognized ids kept in extensionless files (lint finding).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_extension: Vec<String>,
}

/// Drift counts for one classified state set.
pub fn count_drift(states: &[FileLicensingState], diagnostics: &[Diagnostic]) -> Counts {
    let mut counts = Counts::default();
    for s in states {
        match &s.drift {
            DriftClass::Compliant => counts.compliant += 1,
            DriftClass::WrongLicense { .. } => counts.wrong_license += 1,
            DriftClass::CopyrightMismatch { .. } => counts.copyright_mismatch += 1,
            DriftClass::MissingHeader => counts.missing_header += 1,
            DriftClass::Uncovered => counts.uncovered += 1,
            DriftClass::Excluded => counts.excluded += 1,
            DriftClass::Unreadable => counts.unreadable += 1,
        }
        if s.conflict.is_some() {
            counts.conflicts += 1;
        }
    }
    for d in diagnostics {
        if d.code == "contradiction" {
            counts.contradictions += 1;
        }
    }
    counts
}

/// Gate predicate: true only when no drift, conflict, or unreadable file fails
/// the gate. The one formula behind both `summary.pass` and the process exit.
pub fn counts_pass(counts: &Counts) -> bool {
    counts.wrong_license == 0
        && counts.copyright_mismatch == 0
        && counts.missing_header == 0
        && counts.uncovered == 0
        && counts.unreadable == 0
        && counts.conflicts == 0
}

impl Report {
    /// Assemble a report from classified file states plus any changes/diagnostics.
    pub fn build(
        command: &str,
        states: &[FileLicensingState],
        changes: &[FileChange],
        diagnostics: Vec<Diagnostic>,
        partial: Option<bool>,
        meta: ReportMeta,
    ) -> Self {
        let counts = count_drift(states, &diagnostics);
        // Dry-run pass means projected success; every other command reports
        // the evaluated gate, including violations beyond state drift.
        let pass = meta
            .projected_pass
            .unwrap_or_else(|| counts_pass(&counts) && !meta.extra_violations);

        let change_by_path: std::collections::HashMap<&std::path::Path, &FileChange> =
            changes.iter().map(|c| (c.path.as_path(), c)).collect();

        let files = states
            .iter()
            .map(|s| file_entry(s, change_by_path.get(s.path.as_path()).copied()))
            .collect();

        let mut diagnostics = diagnostics;
        diagnostics.sort_by(|a, b| {
            (a.path.clone(), a.code.clone()).cmp(&(b.path.clone(), b.code.clone()))
        });

        Report {
            version: 2,
            command: command.to_string(),
            exit_code: None,
            snapshot: meta.snapshot,
            summary: Summary {
                pass,
                partial,
                complete: meta.complete,
                before_pass: meta.before_pass,
                projected_pass: meta.projected_pass,
                counts,
            },
            files,
            diagnostics,
            writes: meta.writes.iter().map(WriteEntry::from_executed).collect(),
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
        DriftClass::WrongLicense { declared, actual }
        | DriftClass::CopyrightMismatch { declared, actual } => {
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
        metadata_origins: s
            .actual
            .out_of_band
            .as_ref()
            .map(|o| {
                o.origins
                    .iter()
                    .map(|origin| MetadataOriginEntry {
                        metadata: origin.metadata_path.to_string_lossy().replace('\\', "/"),
                        table: origin.table_index,
                        precedence: origin.precedence.as_str().to_string(),
                        licenses: origin.licenses.clone(),
                        copyrights: origin.copyrights.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
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
