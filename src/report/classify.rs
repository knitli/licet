//! Drift classification: join declared intent vs detected actual into a
//! [`FileLicensingState`] with a [`DriftClass`] (FR-003, FR-004, FR-005).

use std::path::PathBuf;

use crate::detect::candidate_licenses;
use crate::domain::{
    ActualLicenseState, DriftClass, FileLicensingState, LicenseIntent, RuleConflict,
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

    let drift = if excluded {
        DriftClass::Excluded
    } else if !actual.encoding_ok {
        DriftClass::Unreadable
    } else if conflict.is_some() {
        // An unresolved conflict means we cannot determine a single declared intent.
        DriftClass::WrongLicense {
            declared: "<conflict>".to_string(),
            actual: actual
                .detected_license
                .clone()
                .unwrap_or_else(|| "<none>".to_string()),
        }
    } else {
        match declared {
            None => DriftClass::Uncovered,
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
    }
}

/// Compare a declared intent against detected actual state.
fn classify_against(intent: &LicenseIntent, actual: &ActualLicenseState) -> DriftClass {
    let candidates = candidate_licenses(actual);
    if candidates.is_empty() {
        return DriftClass::MissingHeader;
    }
    // Compliant if any detected license matches the declared expression semantically.
    let matches = candidates
        .iter()
        .any(|c| spdx::expressions_equal(c, &intent.license_expression));
    if matches {
        DriftClass::Compliant
    } else {
        DriftClass::WrongLicense {
            declared: intent.license_expression.clone(),
            actual: actual
                .detected_license
                .clone()
                .unwrap_or_else(|| candidates.join(", ")),
        }
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

    fn header_state(license: &str) -> ActualLicenseState {
        ActualLicenseState {
            headers: vec![HeaderBlock {
                byte_range: (0, 10),
                license_ids: vec![license.to_string()],
                copyrights: vec![],
                position_after: PositionAfter::FileStart,
            }],
            out_of_band: None,
            detected_license: Some(license.to_string()),
            detected_source: Some(ActualSource::Header),
            detected_copyrights: vec![],
            snippet_licenses: vec![],
            encoding_ok: true,
        }
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
}
