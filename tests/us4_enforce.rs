//! US4 — enforce in hook / CI (SC-006, SC-009, FR-013, FR-025, FR-027).
//! Quickstart Scenario 4.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn staged_subset_blocks_on_drift_and_names_file() {
    let f = Fixture::new();
    f.config(
        "[default]\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"LicenseRef-Marque-1.0\"\n",
    )
    .write(
        "good.rs",
        "// SPDX-License-Identifier: LicenseRef-Marque-1.0\nfn g(){}\n",
    )
    .commit("baseline");
    // Stage a drifted file.
    f.write("bad.rs", "// SPDX-License-Identifier: MIT\nfn b(){}\n")
        .stage_all();

    let out = f.licet().args(["check", "--staged"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "staged drift exits 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("bad.rs"), "names offending file: {stdout}");
}

#[test]
fn compliant_staged_set_passes() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n").commit("baseline");
    f.write("ok.rs", "// SPDX-License-Identifier: MIT\nfn o(){}\n")
        .stage_all();
    let out = f.licet().args(["check", "--staged"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn non_utf8_file_is_unreadable_and_fails_gate() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n");
    // Write invalid UTF-8 bytes.
    std::fs::write(f.path().join("blob.rs"), [0xff, 0xfe, 0x00, 0x01, 0x80]).unwrap();
    let out = f
        .licet()
        .args(["check", "--files", "blob.rs"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("unreadable"));
}

#[test]
fn selection_flags_are_mutually_exclusive() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n").commit("init");
    let out = f
        .licet()
        .args(["check", "--staged", "--changed"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "mutually exclusive flags → usage error"
    );
}

#[test]
fn explain_names_winning_rule() {
    let f = Fixture::new();
    f.config("[[rule]]\next=\"rs\"\nlicense=\"MIT\"\n[[rule]]\nglob=\"examples/**/*.rs\"\nlicense=\"Apache-2.0\"\n")
        .write("examples/d.rs", "fn d(){}\n");
    let out = f
        .licet()
        .args(["check", "--explain", "examples/d.rs"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("glob=examples/**/*.rs"), "{stdout}");
}
// REUSE-IgnoreEnd
