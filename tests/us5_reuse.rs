//! US5 — REUSE compatibility & migration (SC-007, SC-008, FR-014, FR-017, FR-018, FR-028).
//! Quickstart Scenario 5.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn init_generates_config_reproducing_current_licensing() {
    let f = Fixture::new();
    // Existing headers: mostly MIT, one rust file Apache.
    f.write("a.py", "# SPDX-License-Identifier: MIT\nx=1\n")
        .write("b.py", "# SPDX-License-Identifier: MIT\ny=2\n")
        .write("c.md", "<!-- SPDX-License-Identifier: MIT -->\n# c\n")
        .write(
            "lib.rs",
            "// SPDX-License-Identifier: Apache-2.0\nfn l(){}\n",
        )
        .commit("init");

    let out = f.licet().args(["init", "--from-reuse"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cfg = f.read("license.toml");
    // Most common license (MIT) becomes the default; rust (Apache) becomes an ext rule.
    assert!(cfg.contains("[default]") && cfg.contains("MIT"), "{cfg}");
    assert!(
        cfg.contains("ext = \"rs\"") && cfg.contains("Apache-2.0"),
        "{cfg}"
    );
}

#[test]
fn lint_reports_spdx_list_version_and_missing_texts() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");
    let out = f.licet().arg("lint").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("SPDX list"),
        "version transparency (FR-028): {stdout}"
    );
    // MIT text not yet in LICENSES/ → reported missing, available offline.
    assert!(stdout.contains("MIT"), "{stdout}");
}

#[test]
fn version_reports_embedded_spdx_list_version() {
    let f = Fixture::new();
    let out = f.licet().arg("--version").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("licet") && stdout.contains("SPDX license list"),
        "{stdout}"
    );
}

/// The `--version` line is an exact contract: `licet <x.y.z> (SPDX license
/// list <list>)`, with the manifest version and the embedded list snapshot.
#[test]
fn version_line_is_exact() {
    let f = Fixture::new();
    let out = f.licet().arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout,
        format!(
            "licet {} (SPDX license list {})\n",
            env!("CARGO_PKG_VERSION"),
            licet::spdx::spdx_list_version()
        ),
        "exact --version contract"
    );
    // The `add` alias spells the same command as `add-license`.
    let out = f
        .licet()
        .args(["add", "LicenseRef-Acme-1.0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("LicenseRef-Acme-1.0"),
        "alias `add` behaves as `add-license`"
    );
}

#[test]
fn reuse_conformance_after_apply() {
    // SC-007: a reconciled repo passes the real `reuse lint`, when copyright is supplied.
    if which_reuse().is_none() {
        eprintln!("skipping: `reuse` not installed");
        return;
    }
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\ncopyright=\"add:2026 Test Author\"\n")
        .write("a.rs", "fn a(){}\n")
        .write("b.py", "x=1\n")
        .commit("init");
    let apply = f.licet().arg("apply").output().unwrap();
    assert!(
        apply.status.success(),
        "apply: {}",
        String::from_utf8_lossy(&apply.stdout)
    );

    let reuse = std::process::Command::new("reuse")
        .current_dir(f.path())
        .arg("lint")
        .output()
        .unwrap();
    assert!(
        reuse.status.success(),
        "reuse lint should pass on reconciled repo:\n{}\n{}",
        String::from_utf8_lossy(&reuse.stdout),
        String::from_utf8_lossy(&reuse.stderr)
    );
}

fn which_reuse() -> Option<()> {
    std::process::Command::new("reuse")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| ())
}

/// Build a nested-metadata fixture: optional root/child `REUSE.toml`
/// documents over `sub/f.rs` with the given bytes.
fn matrix_fixture(root_doc: Option<&str>, sub_doc: Option<&str>, file: &str) -> Fixture {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"PLACEHOLDER\"\n");
    if let Some(doc) = root_doc {
        f.write("REUSE.toml", doc);
    }
    if let Some(doc) = sub_doc {
        f.write("sub/REUSE.toml", doc);
    }
    std::fs::create_dir_all(f.path().join("sub")).unwrap();
    std::fs::write(f.path().join("sub/f.rs"), file).unwrap();
    f.commit("init");
    f
}

