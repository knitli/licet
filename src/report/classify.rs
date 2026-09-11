//! Drift classification: join declared intent vs detected actual into a
//! [`FileLicensingState`] with a [`DriftClass`] (FR-003, FR-004, FR-005).

use std::path::PathBuf;

use crate::detect::candidate_licenses;
use crate::domain::{
    ActualLicenseState, CopyrightDrift, DriftClass, FileLicensingState, LicenseIntent,
    RuleConflict, combine_licenses, copyright_policy_satisfied,
};
use crate::spdx;

/// Inputs for classifying one file.
pub struct ClassifyInput<'a> {
    pub path: PathBuf,
    /// Matched rule label, or `None` for default/uncovered.
    pub matched_rule: Option<String>,
    pub declared: Option<&'a LicenseIntent>,
    pub actual: ActualLicenseState,
    pub conflict: Option<RuleConflict>,
    pub excluded: bool,
}

/// Classify a single file's drift (data-model §6 state machine).
pub fn classify(input: ClassifyInput) -> FileLicensingState {
    let ClassifyInput {
        path,
        matched_rule,
        declared,
        actual,
        conflict,
        excluded,
    } = input;

    let (drift, copyright_drift) = if excluded {
        (DriftClass::Excluded, None)
    } else if !actual.encoding_ok {
        (DriftClass::Unreadable, None)
    } else if let Some(c) = &conflict {
        // An unresolved conflict means no single declared intent exists. The
        // tied expressions are named descriptively — never a fabricated
        // license token — and the conflict itself stays structured in
        // `conflict` plus a `rule_conflict` diagnostic.
        let tied = c.message.clone();
        let actual_text = actual
            .detected_license
            .clone()
            .unwrap_or_else(|| "<none>".to_string());
        (
            DriftClass::WrongLicense {
                declared: tied,
                actual: actual_text,
            },
            None,
        )
    } else {
        match declared {
            None => (DriftClass::Uncovered, None),
            Some(intent) => classify_against(intent, &actual),
        }
    };

    FileLicensingState {
        path,
        matched_rule,
        declared_intent: declared.cloned(),
        actual,
        drift,
        conflict,
        copyright_drift,
    }
}

/// Compare a declared intent against detected actual state.
///
/// License equality uses **all** effective expressions joined as one `AND`
/// expression, canonicalized once: a matching MIT candidate never hides an
/// additional Apache license (data-model §6). Copyright is compared
/// separately against the policy; when the license matches but copyright does
/// not the drift is `CopyrightMismatch`, and when both differ the copyright
/// mismatch is retained as a diagnostic next to `WrongLicense`.
fn classify_against(
    intent: &LicenseIntent,
    actual: &ActualLicenseState,
) -> (DriftClass, Option<CopyrightDrift>) {
    let candidates = candidate_licenses(actual);
    if candidates.is_empty() {
        return (DriftClass::MissingHeader, None);
    }
    let combined = combine_licenses(&candidates);
    let license_matches = combined
        .as_deref()
        .is_some_and(|c| spdx::expressions_equal(c, &intent.license_expression));
    let copyright_ok =
        copyright_policy_satisfied(&intent.copyright_policy, &actual.detected_copyrights);
    let copyright_drift = if copyright_ok {
        None
    } else {
        Some(CopyrightDrift::new(
            &intent.copyright_policy,
            &actual.detected_copyrights,
        ))
    };
    if !license_matches {
        let drift = DriftClass::WrongLicense {
            declared: intent.license_expression.clone(),
            actual: combined.unwrap_or_else(|| candidates.join(", ")),
        };
        return (drift, copyright_drift);
    }
    if let Some(cd) = copyright_drift {
        let drift = DriftClass::CopyrightMismatch {
            declared: cd.declared.clone(),
            actual: cd.actual.clone(),
        };
        return (drift, Some(cd));
    }
    (DriftClass::Compliant, None)
}

/// One REUSE-validation diagnostic for a file: a stable machine-readable code
/// plus a human message. Codes are never fabricated license strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseDiagnostic {
    pub code: &'static str,
    pub message: String,
}

/// Outcome of validating one file's actual licensing against REUSE 3.3,
/// independently of any declared policy (`lint`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseEval {
    /// False on any violation or incomplete validation.
    pub passed: bool,
    /// True when validation could not be completed (unreadable bytes or an
    /// unread dependency): never a proven violation and never a pass.
    pub incomplete: bool,
    pub diagnostics: Vec<ReuseDiagnostic>,
}

