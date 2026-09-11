//! REUSE ignore blocks and SPDX snippets in detection (FR-030). A snippet's license
//! describes the snippet, not the file, so it never satisfies (or violates) the file's
//! declared intent — but its text still counts for `LICENSES/` completeness. Information
//! inside `REUSE-IgnoreStart`..`REUSE-IgnoreEnd` is dropped entirely.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;

use common::Fixture;

fn check_json(f: &Fixture, file: &str) -> serde_json::Value {
    let out = f
        .licet()
        .args(["check", "--format", "json", "--files", file])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn snippet_file(padding_lines: usize) -> String {
    // A file whose snippet region sits `padding_lines` into the body (past the
    // old 8 KiB head window when large): full-content scans still find it.
    let mut s = String::from("// SPDX-License-Identifier: MIT\nfn f() {}\n");
    for i in 0..padding_lines {
        s.push_str(&format!("// filler line {i}\n"));
    }
    s.push_str(
        "// SPDX-SnippetBegin\n// SPDX-License-Identifier: BSD-3-Clause\nfn vendored() {}\n// SPDX-SnippetEnd\n",
    );
    s
}

#[test]
fn late_snippet_is_detected_and_inventoried() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n");
    f.write("src/lib.rs", &snippet_file(2000)).commit("init");
    let v = check_json(&f, "src/lib.rs");
    assert_eq!(v["summary"]["counts"]["compliant"], 1);
    let out = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = v["license_texts"]["referenced"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    assert!(
        ids.contains(&"BSD-3-Clause"),
        "late snippet referenced: {ids:?}"
    );
}

#[test]
fn excluded_file_snippet_text_stays_required() {
    // Declaration exclusion removes a file from policy drift (policy flows do
    // not even read it), but lint ignores declaration exclusions: the snippet
    // still exists in the tree, so its text stays required there.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"vendored.rs\"]\n");
    f.write(
        "vendored.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn f() {}\n// SPDX-SnippetBegin\n// SPDX-License-Identifier: BSD-3-Clause\nfn v() {}\n// SPDX-SnippetEnd\n",
    )
    .write("LICENSES/MIT.txt", licet::spdx::bundled_text("MIT").unwrap())
    .commit("init");
    let out = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "snippet text still required");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["license_texts"]["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "BSD-3-Clause")
    );
}

#[test]
fn snippet_license_is_inventoried_but_not_file_drift() {
    // File is MIT; an embedded snippet is BSD-3-Clause. The file stays compliant on MIT,
    // and BSD-3-Clause is still referenced so its text must live under LICENSES/.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n");
    f.write(
        "src/lib.rs",
        "// SPDX-License-Identifier: MIT\n\
         // SPDX-FileCopyrightText: 2026 Acme\n\n\
         fn f() {}\n\n\
         // SPDX-SnippetBegin\n\
         // SPDX-SnippetCopyrightText: 2022 Jane Doe\n\
         // SPDX-License-Identifier: BSD-3-Clause\n\
         fn vendored() {}\n\
         // SPDX-SnippetEnd\n",
    )
    .commit("init");

    // Drift sees only the file's own license.
    assert_eq!(
        check_json(&f, "src/lib.rs")["summary"]["counts"]["compliant"],
        1
    );

    // The snippet license is referenced for LICENSES/ completeness.
    let out = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let referenced = v["license_texts"]["referenced"].as_array().unwrap();
    let ids: Vec<&str> = referenced.iter().filter_map(|x| x.as_str()).collect();
    assert!(
        ids.contains(&"BSD-3-Clause"),
        "snippet license referenced: {ids:?}"
    );
    assert!(ids.contains(&"MIT"), "file license referenced: {ids:?}");
}

#[test]
fn snippet_license_does_not_satisfy_intent() {
    // Declared intent is BSD-3-Clause, but only a *snippet* is BSD; the file itself is MIT.
    // The snippet must not make the file compliant — this is WrongLicense.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"BSD-3-Clause\"\n");
    f.write(
        "src/lib.rs",
        "// SPDX-License-Identifier: MIT\n\n\
         fn f() {}\n\n\
         // SPDX-SnippetBegin\n\
         // SPDX-License-Identifier: BSD-3-Clause\n\
         fn vendored() {}\n\
         // SPDX-SnippetEnd\n",
    )
    .commit("init");

    assert_eq!(
        check_json(&f, "src/lib.rs")["summary"]["counts"]["wrong_license"],
        1
    );
}

#[test]
fn ignore_block_hides_spdx_tags() {
    // The file has no real header — its only SPDX line sits inside an ignore block (e.g.
    // example/output text). It must read as missing a header, not as MIT-licensed.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n");
    f.write(
        "src/lib.rs",
        "// REUSE-IgnoreStart\n\
         // SPDX-License-Identifier: MIT\n\
         // REUSE-IgnoreEnd\n\n\
         fn f() {}\n",
    )
    .commit("init");

    assert_eq!(
        check_json(&f, "src/lib.rs")["summary"]["counts"]["missing_header"],
        1
    );
}
// REUSE-IgnoreEnd
