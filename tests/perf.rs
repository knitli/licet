//! Performance gate (SC-006): a full scan of a multi-thousand-file repo must complete
//! well within budget. This is a coarse timing guard (not a microbenchmark) sized to be
//! stable in CI; the spec's 10k-file <1s warm / <3s cold bars are validated on the
//! reference 4-core runner. Here we assert a generous ceiling to catch gross regressions.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;
use std::time::Instant;

#[test]
fn scans_a_few_thousand_files_quickly() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"license.toml\"]\n");
    let n = 3000;
    for i in 0..n {
        let dir = i / 100;
        f.write(
            &format!("src/d{dir}/f{i}.rs"),
            "// SPDX-License-Identifier: MIT\nfn f(){}\n",
        );
    }

    // Cold run (no cache).
    let t0 = Instant::now();
    let out = f.licet().args(["check", "--no-cache"]).output().unwrap();
    let cold = t0.elapsed();
    assert_eq!(out.status.code(), Some(0), "all compliant");

    // Generous ceiling for a 3k-file cold scan on shared CI hardware.
    assert!(
        cold.as_secs_f64() < 10.0,
        "cold scan of {n} files took {cold:?}, expected < 10s (regression guard)"
    );
    eprintln!("cold scan of {n} files: {cold:?}");
}
// REUSE-IgnoreEnd
