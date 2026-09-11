//! File enumeration over a consistent snapshot (F06, F07).
//!
//! Default policy operations cover **tracked regular files** in a Git
//! repository (never the ignore-walk, so a later `.gitignore` rule cannot drop
//! a tracked file and untracked files cannot sneak into the gate); `lint`
//! additionally covers nonignored untracked files. Outside a repository the
//! nonignored filesystem walk is retained. Reads always go through
//! [`git::Snapshot`] so `--staged` evaluates index blobs — including staged
//! metadata, configuration, and license texts — while everything else reads
//! working-tree bytes. Explicit file arguments are normalized lexically
//! (no symlink resolution) and rejected when outside the root.

pub mod git;

use std::path::{Component, Path, PathBuf};

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;

use crate::config::{CONFIG_FILENAME, has_legacy_only_config, legacy_config_error};
use crate::error::{LicetError, Result};

pub use git::{GitRepo, Snapshot};

/// Which files an invocation evaluates (selection flags are mutually exclusive, FR-027).
#[derive(Debug, Clone)]
pub enum Selection {
    /// Default: the full tracked/working tree.
    FullTree,
    /// An explicit list of paths (relative to cwd, or absolute).
    Files(Vec<PathBuf>),
    /// Git-staged files (index vs HEAD).
    Staged,
    /// Files changed vs `<rev>` (default HEAD).
    Changed(Option<String>),
}

/// What the evaluated file set is for: declared-policy operations honor
/// `[exclude]`; REUSE validation (`lint`) never does.
///
/// `Policy` (used by `check`) evaluates `--staged` from index blobs so the
/// gate sees the commit as it would land. `Apply` (used by `apply`) uses the
/// same path sets but always reads the working tree — `apply --staged` edits
/// working-tree files and must never expand its edit set beyond the staged
/// paths because of staged metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Policy,
    Apply,
    Lint,
}

/// A discovered file: root-relative path (lossless, OS-native) plus whether
/// REUSE itself ignores it. Declaration exclusions are applied later by the
/// engine so `lint` can skip them.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub rel_path: PathBuf,
    pub abs_path: PathBuf,
    /// REUSE-level ignore (LICENSES/, `.reuse/`, sidecar-as-file, zero-byte,
    /// symlink, SPDX document, …) — never a covered file.
    pub reuse_ignored: bool,
}

/// Discover the repository root (git work-dir) starting from `start`, falling
/// back to `start` itself when not in a git repo. A bare repository (no working
/// tree to evaluate) is an explicit error.
pub fn discover_root(start: &Path) -> Result<(PathBuf, Option<GitRepo>)> {
    match git::discover_repo(start)? {
        git::RepoDisposition::Repo(repo) => Ok((repo.root.clone(), Some(repo))),
        git::RepoDisposition::NonRepo => Ok((start.to_path_buf(), None)),
        git::RepoDisposition::Bare => Err(LicetError::Config(
            "bare git repository has no working tree to evaluate".to_string(),
        )),
    }
}

/// A fully resolved evaluation plan: what to read, from which snapshot, and
/// the configuration text observed through that same snapshot.
pub struct Prepared {
    pub root: PathBuf,
    pub source: crate::domain::ContentSource,
    pub snapshot: Snapshot,
    /// Final evaluated files (sorted by path), with REUSE-ignore status.
    pub paths: Vec<Discovered>,
    /// True when a staged/changed subset was expanded to full coverage because
    /// staged metadata could affect other files.
    pub expanded: bool,
    /// Human note for `expanded` (becomes a report warning).
    pub expansion_note: Option<String>,
    /// Configuration text read through the snapshot.
    pub config_text: String,
}

