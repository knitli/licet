//! US2 — reconcile to intent (SC-002, SC-004, FR-006..FR-009, FR-020). Quickstart Scenario 2.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn destructive_replaces_license_preserves_copyright() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nglob=\"examples/**/*.rs\"\nlicense=\"MIT OR Apache-2.0\"\n")
        .write(
            "examples/demo.rs",
            "// SPDX-FileCopyrightText: 2026 Marque\n// SPDX-License-Identifier: LicenseRef-MarqueLicense-1.0\nfn d(){}\n",
        )
        .commit("init");

    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let content = f.read("examples/demo.rs");
    assert!(
        content.contains("SPDX-License-Identifier: MIT OR Apache-2.0"),
        "{content}"
    );
    // Copyright line survives the license-only replace (SC-004).
    assert!(
        content.contains("SPDX-FileCopyrightText: 2026 Marque"),
        "{content}"
    );
    assert!(
        !content.contains("LicenseRef-MarqueLicense-1.0"),
        "old license removed: {content}"
    );
}

#[test]
fn apply_then_check_is_clean() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    assert_eq!(
        f.licet().arg("apply").output().unwrap().status.code(),
        Some(0)
    );
    let check = f.licet().arg("check").output().unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

#[test]
fn additive_keeps_both_and_warns_contradiction() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n")
        .commit("init");

    let out = f
        .licet()
        .args(["apply", "--additive", "--format", "json"])
        .output()
        .unwrap();
    let content = f.read("a.rs");
    assert!(content.contains("Apache-2.0"), "old kept: {content}");
    assert!(content.contains("MIT"), "new added: {content}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let diagnostics = v["diagnostics"].as_array().cloned().unwrap_or_default();
    assert!(
        diagnostics.iter().any(|w| w["code"] == "contradiction"),
        "expected contradiction warning: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn dry_run_does_not_modify_files() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    let before = f.read("a.rs");
    let out = f.licet().args(["apply", "--dry-run"]).output().unwrap();
    assert_eq!(f.read("a.rs"), before, "dry-run must not write");
    assert!(String::from_utf8_lossy(&out.stdout).contains("would apply"));
}

#[test]
fn target_header_replaces_chosen_block() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write(
            "a.rs",
            "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n\n// SPDX-License-Identifier: ISC\nfn b(){}\n",
        )
        .commit("init");
    // Replace the 2nd header block (index 1).
    let out = f
        .licet()
        .args(["apply", "--target-header", "1"])
        .output()
        .unwrap();
    assert!(out.status.success() || out.status.code() == Some(1));
    let content = f.read("a.rs");
    // First block untouched, second now MIT.
    assert!(
        content.contains("// SPDX-License-Identifier: Apache-2.0"),
        "{content}"
    );
    assert!(
        content.contains("// SPDX-License-Identifier: MIT"),
        "{content}"
    );
    assert!(!content.contains("ISC"), "second block replaced: {content}");
}