fn check_file(f: &Fixture, intent: &str) -> serde_json::Value {
    std::fs::write(
        f.path().join("license.toml"),
        format!("[default]\nlicense=\"{intent}\"\n"),
    )
    .unwrap();
    let out = f
        .licet()
        .args(["check", "--format", "json", "--files", "sub/f.rs"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn file_entry(v: &serde_json::Value) -> &serde_json::Value {
    v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "sub/f.rs")
        .expect("sub/f.rs in report")
}

#[test]
fn precedence_matrix_closest_and_file() {
    // Row 1: root closest A, file B → B wins (fallback unused).
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        None,
        "// SPDX-License-Identifier: Apache-2.0\nfn x(){}\n",
    );
    let v = check_file(&f, "Apache-2.0");
    assert_eq!(file_entry(&v)["drift"], "compliant");
    assert_eq!(file_entry(&v)["actual"], "Apache-2.0");
}

#[test]
fn precedence_matrix_nested_closest() {
    // Row 2: root closest A, child closest B, bare file → B (nearest fallback).
    // The root supplies the copyright fallback, so both tables contribute.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nSPDX-License-Identifier = \"MIT\"\nSPDX-FileCopyrightText = \"2026 Root\"\n",
        ),
        Some(
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        ),
        "fn x(){}\n",
    );
    let v = check_file(&f, "Apache-2.0");
    assert_eq!(file_entry(&v)["drift"], "compliant");
    assert_eq!(file_entry(&v)["actual"], "Apache-2.0");
    // Both contributing tables are explained with provenance, shallowest first.
    let origins = file_entry(&v)["metadata_origins"].as_array().unwrap();
    assert_eq!(origins.len(), 2);
    assert_eq!(origins[0]["metadata"], "REUSE.toml");
    assert_eq!(origins[0]["copyrights"], serde_json::json!(["2026 Root"]));
    assert_eq!(origins[1]["metadata"], "sub/REUSE.toml");
    assert_eq!(origins[1]["licenses"], serde_json::json!(["Apache-2.0"]));
}

#[test]
fn noncontributing_closest_table_has_no_provenance() {
    // A closest table that loses the per-field fallback race contributes
    // nothing, so it is excluded from the provenance explanation.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        Some(
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        ),
        "fn x(){}\n",
    );
    let v = check_file(&f, "Apache-2.0");
    let origins = file_entry(&v)["metadata_origins"].as_array().unwrap();
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0]["metadata"], "sub/REUSE.toml");
}

#[test]
fn precedence_matrix_child_override_suppresses_file() {
    // Row 3: root closest A, child override B, complete file C → B governs;
    // the file's own license is suppressed, the root stays as fallback.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nSPDX-License-Identifier = \"MIT\"\nSPDX-FileCopyrightText = \"2026 Root\"\n",
        ),
        Some(
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"override\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        ),
        "// SPDX-License-Identifier: GPL-2.0-only\n// SPDX-FileCopyrightText: 2026 File\nfn x(){}\n",
    );
    // Policy equality joins all effective expressions: barrier Apache plus
    // the root fallback MIT.
    let v = check_file(&f, "Apache-2.0 AND MIT");
    assert_eq!(file_entry(&v)["drift"], "compliant");
    assert_eq!(file_entry(&v)["actual"], "Apache-2.0");
    // A lone Apache intent no longer covers the combination, and the
    // suppressed file license satisfies nothing.
    for intent in ["Apache-2.0", "GPL-2.0-only"] {
        let v = check_file(&f, intent);
        assert_eq!(file_entry(&v)["drift"], "wrong_license", "intent {intent}");
    }
}

#[test]
fn precedence_matrix_aggregate_adds_to_file() {
    // Rows 4–5: a parent aggregate always contributes; the closest table is
    // fallback-only. With a file license C present, C stays primary.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        Some(
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        ),
        "// SPDX-License-Identifier: GPL-2.0-only\nfn x(){}\n",
    );
    let v = check_file(&f, "GPL-2.0-only AND MIT");
    assert_eq!(file_entry(&v)["drift"], "compliant");
    assert_eq!(file_entry(&v)["actual"], "GPL-2.0-only");
}

