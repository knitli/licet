//! Detection semantics: complete-content SPDX scans, AST comparison, invalid-value
//! diagnosis, and cross-language header recognition (F12, FR-005, FR-008, FR-030).

mod common;

use common::Fixture;
use licet::detect::detect;
use licet::reuse::oob::OutOfBand;
use std::path::PathBuf;

fn detect_text(name: &str, text: &str) -> licet::domain::ActualLicenseState {
    detect(
        &PathBuf::from(name),
        text.as_bytes(),
        None,
        &OutOfBand::default(),
    )
}

#[test]
fn compound_parenthesized_both_orders_agree() {
    // Same logical set in two shapes: compliant under either declaration order.
    for header in [
        "// SPDX-License-Identifier: (MIT OR Apache-2.0)\n",
        "// SPDX-License-Identifier: Apache-2.0 OR (MIT)\n",
    ] {
        let f = Fixture::new();
        f.config("[default]\nlicense=\"Apache-2.0 OR MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
            .write("a.rs", header)
            .texts(&["MIT", "Apache-2.0"])
            .commit("init");
        let out = f
            .licet()
            .args(["check", "--format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{header}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let entry = report["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == "a.rs")
            .unwrap();
        assert_eq!(entry["drift"], "compliant", "{header}");
    }
}

#[test]
fn tab_separated_operators_pass_gate() {
    // One tag line, tab-separated, lowercase operators: the same logical set as
    // declared. (Identifier case stays canonical here: detection acceptance is
    // strict — lowercase ids are diagnosed as invalid — while comparison
    // tolerates id case; see the spdx unit tests.)
    let f = Fixture::new();
    f.config("[default]\nlicense=\"Apache-2.0 OR MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write(
            "a.rs",
            "// SPDX-FileCopyrightText: 2026 Acme\n// SPDX-License-Identifier: MIT\tor\tApache-2.0\n",
        )
        .texts(&["MIT", "Apache-2.0"])
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn compound_detection_keeps_full_expression() {
    // Detection preserves the header's expression text; comparison is semantic.
    let st = detect_text("a.rs", "// SPDX-License-Identifier: Apache-2.0 OR MIT\n");
    assert_eq!(st.detected_license.as_deref(), Some("Apache-2.0 OR MIT"));
}

#[test]
fn additive_apply_flags_contradiction() {
    // `apply --additive` that leaves two contradictory licenses warns explicitly.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\n")
        .commit("init");
    let out = f
        .licet()
        .args(["apply", "--allow-dirty", "--additive", "--format", "json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "contradiction"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(f.read("a.rs").contains("Apache-2.0"));
    assert!(f.read("a.rs").contains("MIT"));
}

#[test]
fn header_beyond_old_head_cutoff_is_detected() {
    // 9 KiB of leading code once hid the header past the 8 KiB scan cutoff.
    let mut text = "// padding\n".repeat(900);
    text.push_str("// SPDX-License-Identifier: MIT\n");
    assert!(text.len() > 8 * 1024);
    let st = detect_text("a.rs", &text);
    assert_eq!(st.detected_license.as_deref(), Some("MIT"));
}

#[test]
fn invalid_utf8_late_in_file_is_unreadable() {
    let mut bytes = b"// SPDX-License-Identifier: MIT\n".to_vec();
    bytes.extend_from_slice(&[0xff, 0xfe]);
    let st = detect(&PathBuf::from("a.rs"), &bytes, None, &OutOfBand::default());
    assert!(!st.encoding_ok);
}

#[test]
fn invalid_license_value_diagnosed_with_line_and_copyrights_kept() {
    let text = "// SPDX-FileCopyrightText: 2026 Acme\n// SPDX-License-Identifier: Not A License\n";
    let st = detect_text("a.rs", text);
    // No valid license: nothing detected …
    assert_eq!(st.detected_license, None);
    // … but the copyright survives and the value is diagnosed, not dropped.
    assert!(
        st.detected_copyrights
            .iter()
            .any(|c| c.contains("2026 Acme"))
    );
    assert_eq!(st.invalid_license_values.len(), 1);
    assert_eq!(st.invalid_license_values[0].line, 2);
    assert_eq!(st.invalid_license_values[0].value, "Not A License");
}

#[test]
fn invalid_value_warning_reaches_gate_output() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write(
            "a.rs",
            "// SPDX-FileCopyrightText: 2026 Acme\n// SPDX-License-Identifier: Bogus-1.0\n",
        )
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics.iter().any(|w| w["code"] == "invalid_license"
            && w["path"] == "a.rs"
            && w["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Bogus-1.0")),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn orphan_sidecar_content_has_no_effect() {
    // A valid orphan sidecar is diagnosed AND its text leaks into no file state.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("ok.rs", "// SPDX-License-Identifier: MIT\n")
        .write("ghost.rs.license", "SPDX-License-Identifier: Apache-2.0\n")
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "orphan_sidecar" && w["path"] == "ghost.rs.license"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["actual"] == "Apache-2.0"),
        "orphan text must not leak into any file state: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn language_table_headers_detected() {
    // tests/fixtures/table/sample.{py,sh,rb,js,jsx}: each native comment style
    // carries the same header, and detection reads all of them.
    for (name, content) in [
        ("sample.py", include_str!("fixtures/table/sample.py")),
        ("sample.sh", include_str!("fixtures/table/sample.sh")),
        ("sample.rb", include_str!("fixtures/table/sample.rb")),
        ("sample.js", include_str!("fixtures/table/sample.js")),
        ("sample.jsx", include_str!("fixtures/table/sample.jsx")),
    ] {
        let st = detect(
            &PathBuf::from(name),
            content.as_bytes(),
            None,
            &OutOfBand::default(),
        );
        assert_eq!(st.detected_license.as_deref(), Some("MIT"), "{name}");
        assert!(
            st.detected_copyrights
                .iter()
                .any(|c| c.contains("2026 Acme")),
            "{name}"
        );
        assert!(st.encoding_ok, "{name}");
    }
}
