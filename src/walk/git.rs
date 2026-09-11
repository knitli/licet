//! System-Git repository discovery and index-snapshot reads (F06, F07).
//!
//! All repository structure comes from checked `git` subprocesses (never shell
//! text): root discovery, the tracked index (`ls-files --stage -z`), subsets
//! (`diff --name-only -z`), and blob bytes (one `git cat-file --batch` process
//! per scan, addressed by object id). NUL-delimited output is decoded losslessly
//! — Unix paths keep their exact bytes through lookup and writes.
//!
//! [`Snapshot`] is the single read surface both sources implement, so detection,
//! metadata, configuration, and license-text inventory always observe the same
//! selected content ([`crate::domain::ContentSource`]).

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
#[cfg(unix)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::domain::ContentSource;
use crate::error::{LicetError, Result};

/// Resolve the system `git` binary without consulting the working directory
/// ([`crate::tool`]): every subprocess below runs with the evaluated
/// repository as its CWD, which must never supply the executable itself.
fn git_binary() -> Result<PathBuf> {
    crate::tool::resolve("git").map_err(|e| LicetError::Git(e.to_string()))
}

/// Run a checked `git` command in `root`; return raw stdout. Failures retain
/// stderr context. Arguments are passed as an argv array — never shell text.
pub fn git_output(root: &Path, args: &[&OsStr]) -> Result<Vec<u8>> {
    let output = std::process::Command::new(git_binary()?)
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| LicetError::Git(format!("failed to launch git: {e}")))?;
    if !output.status.success() {
        let stderr: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(500)
            .collect();
        return Err(LicetError::Git(format!(
            "git {} failed: {}",
            args.iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
            stderr.trim()
        )));
    }
    Ok(output.stdout)
}

/// A repository with a working tree, discovered via system Git.
#[derive(Debug, Clone)]
pub struct GitRepo {
    /// Absolute working-tree root (`rev-parse --show-toplevel`).
    pub root: PathBuf,
    /// Absolute Git metadata directory (`rev-parse --absolute-git-dir`).
    pub git_dir: PathBuf,
}

/// How `start` relates to a Git repository.
#[derive(Debug)]
pub enum RepoDisposition {
    /// A repository with a working tree.
    Repo(GitRepo),
    /// Not inside a Git working tree: filesystem fallback applies.
    NonRepo,
    /// A bare repository (no working tree to evaluate).
    Bare,
}

/// Discover the repository disposition of `start`, distinguishing "not a
/// repository" from Git being missing or failing (F13).
pub fn discover_repo(start: &Path) -> Result<RepoDisposition> {
    let output = std::process::Command::new(git_binary()?)
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--show-toplevel", "--absolute-git-dir"])
        .output()
        .map_err(|e| LicetError::Git(format!("failed to launch git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Ok(RepoDisposition::NonRepo);
        }
        if stderr.contains("must be run in a work tree") {
            return Ok(RepoDisposition::Bare);
        }
        let excerpt: String = stderr.chars().take(300).collect();
        return Err(LicetError::Git(format!(
            "cannot determine repository state: {}",
            excerpt.trim()
        )));
    }
    let mut lines = output.stdout.split(|b| *b == b'\n');
    let toplevel = lines.next().unwrap_or_default();
    let git_dir = lines.next().unwrap_or_default();
    // Exactly two trailing-newline-terminated lines; anything else (notably an
    // embedded newline in a path) is an explicit failure, not a guess.
    if toplevel.is_empty() || git_dir.is_empty() || lines.next().is_some_and(|l| !l.is_empty()) {
        return Err(LicetError::Git(
            "cannot parse `git rev-parse` output for the repository root".to_string(),
        ));
    }
    let root = bytes_to_path(toplevel)?;
    let git_dir = bytes_to_path(git_dir)?;
    Ok(RepoDisposition::Repo(GitRepo { root, git_dir }))
}

/// Decode NUL-delimited command output bytes into an OS-native path, losslessly
/// on Unix. A path that cannot be represented is an explicit failure, never a
/// lossy replacement (F07).
#[cfg(unix)]
fn bytes_to_path(bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

/// Decode NUL-delimited command output bytes into a path (non-Unix).
#[cfg(not(unix))]
fn bytes_to_path(bytes: &[u8]) -> Result<PathBuf> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(PathBuf::from(s)),
        Err(_) => Err(LicetError::Git(
            "git returned a path that is not valid Unicode on this platform".to_string(),
        )),
    }
}

