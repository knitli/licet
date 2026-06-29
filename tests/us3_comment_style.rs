//! US3 — new comment style persists & round-trips (SC-005, FR-010, FR-011).
//! Quickstart Scenario 3.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn pkl_gets_c_style_header_and_round_trips() {
    let f = Fixture::new();
    f.config(
        "[default]\nlicense=\"LicenseRef-MarqueLicense-1.0\"\n\
         [[comment_style]]\next=\"pkl\"\nstyle=\"c\"\n",
    )
    // The custom LicenseRef text must exist under LICENSES/ for the repo to be compliant
    // (licet no longer scaffolds a placeholder).
    .write(
        "LICENSES/LicenseRef-MarqueLicense-1.0.txt",
        "Marque License 1.0\n\nAll rights reserved.\n",
    )
    .write("hk.pkl", "amends \"...\"\n")
    .commit("init");

    // Apply writes a header using the configured C (//) style — no per-file flag.
    let apply = f
        .licet()
        .args(["apply", "--files", "hk.pkl"])
        .output()
        .unwrap();
    assert_eq!(
        apply.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&apply.stdout)
    );
    let content = f.read("hk.pkl");
    assert!(
        content.contains("// SPDX-License-Identifier: LicenseRef-MarqueLicense-1.0"),
        "{content}"
    );

    // The subsequent check recognizes the header (compliant) — persisted association.
    let check = f
        .licet()
        .args(["check", "--files", "hk.pkl"])
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

#[test]
fn inline_custom_style_is_used() {
    let f = Fixture::new();
    f.config(
        "[default]\nlicense=\"MIT\"\n\
         [[comment_style]]\nfile=\"Jenkinsfile\"\nstyle={ line_prefix = \"//\" }\n",
    )
    .write("Jenkinsfile", "pipeline {}\n")
    .commit("init");

    f.licet()
        .args(["apply", "--files", "Jenkinsfile"])
        .output()
        .unwrap();
    let content = f.read("Jenkinsfile");
    assert!(
        content.starts_with("// SPDX-License-Identifier: MIT"),
        "{content}"
    );
}
// REUSE-IgnoreEnd