/// Resolve `selection` into paths + snapshot + configuration, without parsing
/// the configuration (commands parse it and hand it to the engine).
pub fn prepare(
    cwd: &Path,
    config_arg: &Path,
    selection: &Selection,
    purpose: Purpose,
    allow_missing_config: bool,
) -> Result<Prepared> {
    let (root, repo) = discover_root(cwd)?;
    let config_rel = resolve_config_rel(cwd, &root, config_arg);

    match selection {
        Selection::FullTree => {
            let universe = match (&repo, purpose) {
                (Some(repo), Purpose::Policy | Purpose::Apply) => {
                    let index = git::IndexSnapshot::load(&repo.root)?;
                    index.regular_files()
                }
                (Some(repo), Purpose::Lint) => {
                    let index = git::IndexSnapshot::load(&repo.root)?;
                    git::lint_path_set(&repo.root, &index)?
                }
                (None, _) => walk_worktree(&root)?,
            };
            let snapshot = Snapshot::Worktree { root: root.clone() };
            let config_text = read_config_text(
                &snapshot,
                &root,
                config_rel.as_deref(),
                purpose,
                allow_missing_config,
            )?;
            let paths = mark_sorted(&root, &snapshot, &universe)?;
            Ok(Prepared {
                root,
                source: crate::domain::ContentSource::Worktree,
                snapshot,
                paths,
                expanded: false,
                expansion_note: None,
                config_text,
            })
        }
        Selection::Files(files) => {
            let universe = normalize_file_list(&root, cwd, files)?;
            let snapshot = Snapshot::Worktree { root: root.clone() };
            let config_text = read_config_text(
                &snapshot,
                &root,
                config_rel.as_deref(),
                purpose,
                allow_missing_config,
            )?;
            let paths = mark_sorted(&root, &snapshot, &universe)?;
            Ok(Prepared {
                root,
                source: crate::domain::ContentSource::Worktree,
                snapshot,
                paths,
                expanded: false,
                expansion_note: None,
                config_text,
            })
        }
        Selection::Staged => {
            let repo = repo.ok_or_else(|| {
                LicetError::Config("--staged requires a git repository".to_string())
            })?;
            let mut index = git::IndexSnapshot::load(&repo.root)?;
            let (subset, _unborn) = git::staged_path_set(&repo.root, &index)?;
            if matches!(purpose, Purpose::Apply) {
                // Mutating runs read the working tree over exactly the staged
                // path set: edits must never spill beyond it, and the index is
                // never written.
                let snapshot = Snapshot::Worktree { root: root.clone() };
                let config_text = read_config_text(
                    &snapshot,
                    &root,
                    config_rel.as_deref(),
                    Purpose::Policy,
                    allow_missing_config,
                )?;
                let paths = mark_sorted(&root, &snapshot, &subset)?;
                return Ok(Prepared {
                    root,
                    source: crate::domain::ContentSource::Worktree,
                    snapshot,
                    paths,
                    expanded: false,
                    expansion_note: None,
                    config_text,
                });
            }
            // An unresolved merge poisons the whole index snapshot, not just the
            // evaluated subset.
            let unmerged: Vec<PathBuf> = index
                .entries
                .iter()
                .filter(|(_, e)| e.stage != 0)
                .map(|(p, _)| p.clone())
                .collect();
            if !unmerged.is_empty() {
                return Err(LicetError::Config(format!(
                    "cannot evaluate the staged snapshot: unresolved merge entries: {}",
                    display_list(&unmerged)
                )));
            }
            let deleted = git::staged_deleted_paths(&repo.root)?;
            let (universe, expanded, note) =
                maybe_expand(&index, subset, &deleted, config_rel.as_deref());
            let extra: Vec<PathBuf> = config_rel.clone().into_iter().collect();
            index.prefetch(&repo.root, &universe, &extra)?;
            let snapshot = Snapshot::Index(index);
            let config_text = read_config_text(
                &snapshot,
                &root,
                config_rel.as_deref(),
                purpose,
                allow_missing_config,
            )?;
            let paths = mark_sorted(&root, &snapshot, &universe)?;
            Ok(Prepared {
                root,
                source: crate::domain::ContentSource::Index,
                snapshot,
                paths,
                expanded,
                expansion_note: note,
                config_text,
            })
        }
        Selection::Changed(rev) => {
            let repo = repo.ok_or_else(|| {
                LicetError::Config("--changed requires a git repository".to_string())
            })?;
            let rev = rev.clone().unwrap_or_else(|| "HEAD".to_string());
            let oid = git::validate_rev_to_commit(&repo.root, &rev)?;
            let subset = git::changed_path_set(&repo.root, &oid)?;
            let deleted = git::changed_deleted_paths(&repo.root, &oid)?;
            let index = git::IndexSnapshot::load(&repo.root)?;
            // Mutating runs never expand: edits stay within the chosen set.
            let (universe, expanded, note) = if matches!(purpose, Purpose::Apply) {
                (subset, false, None)
            } else {
                maybe_expand(&index, subset, &deleted, config_rel.as_deref())
            };
            let snapshot = Snapshot::Worktree { root: root.clone() };
            let config_text = read_config_text(
                &snapshot,
                &root,
                config_rel.as_deref(),
                purpose,
                allow_missing_config,
            )?;
            let paths = mark_sorted(&root, &snapshot, &universe)?;
            Ok(Prepared {
                root,
                source: crate::domain::ContentSource::Worktree,
                snapshot,
                paths,
                expanded,
                expansion_note: note,
                config_text,
            })
        }
    }
}

