//! Shared fixtures for integration tests: build a temp git repo with files + config.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

/// A throwaway git repository populated by the test.
pub struct Fixture {
    pub dir: TempDir,
    /// Empty hooks directory, kept outside the repo so `git add -A` in
    /// [`Fixture::commit`] never sweeps it into a fixture commit.
    _hooks: TempDir,
}

impl Fixture {
    /// Create an initialized git repo, isolated from ambient Git identity,
    /// commit signing, and repository hooks (audit §Evidence: an inherited
    /// signing agent once failed an unrelated fixture run).
    pub fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let hooks = TempDir::new().expect("hooks tempdir");
        run_git(dir.path(), &["init", "-q"]);
        run_git(dir.path(), &["config", "user.email", "t@t.co"]);
        run_git(dir.path(), &["config", "user.name", "Test"]);
        run_git(dir.path(), &["config", "commit.gpgsign", "false"]);
        run_git(dir.path(), &["config", "tag.gpgsign", "false"]);
        run_git(
            dir.path(),
            &["config", "core.hooksPath", hooks.path().to_str().unwrap()],
        );
        Fixture { dir, _hooks: hooks }
    }

    /// Run a checked `git` command in the fixture repo; panics with stderr context
    /// on failure. Returns stdout bytes (lossless for non-UTF-8 paths).
    pub fn git(&self, args: &[&str]) -> Vec<u8> {
        let out = Command::new("git")
            .current_dir(self.dir.path())
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write a file (creating parent dirs).
    pub fn write(&self, rel: &str, content: &str) -> &Self {
        let p = self.dir.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
        self
    }

    /// Write `license.toml`.
    pub fn config(&self, toml: &str) -> &Self {
        self.write("license.toml", toml)
    }

    /// Write bundled `LICENSES/<id>.txt` texts (policy `check` requires the
    /// texts referenced by its selected files).
    pub fn texts(&self, ids: &[&str]) -> &Self {
        for id in ids {
            let text =
                licet::spdx::bundled_text(id).unwrap_or_else(|| panic!("no bundled text for {id}"));
            self.write(&format!("LICENSES/{id}.txt"), text);
        }
        self
    }

    /// Stage and commit everything.
    pub fn commit(&self, msg: &str) -> &Self {
        run_git(self.dir.path(), &["add", "-A"]);
        run_git(self.dir.path(), &["commit", "-qm", msg]);
        self
    }

    /// Stage everything without committing.
    pub fn stage_all(&self) -> &Self {
        run_git(self.dir.path(), &["add", "-A"]);
        self
    }

    /// Read a file back.
    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(rel)).unwrap()
    }

    /// A `licet` command rooted in this repo.
    pub fn licet(&self) -> Command {
        let mut cmd = Command::new(cargo_bin("licet"));
        cmd.current_dir(self.dir.path());
        cmd
    }

    /// A `licet` command invoked from a subdirectory of the repo (path
    /// arguments resolve against this cwd, the project root is discovered).
    pub fn licet_in(&self, subdir: &str) -> Command {
        let mut cmd = Command::new(cargo_bin("licet"));
        cmd.current_dir(self.dir.path().join(subdir));
        cmd
    }
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        status.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&status.stderr)
    );
}

/// Path to the built binary (for direct invocation if needed).
pub fn bin() -> PathBuf {
    cargo_bin("licet")
}
