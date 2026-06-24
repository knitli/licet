//! Shared fixtures for integration tests: build a temp git repo with files + config.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

/// A throwaway git repository populated by the test.
pub struct Fixture {
    pub dir: TempDir,
}

impl Fixture {
    /// Create an initialized git repo.
    pub fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        run_git(dir.path(), &["init", "-q"]);
        run_git(dir.path(), &["config", "user.email", "t@t.co"]);
        run_git(dir.path(), &["config", "user.name", "Test"]);
        Fixture { dir }
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
