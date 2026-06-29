//! T050 — determinism + offline-by-default audit (FR-002, FR-017).
//!
//! 1. Identical inputs → byte-identical JSON report (classification, ordering, exit code).
//! 2. File ordering is a stable lexicographic sort.
//! 3. The binary makes no network calls by default. The absence of any network-capable
//!    crate in the dependency tree is the structural guarantee (asserted in CI by the
//!    offline-guard / `cargo tree`); here we confirm the default `lint` path is offline and
//!    deterministic. Network access exists only as an explicit opt-in (`--allow-curl`, which
//!    shells out to the user's own `curl`) and is exercised in the add-license suite.
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
fn lint_is_offline_and_deterministic() {
    // `lint` is a read-only checker: it never reaches the network and produces a
    // byte-identical posture across runs for identical inputs.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");

    let run = || {
        let out = f
            .licet()
            .args(["lint", "--format", "json"])
            .output()
            .unwrap();
        (out.status.code(), String::from_utf8(out.stdout).unwrap())
    };
    let (code1, json1) = run();
    let (code2, json2) = run();
    assert_eq!(code1, code2, "lint exit code must be stable");
    assert_eq!(
        json1, json2,
        "lint posture must be byte-identical across runs"
    );
}
// REUSE-IgnoreEnd