#[test]
fn precedence_matrix_aggregate_without_file() {
    // Row 5: aggregate A + closest B over a bare file → A primary, B fallback.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        Some(
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        ),
        "fn x(){}\n",
    );
    let v = check_file(&f, "MIT AND Apache-2.0");
    assert_eq!(file_entry(&v)["drift"], "compliant");
    assert_eq!(file_entry(&v)["actual"], "MIT");
}

#[test]
fn precedence_matrix_rootmost_override_wins() {
    // Row 6: two overrides, rootmost A governs; B and the file are suppressed.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nprecedence = \"override\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        Some(
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"override\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        ),
        "// SPDX-License-Identifier: GPL-2.0-only\nfn x(){}\n",
    );
    let v = check_file(&f, "MIT");
    assert_eq!(file_entry(&v)["drift"], "compliant");
    assert_eq!(file_entry(&v)["actual"], "MIT");
    let v = check_file(&f, "Apache-2.0");
    assert_eq!(file_entry(&v)["drift"], "wrong_license");
}

#[test]
fn precedence_matrix_child_aggregate_adds_to_file() {
    // Row 7: root closest A, child aggregate B, file C → C primary, B added.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        Some(
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        ),
        "// SPDX-License-Identifier: GPL-2.0-only\nfn x(){}\n",
    );
    let v = check_file(&f, "GPL-2.0-only AND Apache-2.0");
    assert_eq!(file_entry(&v)["drift"], "compliant");
    assert_eq!(file_entry(&v)["actual"], "GPL-2.0-only");
}

#[test]
fn precedence_matrix_sidecar_beats_source() {
    // Row 8: a `.license` sidecar carries the file's licensing information in
    // place of the source header (REUSE 3.3 §"Order of precedence").
    let f = Fixture::new();
    f.config("[default]\nlicense=\"PLACEHOLDER\"\n")
        .write(
            "a.rs",
            "// SPDX-License-Identifier: GPL-2.0-only\nfn a(){}\n",
        )
        .write(
            "a.rs.license",
            "SPDX-License-Identifier: Apache-2.0\nSPDX-FileCopyrightText: 2026 Acme\n",
        )
        .commit("init");
    std::fs::write(
        f.path().join("license.toml"),
        "[default]\nlicense=\"Apache-2.0\"\n",
    )
    .unwrap();
    let out = f
        .licet()
        .args(["check", "--format", "json", "--files", "a.rs"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let entry = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "a.rs")
        .unwrap();
    assert_eq!(entry["drift"], "compliant");
    assert_eq!(entry["actual"], "Apache-2.0");
}

#[test]
fn root_copyright_only_override_leaves_license_missing() {
    // An override that omits the license suppresses the file's license without
    // reopening it: the file is missing a license even though its header names
    // one, while copyright still validates from the file in task-5 lint.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nprecedence = \"override\"\nSPDX-FileCopyrightText = \"2026 Root\"\n",
        ),
        None,
        "// SPDX-License-Identifier: Apache-2.0\n// SPDX-FileCopyrightText: 2026 File\nfn x(){}\n",
    );
    let v = check_file(&f, "MIT");
    assert_eq!(file_entry(&v)["drift"], "missing_header");
}

#[test]
fn parent_aggregate_survives_child_override() {
    // Parent aggregate A is farther than the child override barrier, so it is
    // retained; the file itself is suppressed.
    let f = matrix_fixture(
        Some(
            "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        Some(
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"override\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
        ),
        "// SPDX-License-Identifier: GPL-2.0-only\nfn x(){}\n",
    );
    let v = check_file(&f, "Apache-2.0 AND MIT");
    assert_eq!(file_entry(&v)["drift"], "compliant", "combination covers");
    // Neither half of the combination covers it alone, and the suppressed
    // file license satisfies nothing.
    for intent in ["MIT", "Apache-2.0", "GPL-2.0-only"] {
        let v = check_file(&f, intent);
        assert_eq!(file_entry(&v)["drift"], "wrong_license", "intent {intent}");
    }
}

