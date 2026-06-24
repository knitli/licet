//! File enumeration: full gitignore-aware tree walk (`ignore`), git subset selection
//! (staged/changed/file-list), symlink-dedup, and exclusion filtering
//! (FR-013, FR-016, FR-027, Edge Cases).

pub mod cache;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::error::{LicetError, Result};

/// Which files an invocation evaluates (selection flags are mutually exclusive, FR-027).
#[derive(Debug, Clone)]
pub enum Selection {
    /// Default: the full tracked/working tree.
    FullTree,
    /// An explicit list of repo-relative or absolute paths.
    Files(Vec<PathBuf>),
    /// Git-staged files (index vs HEAD).
    Staged,
    /// Files changed vs `<rev>` (default HEAD).
    Changed(Option<String>),
}

/// Discover the repository root (git work-dir) starting from `start`, falling back to
/// `start` itself when not in a git repo.
pub fn discover_root(start: &Path) -> PathBuf {
    match gix::discover(start) {
        Ok(repo) => repo
            .work_dir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| start.to_path_buf()),
        Err(_) => start.to_path_buf(),
    }
}

/// Patterns always excluded from coverage: the `LICENSES/` text tree and `.reuse/`
/// metadata are REUSE infrastructure, not annotatable source (FR-014, FR-016).
const IMPLICIT_EXCLUDES: &[&str] = &["LICENSES/**", ".reuse/**", "REUSE.toml"];

/// Build the exclusion matcher from config glob patterns plus implicit REUSE excludes.
fn build_excludes(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in IMPLICIT_EXCLUDES {
        builder.add(Glob::new(p).expect("valid implicit exclude"));
    }
    for p in patterns {
        let glob = Glob::new(p)
            .map_err(|e| LicetError::Config(format!("invalid exclude glob `{p}`: {e}")))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| LicetError::Internal(format!("glob build: {e}")))
}

/// A discovered file: repo-relative path plus whether it is excluded.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub rel_path: PathBuf,
    pub abs_path: PathBuf,
    pub excluded: bool,
}

/// Enumerate files to evaluate, applying selection, exclusions, and symlink dedup.
pub fn enumerate(
    root: &Path,
    selection: &Selection,
    exclude_patterns: &[String],
) -> Result<Vec<Discovered>> {
    let excludes = build_excludes(exclude_patterns)?;
    let rel_paths = match selection {
        Selection::FullTree => walk_full_tree(root)?,
        Selection::Files(files) => normalize_file_list(root, files),
        Selection::Staged => git_subset(
            root,
            &["diff", "--name-only", "--cached", "--diff-filter=d"],
        )?,
        Selection::Changed(rev) => {
            let rev = rev.clone().unwrap_or_else(|| "HEAD".to_string());
            git_subset(root, &["diff", "--name-only", "--diff-filter=d", &rev])?
        }
    };

    // Symlink dedup: a file reached via symlink is annotated once (Edge Cases).
    let mut seen_targets: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();
    for rel in rel_paths {
        let abs = root.join(&rel);
        let canonical = abs.canonicalize().unwrap_or_else(|_| abs.clone());
        if !seen_targets.insert(canonical) {
            continue; // already annotated via another path
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let excluded = excludes.is_match(&rel_str);
        out.push(Discovered {
            rel_path: rel,
            abs_path: abs,
            excluded,
        });
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// Full gitignore-aware parallel-capable walk; returns repo-relative file paths.
fn walk_full_tree(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .parents(true)
        .build();
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if let Ok(rel) = entry.path().strip_prefix(root) {
                // Skip VCS internals and our own cache.
                let s = rel.to_string_lossy();
                if s.starts_with(".git/") || s == ".git" || s == ".licet-cache" {
                    continue;
                }
                paths.push(rel.to_path_buf());
            }
        }
    }
    Ok(paths)
}

/// Normalize an explicit file list to repo-relative paths.
fn normalize_file_list(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    files
        .iter()
        .map(|f| {
            if f.is_absolute() {
                f.strip_prefix(root).unwrap_or(f).to_path_buf()
            } else {
                // May already be repo-relative, or relative to cwd.
                let abs = std::env::current_dir()
                    .unwrap_or_else(|_| root.to_path_buf())
                    .join(f);
                abs.strip_prefix(root)
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|_| f.clone())
            }
        })
        .collect()
}

/// Run a `git` selection command and parse `--name-only` output into repo-relative paths.
///
/// Selection is a cold, small-N operation (hook/CI subset); the full-tree hot path stays
/// pure-Rust via `ignore`. Diff computation uses git directly for fidelity with the user's
/// exact staged/changed semantics.
fn git_subset(root: &Path, args: &[&str]) -> Result<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| LicetError::Git(format!("failed to run git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(LicetError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    let paths = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_match() {
        let set = build_excludes(&["vendor/**".to_string(), "*.lock".to_string()]).unwrap();
        assert!(set.is_match("vendor/x.rs"));
        assert!(set.is_match("Cargo.lock"));
        assert!(!set.is_match("src/lib.rs"));
    }
}
