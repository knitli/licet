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

    // Stateless run (scans never cache; the flag is a deprecated no-op).
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

/// Audit F16: SPDX parsing once did quadratic work on long `LicenseRef`
/// suffixes (about 34 ms at 1 KiB, 424 ms at 4 KiB, past a 4 s timeout at
/// 16 KiB in a debug build). After AST normalization the work must be
/// ~linear: increasing sizes with a generous hang guard only — no
/// microsecond assertions in ordinary tests.
#[test]
fn long_licenseref_scales_without_quadratic_blowup() {
    fn check_seconds_for_id_len(len: usize) -> f64 {
        let f = Fixture::new();
        f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"license.toml\"]\n")
            .write(
                "big.rs",
                &format!(
                    "// SPDX-License-Identifier: LicenseRef-{}\nfn x(){{}}\n",
                    "A".repeat(len)
                ),
            )
            .commit("init");
        let t0 = Instant::now();
        let out = f
            .licet()
            .args(["check", "--files", "big.rs"])
            .output()
            .unwrap();
        // Completes (wrong license, missing custom text) rather than hanging.
        assert_eq!(out.status.code(), Some(1));
        t0.elapsed().as_secs_f64()
    }

    let t4 = check_seconds_for_id_len(4_000);
    let t16 = check_seconds_for_id_len(16_000);
    eprintln!("licenseref 4k: {t4:.3}s, 16k: {t16:.3}s");
    assert!(
        t16 < 20.0,
        "16 KiB LicenseRef took {t16:.1}s (hang guard; the old parser exceeded 4 s)"
    );
    // Linear work quadruples for 4x input; quadratic would be ~16x. The 8x
    // bar plus a 50 ms noise floor leaves wide margin on shared hardware.
    assert!(
        t16 < 8.0 * t4.max(0.05),
        "16 KiB took {t16:.3}s vs 4 KiB {t4:.3}s — worse than linear"
    );
}
// REUSE-IgnoreEnd
