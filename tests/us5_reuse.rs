//! US5 — REUSE compatibility & migration (SC-007, SC-008, FR-014, FR-017, FR-018, FR-028).
//! Quickstart Scenario 5.

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