#[test]
fn apply_materializes_projected_texts_only() {
    // Apply supplies texts for its projected state: the MIT intent it writes.
    // The stale replaced Apache license and the unmatched rule's license are
    // never fetched; nothing is ever deleted.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\nglob=\"special/**\"\nlicense=\"Apache-2.0\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n")
        .commit("init");
    let out = f
        .licet()
        .args(["apply", "--files", "a.rs"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        f.path().join("LICENSES/MIT.txt").exists(),
        "projected intent text is supplied"
    );
    assert!(
        !f.path().join("LICENSES/Apache-2.0.txt").exists(),
        "stale replaced and unmatched-rule licenses are not fetched"
    );
}

#[test]
fn destructive_apply_strips_additional_licenses() {
    // One matching candidate never hides an additional license: a file with
    // MIT plus Apache under a lone MIT intent is wrong_license, and
    // destructive apply reduces it to exactly the declared expression.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write(
            "a.rs",
            "// SPDX-License-Identifier: MIT OR Apache-2.0\nfn a(){}\n",
        )
        // Referenced texts exist up front (the tracked snapshot cannot see
        // files apply materializes but leaves untracked).
        .texts(&["MIT", "Apache-2.0"])
        .commit("init");
    let out = f
        .licet()
        .args(["apply", "--files", "a.rs"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let content = f.read("a.rs");
    assert!(
        content.contains("// SPDX-License-Identifier: MIT\n"),
        "reduced to exactly MIT: {content}"
    );
    assert!(!content.contains("Apache-2.0"), "{content}");
    let out = f
        .licet()
        .args(["check", "--files", "a.rs"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn program_text_tag_is_unfixable_and_untouched() {
    // A tag in program text (no comment span) must not be rewritten: apply
    // reports an actionable `unfixable` diagnostic, writes nothing, and fails
    // the gate.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write(
            "a.rs",
            "FOO=SPDX-License-Identifier: Apache-2.0\nfn a(){}\n",
        )
        // The fixture's own config file carries a header so the only failure
        // is the unfixable one (exit 1, not partial).
        .write(
            "license.toml",
            "# SPDX-License-Identifier: MIT\n[default]\nlicense=\"MIT\"\n",
        )
        .commit("init");
    let before = f.read("a.rs");

    let out = f
        .licet()
        .args(["apply", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(f.read("a.rs"), before, "program text must not be rewritten");

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let diagnostics = v["diagnostics"].as_array().cloned().unwrap_or_default();
    assert!(
        diagnostics
            .iter()
            .any(|w| w["code"] == "unfixable" && w["path"] == "a.rs"),
        "expected unfixable warning for a.rs: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let files = v["files"].as_array().cloned().unwrap_or_default();
    let entry = files
        .iter()
        .find(|e| e["path"] == "a.rs")
        .expect("a.rs in report");
    assert_eq!(entry["change"]["applied"], false);
}

#[test]
fn matching_license_missing_copyright_is_a_real_change() {
    // Copyright intent applies even when the license already matches: a
    // missing requested notice is written, not skipped as compliant.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\ncopyright=\"add:2026 Acme\"\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");

    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        f.read("a.rs"),
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\nfn a(){}\n"
    );

    let check = f.licet().arg("check").output().unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

/// Byte-hash every tracked file: dry-run must change nothing on disk.
fn tree_hash(f: &common::Fixture) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let root = f.path();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            if p.join(".git").exists() || p.ends_with(".git") {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(
                    p.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    // The temp dir itself may hold other files; the LICENSES absence check
    // below pins non-creation separately.
    out
}

#[test]
fn dry_run_changes_nothing_and_previews_planned_paths() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n")
        .commit("init");
    let before = tree_hash(&f);

    let out = f
        .licet()
        .args(["apply", "--dry-run", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(tree_hash(&f), before, "dry-run changed the tree");
    assert!(
        !f.path().join("LICENSES").exists(),
        "dry-run created LICENSES/"
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let writes = v["writes"].as_array().cloned().unwrap_or_default();
    assert!(!writes.is_empty(), "dry-run previews planned writes");
    assert!(
        writes.iter().all(|w| w["status"] == "planned"),
        "dry-run writes are all planned: {writes:?}"
    );
    let src_write = writes
        .iter()
        .find(|w| w["path"] == "a.rs" && w["kind"] == "source")
        .expect("planned source write for a.rs");
    assert!(
        src_write["before_text"]
            .as_str()
            .unwrap()
            .contains("Apache-2.0"),
        "exact before text previewed"
    );
    assert!(
        src_write["after_text"]
            .as_str()
            .unwrap()
            .contains("SPDX-License-Identifier: MIT"),
        "exact after text previewed"
    );
    // Fixable drift projects success.
    assert_eq!(v["summary"]["projected_pass"], true);
    assert_eq!(v["summary"]["pass"], true);
}

#[test]
fn reapply_converges_to_empty_writes() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Re-run converges: no drift left, so no writes at all.
    let out = f
        .licet()
        .args(["apply", "--allow-dirty", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v.get("writes")
            .is_none_or(|w| w.as_array().is_some_and(|a| a.is_empty())),
        "re-apply emits an empty changed set: {v}"
    );
}

#[test]
fn dry_run_preview_paths_match_real_applied_paths() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n")
        .write("b.rs", "fn b(){}\n")
        .commit("init");

    let dry = f
        .licet()
        .args(["apply", "--dry-run", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    let planned: std::collections::BTreeSet<String> = v["writes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|w| w["path"].as_str().unwrap().to_string())
        .collect();

    let real = f
        .licet()
        .args(["apply", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        real.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&real.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&real.stdout).unwrap();
    let applied: std::collections::BTreeSet<String> = v["writes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|w| w["status"] == "applied")
        .map(|w| w["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(planned, applied, "preview/apply path equality");
}

#[test]
fn custom_text_missing_blocks_apply_with_exit_1() {
    // A referenced custom text nobody supplied is an explicit blocker: the
    // header is still written per intent, but the gate cannot pass.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"LicenseRef-MarqueLicense-1.0\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");

    let out = f
        .licet()
        .args(["apply", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        f.read("a.rs")
            .contains("SPDX-License-Identifier: LicenseRef-MarqueLicense-1.0"),
        "header still written per intent"
    );
    assert!(!f.path().join("LICENSES").exists(), "no text scaffolded");

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let writes = v["writes"].as_array().cloned().unwrap_or_default();
    let blocked = writes
        .iter()
        .find(|w| w["status"] == "blocked" && w["kind"] == "license_text")
        .expect("blocked license_text record");
    assert_eq!(blocked["path"], "LICENSES/LicenseRef-MarqueLicense-1.0.txt");
    assert_eq!(v["summary"]["pass"], false);
}

#[test]
fn dry_run_never_fetches_missing_fetchable_text() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"CDDL-1.0\"\n")
        .write("a.rs", "// SPDX-License-Identifier: CDDL-1.0\nfn a(){}\n")
        .commit("init");
    let before = tree_hash(&f);

    let out = f
        .licet()
        .args(["apply", "--dry-run", "--format", "json"])
        .output()
        .unwrap();
    // A required fetch that has not occurred fails the projection without
    // any network use: zero filesystem changes, zero curl invocations (the
    // sandbox has no network; any fetch attempt would error loudly).
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(tree_hash(&f), before, "dry-run changed the tree");
    assert!(!f.path().join("LICENSES").exists());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["summary"]["projected_pass"], false);
    let writes = v["writes"].as_array().cloned().unwrap_or_default();
    assert!(
        writes.iter().any(|w| w["status"] == "blocked"
            && w["message"]
                .as_str()
                .unwrap_or_default()
                .contains("https://")),
        "blocked fetch names its URL: {writes:?}"
    );
}

#[test]
fn human_output_uses_distinct_write_words() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");

    let dry = f.licet().args(["apply", "--dry-run"]).output().unwrap();
    let text = String::from_utf8_lossy(&dry.stdout).into_owned();
    assert!(text.contains("[planned]"), "planned word: {text}");

    let real = f.licet().arg("apply").output().unwrap();
    let text = String::from_utf8_lossy(&real.stdout).into_owned();
    assert!(text.contains("[applied]"), "applied word: {text}");

    // A blocked inventory requirement says so explicitly.
    let g = Fixture::new();
    g.config("[default]\nlicense=\"LicenseRef-MarqueLicense-1.0\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    let out = g.licet().arg("apply").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("[blocked]"), "blocked word: {text}");
}

#[test]
fn additive_success_with_unfixable_remainder_is_exit_1_not_partial() {
    // Writes that all succeed but leave declaration drift are violations
    // (exit 1), not partial: partial means the tool itself failed partway.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .write(
            "b.rs",
            "FOO=SPDX-License-Identifier: Apache-2.0\nfn b(){}\n",
        )
        .write(
            "license.toml",
            "# SPDX-License-Identifier: MIT\n[default]\nlicense=\"MIT\"\n",
        )
        .commit("init");

    let out = f
        .licet()
        .args(["apply", "--additive", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "not partial: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        f.read("a.rs").contains("SPDX-License-Identifier: MIT"),
        "fixable file fixed"
    );
    let before = "FOO=SPDX-License-Identifier: Apache-2.0\nfn b(){}\n";
    assert_eq!(f.read("b.rs"), before, "unfixable file untouched");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["summary"]["partial"], false);
}

#[test]
fn php_header_inserts_after_open_tag() {
    // The PHP open tag must stay the first line; the header follows it.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("index.php", "<?php\necho 'hi';\n")
        .commit("init");

    let out = f.licet().arg("apply").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let content = f.read("index.php");
    let mut lines = content.lines();
    assert_eq!(lines.next(), Some("<?php"));
    assert!(
        lines
            .next()
            .unwrap_or("")
            .contains("SPDX-License-Identifier: MIT"),
        "{content}"
    );

    let check = f.licet().arg("check").output().unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}
// REUSE-IgnoreEnd
