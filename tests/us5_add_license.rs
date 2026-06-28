//! US5 — `add-license` offline text materialization (FR-017, FR-029).
//! The offline analog of `reuse download`: copy referenced texts into `LICENSES/` from the
//! embedded bundle, without touching source files or the config. Quickstart Scenario 5.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn explicit_id_materializes_bundled_text() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f.licet().args(["add-license", "MIT"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = f.read("LICENSES/MIT.txt");
    assert!(
        text.contains("Permission is hereby granted"),
        "real MIT text, not a placeholder: {text}"
    );
}

#[test]
fn all_materializes_referenced_but_missing_from_config() {
    let f = Fixture::new();
    // Default MIT + an `rs` rule for Apache-2.0; both referenced, neither present yet.
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"Apache-2.0\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n")
        .commit("init");
    let out = f.licet().args(["add-license", "--all"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(f.path().join("LICENSES/MIT.txt").exists());
    assert!(f.path().join("LICENSES/Apache-2.0.txt").exists());
}

#[test]
fn re_run_is_idempotent_and_succeeds() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    assert!(
        f.licet()
            .args(["add", "MIT"])
            .output()
            .unwrap()
            .status
            .success()
    );
    // Second run: already present, nothing written, still exit 0.
    let out = f.licet().args(["add", "MIT"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Already present"), "{stdout}");
}

#[test]
fn unknown_id_unavailable_offline_exits_violations() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f
        .licet()
        .args(["add", "Definitely-Not-A-License-9.9"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Definitely-Not-A-License-9.9"),
        "names the unavailable id: {stdout}"
    );
}

#[test]
fn license_ref_is_scaffolded_as_placeholder() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f
        .licet()
        .args(["add", "LicenseRef-Acme-1.0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = f.read("LICENSES/LicenseRef-Acme-1.0.txt");
    assert!(text.contains("TODO"), "placeholder scaffold: {text}");
}

#[test]
fn no_target_is_usage_error() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f.licet().arg("add-license").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "neither ids nor --all → exit 2");
}

#[test]
fn ids_and_all_together_is_usage_error() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f
        .licet()
        .args(["add-license", "MIT", "--all"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "both ids and --all → exit 2");
}

#[test]
fn writes_only_under_licenses_and_does_not_require_clean_tree() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    // Make the tree dirty — unlike `apply`, `add-license` must still proceed.
    f.write("a.rs", "fn a(){ /* edited */ }\n");
    let before = f.read("a.rs");
    let before_cfg = f.read("license.toml");

    let out = f.licet().args(["add", "MIT"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "dirty tree must not block add-license: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Source file and config are untouched; only LICENSES/ changed.
    assert_eq!(f.read("a.rs"), before, "source file must be untouched");
    assert_eq!(
        f.read("license.toml"),
        before_cfg,
        "config must be untouched"
    );
    assert!(f.path().join("LICENSES/MIT.txt").exists());
}

#[test]
fn json_output_is_well_formed() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f
        .licet()
        .args(["add-license", "MIT", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["command"], "add-license");
    assert_eq!(v["summary"]["pass"], true);
    assert!(v["license_texts"]["spdx_list_version"].is_string());
}
// REUSE-IgnoreEnd