/// Split NUL-delimited output into raw path byte strings, dropping the trailing empty.
fn split_nul_paths(out: &[u8]) -> Vec<&[u8]> {
    let mut parts: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    if parts.last() == Some(&&[][..]) {
        parts.pop();
    }
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// One index entry from `ls-files --stage -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// File mode (e.g. `0o100644` regular, `0o120000` symlink, `0o160000` gitlink).
    pub mode: u32,
    /// Blob/object id (hex).
    pub oid: String,
    /// Merge stage (`0` = resolved).
    pub stage: u32,
}

impl IndexEntry {
    /// True for a regular file blob (not symlink, gitlink, …).
    pub fn is_regular_file(&self) -> bool {
        self.mode & 0o170000 == 0o100000
    }
}

/// Parse one `ls-files --stage -z` record: `<mode> SP <oid> SP <stage> TAB <path>`.
fn parse_stage_record(rec: &[u8]) -> Result<(PathBuf, IndexEntry)> {
    let tab = rec
        .iter()
        .position(|b| *b == b'\t')
        .ok_or_else(|| LicetError::Git("cannot parse `git ls-files --stage` output".to_string()))?;
    let (meta, path_bytes) = rec.split_at(tab);
    let path_bytes = &path_bytes[1..];
    let meta = std::str::from_utf8(meta)
        .map_err(|_| LicetError::Git("cannot parse `git ls-files --stage` output".to_string()))?;
    let mut parts = meta.split(' ');
    let mode = parts.next().and_then(|m| u32::from_str_radix(m, 8).ok());
    let oid = parts.next().map(str::to_string);
    let stage = parts.next().and_then(|s| s.parse::<u32>().ok());
    match (mode, oid, stage) {
        (Some(mode), Some(oid), Some(stage)) => {
            Ok((bytes_to_path(path_bytes)?, IndexEntry { mode, oid, stage }))
        }
        _ => Err(LicetError::Git(
            "cannot parse `git ls-files --stage` output".to_string(),
        )),
    }
}

/// The full tracked index plus prefetched blobs: one consistent snapshot.
#[derive(Debug, Clone)]
pub struct IndexSnapshot {
    /// All index entries by root-relative path (lossless).
    pub entries: HashMap<PathBuf, IndexEntry>,
    blobs: HashMap<String, Vec<u8>>,
}

impl IndexSnapshot {
    /// Load every index entry. Blob bytes arrive via [`IndexSnapshot::prefetch`].
    pub fn load(root: &Path) -> Result<Self> {
        let out = git_output(
            root,
            &[
                OsStr::new("ls-files"),
                OsStr::new("--stage"),
                OsStr::new("-z"),
            ],
        )?;
        let mut entries = HashMap::new();
        for rec in split_nul_paths(&out) {
            let (path, entry) = parse_stage_record(rec)?;
            entries.insert(path, entry);
        }
        Ok(IndexSnapshot {
            entries,
            blobs: HashMap::new(),
        })
    }

