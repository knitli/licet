//! Report contract — `report.schema.json` stays valid JSON and real command
//! output carries every field the contract requires (FR-004, FR-021; task 7f).
//!
//! A prior edit left the schema with a misplaced brace: it parsed only after
//! shedding `diagnostics`/`writes`/`license_texts` out of `properties`. This
//! suite pins both the schema structure and a real `check` report against it.

mod common;
use common::Fixture;

use std::path::PathBuf;

fn contract_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("specs/001-declarative-license-headers/contracts/report.schema.json")
}

/// The contract must be well-formed JSON with the v2 shape: `diagnostics`,
/// `writes`, and `license_texts` live inside `properties`, not beside it.
#[test]
fn report_schema_is_valid_v2() {
    let text = std::fs::read_to_string(contract_path()).expect("schema file must exist");
    let schema: serde_json::Value =
        serde_json::from_str(&text).expect("report.schema.json must be valid JSON");
    assert_eq!(schema["properties"]["version"]["const"], 2);
    for key in ["diagnostics", "writes", "license_texts"] {
        assert!(
            schema["properties"].get(key).is_some(),
            "`{key}` must live inside `properties`"
        );
        assert!(
            schema.get(key).is_none(),
            "`{key}` must not be a sibling of `properties`"
        );
    }
    for key in ["version", "command", "summary", "files"] {
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::Value::String(key.to_string())),
            "top-level `{key}` must be required"
        );
    }
}

/// Every command's `--format json` stdout is exactly one parseable document:
/// no progress prefix/suffix may pollute the stream (task 9).
#[test]
fn every_command_json_stdout_is_one_document() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write("b.py", "x = 1\n")
        .texts(&["MIT"])
        .commit("init");

    // check (gate failure still yields one document).
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let check_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(check_report["command"], "check");

    // apply dry-run predicts without writing.
    let out = f
        .licet()
        .args(["apply", "--dry-run", "--format", "json"])
        .output()
        .unwrap();
    let apply_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(apply_report["command"], "apply");

    // lint over actual metadata.
    let out = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    let lint_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(lint_report["command"], "lint");

    // init to a fresh output.
    let out = f
        .licet()
        .args(["init", "--format", "json", "--output", "gen.toml"])
        .output()
        .unwrap();
    let init_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(init_report["command"], "init");

    // add-license materializes one missing bundled text.
    let out = f
        .licet()
        .args(["add-license", "Apache-2.0", "--format", "json"])
        .output()
        .unwrap();
    let add_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(add_report["command"], "add-license");

    for report in [
        &check_report,
        &apply_report,
        &lint_report,
        &init_report,
        &add_report,
    ] {
        assert_eq!(report["version"], 2, "{report}");
    }
}

/// A real `check --format json` report satisfies the contract's required fields.
#[test]
fn check_json_report_satisfies_required_contract_fields() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a_good.py", "# SPDX-License-Identifier: MIT\nx=1\n")
        .write("b_wrong.py", "# SPDX-License-Identifier: Apache-2.0\ny=2\n")
        .texts(&["MIT", "Apache-2.0"])
        .commit("init");

    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    // JSON stdout is exactly one serialized document.
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["version"], 2);
    assert_eq!(report["command"], "check");
    assert_eq!(report["summary"]["pass"], false);
    assert_eq!(report["summary"]["complete"], true);
    assert!(report["summary"]["counts"].is_object());
    let files = report["files"].as_array().unwrap();
    for entry in files {
        assert!(entry.get("path").is_some());
        assert!(entry.get("drift").is_some());
    }
    let drift_of = |name: &str| {
        files
            .iter()
            .find(|e| e["path"] == name)
            .unwrap_or_else(|| panic!("{name} must be reported"))["drift"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(drift_of("a_good.py"), "compliant");
    assert_eq!(drift_of("b_wrong.py"), "wrong_license");
    // Diagnostics and writes keys exist in the shape even when empty they may
    // be omitted; when present they must be arrays.
    for key in ["diagnostics", "writes"] {
        if let Some(v) = report.get(key) {
            assert!(v.is_array(), "`{key}` must be an array");
        }
    }
}