#[test]
fn staged_nested_reuse_toml_reads_index_bytes() {
    // Nested metadata participates in the staged snapshot: prefetched index
    // blobs, never the working copy.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"license.toml\"]\n")
        .write("sub/f.rs", "// SPDX-License-Identifier: MIT\n")
        .write(
            "sub/REUSE.toml",
            "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"override\"\nSPDX-License-Identifier = \"MIT\"\n",
        )
        .commit("base");
    f.write(
        "sub/REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"override\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
    )
    .stage_all();
    f.write(
        "sub/REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"override\"\nSPDX-License-Identifier = \"MIT\"\n",
    );
    let out = f
        .licet()
        .args(["check", "--staged", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "sub/f.rs" && entry["actual"] == "Apache-2.0"),
        "staged nested annotation governs: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn license_array_covers_binary_and_inventories_both_texts() {
    // One table with two expressions covers a binary asset; both expressions
    // are effective and both texts are inventoried.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT AND Apache-2.0\"\n");
    std::fs::write(f.path().join("logo.png"), [0xffu8, 0xfe, 0x00, 0x41]).unwrap();
    f.write(
        "REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"logo.png\"\nprecedence = \"aggregate\"\n\
         SPDX-License-Identifier = [\"MIT\", \"Apache-2.0\"]\nSPDX-FileCopyrightText = \"2026 Acme\"\n",
    )
    .commit("init");
    let out = f
        .licet()
        .args(["check", "--format", "json", "--files", "logo.png"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let entry = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "logo.png")
        .unwrap();
    assert_eq!(entry["drift"], "compliant", "{v:?}");
    // Both expressions are inventoried even though the binary has no header.
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
    assert!(ids.contains(&"MIT"), "MIT inventoried: {ids:?}");
    assert!(
        ids.contains(&"Apache-2.0"),
        "second array expression inventoried: {ids:?}"
    );
    assert!(
        v["license_texts"]["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "Apache-2.0"),
        "missing second text is reported"
    );
}

#[test]
fn dep5_aggregates_with_file_header() {
    // Legacy dep5 adds its license to the file header's (REUSE 3.3 §"Order of
    // precedence"): the file stays primary and both texts are inventoried.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT AND Apache-2.0\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write(
            ".reuse/dep5",
            "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\
             \n\
             Files: a.rs\nCopyright: 2026 Acme\nLicense: Apache-2.0\n",
        )
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--format", "json", "--files", "a.rs"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let entry = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "a.rs")
        .unwrap();
    assert_eq!(entry["drift"], "compliant", "{v:?}");
    assert_eq!(entry["actual"], "MIT");
    assert_eq!(entry["actual_source"], "header");
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
        ids.contains(&"Apache-2.0"),
        "dep5 license inventoried: {ids:?}"
    );
}

#[test]
fn malformed_reuse_metadata_fails_before_writes() {
    // Every malformed-metadata shape fails with exit 2 naming the document,
    // and `apply` changes no file bytes.
    let cases: &[(&str, &str)] = &[
        (
            "malformed-toml",
            "version = 1\n[[annotations]]\npath = \nSPDX-License-Identifier = \"MIT\"\n",
        ),
        (
            "version-2",
            "version = 2\n[[annotations]]\npath = \"a.rs\"\n",
        ),
        (
            "missing-version",
            "[[annotations]]\npath = \"a.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        (
            "missing-path",
            "version = 1\n[[annotations]]\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        (
            "bad-precedence",
            "version = 1\n[[annotations]]\npath = \"a.rs\"\nprecedence = \"bogus\"\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        (
            "bad-expression",
            "version = 1\n[[annotations]]\npath = \"a.rs\"\nSPDX-License-Identifier = \"NOT-A-LICENSE\"\n",
        ),
    ];
    for (name, doc) in cases {
        let f = Fixture::new();
        f.config("[default]\nlicense=\"MIT\"\n")
            .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
            .write("REUSE.toml", doc)
            .commit("init");
        let before = f.read("a.rs");
        let out = f.licet().args(["apply", "--allow-dirty"]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{name}: exit 2");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("REUSE.toml"),
            "{name}: error names the document: {stderr}"
        );
        assert_eq!(f.read("a.rs"), before, "{name}: no writes happen");
        assert_eq!(f.read("REUSE.toml"), *doc, "{name}: metadata untouched");
    }
}

#[test]
fn apply_never_creates_reuse_toml_beside_dep5() {
    // With only `.reuse/dep5` present, a fix that would need a REUSE.toml
    // annotation is reported as unfixable (nonzero) instead of creating the
    // mutually-exclusive document.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write(
            ".reuse/dep5",
            "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n",
        )
        .commit("init");
    std::fs::write(f.path().join("logo.png"), [0xffu8, 0xfe, 0x00, 0x41]).unwrap();
    let out = f
        .licet()
        .args([
            "apply",
            "--allow-dirty",
            "--non-annotatable",
            "reuse-toml",
            "--files",
            "logo.png",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        !f.path().join("REUSE.toml").exists(),
        "no REUSE.toml may be created beside .reuse/dep5"
    );
}

#[test]
fn lint_requires_copyright_independently_of_license_intent() {
    let f = Fixture::new();
    f.config("# SPDX-License-Identifier: MIT\n# SPDX-FileCopyrightText: 2026 Test\n[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\n")
        .write("LICENSES/MIT.txt", licet::spdx::bundled_text("MIT").unwrap());
    let out = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["summary"]["pass"], false);
    assert!(
        out.stdout
            .windows(b"missing_copyright".len())
            .any(|bytes| bytes == b"missing_copyright")
    );
}