    /// Regular-file paths in the index (submodules and symlinks excluded).
    pub fn regular_files(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|(_, e)| e.is_regular_file() && e.stage == 0)
            .map(|(p, _)| p.clone())
            .collect();
        paths.sort();
        paths
    }

    /// Fetch every blob for `paths` (plus their sidecars and `extra`, e.g. the
    /// effective config) with a single `git cat-file --batch` process addressed
    /// by object id.
    pub fn prefetch(&mut self, root: &Path, paths: &[PathBuf], extra: &[PathBuf]) -> Result<()> {
        let mut oids: HashSet<&str> = HashSet::new();
        let want = self.prefetch_want_list(paths, extra);
        for p in &want {
            if let Some(e) = self.entries.get(p)
                && e.is_regular_file()
                && e.stage == 0
            {
                oids.insert(e.oid.as_str());
            }
        }
        // Skip objects already held.
        let missing: Vec<&str> = oids
            .into_iter()
            .filter(|o| !self.blobs.contains_key(*o))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let mut request = Vec::new();
        for oid in &missing {
            request.extend_from_slice(oid.as_bytes());
            request.push(b'\n');
        }
        let mut child = std::process::Command::new(git_binary()?)
            .arg("-C")
            .arg(root)
            .args(["cat-file", "--batch"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| LicetError::Git(format!("failed to launch git cat-file: {e}")))?;
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| LicetError::Git("git cat-file has no stdin".to_string()))?
            .write_all(&request)
            .map_err(|e| LicetError::Git(format!("failed to query git cat-file: {e}")))?;
        drop(child.stdin.take());
        let output = child
            .wait_with_output()
            .map_err(|e| LicetError::Git(format!("git cat-file failed: {e}")))?;
        if !output.status.success() {
            return Err(LicetError::Git("git cat-file --batch failed".to_string()));
        }
        parse_batch_output(&output.stdout, &missing, &mut self.blobs)?;
        Ok(())
    }

    /// Everything one `git cat-file --batch` round must fetch: the requested
    /// paths plus their sidecars and `extra` (e.g. the effective config),
    /// every `REUSE.toml` document at any depth, the root `.reuse/dep5`, and
    /// every license text — so staged metadata/config/text changes resolve
    /// through the index, never the working copy. Pure assembly over the
    /// index entries, directly unit-testable.
    fn prefetch_want_list(&self, paths: &[PathBuf], extra: &[PathBuf]) -> Vec<PathBuf> {
        let mut want: Vec<PathBuf> = Vec::with_capacity(paths.len() * 2 + extra.len());
        for p in paths {
            want.push(p.clone());
            want.push(sidecar_for(p));
        }
        want.extend(extra.iter().cloned());
        // Repository-level metadata and every license text participate in the
        // same snapshot so staged metadata/config/text changes are visible.
        want.push(PathBuf::from("REUSE.toml"));
        want.push(PathBuf::from(".reuse/dep5"));
        for (p, e) in &self.entries {
            if e.is_regular_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n == "REUSE.toml")
            {
                want.push(p.clone());
            }
        }
        for (p, e) in &self.entries {
            if e.is_regular_file() && p.starts_with("LICENSES/") {
                want.push(p.clone());
            }
        }
        want
    }

    /// Blob bytes for a root-relative path: `None` when absent from the index,
    /// a gitlink, or a symlink (both ignored per REUSE, never followed).
    /// Unmerged entries and unknown objects are errors, never silent content.
    pub fn read(&self, rel: &Path) -> Result<Option<Vec<u8>>> {
        let entry = match self.entries.get(rel) {
            Some(e) => e,
            None => return Ok(None),
        };
        if entry.stage != 0 {
            return Err(LicetError::Git(format!(
                "cannot evaluate {}: unresolved merge stage {} in the index",
                rel.display(),
                entry.stage
            )));
        }
        if !entry.is_regular_file() {
            return Ok(None);
        }
        match self.blobs.get(&entry.oid) {
            Some(bytes) => Ok(Some(bytes.clone())),
            None => Err(LicetError::Internal(format!(
                "index object {} for {} was not prefetched",
                entry.oid,
                rel.display()
            ))),
        }
    }

    /// Names (`MIT`, `LicenseRef-X`, …) of regular `LICENSES/*.txt` blobs.
    pub fn license_text_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        for (p, e) in &self.entries {
            if !e.is_regular_file() || e.stage != 0 {
                continue;
            }
            // Exactly `LICENSES/<stem>.txt` (component-wise; nested LICENSES
            // directories are not a project-wide exemption).
            if p.parent() != Some(Path::new("LICENSES")) {
                continue;
            }
            let is_txt = p.extension().map(|x| x == "txt").unwrap_or(false);
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_txt {
                names.insert(stem.to_string());
            }
        }
        names
    }
}

