//! US5 — `add-license` offline text materialization (FR-017, FR-029).
//! The offline analog of `reuse download`: copy referenced texts into `LICENSES/` from the
//! embedded bundle, without touching source files or the config. Quickstart Scenario 5.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn explicit_id_materializes_bundled_text() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f.licet().args(["add-license", "MIT"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = f.read("LICENSES/MIT.txt");
    assert!(
        text.contains("Permission is hereby granted"),
        "real MIT text, not a placeholder: {text}"
    );
}

#[test]
fn all_materializes_referenced_but_missing_from_config() {
    let f = Fixture::new();
    // Default MIT + an `rs` rule for Apache-2.0; both referenced, neither present yet.
    f.config("[default]\nlicense=\"MIT\"\n[[rule]]\next=\"rs\"\nlicense=\"Apache-2.0\"\n")
        .write("a.rs", "// SPDX-License-Identifier: Apache-2.0\nfn a(){}\n")
        .commit("init");
    let out = f.licet().args(["add-license", "--all"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(f.path().join("LICENSES/MIT.txt").exists());
    assert!(f.path().join("LICENSES/Apache-2.0.txt").exists());
}

#[test]
fn re_run_is_idempotent_and_succeeds() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    assert!(
        f.licet()
            .args(["add", "MIT"])
            .output()
            .unwrap()
            .status
            .success()
    );
    // Second run: already present, nothing written, still exit 0.
    let out = f.licet().args(["add", "MIT"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Already present"), "{stdout}");
}

#[test]
fn unknown_id_is_usage_error_before_any_write() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f
        .licet()
        .args(["add", "Definitely-Not-A-License-9.9"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Definitely-Not-A-License-9.9"),
        "names the invalid id: {stderr}"
    );
    assert!(
        !f.path().join("LICENSES").exists(),
        "invalid id must not create any directory"
    );
}

#[test]
fn unbundled_standard_id_without_network_exits_violations() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f.licet().args(["add", "Apache-1.0"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Apache-1.0"),
        "names the unavailable id: {stdout}"
    );
    assert!(
        !f.path().join("LICENSES/Apache-1.0.txt").exists(),
        "offline run must not create the text"
    );
}

#[test]
fn license_ref_requires_manual_text_without_scaffolding() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f
        .licet()
        .args(["add", "LicenseRef-Acme-1.0"])
        .output()
        .unwrap();
    // Custom texts must be supplied by the maintainer: reported missing (exit 1),
    // never downloaded, and never scaffolded with placeholder prose.
    assert_eq!(out.status.code(), Some(1));
    assert!(
        !f.path().join("LICENSES/LicenseRef-Acme-1.0.txt").exists(),
        "no placeholder scaffold may be invented"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("LicenseRef-Acme-1.0"),
        "names the required text: {stdout}"
    );
}

#[test]
fn path_escape_id_is_usage_error_and_keeps_victim() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n")
        .write("victim.txt", "ORIGINAL")
        .commit("init");
    let out = f
        .licet()
        .args(["add-license", "../victim", "--allow-network"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(f.read("victim.txt"), "ORIGINAL");
}

// Fake-curl tests: a `curl` shim on PATH records invocation via a marker file.

#[cfg(unix)]
mod fake_curl {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Create a dir containing an executable `curl` shim plus its marker path.
    /// The shim touches `$FAKE_MARKER` on every invocation so tests can assert
    /// curl was (not) called. Payload behavior is embedded per test.
    fn shim_dir(script: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let curl = dir.path().join("curl");
        std::fs::write(&curl, script).unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let marker = dir.path().join("invoked");
        (dir, marker)
    }

    fn path_with(dir: &tempfile::TempDir) -> std::ffi::OsString {
        let mut paths = vec![dir.path().to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        std::env::join_paths(paths).unwrap()
    }

    const RECORD_AND_FAIL: &str = "#!/bin/sh\ntouch \"$FAKE_MARKER\"\nexit 1\n";

    const RECORD_AND_WRITE_PAYLOAD: &str = "#!/bin/sh\ntouch \"$FAKE_MARKER\"\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\nprintf '%s' \"$FAKE_PAYLOAD\" > \"$out\"\nexit 0\n";

    #[test]
    fn escape_id_exits_before_invoking_curl() {
        let f = Fixture::new();
        f.write("a.rs", "fn a(){}\n")
            .write("victim.txt", "ORIGINAL")
            .commit("init");
        let (shim, marker) = shim_dir(RECORD_AND_FAIL);
        let out = f
            .licet()
            .env("PATH", path_with(&shim))
            .env("FAKE_MARKER", &marker)
            .args(["add-license", "../victim", "--allow-network"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(f.read("victim.txt"), "ORIGINAL");
        assert!(!marker.exists(), "curl must not be invoked for invalid ids");
    }

    #[test]
    fn bundled_id_never_invokes_curl() {
        let f = Fixture::new();
        f.write("a.rs", "fn a(){}\n").commit("init");
        let (shim, marker) = shim_dir(RECORD_AND_FAIL);
        let out = f
            .licet()
            .env("PATH", path_with(&shim))
            .env("FAKE_MARKER", &marker)
            .args(["add-license", "MIT", "--allow-network"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!marker.exists(), "bundled text must not invoke curl");
        assert!(
            f.read("LICENSES/MIT.txt")
                .contains("Permission is hereby granted")
        );
    }

    #[test]
    fn fetch_success_installs_text() {
        let f = Fixture::new();
        f.write("a.rs", "fn a(){}\n").commit("init");
        let (shim, marker) = shim_dir(RECORD_AND_WRITE_PAYLOAD);
        let out = f
            .licet()
            .env("PATH", path_with(&shim))
            .env("FAKE_MARKER", &marker)
            .env("FAKE_PAYLOAD", "FAKE APACHE-1.0 TEXT")
            .args(["add-license", "Apache-1.0", "--allow-network"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(marker.exists(), "curl must be invoked for unbundled ids");
        assert_eq!(f.read("LICENSES/Apache-1.0.txt"), "FAKE APACHE-1.0 TEXT");
        // Exactly one file is written: the requested id at its destination.
        let entries: Vec<_> = std::fs::read_dir(f.path().join("LICENSES"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("Apache-1.0.txt")]);
    }

    #[test]
    fn fetch_failure_preserves_destination() {
        let f = Fixture::new();
        f.write("a.rs", "fn a(){}\n")
            .write("LICENSES/Apache-1.0.txt", "KEEP")
            .commit("init");
        // Exit 22 (HTTP error with --fail) must not touch the destination. The
        // file counts as present, so the run still succeeds.
        let (shim, marker) = shim_dir("#!/bin/sh\ntouch \"$FAKE_MARKER\"\nexit 22\n");
        let out = f
            .licet()
            .env("PATH", path_with(&shim))
            .env("FAKE_MARKER", &marker)
            .args(["add-license", "Apache-1.0", "--allow-network"])
            .output()
            .unwrap();
        assert_eq!(f.read("LICENSES/Apache-1.0.txt"), "KEEP");
        assert!(!marker.exists(), "present text must not trigger a download");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Without the pre-existing text, the same failure is exit 1 with no file.
        let g = Fixture::new();
        g.write("a.rs", "fn a(){}\n").commit("init");
        let out = g
            .licet()
            .env("PATH", path_with(&shim))
            .env("FAKE_MARKER", &marker)
            .args([
                "add-license",
                "Apache-1.0",
                "--allow-network",
                "--format",
                "json",
            ])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(!g.path().join("LICENSES/Apache-1.0.txt").exists());
        // JSON mode stays parseable even on download failure.
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["command"], "add-license");
        assert_eq!(v["summary"]["pass"], false);
    }

    #[test]
    fn empty_and_oversized_downloads_are_rejected() {
        for (name, script) in [
            (
                "empty",
                "#!/bin/sh\ntouch \"$FAKE_MARKER\"\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\n: > \"$out\"\nexit 0\n",
            ),
            (
                "oversized",
                "#!/bin/sh\ntouch \"$FAKE_MARKER\"\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\nhead -c 4194305 /dev/zero > \"$out\"\nexit 0\n",
            ),
        ] {
            let f = Fixture::new();
            f.write("a.rs", "fn a(){}\n").commit("init");
            let (shim, marker) = shim_dir(script);
            let out = f
                .licet()
                .env("PATH", path_with(&shim))
                .env("FAKE_MARKER", &marker)
                .args(["add-license", "Apache-1.0", "--allow-network"])
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(1), "{name}");
            assert!(marker.exists(), "{name}: curl must have been attempted");
            assert!(
                !f.path().join("LICENSES/Apache-1.0.txt").exists(),
                "{name}: no text may be installed"
            );
        }
    }

    #[test]
    fn curl_launch_failure_preserves_existing() {
        // A present text never triggers a download, even with a broken curl.
        let shim = tempfile::tempdir().unwrap();
        std::fs::write(shim.path().join("curl"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(
            shim.path().join("curl"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let f = Fixture::new();
        f.write("a.rs", "fn a(){}\n")
            .write("LICENSES/Apache-1.0.txt", "KEEP")
            .commit("init");
        let out = f
            .licet()
            .env("PATH", path_with(&shim))
            .args(["add-license", "Apache-1.0", "--allow-network"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(f.read("LICENSES/Apache-1.0.txt"), "KEEP");

        // An isolated PATH with a non-executable `curl` (spawn fails) and a
        // symlinked `git` (repo discovery keeps working): the missing text is
        // exit 1 and nothing is installed.
        let isolated = tempfile::tempdir().unwrap();
        std::fs::write(isolated.path().join("curl"), "not executable\n").unwrap();
        std::fs::set_permissions(
            isolated.path().join("curl"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let git_src = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|d| d.join("git"))
            .find(|p| p.is_file())
            .expect("a git binary on PATH for the fixture");
        std::os::unix::fs::symlink(&git_src, isolated.path().join("git")).unwrap();
        let g = Fixture::new();
        g.write("a.rs", "fn a(){}\n").commit("init");
        let out = g
            .licet()
            .env("PATH", isolated.path())
            .args(["add-license", "Apache-1.0", "--allow-network"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(!g.path().join("LICENSES/Apache-1.0.txt").exists());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("download failed"), "{stdout}");
    }
}

#[test]
fn no_target_is_usage_error() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f.licet().arg("add-license").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "neither ids nor --all → exit 2");
}

#[test]
fn ids_and_all_together_is_usage_error() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f
        .licet()
        .args(["add-license", "MIT", "--all"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "both ids and --all → exit 2");
}

#[test]
fn writes_only_under_licenses_and_does_not_require_clean_tree() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");
    // Make the tree dirty — unlike `apply`, `add-license` must still proceed.
    f.write("a.rs", "fn a(){ /* edited */ }\n");
    let before = f.read("a.rs");
    let before_cfg = f.read("licet.toml");

    let out = f.licet().args(["add", "MIT"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "dirty tree must not block add-license: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Source file and config are untouched; only LICENSES/ changed.
    assert_eq!(f.read("a.rs"), before, "source file must be untouched");
    assert_eq!(f.read("licet.toml"), before_cfg, "config must be untouched");
    assert!(f.path().join("LICENSES/MIT.txt").exists());
}

#[test]
fn json_output_is_well_formed() {
    let f = Fixture::new();
    f.write("a.rs", "fn a(){}\n").commit("init");
    let out = f
        .licet()
        .args(["add-license", "MIT", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["command"], "add-license");
    assert_eq!(v["summary"]["pass"], true);
    assert!(v["license_texts"]["spdx_list_version"].is_string());
}

#[test]
fn all_flag_propagates_config_errors() {
    // `add-license --all` scans the union of desired and actual references,
    // but a malformed policy config is usage error 2 — never swallowed.
    let f = Fixture::new();
    f.config("[default\nbroken\n")
        .write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n")
        .commit("init");
    let out = f.licet().args(["add-license", "--all"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
// REUSE-IgnoreEnd
