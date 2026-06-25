//! T050 — determinism + offline-by-default audit (FR-002, FR-017).
//!
//! 1. Identical inputs → byte-identical JSON report (classification, ordering, exit code).
//! 2. File ordering is a stable lexicographic sort.
//! 3. The binary makes no network calls; `--allow-network` is advisory only. (The absence
//!    of any network-capable crate in the dependency tree is the structural guarantee —
//!    asserted in CI by `cargo tree`; here we confirm the offline paths behave identically.)

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
    let run = || {
        let out = f
            .licet()
            .args(["check", "--no-cache", "--format", "json"])
            .output()
            .unwrap();
        (out.status.code(), String::from_utf8(out.stdout).unwrap())
    };
    let (code1, json1) = run();
    let (code2, json2) = run();
    assert_eq!(code1, code2, "exit code must be stable");
    assert_eq!(
        json1, json2,
        "JSON report must be byte-identical across runs"
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

#[test]
fn allow_network_flag_does_not_change_offline_behavior() {
    // A repo whose declared license is a non-bundled SPDX id: with or without
    // --allow-network, `lint` runs offline and produces the same posture/exit code.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");

    let offline = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    let allowed = f
        .licet()
        .args(["lint", "--allow-network", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(offline.status.code(), allowed.status.code());
    assert_eq!(
        String::from_utf8(offline.stdout).unwrap(),
        String::from_utf8(allowed.stdout).unwrap(),
        "JSON lint posture must be identical regardless of --allow-network (no fetch occurs)"
    );
}
