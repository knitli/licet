//! US2 — reconcile to intent (SC-002, SC-004, FR-006..FR-009, FR-020). Quickstart Scenario 2.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn destructive_replaces_license_preserves_copyright() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nglob=\"examples/**/*.rs\"\nlicense=\"MIT OR Apache-2.0\"\n")
        .write(
            "examples/demo.rs",
            "// SPDX-FileCopyrightText: 2026 Marque\n// SPDX-License-Identifier: LicenseRef-MarqueLicense-1.0\nfn d(){}\n",
        )
        .commit("init");

    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let content = f.read("examples/demo.rs");
    assert!(
        content.contains("SPDX-License-Identifier: MIT OR Apache-2.0"),
        "{content}"
    );
    // Copyright line survives the license-only replace (SC-004).
    assert!(
        content.contains("SPDX-FileCopyrightText: 2026 Marque"),
        "{content}"
    );
    assert!(
        !content.contains("LicenseRef-MarqueLicense-1.0"),
        "old license removed: {content}"
    );
}

#[test]
fn apply_then_check_is_clean() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    assert_eq!(
        f.licet().arg("apply").output().unwrap().status.code(),
        Some(0)
    );
    let check = f.licet().arg("check").output().unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

#[test]
fn additive_keeps_both_and_warns_contradiction() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n")
        .commit("init");

    let out = f
        .licet()
        .args(["apply", "--additive", "--format", "json"])
        .output()
        .unwrap();
    let content = f.read("a.rs");
    assert!(content.contains("Apache-2.0"), "old kept: {content}");
    assert!(content.contains("MIT"), "new added: {content}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let warnings = v["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings.iter().any(|w| w["kind"] == "contradiction"),
        "expected contradiction warning: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn dry_run_does_not_modify_files() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    let before = f.read("a.rs");
    let out = f.licet().args(["apply", "--dry-run"]).output().unwrap();
    assert_eq!(f.read("a.rs"), before, "dry-run must not write");
    assert!(String::from_utf8_lossy(&out.stdout).contains("would apply"));
}

#[test]
fn target_header_replaces_chosen_block() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write(
            "a.rs",
            "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n\n// SPDX-License-Identifier: ISC\nfn b(){}\n",
        )
        .commit("init");
    // Replace the 2nd header block (index 1).
    let out = f
        .licet()
        .args(["apply", "--target-header", "1"])
        .output()
        .unwrap();
    assert!(out.status.success() || out.status.code() == Some(1));
    let content = f.read("a.rs");
    // First block untouched, second now MIT.
    assert!(
        content.contains("// SPDX-License-Identifier: Apache-2.0"),
        "{content}"
    );
    assert!(
        content.contains("// SPDX-License-Identifier: MIT"),
        "{content}"
    );
    assert!(!content.contains("ISC"), "second block replaced: {content}");
}
// REUSE-IgnoreEnd
