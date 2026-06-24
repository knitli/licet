//! Apply-safety, config-error, and detection-precedence suites
//! (FR-024, SC-010, FR-003a, config validation → exit 2).

mod common;
use common::Fixture;

#[test]
fn apply_refuses_on_dirty_tree() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    // Make the tree dirty.
    f.write("a.rs", "fn a(){}\n// edited\n");

    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "dirty tree refusal is a usage error"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("uncommitted"));
}

#[test]
fn apply_allow_dirty_overrides() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n");
    // Never committed → dirty, but --allow-dirty proceeds.
    let out = f.licet().args(["apply", "--allow-dirty"]).output().unwrap();
    assert_ne!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(f.read("a.rs").contains("SPDX-License-Identifier: MIT"));
}

#[test]
fn invalid_config_is_usage_error() {
    let f = Fixture::new();
    f.config("[[rule]]\next=\"rs\"\nlicense=\"Not A Real License\"\n")
        .write("a.rs", "fn a(){}\n");
    let out = f.licet().arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("configuration error"));
}

#[test]
fn out_of_band_wins_over_header_with_source_override_warning() {
    // FR-003a: REUSE.toml disagrees with the in-file header → out-of-band authoritative,
    // non-failing source_override warning emitted.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n")
        .write(
            "REUSE.toml",
            "version = 1\n[[annotations]]\npath = \"a.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
        );

    let out = f
        .licet()
        .args(["check", "--format", "json", "--files", "a.rs"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // Out-of-band MIT satisfies the MIT default → compliant despite the Apache header.
    assert_eq!(
        v["summary"]["counts"]["compliant"],
        1,
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let warnings = v["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings.iter().any(|w| w["kind"] == "source_override"),
        "expected source_override warning: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn line_endings_preserved_on_write() {
    // FR-026: CRLF files keep CRLF after a header insert.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\r\nfn b(){}\r\n")
        .commit("init");
    f.licet().arg("apply").output().unwrap();
    let content = f.read("a.rs");
    assert!(
        content.contains("SPDX-License-Identifier: MIT\r\n"),
        "CRLF preserved: {content:?}"
    );
}