/// Parse `cat-file --batch` output for exactly the requested oids:
/// `<oid> SP blob SP <size> LF <size bytes> LF`, or `<oid> SP missing LF`.
fn parse_batch_output(
    out: &[u8],
    missing: &[&str],
    blobs: &mut HashMap<String, Vec<u8>>,
) -> Result<()> {
    let mut rest = out;
    let mut seen: HashSet<&str> = HashSet::new();
    for oid in missing {
        // Header line.
        let nl = rest.iter().position(|b| *b == b'\n').ok_or_else(|| {
            LicetError::Git("truncated `git cat-file --batch` output".to_string())
        })?;
        let header = std::str::from_utf8(&rest[..nl]).map_err(|_| {
            LicetError::Git("cannot parse `git cat-file --batch` output".to_string())
        })?;
        rest = &rest[nl + 1..];
        let mut parts = header.split(' ');
        let got_oid = parts.next().unwrap_or_default();
        let kind = parts.next().unwrap_or_default();
        if got_oid != *oid {
            return Err(LicetError::Git(format!(
                "git cat-file answered {got_oid} for requested {oid}"
            )));
        }
        if kind == "missing" {
            return Err(LicetError::Git(format!(
                "index object {oid} is missing from the object store"
            )));
        }
        if kind != "blob" {
            return Err(LicetError::Git(format!(
                "index object {oid} is a {kind}, not a blob"
            )));
        }
        let size: usize = parts
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| LicetError::Git("cannot parse `git cat-file` blob size".to_string()))?;
        if rest.len() < size + 1 || rest[size] != b'\n' {
            return Err(LicetError::Git(
                "truncated `git cat-file --batch` blob".to_string(),
            ));
        }
        blobs.insert(oid.to_string(), rest[..size].to_vec());
        rest = &rest[size + 1..];
        seen.insert(*oid);
    }
    Ok(())
}

/// `<path>.license`: the sidecar companion of a root-relative path.
pub fn sidecar_for(rel: &Path) -> PathBuf {
    let mut os = rel.as_os_str().to_owned();
    os.push(".license");
    PathBuf::from(os)
}

/// Validate `rev` as a commit and return its object id (hex), so only a
/// resolved id ever reaches diff (F07).
pub fn validate_rev_to_commit(root: &Path, rev: &str) -> Result<String> {
    let arg = format!("{rev}^{{commit}}");
    let out = git_output(
        root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new(&arg),
        ],
    )
    .map_err(|_| LicetError::Config(format!("`{rev}` does not resolve to a commit")))?;
    let oid = String::from_utf8_lossy(&out).trim().to_string();
    if oid.len() != 40 || !oid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(LicetError::Config(format!(
            "`{rev}` does not resolve to a commit"
        )));
    }
    Ok(oid)
}

/// Index-vs-HEAD path set for `--staged`, or the full index on unborn HEAD
/// (where diff-vs-HEAD cannot run). Returns `(paths, full_index)` where the flag
/// records the unborn-HEAD fallback.
pub fn staged_path_set(root: &Path, index: &IndexSnapshot) -> Result<(Vec<PathBuf>, bool)> {
    let head = git_output(
        root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new("HEAD^{commit}"),
        ],
    );
    if head.is_err() {
        // Unborn HEAD: every indexed regular file is the staged set.
        return Ok((index.regular_files(), true));
    }
    let out = git_output(
        root,
        &[
            OsStr::new("diff"),
            OsStr::new("--name-only"),
            OsStr::new("-z"),
            OsStr::new("--cached"),
            OsStr::new("--diff-filter=d"),
            OsStr::new("--"),
        ],
    )?;
    let mut paths = Vec::new();
    for raw in split_nul_paths(&out) {
        paths.push(bytes_to_path(raw)?);
    }
    paths.sort();
    Ok((paths, false))
}

