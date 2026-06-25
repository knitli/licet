//! US1 (T014) — snapshot tests for the human-readable drift report (FR-004, FR-020).
//! These pin the exact rendered text of `licet check` so layout regressions are caught.
//! File ordering is deterministic (walk sorts by relative path), and all paths are
//! repo-relative, so the snapshots are stable across machines/tempdirs.

mod common;
use common::Fixture;

/// A mixed repo: compliant, wrong-license (via default and via an ext rule), and a
/// missing header. Exercises the per-file lines + the summary/result footer.
#[test]
fn drift_report_human() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"Apache-2.0\"\n")
        .write("a_good.py", "# SPDX-License-Identifier: MIT\nx=1\n")
        .write(
            "b_wrong.py",
            "# SPDX-License-Identifier: GPL-3.0-only\ny=2\n",
        )
        .write("c_missing.py", "z=3\n")
        .write("d_wrong.rs", "// SPDX-License-Identifier: MIT\nfn d(){}\n")
        .commit("init");

    let out = f.licet().args(["check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "drift present → exit 1");
    let stdout = String::from_utf8(out.stdout).unwrap();
    insta::assert_snapshot!(stdout);
}

/// No default and a single non-matching rule → an uncovered file, which fails the gate.
#[test]
fn uncovered_report_human() {
    let f = Fixture::new();
    f.config("[[rule]]\next=\"rs\"\nlicense=\"MIT\"\n")
        .write("orphan.py", "x=1\n")
        .commit("init");

    let out = f.licet().args(["check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "uncovered → exit 1");
    let stdout = String::from_utf8(out.stdout).unwrap();
    insta::assert_snapshot!(stdout);
}

/// A fully compliant repo renders an empty file section with a PASS footer.
#[test]
fn compliant_report_human() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"license.toml\"]\n")
        .write("a.py", "# SPDX-License-Identifier: MIT\nx=1\n")
        .commit("init");

    let out = f.licet().args(["check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "all compliant → exit 0");
    let stdout = String::from_utf8(out.stdout).unwrap();
    insta::assert_snapshot!(stdout);
}