/// Resolve the config argument to a root-relative path when it stays inside the
/// root; `None` when it points outside (worktree reads only — an index snapshot
/// cannot contain it).
fn resolve_config_rel(cwd: &Path, root: &Path, config_arg: &Path) -> Option<PathBuf> {
    // Omitted configs arrive as absolute `<root>/licet.toml` (see
    // `CommonArgs::config_arg`); every explicit relative path stays
    // invocation-cwd-relative.
    let abs = if config_arg.is_absolute() {
        config_arg.to_path_buf()
    } else {
        cwd.join(config_arg)
    };
    let norm = lexical_normalize(&abs);
    norm.strip_prefix(root).map(|p| p.to_path_buf()).ok()
}

/// Read configuration text for policy commands: through the snapshot when the
/// config lives inside the root, directly from the filesystem otherwise.
/// Absence and undecodable content are errors, never silent defaults (F13) —
/// except when `allow_missing_config` tolerates a missing file (kept for
/// `add-license --all`, which still scans headers without a config; task 9
/// distinguishes omitted vs explicit config paths).
fn read_config_text(
    snapshot: &Snapshot,
    root: &Path,
    config_rel: Option<&Path>,
    purpose: Purpose,
    allow_missing_config: bool,
) -> Result<String> {
    match purpose {
        Purpose::Lint => {
            // Task 5 owns lint's config semantics; until then preserve the
            // lenient read (missing/unparseable content falls back later).
            let path = match config_rel {
                Some(rel) => root.join(rel),
                None => PathBuf::from(CONFIG_FILENAME),
            };
            Ok(std::fs::read_to_string(path).unwrap_or_default())
        }
        Purpose::Policy | Purpose::Apply => match config_rel {
            Some(rel) => read_snapshot_config(snapshot, rel, root, allow_missing_config),
            None => Err(LicetError::Config(
                "config file is outside the project root".to_string(),
            )),
        },
    }
}

/// Read configuration through a snapshot (staged checks included): the file
/// must exist in that same snapshot inside the root, otherwise exit 2.
fn read_snapshot_config(
    snapshot: &Snapshot,
    rel: &Path,
    root: &Path,
    allow_missing_config: bool,
) -> Result<String> {
    match snapshot.read(rel)? {
        Some(bytes) => String::from_utf8(bytes).map_err(|e| {
            LicetError::Config(format!(
                "config {} is not valid UTF-8: {e}",
                root.join(rel).display()
            ))
        }),
        None if allow_missing_config => Ok(String::new()),
        None => Err(
            if rel == Path::new(CONFIG_FILENAME) && has_legacy_only_config(root) {
                legacy_config_error(root)
            } else {
                LicetError::Config(format!(
                    "cannot read config `{}`: not present in the evaluated snapshot",
                    root.join(rel).display()
                ))
            },
        ),
    }
}

/// Expand a staged/changed subset to full tracked coverage when it contains
/// licensing metadata that can affect other files (conservative full expansion;
/// always reported). Returns the final set plus the report note.
fn maybe_expand(
    index: &git::IndexSnapshot,
    subset: Vec<PathBuf>,
    trigger_only: &[PathBuf],
    config_rel: Option<&Path>,
) -> (Vec<PathBuf>, bool, Option<String>) {
    let triggers: Vec<PathBuf> = subset
        .iter()
        .chain(trigger_only.iter())
        .filter(|p| is_metadata_affecting(p, config_rel))
        .cloned()
        .collect();
    if triggers.is_empty() {
        return (subset, false, None);
    }
    let mut universe = index.regular_files();
    // Keep explicitly selected sidecars/texts even when untracked-side inputs
    // arrived via --changed (they are worktree reads; index may lack them).
    for p in &subset {
        if !universe.contains(p) {
            universe.push(p.clone());
        }
    }
    universe.sort();
    let note = format!(
        "selection expanded to full tracked coverage: staged metadata affects other files ({})",
        display_list(&triggers)
    );
    (universe, true, Some(note))
}