/// Staged deletions (filtered out of the evaluable set): a deleted metadata file
/// can still affect other files, so deletions feed expansion triggers. Empty on
/// unborn HEAD, where the evaluable set is already the full index.
pub fn staged_deleted_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let head = git_output(
        root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new("HEAD^{commit}"),
        ],
    );
    if head.is_err() {
        return Ok(Vec::new());
    }
    let out = git_output(
        root,
        &[
            OsStr::new("diff"),
            OsStr::new("--name-only"),
            OsStr::new("-z"),
            OsStr::new("--cached"),
            OsStr::new("--diff-filter=D"),
            OsStr::new("--"),
        ],
    )?;
    let mut paths = Vec::new();
    for raw in split_nul_paths(&out) {
        paths.push(bytes_to_path(raw)?);
    }
    paths.sort();
    Ok(paths)
}

/// Working-tree-vs-commit path set for `--changed <oid>`.
pub fn changed_path_set(root: &Path, commit_oid: &str) -> Result<Vec<PathBuf>> {
    let out = git_output(
        root,
        &[
            OsStr::new("diff"),
            OsStr::new("--name-only"),
            OsStr::new("-z"),
            OsStr::new("--diff-filter=d"),
            OsStr::new(commit_oid),
            OsStr::new("--"),
        ],
    )?;
    let mut paths = Vec::new();
    for raw in split_nul_paths(&out) {
        paths.push(bytes_to_path(raw)?);
    }
    paths.sort();
    Ok(paths)
}

/// Worktree deletions vs a commit: trigger-only paths for `--changed` expansion.
pub fn changed_deleted_paths(root: &Path, commit_oid: &str) -> Result<Vec<PathBuf>> {
    let out = git_output(
        root,
        &[
            OsStr::new("diff"),
            OsStr::new("--name-only"),
            OsStr::new("-z"),
            OsStr::new("--diff-filter=D"),
            OsStr::new(commit_oid),
            OsStr::new("--"),
        ],
    )?;
    let mut paths = Vec::new();
    for raw in split_nul_paths(&out) {
        paths.push(bytes_to_path(raw)?);
    }
    paths.sort();
    Ok(paths)
}

