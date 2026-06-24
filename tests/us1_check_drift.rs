//! US1 — declarative drift detection (SC-001, SC-003, FR-004). Mirrors quickstart Scenario 1.

mod common;
use common::Fixture;

const MARQUE: &str = r#"
[default]
license = "MIT OR Apache-2.0"
[[rule]]
ext = "rs"
license = "LicenseRef-MarqueLicense-1.0"
[[rule]]
glob = "examples/**/*.rs"
license = "MIT OR Apache-2.0"
[exclude]
paths = ["vendor/**"]
"#;

fn marque_repo() -> Fixture {
    let f = Fixture::new();
    f.config(MARQUE)
        .write(
            "src/lib.rs",
            "// SPDX-License-Identifier: LicenseRef-MarqueLicense-1.0\nfn x(){}\n",
        )
        .write(
            "examples/demo.rs",
            "// SPDX-License-Identifier: LicenseRef-MarqueLicense-1.0\nfn d(){}\n",
        )
        .write(
            "README.md",
            "<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->\n# hi\n",
        )
        .write("nolicense.txt.rs", "fn y(){}\n")
        .write("uncovered.unknownext", "data\n")
        .write("vendor/x.rs", "vendored\n");
    f
}

#[test]
fn check_reports_drift_classes_and_exits_1() {
    let f = marque_repo();
    let out = f.licet().arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "drift must exit 1");
    let stdout = String::from_utf8_lossy(&out.stdout);

    // examples/demo.rs carries the Marque license but the glob rule declares MIT OR Apache-2.0.
    assert!(
        stdout.contains("wrong_license") && stdout.contains("examples/demo.rs"),
        "{stdout}"
    );
    // A rust file with no header is missing_header.
    assert!(
        stdout.contains("missing_header") && stdout.contains("nolicense.txt.rs"),
        "{stdout}"
    );
    // A file no rule/default... actually default covers everything, so uncovered only without default.
    // vendor is excluded, not flagged as a failure.
    assert!(
        !stdout.contains("vendor/x.rs"),
        "excluded file must not be flagged: {stdout}"
    );
}

#[test]
fn compliant_files_pass() {
    let f = Fixture::new();
    f.config(MARQUE).write(
        "src/lib.rs",
        "// SPDX-License-Identifier: LicenseRef-MarqueLicense-1.0\nfn x(){}\n",
    );
    // Evaluate only the compliant rust file (FR-013 subset) — exits 0.
    let out = f
        .licet()
        .args(["check", "--files", "src/lib.rs"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "compliant subset exits 0: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn uncovered_without_default_fails_gate() {
    let f = Fixture::new();
    // No [default], so an unmatched file is Uncovered (FR-012a gate failure).
    f.config("[[rule]]\next=\"rs\"\nlicense=\"MIT\"\n")
        .write("notes.md", "text\n");
    let out = f
        .licet()
        .args(["check", "--files", "notes.md"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("uncovered"));
}

#[test]
fn json_output_has_summary_counts() {
    let f = marque_repo();
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["version"], 1);
    assert!(v["summary"]["counts"]["wrong_license"].as_u64().unwrap() >= 1);
    assert_eq!(v["summary"]["pass"], false);
}

#[test]
fn semantic_expression_equivalence_is_compliant() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT OR Apache-2.0\"\n")
        // Reversed order — semantically equal (FR-005).
        .write(
            "a.rs",
            "// SPDX-License-Identifier: Apache-2.0 OR MIT\nfn a(){}\n",
        );
    let out = f
        .licet()
        .args(["check", "--files", "a.rs"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