/// Validate actual file state against REUSE 3.3: every covered file needs a
/// license expression and a copyright notice, and every license value must be
/// well-formed. `excluded` files are not covered and always pass.
/// `read_error` carries the snapshot read failure when the file could not be
/// read at all (an unread dependency → incomplete, not a violation).
pub fn evaluate_reuse(
    excluded: bool,
    actual: &ActualLicenseState,
    read_error: Option<&str>,
) -> ReuseEval {
    if excluded {
        return ReuseEval {
            passed: true,
            incomplete: false,
            diagnostics: Vec::new(),
        };
    }
    let mut diagnostics = Vec::new();
    let mut incomplete = false;
    if let Some(reason) = read_error {
        incomplete = true;
        diagnostics.push(ReuseDiagnostic {
            code: "read_error",
            message: format!("cannot validate file: {reason}"),
        });
    } else if !actual.encoding_ok {
        incomplete = true;
        diagnostics.push(ReuseDiagnostic {
            code: "unsupported_encoding",
            message: "file is not valid UTF-8 and has no out-of-band coverage; \
                      licensing cannot be determined"
                .to_string(),
        });
    }
    if !actual.invalid_license_values.is_empty() {
        let details = actual
            .invalid_license_values
            .iter()
            .map(|v| format!("line {}: `{}` ({})", v.line, v.value, v.reason))
            .collect::<Vec<_>>()
            .join("; ");
        diagnostics.push(ReuseDiagnostic {
            code: "invalid_license",
            message: format!("invalid SPDX-License-Identifier value: {details}"),
        });
    }
    // The reference tool flattens snippet notices into the file's info, so
    // REUSE validation counts snippet licenses and copyrights — while declared
    // policy never does (decision 5: snippets never satisfy file policy).
    if candidate_licenses(actual).is_empty() && actual.snippet_licenses.is_empty() {
        diagnostics.push(ReuseDiagnostic {
            code: "missing_license",
            message: "no license expression covers this file".to_string(),
        });
    }
    let has_copyright = actual
        .detected_copyrights
        .iter()
        .chain(actual.snippet_copyrights.iter())
        .any(|c| !c.trim().is_empty());
    if !has_copyright {
        diagnostics.push(ReuseDiagnostic {
            code: "missing_copyright",
            message: "no copyright notice covers this file".to_string(),
        });
    }
    let passed = diagnostics.is_empty() && !incomplete;
    ReuseEval {
        passed,
        incomplete,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ActualSource, CopyrightPolicy, HeaderBlock, PositionAfter};

    fn intent(expr: &str) -> LicenseIntent {
        LicenseIntent {
            license_expression: expr.to_string(),
            copyright_policy: CopyrightPolicy::Preserve,
        }
    }

    fn header_state_full(licenses: &[&str], copyrights: &[&str]) -> ActualLicenseState {
        ActualLicenseState {
            headers: vec![HeaderBlock {
                byte_range: (0, 10),
                license_ids: licenses.iter().map(|s| s.to_string()).collect(),
                license_spans: licenses.iter().map(|_| (0, 0)).collect(),
                copyrights: copyrights.iter().map(|s| s.to_string()).collect(),
                copyright_spans: copyrights.iter().map(|_| (0, 0)).collect(),
                position_after: PositionAfter::FileStart,
            }],
            out_of_band: None,
            detected_license: licenses.first().map(|s| s.to_string()),
            detected_source: Some(ActualSource::Header),
            detected_copyrights: copyrights.iter().map(|s| s.to_string()).collect(),
            snippet_licenses: vec![],
            snippet_copyrights: vec![],
            encoding_ok: true,
            invalid_license_values: vec![],
        }
    }

    fn header_state(license: &str) -> ActualLicenseState {
        header_state_full(&[license], &[])
    }

    fn input<'a>(
        declared: Option<&'a LicenseIntent>,
        actual: ActualLicenseState,
    ) -> ClassifyInput<'a> {
        ClassifyInput {
            path: PathBuf::from("f.rs"),
            matched_rule: None,
            declared,
            actual,
            conflict: None,
            excluded: false,
        }
    }

    #[test]
    fn compliant_when_match() {
        let i = intent("MIT");
        let s = classify(input(Some(&i), header_state("MIT")));
        assert_eq!(s.drift, DriftClass::Compliant);
    }

    #[test]
    fn compliant_semantic_reorder() {
        let i = intent("MIT OR Apache-2.0");
        let s = classify(input(Some(&i), header_state("Apache-2.0 OR MIT")));
        assert_eq!(s.drift, DriftClass::Compliant);
    }

    #[test]
    fn wrong_license() {
        let i = intent("MIT");
        let s = classify(input(Some(&i), header_state("Apache-2.0")));
        assert!(matches!(s.drift, DriftClass::WrongLicense { .. }));
    }

    #[test]
    fn missing_header() {
        let i = intent("MIT");
        let s = classify(input(
            Some(&i),
            ActualLicenseState {
                encoding_ok: true,
                ..Default::default()
            },
        ));
        assert_eq!(s.drift, DriftClass::MissingHeader);
    }

    #[test]
    fn uncovered_when_no_intent() {
        let s = classify(input(
            None,
            ActualLicenseState {
                encoding_ok: true,
                ..Default::default()
            },
        ));
        assert_eq!(s.drift, DriftClass::Uncovered);
    }

    #[test]
    fn unreadable_when_bad_encoding() {
        let i = intent("MIT");
        let s = classify(input(
            Some(&i),
            ActualLicenseState {
                encoding_ok: false,
                ..Default::default()
            },
        ));
        assert_eq!(s.drift, DriftClass::Unreadable);
    }

    fn intent_with_copyright(license: &str, copyright: &str) -> LicenseIntent {
        LicenseIntent {
            license_expression: license.to_string(),
            copyright_policy: CopyrightPolicy::PreserveAndAdd(copyright.to_string()),
        }
    }

    #[test]
    fn copyright_add_requires_notice_plus_existing() {
        // License matches; the requested notice is present alongside another.
        let s = classify(input(
            Some(&intent_with_copyright("MIT", "2026 Acme")),
            header_state_full(&["MIT"], &["2024 Old", "2026 Acme"]),
        ));
        assert_eq!(s.drift, DriftClass::Compliant);
        assert!(s.copyright_drift.is_none());
    }

    #[test]
    fn copyright_add_missing_notice_is_mismatch() {
        let s = classify(input(
            Some(&intent_with_copyright("MIT", "2026 Acme")),
            header_state_full(&["MIT"], &["2024 Someone"]),
        ));
        assert!(
            matches!(s.drift, DriftClass::CopyrightMismatch { .. }),
            "got {:?}",
            s.drift
        );
        let cd = s.copyright_drift.expect("diagnostic retained");
        assert_eq!(cd.declared, "add:2026 Acme");
    }

    #[test]
    fn copyright_add_without_any_notice_is_mismatch() {
        // `add` requires existing notices, not just the requested one: a bare
        // file does not satisfy it.
        let s = classify(input(
            Some(&intent_with_copyright("MIT", "2026 Acme")),
            header_state_full(&["MIT"], &[]),
        ));
        assert!(matches!(s.drift, DriftClass::CopyrightMismatch { .. }));
    }

    #[test]
    fn license_and_copyright_both_differ_keep_diagnostic() {
        let s = classify(input(
            Some(&intent_with_copyright("MIT", "2026 Acme")),
            header_state_full(&["Apache-2.0"], &["2024 Someone"]),
        ));
        assert!(matches!(s.drift, DriftClass::WrongLicense { .. }));
        assert!(
            s.copyright_drift.is_some(),
            "copyright mismatch retained as diagnostic"
        );
    }

    #[test]
    fn copyright_replace_needs_exact_notice() {
        let target = LicenseIntent {
            license_expression: "MIT".to_string(),
            copyright_policy: CopyrightPolicy::Replace("2026 Acme".to_string()),
        };
        let exact = classify(input(
            Some(&target),
            header_state_full(&["MIT"], &["2026 Acme"]),
        ));
        assert_eq!(exact.drift, DriftClass::Compliant);
        let extra = classify(input(
            Some(&target),
            header_state_full(&["MIT"], &["2026 Acme", "2024 Old"]),
        ));
        assert!(matches!(extra.drift, DriftClass::CopyrightMismatch { .. }));
    }

    #[test]
    fn multi_license_combination_must_match() {
        // One matching candidate never hides an additional license.
        let single = intent("MIT");
        let s = classify(input(
            Some(&single),
            header_state_full(&["MIT", "Apache-2.0"], &[]),
        ));
        assert!(matches!(s.drift, DriftClass::WrongLicense { .. }));
        let combo = intent("MIT AND Apache-2.0");
        let s = classify(input(
            Some(&combo),
            header_state_full(&["MIT", "Apache-2.0"], &[]),
        ));
        assert_eq!(s.drift, DriftClass::Compliant);
    }

    #[test]
    fn reuse_eval_needs_license_and_copyright() {
        let bare = ActualLicenseState {
            encoding_ok: true,
            ..Default::default()
        };
        let eval = evaluate_reuse(false, &bare, None);
        assert!(!eval.passed && !eval.incomplete);
        let codes: Vec<_> = eval.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"missing_license"));
        assert!(codes.contains(&"missing_copyright"));
        // Excluded files are not covered: nothing required.
        let eval = evaluate_reuse(true, &bare, None);
        assert!(eval.passed);
    }

    #[test]
    fn reuse_eval_unreadable_is_incomplete() {
        let bad = ActualLicenseState {
            encoding_ok: false,
            ..Default::default()
        };
        let eval = evaluate_reuse(false, &bad, None);
        assert!(!eval.passed && eval.incomplete);
        assert!(
            eval.diagnostics
                .iter()
                .any(|d| d.code == "unsupported_encoding")
        );
        let good = ActualLicenseState {
            encoding_ok: true,
            ..Default::default()
        };
        let eval = evaluate_reuse(false, &good, Some("snapshot read failed"));
        assert!(!eval.passed && eval.incomplete);
        assert!(eval.diagnostics.iter().any(|d| d.code == "read_error"));
    }
}
