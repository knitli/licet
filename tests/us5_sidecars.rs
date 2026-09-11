//! `.license` sidecars and REUSE.toml coverage for non-annotatable files (FR-015),
//! plus REUSE 3.3 precedence on detection (FR-003a).
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

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
    // A non-annotatable file already covered by a REUSE.toml `override` entry
    // whose license is wrong. `apply` appends a superseding `override` stanza
    // (last match wins) rather than rewriting the stale entry in place, so the
    // original document survives byte-for-byte and a rerun converges.
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

    // No sidecar written; the stale stanza is untouched and the appended
    // `override` stanza governs.
    assert!(!f.path().join("logo.png.license").exists());
    let reuse = f.read("REUSE.toml");
    assert!(
        reuse.contains("SPDX-License-Identifier = \"CC-BY-4.0\""),
        "{reuse}"
    );
    assert_eq!(
        reuse.matches("[[annotations]]").count(),
        2,
        "superseding stanza appended: {reuse}"
    );
    assert!(
        reuse.contains("precedence = \"override\"\nSPDX-License-Identifier = \"CC-BY-4.0\""),
        "appended stanza keeps the barrier: {reuse}"
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

#[test]
fn additive_sidecar_preserves_old_identifiers() {
    // Additive sidecar coverage keeps the old license lines and notices while
    // adding the declared ones (FR-006).
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nfile=\"logo.png\"\nlicense=\"CC-BY-4.0\"\n");
    std::fs::write(f.path().join("logo.png"), PNG).unwrap();
    f.write(
        "logo.png.license",
        "SPDX-FileCopyrightText: 2026 Acme\nSPDX-License-Identifier: MIT\n",
    );
    f.commit("init");

    let out = f
        .licet()
        .args(["apply", "--additive", "--files", "logo.png"])
        .output()
        .unwrap();
    // The write succeeds (both identifiers land) but the old record still
    // counts as drift: exit 1, not partial.
    assert_eq!(
        out.status.code(),
        Some(1),
        "apply output: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let sidecar = f.read("logo.png.license");
    assert!(
        sidecar.contains("SPDX-License-Identifier: MIT"),
        "old identifier kept: {sidecar:?}"
    );
    assert!(
        sidecar.contains("SPDX-License-Identifier: CC-BY-4.0"),
        "declared identifier added: {sidecar:?}"
    );
    assert!(
        sidecar.contains("SPDX-FileCopyrightText: 2026 Acme"),
        "notice kept: {sidecar:?}"
    );
}

#[test]
fn override_without_license_leaves_file_missing_header() {
    // An override stanza that carries no license is ineffective for
    // licensing: the file still fails the gate with its own path, and apply
    // fixes it where the coverage lives.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nfile=\"logo.png\"\nlicense=\"CC-BY-4.0\"\n");
    std::fs::write(f.path().join("logo.png"), PNG).unwrap();
    f.write(
        "REUSE.toml",
        "version = 1\n\n[[annotations]]\npath = \"logo.png\"\nprecedence = \"override\"\n\
         SPDX-FileCopyrightText = \"2026 Acme\"\n",
    );
    f.commit("init");

    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let entry = v["files"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|e| e["path"] == "logo.png")
        .expect("logo.png in report");
    // The binary carries no detectable license anywhere: without a usable
    // license record the override is ineffective for licensing.
    assert_eq!(entry["drift"], "unreadable");

    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        check_compliant(&f, "logo.png")["summary"]["counts"]["compliant"],
        1
    );
}

#[test]
fn toml_covered_source_file_gets_annotation_not_header() {
    // Provenance picks the destination before any comment syntax: a source
    // file already covered by REUSE.toml is fixed in the document, never with
    // an in-file header — even though a comment style exists for it.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nfile=\"a.rs\"\nlicense=\"Apache-2.0\"\n")
        .write("a.rs", "fn a(){}\n")
        .write(
            "REUSE.toml",
            "version = 1\n\n[[annotations]]\npath = \"a.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
        )
        .commit("init");

    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(f.read("a.rs"), "fn a(){}\n", "no in-file header inserted");
    let reuse = f.read("REUSE.toml");
    assert_eq!(reuse.matches("[[annotations]]").count(), 2, "{reuse}");
    assert!(
        reuse.contains("SPDX-License-Identifier = \"Apache-2.0\""),
        "{reuse}"
    );

    let check = f.licet().arg("check").output().unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}
// REUSE-IgnoreEnd