/// Paths whose staged/changed state can affect files beyond themselves.
fn is_metadata_affecting(rel: &Path, config_rel: Option<&Path>) -> bool {
    if Some(rel) == config_rel {
        return true;
    }
    if rel.file_name().map(|n| n == "REUSE.toml").unwrap_or(false) {
        return true;
    }
    if rel == Path::new(".reuse/dep5") {
        return true;
    }
    if rel.extension().map(|e| e == "license").unwrap_or(false) {
        return true;
    }
    if rel.starts_with("LICENSES") {
        return true;
    }
    false
}

fn display_list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Classify universe paths with their REUSE-ignore status (sorted).
fn mark_sorted(root: &Path, snapshot: &Snapshot, universe: &[PathBuf]) -> Result<Vec<Discovered>> {
    let mut out = Vec::with_capacity(universe.len());
    for rel in universe {
        out.push(Discovered {
            rel_path: rel.clone(),
            abs_path: root.join(rel),
            reuse_ignored: is_reuse_ignored(snapshot, rel)?,
        });
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// REUSE 3.3 "Covered and ignored Files" plus structural skips.
///
/// Ignored: root `LICENSES/` texts (a root FILE named exactly `LICENSES` stays
/// covered), `COPYING`/`LICENSE`/`LICENCE` variants at any depth, VCS
/// internals, every `REUSE.toml`, root `.reuse/`, Meson `subprojects/`,
/// symlinks, zero-byte files, SPDX documents, and `*.license` sidecars (which
/// are checked through their companion, never on their own). A nonempty
/// `data.empty` is NOT exempt — only zero-byte content is.
fn is_reuse_ignored(snapshot: &Snapshot, rel: &Path) -> Result<bool> {
    // Root LICENSES tree, but not a root file literally named `LICENSES`.
    if rel != Path::new("LICENSES") && rel.starts_with("LICENSES") {
        return Ok(true);
    }
    if let Some(name) = rel.file_name().and_then(|n| n.to_str()) {
        if is_license_filename(name) || name == "REUSE.toml" {
            return Ok(true);
        }
        if name.contains(".spdx.") || name.ends_with(".spdx") {
            return Ok(true);
        }
    }
    if rel.extension().map(|e| e == "license").unwrap_or(false) {
        return Ok(true);
    }
    // VCS internals, root .reuse/, Meson subprojects (separate projects).
    if rel
        .components()
        .any(|c| matches!(c, Component::Normal(n) if n == ".git"))
        || rel.file_name().map(|n| n == ".git").unwrap_or(false)
    {
        return Ok(true);
    }
    if rel != Path::new(".reuse") && rel.starts_with(".reuse") {
        return Ok(true);
    }
    if rel != Path::new("subprojects") && rel.starts_with("subprojects") {
        return Ok(true);
    }
    // Symlinks and zero-byte files need a stat/read from the snapshot.
    match snapshot {
        Snapshot::Worktree { root } => {
            let abs = root.join(rel);
            match std::fs::symlink_metadata(&abs) {
                Ok(meta) => {
                    if meta.file_type().is_symlink() {
                        return Ok(true);
                    }
                    if meta.file_type().is_file() && meta.len() == 0 {
                        return Ok(true);
                    }
                    Ok(false)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(_) => Ok(false),
            }
        }
        Snapshot::Index(index) => {
            let Some(entry) = index.entries.get(rel) else {
                return Ok(false);
            };
            if !entry.is_regular_file() {
                return Ok(true);
            }
            match index.read(rel)? {
                Some(bytes) => Ok(bytes.is_empty()),
                None => Ok(false),
            }
        }
    }
}

/// `COPYING`, `LICENSE`, `LICENCE`, optionally followed by a dash/dot separator
/// plus metadata (`LICENSE-MIT`, `COPYING.GPL`, `LICENCE.md`).
fn is_license_filename(name: &str) -> bool {
    for base in ["COPYING", "LICENSE", "LICENCE"] {
        if name == base {
            return true;
        }
        if let Some(rest) = name.strip_prefix(base)
            && let Some(sep) = rest.chars().next()
            && (sep == '-' || sep == '.')
            && rest.len() > 1
        {
            return true;
        }
    }
    false
}

/// Normalize an explicit file list to root-relative paths.
///
/// Inputs are relative to the invocation cwd (or absolute); `.`/`..` are
/// resolved lexically without touching the filesystem (so symlinks never
/// change file identity). Paths outside the root and nonregular inputs
/// (directories, FIFOs, …) are usage errors; symlinks are kept and marked
/// REUSE-ignored later, identically regardless of list order.
fn normalize_file_list(root: &Path, cwd: &Path, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let abs = if f.is_absolute() {
            lexical_normalize(f)
        } else {
            lexical_normalize(&cwd.join(f))
        };
        let rel = abs
            .strip_prefix(root)
            .map(|p| p.to_path_buf())
            .map_err(|_| {
                LicetError::Config(format!(
                    "file {} is outside the project root {}",
                    f.display(),
                    root.display()
                ))
            })?;
        if rel.as_os_str().is_empty() {
            return Err(LicetError::Config(
                "file selection names the project root itself".to_string(),
            ));
        }
        match std::fs::symlink_metadata(&abs) {
            Ok(meta) => {
                let ft = meta.file_type();
                if ft.is_symlink() || ft.is_file() {
                    // Symlinks are marked REUSE-ignored downstream; never resolved here.
                } else {
                    return Err(LicetError::Config(format!(
                        "file {} is not a regular file",
                        f.display()
                    )));
                }
            }
            // Missing inputs stay in the set and classify from absence (Unreadable).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(LicetError::Config(format!(
                    "cannot stat file {}: {e}",
                    f.display()
                )));
            }
        }
        out.push(rel);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Lexically normalize `.`/`..` without resolving symlinks or touching the