/// Tracked-plus-nonignored-untracked paths for Git-backed `lint`.
pub fn lint_path_set(root: &Path, index: &IndexSnapshot) -> Result<Vec<PathBuf>> {
    let mut set: HashSet<PathBuf> = index.entries.keys().cloned().collect();
    let out = git_output(
        root,
        &[
            OsStr::new("ls-files"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("-z"),
        ],
    )?;
    for raw in split_nul_paths(&out) {
        set.insert(bytes_to_path(raw)?);
    }
    let mut paths: Vec<PathBuf> = set.into_iter().collect();
    paths.sort();
    Ok(paths)
}

/// The single read surface: working-tree files or one prefetched index.
#[derive(Debug, Clone)]
pub enum Snapshot {
    Worktree { root: PathBuf },
    Index(IndexSnapshot),
}

impl Snapshot {
    pub fn source(&self) -> ContentSource {
        match self {
            Snapshot::Worktree { .. } => ContentSource::Worktree,
            Snapshot::Index(_) => ContentSource::Index,
        }
    }

    /// File bytes at a root-relative path: `None` when absent (or ignored by
    /// kind: symlink, gitlink). Read/permission failures are errors with the
    /// path attached — never silent absence (F13).
    pub fn read(&self, rel: &Path) -> Result<Option<Vec<u8>>> {
        match self {
            Snapshot::Index(index) => index.read(rel),
            Snapshot::Worktree { root } => match std::fs::read(root.join(rel)) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(LicetError::Io(std::io::Error::new(
                    e.kind(),
                    format!("cannot read {}: {e}", rel.display()),
                ))),
            },
        }
    }

    /// Candidate license-text files: root `LICENSES/` top-level entries in this
    /// snapshot (sorted). Symlinks and non-regular entries never count (F02);
    /// name analysis (suffixes, recognized ids, duplicates) lives in the
    /// inventory, which reads each candidate through this same snapshot.
    pub fn license_text_candidates(&self) -> Vec<PathBuf> {
        let mut out = match self {
            Snapshot::Index(index) => index
                .entries
                .iter()
                .filter(|(p, e)| e.is_regular_file() && p.parent() == Some(Path::new("LICENSES")))
                .map(|(p, _)| p.clone())
                .collect(),
            Snapshot::Worktree { root } => {
                let mut names = Vec::new();
                let dir = root.join("LICENSES");
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for e in entries.flatten() {
                        let path = e.path();
                        let Ok(meta) = std::fs::symlink_metadata(&path) else {
                            continue;
                        };
                        if !meta.file_type().is_file() {
                            continue;
                        }
                        if let Ok(rel) = path.strip_prefix(root) {
                            names.push(rel.to_path_buf());
                        }
                    }
                }
                names
            }
        };
        out.sort();
        out
    }

    /// Identifiers with a text file under `LICENSES/` in this snapshot.
    /// Symlinked entries never count (F02).
    pub fn license_text_names(&self) -> HashSet<String> {
        match self {
            Snapshot::Index(index) => index.license_text_names(),
            Snapshot::Worktree { root } => {
                let mut names = HashSet::new();
                let dir = root.join("LICENSES");
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for e in entries.flatten() {
                        let path = e.path();
                        if path.extension().map(|x| x == "txt").unwrap_or(false)
                            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                            && let Ok(meta) = std::fs::symlink_metadata(&path)
                            && meta.file_type().is_file()
                        {
                            names.insert(stem.to_string());
                        }
                    }
                }
                names
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stage_record() {
        let rec = b"100644 abcdef0123456789abcdef0123456789abcdef01 0\tcaf\xc3\xa9.rs";
        let (path, entry) = parse_stage_record(rec).unwrap();
        assert_eq!(path, PathBuf::from("caf\u{e9}.rs"));
        assert_eq!(entry.mode, 0o100644);
        assert_eq!(entry.stage, 0);
        assert!(entry.is_regular_file());
    }

    #[test]
    fn parses_stage_modes() {
        let rec = b"120000 0000000000000000000000000000000000000000 0\tlink";
        let (_, entry) = parse_stage_record(rec).unwrap();
        assert!(!entry.is_regular_file());
        let rec = b"160000 abcdef0123456789abcdef0123456789abcdef01 0\tsub";
        let (_, entry) = parse_stage_record(rec).unwrap();
        assert!(!entry.is_regular_file());
    }

    #[test]
    fn parses_batch_blob_exactly() {
        let mut blobs = HashMap::new();
        // size 4: content bytes `AB\nC`, then the framing LF, then trailing output.
        let out = b"deadbeef blob 4\nAB\nC\nTRAILING";
        parse_batch_output(out, &["deadbeef"], &mut blobs).unwrap();
        assert_eq!(blobs["deadbeef"], b"AB\nC");
    }

    #[test]
    fn batch_missing_is_an_error() {
        let mut blobs = HashMap::new();
        let out = b"deadbeef missing\n";
        assert!(parse_batch_output(out, &["deadbeef"], &mut blobs).is_err());
    }

    #[test]
    fn want_list_pairs_sidecars_and_metadata() {
        // The fetch set is exactly: requested paths + sidecars + extra,
        // root metadata documents, every nested REUSE.toml, and every
        // license text — symlinks and gitlinks never qualify.
        let entry = |mode: u32| IndexEntry {
            mode,
            oid: "abc".to_string(),
            stage: 0,
        };
        let snap = IndexSnapshot {
            entries: HashMap::from([
                (PathBuf::from("sub/REUSE.toml"), entry(0o100644)),
                (PathBuf::from("LICENSES/MIT.txt"), entry(0o100644)),
                (PathBuf::from("link.rs"), entry(0o120000)),
                (PathBuf::from("submod"), entry(0o160000)),
            ]),
            blobs: HashMap::new(),
        };
        let mut want =
            snap.prefetch_want_list(&[PathBuf::from("a.rs")], &[PathBuf::from("licet.toml")]);
        want.sort();
        assert_eq!(
            want,
            vec![
                PathBuf::from(".reuse/dep5"),
                PathBuf::from("LICENSES/MIT.txt"),
                PathBuf::from("REUSE.toml"),
                PathBuf::from("a.rs"),
                PathBuf::from("a.rs.license"),
                PathBuf::from("licet.toml"),
                PathBuf::from("sub/REUSE.toml"),
            ]
        );
    }
}
