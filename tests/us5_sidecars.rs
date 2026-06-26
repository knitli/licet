//! `.license` sidecars and REUSE.toml coverage for non-annotatable files (FR-015),
//! plus REUSE 3.3 precedence on detection (FR-003a).

mod common;

use common::Fixture;

/// Bytes that are not valid UTF-8 — a stand-in for a binary asset.
const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00, 0x10,
];

fn check_compliant(f: &Fixture, file: &str) -> serde_json::Value {
    let out = f
        .licet()
        .args(["check", "--format", "json", "--files", file])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn binary_gets_sidecar_by_default_and_round_trips() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nfile=\"logo.png\"\nlicense=\"CC-BY-4.0\"\n");
    std::fs::write(f.path().join("logo.png"), PNG).unwrap();
    f.commit("init");

    // Before apply: an uncovered binary reads as unreadable and fails the gate.
    let before = check_compliant(&f, "logo.png");
    assert_eq!(before["summary"]["counts"]["unreadable"], 1);

    let out = f.licet().arg("apply").output().unwrap();
    assert!(
        out.status.success(),
        "apply failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let sidecar = f.read("logo.png.license");
    assert!(
        sidecar.contains("SPDX-License-Identifier: CC-BY-4.0"),
        "sidecar body: {sidecar:?}"
    );
    assert!(
        !sidecar.contains("//") && !sidecar.contains('#'),
        "sidecar must be bare SPDX lines: {sidecar:?}"
    );

    // Round-trip: the asset is now compliant via its sidecar.
    let after = check_compliant(&f, "logo.png");
    assert_eq!(after["summary"]["counts"]["compliant"], 1, "{}", after);
}

#[test]
fn text_without_comment_style_gets_sidecar() {
    // A JSON file is UTF-8 text but has no comment syntax → non-annotatable.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n");
    f.write("data.json", "{\"k\": 1}\n").commit("init");

    let out = f.licet().arg("apply").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let sidecar = f.read("data.json.license");
    assert!(
        sidecar.contains("SPDX-License-Identifier: MIT"),
        "{sidecar:?}"
    );
    assert_eq!(
        check_compliant(&f, "data.json")["summary"]["counts"]["compliant"],
        1
    );
}

#[test]
fn reuse_toml_strategy_appends_entry_and_round_trips() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[output]\nnon_annotatable=\"reuse-toml\"\n");
    std::fs::write(f.path().join("logo.png"), PNG).unwrap();
    f.commit("init");

    let out = f.licet().arg("apply").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // No sidecar; a REUSE.toml entry instead.
    assert!(!f.path().join("logo.png.license").exists());
    let reuse = f.read("REUSE.toml");
    assert!(reuse.contains("path = \"logo.png\""), "{reuse:?}");
    assert!(
        reuse.contains("SPDX-License-Identifier = \"MIT\""),
        "{reuse:?}"
    );

    assert_eq!(
        check_compliant(&f, "logo.png")["summary"]["counts"]["compliant"],
        1
    );
}

#[test]
fn cli_flag_overrides_config_strategy() {
    // Config says sidecar (default); the flag forces reuse-toml.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n");
    std::fs::write(f.path().join("logo.png"), PNG).unwrap();
    f.commit("init");

    let out = f
        .licet()
        .args(["apply", "--non-annotatable", "reuse-toml"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    assert!(!f.path().join("logo.png.license").exists());
    assert!(f.read("REUSE.toml").contains("path = \"logo.png\""));
}

#[test]
fn reuse_toml_override_wrong_license_is_fixed_in_place() {
    // The previously-documented residual edge: a non-annotatable file already covered by a
    // REUSE.toml `override` entry whose license is wrong. `apply` now rewrites that entry's
    // license in place rather than telling the user to fix it by hand.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nfile=\"logo.png\"\nlicense=\"CC-BY-4.0\"\n");
    std::fs::write(f.path().join("logo.png"), PNG).unwrap();
    f.write(
        "REUSE.toml",
        "version = 1\n\n[[annotations]]\npath = \"logo.png\"\n\
         precedence = \"override\"\nSPDX-License-Identifier = \"MIT\"\n",
    );
    f.commit("init");

    // Override coverage with the wrong license fails the gate before apply.
    let before = check_compliant(&f, "logo.png");
    assert_eq!(before["summary"]["counts"]["wrong_license"], 1, "{before}");

    let out = f.licet().arg("apply").output().unwrap();
    assert!(
        out.status.success(),
        "apply failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // No sidecar written; the REUSE.toml entry itself was corrected in place.
    assert!(!f.path().join("logo.png.license").exists());
    let reuse = f.read("REUSE.toml");
    assert!(
        reuse.contains("SPDX-License-Identifier = \"CC-BY-4.0\""),
        "{reuse}"
    );
    assert!(
        !reuse.contains("MIT"),
        "stale license must be gone: {reuse}"
    );
    assert_eq!(
        reuse.matches("[[annotations]]").count(),
        1,
        "no duplicate block: {reuse}"
    );
    assert!(
        reuse.contains("precedence = \"override\""),
        "precedence preserved: {reuse}"
    );

    assert_eq!(
        check_compliant(&f, "logo.png")["summary"]["counts"]["compliant"],
        1
    );
}

#[test]
fn reuse_toml_glob_override_appends_specific_entry() {
    // When the covering entry is a glob, editing it would change its siblings, so `apply`
    // appends a more-specific exact-path override instead (REUSE 3.3 last match wins).
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nfile=\"logo.png\"\nlicense=\"CC-BY-4.0\"\n");
    std::fs::write(f.path().join("logo.png"), PNG).unwrap();
    std::fs::write(f.path().join("other.png"), PNG).unwrap();
    f.write(
        "REUSE.toml",
        "version = 1\n\n[[annotations]]\npath = \"*.png\"\n\
         precedence = \"override\"\nSPDX-License-Identifier = \"MIT\"\n",
    );
    f.commit("init");

    let out = f.licet().arg("apply").output().unwrap();
    assert!(
        out.status.success(),
        "apply failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let reuse = f.read("REUSE.toml");
    assert!(reuse.contains("path = \"*.png\""), "glob kept: {reuse}");
    assert!(
        reuse.contains("path = \"logo.png\""),
        "exact override added: {reuse}"
    );
    assert_eq!(reuse.matches("[[annotations]]").count(), 2, "{reuse}");

    // logo.png now resolves to CC-BY-4.0; other.png still rides the glob at MIT.
    assert_eq!(
        check_compliant(&f, "logo.png")["summary"]["counts"]["compliant"],
        1
    );
    assert_eq!(
        check_compliant(&f, "other.png")["summary"]["counts"]["compliant"],
        1
    );
}

#[test]
fn reuse_toml_write_is_idempotent() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[output]\nnon_annotatable=\"reuse-toml\"\n");
    std::fs::write(f.path().join("logo.png"), PNG).unwrap();
    f.commit("init");

    f.licet().arg("apply").output().unwrap();
    let first = f.read("REUSE.toml");
    // A second apply (sidecar/REUSE coverage already present) must not duplicate the entry.
    f.licet().args(["apply", "--allow-dirty"]).output().unwrap();
    let second = f.read("REUSE.toml");
    assert_eq!(first, second, "REUSE.toml entry duplicated on re-apply");
    assert_eq!(first.matches("path = \"logo.png\"").count(), 1);
}