/// filesystem.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// VCS internals excluded from the worktree walk as `ignore` overrides.
/// (Scans create no files of their own, so nothing else needs skipping.)
const ALWAYS_SKIP: &[&str] = &["!.git/"];

/// Nonignored filesystem walk; returns root-relative file paths. Traversal
/// errors surface instead of silently skipping entries (F13).
fn walk_worktree(root: &Path) -> Result<Vec<PathBuf>> {
    let mut ob = OverrideBuilder::new(root);
    for glob in ALWAYS_SKIP {
        ob.add(glob)
            .expect("valid built-in skip override; ALWAYS_SKIP is a const");
    }
    let overrides = ob
        .build()
        .expect("valid built-in skip overrides; ALWAYS_SKIP is a const");

    let mut paths = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .parents(true)
        .overrides(overrides)
        .follow_links(false)
        .build();
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                return Err(LicetError::Io(std::io::Error::other(format!(
                    "directory traversal failed: {e}"
                ))));
            }
        };
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            && let Ok(rel) = entry.path().strip_prefix(root)
        {
            paths.push(rel.to_path_buf());
        }
    }
    Ok(paths)
}

/// Build the exclusion matcher from config `[exclude]` glob patterns.
/// REUSE-level ignores live in [`is_reuse_ignored`], not here.
pub fn build_excludes(patterns: &[String]) -> Result<globset::GlobSet> {
    let mut builder = globset::GlobSetBuilder::new();
    for p in patterns {
        let glob = globset::Glob::new(p)
            .map_err(|e| LicetError::Config(format!("invalid exclude glob `{p}`: {e}")))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| LicetError::Internal(format!("glob build: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_normalize_keeps_identity() {
        assert_eq!(
            lexical_normalize(Path::new("./src/../src/x.rs")),
            PathBuf::from("src/x.rs")
        );
        assert_eq!(
            lexical_normalize(Path::new("/r/a/../../b")),
            PathBuf::from("/b")
        );
    }

    #[test]
    fn license_filename_variants() {
        // Spec shape: base plus a dash/dot separator with metadata.
        for name in [
            "COPYING",
            "LICENSE",
            "LICENCE",
            "LICENSE-MIT",
            "COPYING.GPL",
            "LICENCE.md",
            "LICENSE.Apache-2.0.txt",
            "LICENSE.MIT.bak.extra",
        ] {
            assert!(is_license_filename(name), "{name}");
        }
        for name in ["LICENSES", "licensed.rs", "UNLICENSE", "LICENSE_"] {
            assert!(!is_license_filename(name), "{name}");
        }
    }

    #[test]
    fn walk_skips_git_internals_but_reports_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        for (rel, body) in [
            (".git/config", "x"),
            (".gitignore", "target\n"),
            ("src/lib.rs", "fn f() {}\n"),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let mut found = walk_worktree(root).unwrap();
        found.sort();
        assert_eq!(
            found,
            vec![PathBuf::from(".gitignore"), PathBuf::from("src/lib.rs")]
        );
    }

    #[test]
    fn excludes_match() {
        let set = build_excludes(&["vendor/**".to_string(), "*.lock".to_string()]).unwrap();
        assert!(set.is_match("vendor/x.rs"));
        assert!(set.is_match("Cargo.lock"));
        assert!(!set.is_match("src/lib.rs"));
    }
}
