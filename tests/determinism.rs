//! T050 — determinism + offline-by-default audit (FR-002, FR-017).
//!
//! 1. Identical inputs → byte-identical JSON report (classification, ordering, exit code).
//! 2. File ordering is a stable lexicographic sort.
//! 3. The binary makes no network calls; `--allow-network` is advisory only. (The absence
//!    of any network-capable crate in the dependency tree is the structural guarantee —
//!    asserted in CI by `cargo tree`; here we confirm the offline paths behave identically.)
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

fn mixed_repo() -> Fixture {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"Apache-2.0\"\n")
        .write("z.py", "# SPDX-License-Identifier: MIT\nz=1\n")
        .write("a.py", "a=1\n")
        .write(
            "m.rs",
            "// SPDX-License-Identifier: GPL-3.0-only\nfn m(){}\n",
        )
        .write(
            "sub/b.rs",
            "// SPDX-License-Identifier: Apache-2.0\nfn b(){}\n",
        )
        .commit("init");
    f
}

#[test]
fn identical_inputs_produce_byte_identical_reports() {
    let f = mixed_repo();
    let run = |extra: &[&str]| {
        let out = f
            .licet()
            .args(["check", "--format", "json"])
            .args(extra)
            .output()
            .unwrap();
        (out.status.code(), String::from_utf8(out.stdout).unwrap())
    };
    let (code1, json1) = run(&[]);
    let (code2, json2) = run(&[]);
    assert_eq!(code1, code2, "exit code must be stable");
    assert_eq!(
        json1, json2,
        "JSON report must be byte-identical across runs"
    );
    // The deprecated flags are true no-ops: same bytes out, no files created.
    let (code3, json3) = run(&["--no-cache"]);
    assert_eq!((code1, json1), (code3, json3));
    assert!(
        !f.path().join(".git/licet-cache").exists(),
        "stateless scans must not create cache files"
    );
}

#[test]
fn file_ordering_is_lexicographically_sorted() {
    let f = mixed_repo();
    let out = f
        .licet()
        .args(["check", "--no-cache", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let paths: Vec<String> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "files must be emitted in sorted order");
}

/// Output equality cannot prove no subprocess ran; a call assertion can: even
/// with `--allow-network` and a missing standard text, `lint` only diagnoses
/// and never spawns `curl` (Unix-only: the shim needs an executable bit).
#[cfg(unix)]
#[test]
fn lint_never_invokes_curl_even_with_allow_network() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-1.0\nfn a(){}\n")
        .commit("init");

    let shim = tempfile::tempdir().unwrap();
    std::fs::write(
        shim.path().join("curl"),
        "#!/bin/sh\ntouch \"$FAKE_MARKER\"\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(
        shim.path().join("curl"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let marker = shim.path().join("invoked");
    let mut paths = vec![shim.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let out = f
        .licet()
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("FAKE_MARKER", &marker)
        .args(["lint", "--allow-network", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "missing text fails the gate");
    assert!(!marker.exists(), "lint must never invoke curl");
}
// REUSE-IgnoreEnd
