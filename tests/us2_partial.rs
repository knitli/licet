//! US2 — partial apply (FR-021): when some writes succeed and others fail, exit 3 and
//! report changed vs unchanged files.

#![cfg(unix)]

mod common;
use common::Fixture;
use std::os::unix::fs::PermissionsExt;

#[test]
fn partial_apply_exits_3_and_reports_changed_vs_unchanged() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("ok/a.rs", "fn a(){}\n")
        .write("locked/b.rs", "fn b(){}\n")
        .commit("init");

    // Make `locked/` unwritable so the atomic temp-file create fails for b.rs, while a.rs
    // in `ok/` still applies — a genuine partial outcome.
    let locked = f.path().join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

    let out = f.licet().args(["apply", "--format", "json"]).output().unwrap();

    // Restore perms so the TempDir can be cleaned up.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).ok();

    assert_eq!(
        out.status.code(),
        Some(3),
        "partial apply must exit 3: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["summary"]["partial"], true);
    // The writable file was changed; a partial_apply warning names the failed one.
    let warnings = v["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings.iter().any(|w| w["kind"] == "partial_apply"),
        "expected partial_apply warning: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(f.read("ok/a.rs").contains("SPDX-License-Identifier: MIT"));
}