fn lint_json(f: &Fixture) -> (i32, serde_json::Value) {
    let out = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    (out.status.code().unwrap(), v)
}

fn warning_kinds(v: &serde_json::Value) -> Vec<String> {
    v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["code"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn lint_succeeds_without_any_config_when_metadata_complete() {
    // REUSE validation is declaration-independent: no license.toml at all.
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write(
        "LICENSES/MIT.txt",
        licet::spdx::bundled_text("MIT").unwrap(),
    );
    let (code, report) = lint_json(&f);
    assert_eq!(code, 0, "complete metadata needs no declaration: {report}");
}

#[test]
fn lint_empty_known_text_is_accepted() {
    // An empty but correctly named text satisfies presence (the reference
    // tool likewise reports no finding for it): licet never claims to prove
    // legal correctness of file contents.
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write("LICENSES/MIT.txt", "");
    let (code, report) = lint_json(&f);
    assert_eq!(code, 0, "{report}");
}

#[test]
fn lint_unused_text_fails_with_precise_set() {
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write(
        "LICENSES/MIT.txt",
        licet::spdx::bundled_text("MIT").unwrap(),
    )
    .write(
        "LICENSES/Apache-2.0.txt",
        licet::spdx::bundled_text("Apache-2.0").unwrap(),
    );
    let (code, report) = lint_json(&f);
    assert_eq!(code, 1);
    assert!(warning_kinds(&report).contains(&"unused_license_text".to_string()));
    assert_eq!(
        report["license_texts"]["unused"],
        serde_json::json!(["Apache-2.0"])
    );
    assert_eq!(report["license_texts"]["missing"], serde_json::json!([]));
}

#[test]
fn lint_unknown_file_is_bad_text() {
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write(
        "LICENSES/MIT.txt",
        licet::spdx::bundled_text("MIT").unwrap(),
    )
    .write("LICENSES/Unknown-Thing.txt", "mystery\n");
    let (code, report) = lint_json(&f);
    assert_eq!(code, 1);
    assert!(warning_kinds(&report).contains(&"bad_license_text".to_string()));
    assert_eq!(
        report["license_texts"]["unrecognized"],
        serde_json::json!(["LICENSES/Unknown-Thing.txt"])
    );
}

#[test]
fn lint_extensionless_text_reports_missing_extension() {
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write("LICENSES/MIT", licet::spdx::bundled_text("MIT").unwrap());
    let (code, report) = lint_json(&f);
    assert_eq!(code, 1, "strict lint reports the missing extension");
    assert!(warning_kinds(&report).contains(&"missing_license_extension".to_string()));
    assert_eq!(report["license_texts"]["missing"], serde_json::json!([]));
    // ...while policy check still recognizes the text.
    f.config("[default]\nlicense=\"MIT\"\n");
    let out = f
        .licet()
        .args(["check", "--files", "a.rs"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn lint_duplicate_texts_are_usage_error() {
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write(
        "LICENSES/MIT.txt",
        licet::spdx::bundled_text("MIT").unwrap(),
    )
    .write("LICENSES/MIT.md", licet::spdx::bundled_text("MIT").unwrap());
    let out = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("MIT"), "names the duplicated id: {stderr}");
}

#[test]
fn lint_undecodable_text_is_incomplete_not_violation() {
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    );
    std::fs::create_dir_all(f.path().join("LICENSES")).unwrap();
    std::fs::write(f.path().join("LICENSES/MIT.txt"), [0xffu8, 0xfe]).unwrap();
    let (code, report) = lint_json(&f);
    assert_eq!(code, 1);
    assert!(warning_kinds(&report).contains(&"unsupported_encoding".to_string()));
    assert_eq!(report["summary"]["pass"], false);
}

#[test]
fn lint_requires_exception_texts() {
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write("LICENSES/GPL-2.0-only.txt", "gpl\n");
    let (code, report) = lint_json(&f);
    assert_eq!(code, 1);
    assert_eq!(
        report["license_texts"]["missing"],
        serde_json::json!(["Classpath-exception-2.0"])
    );
}

#[test]
fn lint_explicit_config_missing_or_malformed_is_usage_error() {
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write(
        "LICENSES/MIT.txt",
        licet::spdx::bundled_text("MIT").unwrap(),
    );
    for cfg in ["does/not-exist.toml", "license.toml"] {
        if cfg == "license.toml" {
            f.write("license.toml", "[default\nbroken\n");
        }
        let out = f.licet().args(["lint", "--config", cfg]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "explicit config {cfg}");
    }
}

#[test]
fn lint_explicit_valid_config_is_accepted_but_ignored() {
    let f = Fixture::new();
    // The config file itself is a covered file, so it carries its own tags.
    f.config("# SPDX-License-Identifier: MIT\n# SPDX-FileCopyrightText: 2026 Acme\n[default]\nlicense=\"Apache-2.0\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n")
        .write("LICENSES/MIT.txt", licet::spdx::bundled_text("MIT").unwrap());
    // The declared Apache intent is irrelevant: actual MIT metadata validates.
    let out = f
        .licet()
        .args(["lint", "--format", "json", "--config", "license.toml"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("deprecat"), "deprecation notice: {stderr}");
}

#[test]
fn lint_annotated_but_malformed_policy_toml_is_covered_file() {
    // An auto-discovered license.toml that fails to parse is not a lint
    // blocker: with its own SPDX tags it validates like any covered file.
    let f = Fixture::new();
    f.write(
        "license.toml",
        "# SPDX-License-Identifier: MIT\n# SPDX-FileCopyrightText: 2026 Acme\n[default\nbroken\n",
    )
    .write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n",
    )
    .write(
        "LICENSES/MIT.txt",
        licet::spdx::bundled_text("MIT").unwrap(),
    );
    let (code, _) = lint_json(&f);
    assert_eq!(code, 0);
}

#[test]
fn reuse_and_dep5_coexistence_fails_before_writes() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write(
            "REUSE.toml",
            "version = 1\n[[annotations]]\npath = \"a.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
        )
        .write(
            ".reuse/dep5",
            "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n",
        )
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("REUSE.toml") && stderr.contains(".reuse/dep5"),
        "error names both documents: {stderr}"
    );
}

/// Task 8 round-trip: `init` must preserve every observed effective license —
/// in-file headers, an extensionless exception, a sidecar-covered binary, and
/// nested REUSE coverage — while leaving the unlicensed file uncovered.
/// Projection is asserted behaviorally (per-file drift/actual through the real
/// binary), never by substring presence in the generated TOML.
#[test]
fn init_round_trip_preserves_all_observed_licensing() {
    let f = Fixture::new();
    f.write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write("b.rs", "// SPDX-License-Identifier: MIT\nfn b(){}\n")
        .write("c.rs", "// SPDX-License-Identifier: Apache-2.0\nfn c(){}\n")
        .write("NOTICE", "SPDX-License-Identifier: BSD-3-Clause\nnotes\n")
        .write(
            "asset.bin.license",
            "SPDX-License-Identifier: MIT\nSPDX-FileCopyrightText: 2026 Test\n",
        )
        .write(
            "sub/REUSE.toml",
            "version = 1\n[[annotations]]\npath = \"covered.dat\"\nSPDX-License-Identifier = \"ISC\"\n",
        )
        .write("sub/covered.dat", "opaque\n")
        .write("todo.py", "x = 1\n")
        .texts(&["MIT", "Apache-2.0", "BSD-3-Clause", "ISC"]);
    std::fs::write(f.path().join("asset.bin"), [0x00, 0xFF, 0x89, 0x50]).unwrap();
    f.commit("init");

    // Generate to a fresh output, then promote it to the policy config.
    let out = f
        .licet()
        .args(["init", "--output", "gen.toml"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "init succeeds: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::copy(f.path().join("gen.toml"), f.path().join("license.toml")).unwrap();

    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "unknown file fails the gate: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let drift_of = |name: &str| {
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == name)
            .unwrap_or_else(|| panic!("{name} must be reported"))["drift"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let actual_of = |name: &str| {
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == name)
            .unwrap()["actual"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // Every known effective license is equal before/after generation.
    for (name, lic) in [
        ("a.rs", "MIT"),
        ("b.rs", "MIT"),
        ("c.rs", "Apache-2.0"),
        ("NOTICE", "BSD-3-Clause"),
        ("asset.bin", "MIT"),
        ("sub/covered.dat", "ISC"),
    ] {
        assert_eq!(drift_of(name), "compliant", "{name} keeps its license");
        assert_eq!(actual_of(name), lic, "{name} effective license preserved");
    }
    // The unlicensed file remains uncovered — no default was invented for it.
    assert_eq!(drift_of("todo.py"), "uncovered");
}

/// `init` creates new by default and refuses to overwrite without `--force`;
/// `--force` replaces exactly the observed bytes. A failure changes neither
/// sources nor the existing config.
#[test]
fn init_refuses_overwrite_without_force() {
    let f = Fixture::new();
    f.write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");

    let out = f.licet().arg("init").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let first = f.read("license.toml");

    let out = f.licet().arg("init").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "second init refuses without --force"
    );
    assert_eq!(f.read("license.toml"), first, "existing config untouched");
    assert_eq!(
        f.read("a.rs"),
        "// SPDX-License-Identifier: MIT\nfn a(){}\n",
        "source untouched"
    );

    let out = f.licet().args(["init", "--force"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "--force regenerates: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        f.read("license.toml"),
        first,
        "identical intent → identical bytes"
    );
}

/// `init --format json` joins the v2 envelope: one document on stdout with the
/// config write record and unknown paths as diagnostics.
#[test]
fn init_json_joins_report_envelope() {
    let f = Fixture::new();
    f.write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write("todo.py", "x = 1\n")
        .commit("init");

    let out = f
        .licet()
        .args(["init", "--format", "json", "--output", "gen.toml"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["version"], 2);
    assert_eq!(report["command"], "init");
    assert_eq!(report["summary"]["pass"], true);
    let writes = report["writes"].as_array().unwrap();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0]["kind"], "config");
    assert_eq!(writes[0]["status"], "applied");
    assert!(writes[0]["after_text"].as_str().unwrap().contains("MIT"));
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d["path"] == "todo.py" && d["code"] == "missing_license"),
        "unknown path reported: {diagnostics:?}"
    );
}

/// A symlinked destination is never followed or truncated, even with `--force`.
#[cfg(unix)]
#[test]
fn init_never_writes_through_symlink_destination() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write("real.toml", "SENTINEL = 1\n")
        .commit("init");
    symlink(f.path().join("real.toml"), f.path().join("link.toml")).unwrap();

    let out = f
        .licet()
        .args(["init", "--force", "--output", "link.toml"])
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "symlink destination refused");
    assert_eq!(
        f.read("real.toml"),
        "SENTINEL = 1\n",
        "link target untouched"
    );
}

/// Paths with quotes survive generation: TOML escaping round-trips and the
/// projection still verifies (Unix-only: quotes are illegal on Windows).
#[cfg(unix)]
#[test]
fn init_handles_quote_paths() {
    let f = Fixture::new();
    f.write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write("we\"ird.rs", "// SPDX-License-Identifier: MIT\nfn w(){}\n")
        .commit("init");

    let out = f
        .licet()
        .args(["init", "--output", "gen.toml"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "quoted path generates: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::copy(f.path().join("gen.toml"), f.path().join("license.toml")).unwrap();
    let out = f
        .licet()
        .args(["check", "--format", "json", "--files", "we\"ird.rs"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["files"][0]["drift"], "compliant");
}
// REUSE-IgnoreEnd
