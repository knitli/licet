//! US4 — enforce in hook / CI (SC-006, SC-009, FR-013, FR-025, FR-027).
//! Quickstart Scenario 4.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn staged_subset_blocks_on_drift_and_names_file() {
    let f = Fixture::new();
    f.config(
        "[default]\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"LicenseRef-Marque-1.0\"\n",
    )
    .write(
        "good.rs",
        "// SPDX-License-Identifier: LicenseRef-Marque-1.0\nfn g(){}\n",
    )
    .commit("baseline");
    // Stage a drifted file.
    f.write("bad.rs", "// SPDX-License-Identifier: MIT\nfn b(){}\n")
        .stage_all();

    let out = f.licet().args(["check", "--staged"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "staged drift exits 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("bad.rs"), "names offending file: {stdout}");
}

#[test]
fn compliant_staged_set_passes() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .commit("baseline");
    f.write("ok.rs", "// SPDX-License-Identifier: MIT\nfn o(){}\n")
        .texts(&["MIT"])
        .stage_all();
    let out = f.licet().args(["check", "--staged"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn staged_check_reads_index_bytes() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("f.rs", "// SPDX-License-Identifier: MIT\n")
        .commit("base");
    f.write("f.rs", "// SPDX-License-Identifier: Apache-2.0\n")
        .stage_all();
    f.write("f.rs", "// SPDX-License-Identifier: MIT\n");
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
            .any(|entry| entry["path"] == "f.rs" && entry["actual"] == "Apache-2.0")
    );
}

/// The index content tree before/after a read-only command: staged checks must
/// never mutate the index.
fn index_tree(f: &Fixture) -> String {
    String::from_utf8_lossy(&f.git(&["write-tree"]))
        .trim()
        .to_string()
}

#[test]
fn staged_check_reverse_content_case() {
    // Index says MIT (compliant), worktree says Apache: the gate sees the index.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("f.rs", "// SPDX-License-Identifier: Apache-2.0\n")
        .texts(&["MIT", "Apache-2.0"])
        .commit("base");
    f.write("f.rs", "// SPDX-License-Identifier: MIT\n")
        .stage_all();
    f.write("f.rs", "// SPDX-License-Identifier: Apache-2.0\n");
    let before = index_tree(&f);
    let out = f
        .licet()
        .args(["check", "--staged", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "f.rs" && entry["actual"] == "MIT"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        index_tree(&f),
        before,
        "staged reads must not mutate the index"
    );
}

#[test]
fn non_utf8_file_is_unreadable_and_fails_gate() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n");
    // Write invalid UTF-8 bytes.
    std::fs::write(f.path().join("blob.rs"), [0xff, 0xfe, 0x00, 0x01, 0x80]).unwrap();
    let out = f
        .licet()
        .args(["check", "--files", "blob.rs"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("unreadable"));
}

#[test]
fn file_selection_normalizes_dot_segments() {
    // `src/x.rs`, `./src/x.rs`, and `src/../src/x.rs` name one file.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("src/x.rs", "fn x(){}\n")
        .commit("init");
    for spelling in ["src/x.rs", "./src/x.rs", "src/../src/x.rs"] {
        let out = f
            .licet()
            .args(["check", "--files", spelling, "--format", "json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{spelling}");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let files = report["files"].as_array().unwrap();
        assert_eq!(files.len(), 1, "{spelling}");
        assert_eq!(files[0]["path"], "src/x.rs", "{spelling}");
    }
}

#[test]
fn file_selection_from_subdirectory() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "fn a(){}\n")
        .write("sub/b.rs", "fn b(){}\n")
        .commit("init");
    // Parent-relative selection from inside sub/.
    let out = f
        .licet_in("sub")
        .args(["check", "--files", "../a.rs", "--format", "json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["files"].as_array().unwrap().len(), 1);
    assert_eq!(report["files"][0]["path"], "a.rs");
    // Sibling-relative selection from inside sub/.
    let out = f
        .licet_in("sub")
        .args(["check", "--files", "b.rs", "--format", "json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["files"][0]["path"], "sub/b.rs");
}

#[test]
fn file_selection_rejects_outside_root() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n").commit("init");
    for spelling in [
        "../outside.rs",
        "sub/../../outside.rs",
        "/absolutely/outside.rs",
    ] {
        let out = f
            .licet()
            .args(["check", "--files", spelling])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{spelling}");
    }
}

#[cfg(unix)]
#[test]
fn file_selection_skips_symlinks_regardless_of_order() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("real.rs", "// SPDX-License-Identifier: MIT\n")
        .texts(&["MIT"])
        .commit("init");
    symlink("real.rs", f.path().join("alias.rs")).unwrap();
    for files in [["alias.rs", "real.rs"], ["real.rs", "alias.rs"]] {
        let out = f
            .licet()
            .args(["check", "--files", files[0], files[1], "--format", "json"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{files:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let entries = report["files"].as_array().unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e["path"] == "real.rs" && e["drift"] == "compliant")
        );
        assert!(
            entries
                .iter()
                .any(|e| e["path"] == "alias.rs" && e["drift"] == "excluded")
        );
    }
    // The symlink itself is never replaced by an ordinary file.
    assert!(
        std::fs::symlink_metadata(f.path().join("alias.rs"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

// Non-UTF-8 filenames cannot be created on filesystems that enforce UTF-8
// names (e.g. macOS APFS rejects them at creation); Linux CI covers this.
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_filename_survives_selection() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .commit("init");
    // Latin-1 `caf\xe9.rs`: not valid UTF-8, but a real tracked file.
    let name = std::ffi::OsString::from_vec(b"caf\xe9.rs".to_vec());
    std::fs::write(f.path().join(&name), b"fn x() {}\n").unwrap();
    f.git(&["add", "-A"]);
    // Explicit selection by byte-exact name evaluates the file (missing header
    // proves identity survived; a lossy path would be unreadable/absent).
    let out = f
        .licet()
        .arg("check")
        .arg("--files")
        .arg(&name)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("missing_header"));
    // Full-tree coverage sees it too.
    let out = f.licet().arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[cfg(unix)]
#[test]
fn literal_backslash_is_not_a_separator() {
    // A Unix filename containing a literal backslash must not be split.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .commit("init");
    std::fs::write(f.path().join("a\\b.rs"), b"fn x() {}\n").unwrap();
    f.git(&["add", "-A"]);
    let out = f
        .licet()
        .args(["check", "--files", "a\\b.rs", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["files"].as_array().unwrap().len(), 1);
    assert_eq!(report["files"][0]["drift"], "missing_header");
}

#[test]
fn reuse_ignore_matrix() {
    use licet::spdx::bundled_text;
    let mit = bundled_text("MIT").unwrap();
    // A root file literally named `LICENSES` is covered (not ignored); it gets
    // its own fixture because `LICENSES/` is a directory everywhere else here.
    let g = Fixture::new();
    g.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("LICENSES", "I am an ordinary covered file\n")
        .commit("init");
    let out = g
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == "LICENSES" && e["drift"] == "missing_header"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("LICENSES/MIT.txt", mit)
        .write("COPYING", "x\n")
        .write("LICENSE-MIT", "x\n")
        .write("LICENCE.md", "x\n")
        .write("sub/COPYING.GPL", "x\n")
        .write("sub/LICENSE-MIT", "x\n")
        .write("sub/LICENCE.md", "x\n")
        .write("sub/LICENSES/a.rs", "fn a(){}\n")
        .write("empty.rs", "")
        .write("data.empty", "fn d(){}\n")
        .write("sbom.spdx.json", "{}\n")
        .write("subprojects/foo/a.rs", "fn a(){}\n")
        .write("ok.rs", "// SPDX-License-Identifier: MIT\n")
        .write(
            "REUSE.toml",
            "version = 1\n[[annotations]]\npath = \"ok.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
        )
        // NOTE: no `.reuse/dep5` here — REUSE.toml and DEP5 are mutually
        // exclusive and their coexistence fails the scan (see
        // `reuse_and_dep5_coexistence_fails_before_writes` in us5_reuse.rs).
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let files = report["files"].as_array().unwrap();
    let drift_of = |p: &str| {
        files
            .iter()
            .find(|e| e["path"] == p)
            .map(|e| e["drift"].as_str().unwrap().to_string())
    };
    // Covered: headerless sources fail; the compliant file passes.
    for p in ["sub/LICENSES/a.rs", "data.empty"] {
        assert_eq!(drift_of(p).as_deref(), Some("missing_header"), "{p}");
    }
    assert_eq!(drift_of("ok.rs").as_deref(), Some("compliant"));
    // Ignored by REUSE, at root and at depth.
    for p in [
        "LICENSES/MIT.txt",
        "COPYING",
        "LICENSE-MIT",
        "LICENCE.md",
        "sub/COPYING.GPL",
        "sub/LICENSE-MIT",
        "sub/LICENCE.md",
        "empty.rs",
        "sbom.spdx.json",
        "subprojects/foo/a.rs",
        "REUSE.toml",
    ] {
        assert_eq!(drift_of(p).as_deref(), Some("excluded"), "{p}");
    }
}

#[test]
fn lint_ignores_declaration_exclusions_but_sees_untracked() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\", \"hide.rs\"]\n")
        .write("ok.rs", "// SPDX-License-Identifier: MIT\n")
        .write("hide.rs", "fn h(){}\n")
        .texts(&["MIT"])
        .commit("init");
    // Policy check honors the exclusion: clean gate.
    let out = f.licet().arg("check").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // REUSE validation does not: the excluded-but-unlicensed file still fails.
    let out = f.licet().arg("lint").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    // Untracked files are invisible to the policy gate but visible to lint.
    f.write("new.rs", "fn n(){}\n");
    let out = f.licet().arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let out = f.licet().arg("lint").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn tracked_gitignored_file_stays_covered() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    // A later ignore rule cannot drop a tracked file from policy coverage.
    f.write(".gitignore", "a.rs\n");
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == "a.rs" && e["drift"] == "missing_header"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn linked_worktree_dot_git_file_not_scanned() {
    // In a linked worktree `.git` is a control FILE and enumeration comes from
    // `ls-files`, so internals can never leak into coverage.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("ok.rs", "// SPDX-License-Identifier: MIT\n")
        .texts(&["MIT"])
        .commit("init");
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("wt");
    f.git(&["worktree", "add", "--detach", wt_path.to_str().unwrap()]);
    assert!(
        wt_path.join(".git").is_file(),
        "linked worktree uses a .git file"
    );
    let out = std::process::Command::new(common::bin())
        .current_dir(&wt_path)
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        !report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"].as_str().unwrap_or_default().starts_with(".git")),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn git_internals_never_evaluated() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("ok.rs", "// SPDX-License-Identifier: MIT\n")
        .texts(&["MIT"])
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        !report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"].as_str().unwrap_or_default().starts_with(".git/")),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn selection_flags_are_mutually_exclusive() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n").commit("init");
    let out = f
        .licet()
        .args(["check", "--staged", "--changed"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "mutually exclusive flags → usage error"
    );
}

#[test]
fn staged_check_on_unborn_head_evaluates_index() {
    // No commit exists: the staged set is the full index.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("f.rs", "// SPDX-License-Identifier: MIT\n")
        .texts(&["MIT"])
        .stage_all();
    let out = f
        .licet()
        .args(["check", "--staged", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A file removed from the index on unborn HEAD is simply not evaluated.
    f.git(&["rm", "--cached", "-q", "f.rs"]);
    let out = f.licet().args(["check", "--staged"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn staged_rename_evaluates_new_path() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\n")
        .texts(&["MIT"])
        .commit("base");
    f.git(&["mv", "a.rs", "b.rs"]);
    let out = f
        .licet()
        .args(["check", "--staged", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let files = report["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .any(|e| e["path"] == "b.rs" && e["drift"] == "compliant")
    );
    assert!(!files.iter().any(|e| e["path"] == "a.rs"));
}

#[test]
fn unresolved_merge_index_is_usage_error() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("m.rs", "// base\n")
        .commit("base");
    f.git(&["checkout", "-qb", "side"]);
    f.write("m.rs", "// side\n").commit("side");
    f.git(&["checkout", "-q", "-"]);
    f.write("m.rs", "// main\n").commit("main");
    // Conflict the merge without asserting success.
    let merge = std::process::Command::new("git")
        .current_dir(f.path())
        .args(["merge", "--no-commit", "side"])
        .output()
        .unwrap();
    assert!(
        !merge.status.success(),
        "fixture must produce a real conflict"
    );
    let out = f.licet().args(["check", "--staged"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unresolved merge"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn staged_sidecar_supersedes_worktree_sidecar() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .texts(&["MIT", "Apache-2.0"])
        .commit("base");
    // Stage an Apache sidecar, then rewrite the working copy to MIT: the gate
    // sees the staged sidecar.
    f.write("a.rs.license", "SPDX-License-Identifier: Apache-2.0\n")
        .stage_all();
    f.write("a.rs.license", "SPDX-License-Identifier: MIT\n");
    let before = index_tree(&f);
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
            .any(|entry| entry["path"] == "a.rs" && entry["actual"] == "Apache-2.0"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        index_tree(&f),
        before,
        "staged reads must not mutate the index"
    );
}

#[test]
fn staged_metadata_change_expands_to_full_set() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\n")
        // Apache text up front: the staged override is about to reference it.
        .texts(&["MIT", "Apache-2.0"])
        .commit("base");
    // Stage only metadata: an overriding Apache annotation for a.rs. The
    // unchanged source is still affected, so the subset must expand.
    f.write(
        "REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"a.rs\"\nprecedence = \"override\"\n\
         SPDX-License-Identifier = \"Apache-2.0\"\n",
    )
    .stage_all();
    let out = f
        .licet()
        .args(["check", "--staged", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let files = report["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .any(|e| e["path"] == "a.rs" && e["drift"] == "wrong_license"),
        "expansion must evaluate the unchanged source: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "selection_expanded"),
        "expansion must be reported: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn staged_license_text_deletion_expands_selection() {
    use licet::spdx::bundled_text;
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\n")
        .write("LICENSES/MIT.txt", bundled_text("MIT").unwrap())
        .commit("base");
    // Stage only the text deletion: nothing evaluable remains in the subset,
    // but the deletion can affect other files, so the check must expand.
    f.git(&["rm", "-q", "LICENSES/MIT.txt"]);
    let out = f
        .licet()
        .args(["check", "--staged", "--format", "json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "selection_expanded"),
        "text deletion must expand the selection: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == "a.rs"),
        "expansion must evaluate the referencing source: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    // No exit-code assertion: `check` gains license-text inventory in task 5,
    // which turns this missing text into a failure.
}

#[test]
fn staged_custom_config_must_be_in_index() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("f.rs", "// SPDX-License-Identifier: MIT\n")
        .commit("base");
    f.write("custom.toml", "[default]\nlicense=\"Apache-2.0\"\n");
    // Untracked custom config: not part of the staged snapshot.
    let out = f
        .licet()
        .args(["check", "--staged", "--config", "custom.toml"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // Once staged, the check reads the staged config (Apache default makes the
    // MIT file drift — proving the index bytes were used, not licet.toml).
    f.git(&["add", "custom.toml"]);
    let out = f
        .licet()
        .args([
            "check",
            "--staged",
            "--config",
            "custom.toml",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "f.rs"
                && entry["actual"] == "MIT"
                && entry["drift"] == "wrong_license"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn apply_staged_edits_worktree_leaves_index() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("f.rs", "// SPDX-License-Identifier: MIT\n")
        .commit("base");
    f.write("f.rs", "// SPDX-License-Identifier: Apache-2.0\n")
        .stage_all();
    let staged_before = String::from_utf8_lossy(&f.git(&["show", ":f.rs"])).into_owned();
    assert!(staged_before.contains("Apache-2.0"));
    let out = f
        .licet()
        .args(["apply", "--staged", "--allow-dirty"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Worktree fixed, index untouched.
    assert!(f.read("f.rs").contains("SPDX-License-Identifier: MIT"));
    assert!(
        String::from_utf8_lossy(&f.git(&["show", ":f.rs"])).contains("Apache-2.0"),
        "apply must never stage its edits"
    );
    // A staged-but-superseded worktree needs no write and still passes.
    f.write("f.rs", "// SPDX-License-Identifier: MIT\n");
    let out = f
        .licet()
        .args(["apply", "--staged", "--allow-dirty"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&f.git(&["show", ":f.rs"])).contains("Apache-2.0"));
}

#[test]
fn explain_names_winning_rule() {
    let f = Fixture::new();
    f.config("[[rule]]\next=\"rs\"\nlicense=\"MIT\"\n[[rule]]\nglob=\"examples/**/*.rs\"\nlicense=\"Apache-2.0\"\n")
        .write("examples/d.rs", "fn d(){}\n")
        // Default coverage is tracked files; commit so the path is evaluated
        // (task 9 makes --explain resolve arbitrary paths directly).
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--explain", "examples/d.rs"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("glob=examples/**/*.rs"), "{stdout}");
}

#[test]
fn explain_reports_winner_losers_and_drift() {
    let f = Fixture::new();
    f.config("[[rule]]\next=\"rs\"\nlicense=\"MIT\"\n[[rule]]\nglob=\"examples/**/*.rs\"\nlicense=\"Apache-2.0\"\n")
        .write("examples/d.rs", "// SPDX-License-Identifier: MIT\nfn d(){}\n")
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--explain", "examples/d.rs"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("winning rule #2"), "{stdout}");
    assert!(stdout.contains("losing rule #1"), "{stdout}");
    assert!(stdout.contains("specificity"), "{stdout}");
    assert!(stdout.contains("wrong_license"), "{stdout}");
    assert!(stdout.contains("exclusions: none"), "{stdout}");
}

#[test]
fn explain_outside_selected_set_is_usage_error() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write("b.rs", "// SPDX-License-Identifier: MIT\nfn b(){}\n")
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--files", "a.rs", "--explain", "b.rs"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "outside the selected set → exit 2"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("outside the selected file set"), "{stderr}");
}

#[test]
fn explain_nonexistent_path_is_usage_error() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n").commit("init");
    let out = f
        .licet()
        .args(["check", "--explain", "nope.rs"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "nonexistent → exit 2, not success"
    );
}

#[test]
fn default_config_resolves_from_root_in_subdirectory() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write("sub/b.rs", "// SPDX-License-Identifier: MIT\nfn b(){}\n")
        .texts(&["MIT"])
        .commit("init");
    // No --config: the omitted default comes from the discovered root.
    let out = f.licet_in("sub").arg("check").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "root config applies from subdir: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn explicit_missing_config_is_usage_error() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .write("sub/b.rs", "// SPDX-License-Identifier: MIT\nfn b(){}\n")
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--config", "nope.toml"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // An explicit relative path stays cwd-relative: from a subdir without
    // its own licet.toml this names a missing file, unlike the omitted
    // default which resolves from the root.
    let out = f
        .licet_in("sub")
        .args(["check", "--config", "licet.toml"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn legacy_only_config_errors_with_rename_hint() {
    // Default load reads `licet.toml`; a lone `license.toml` is the pre-rename
    // filename and must point at the rename, not fail as "missing config".
    let f = Fixture::new();
    f.write("license.toml", "[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");
    let out = f.licet().arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("license.toml") && stderr.contains("licet.toml"),
        "rename hint names both files: {stderr}"
    );
}

#[test]
fn explicit_legacy_config_path_still_reads() {
    // Escape hatch: an explicitly named path is read as-is, whatever its name.
    let f = Fixture::new();
    f.write("license.toml", "[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .texts(&["MIT"])
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--config", "license.toml", "--files", "a.rs"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "explicit legacy path works: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_refuses_default_when_legacy_config_present() {
    // `init` must not write a competing `licet.toml` next to a legacy file
    // that would then be silently ignored.
    let f = Fixture::new();
    f.write("license.toml", "[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");
    let out = f.licet().arg("init").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("licet.toml"),
        "init points at the rename: {stderr}"
    );
    assert!(
        !f.path().join("licet.toml").exists(),
        "no competing default written"
    );
}

#[test]
fn broken_stdout_pipe_exits_quietly() {
    use std::io::Read;
    use std::process::Stdio;
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n")
        .texts(&["MIT"]);
    // Enough files that the JSON report exceeds the pipe buffer, so the
    // writer deterministically hits EPIPE once the reader goes away.
    for i in 0..2000 {
        std::fs::write(
            f.path().join(format!("f{i:04}.rs")),
            "// SPDX-License-Identifier: MIT\nfn f(){}\n",
        )
        .unwrap();
    }
    f.commit("init");

    let mut child = f
        .licet()
        .args(["check", "--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut head = [0u8; 16];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut head)
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "closed pipe terminates quietly, got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "no panic on EPIPE: {stderr}");
}

#[test]
fn explain_json_is_a_single_parseable_document() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");
    let out = f
        .licet()
        .args(["check", "--explain", "a.rs", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["version"], 2);
    assert_eq!(report["files"].as_array().unwrap().len(), 1);
    assert_eq!(report["files"][0]["path"], "a.rs");
    assert_eq!(report["files"][0]["drift"], "compliant");
}

#[test]
fn equal_specificity_rule_conflict_is_nonzero_with_path() {
    // Two identical selectors with differing full intent surface a conflict,
    // never a silent pick: duplicate selectors fail config loading with the
    // selector and both intents named, before anything is evaluated.
    let f = Fixture::new();
    f.config(
        "[[rule]]\nfile=\"a.rs\"\nlicense=\"MIT\"\n[[rule]]\nfile=\"a.rs\"\nlicense=\"Apache-2.0\"\n",
    )
    .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
    .commit("init");

    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(stderr.contains("file=a.rs"), "{stderr}");
    assert!(
        stderr.contains("MIT") && stderr.contains("Apache-2.0"),
        "{stderr}"
    );

    // Apply cannot resolve it either: nonzero, file untouched.
    let before = f.read("a.rs");
    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(f.read("a.rs"), before);
}

#[test]
fn resolution_time_equal_specificity_conflict_names_rules() {
    // Distinct selectors of equal specificity with differing intent conflict
    // at resolution: the diagnostic carries the path and both tied rules.
    let f = Fixture::new();
    f.config("[[rule]]\nglob=\"*.rs\"\nlicense=\"MIT\"\n[[rule]]\nglob=\"a.*\"\nlicense=\"Apache-2.0\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");

    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let diagnostics = v["diagnostics"].as_array().cloned().unwrap_or_default();
    let conflict = diagnostics
        .iter()
        .find(|d| d["code"] == "rule_conflict")
        .expect("rule_conflict diagnostic");
    assert_eq!(conflict["path"], "a.rs");
    let message = conflict["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("glob=*.rs") && message.contains("glob=a.*"),
        "{message}"
    );

    // Apply leaves the conflicted file alone and stays nonzero.
    let before = f.read("a.rs");
    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(f.read("a.rs"), before);
}
// REUSE-IgnoreEnd
