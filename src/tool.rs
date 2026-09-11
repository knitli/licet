//! Trusted resolution of external helper binaries (`git`, `curl`) (F06, F07).
//!
//! Every licet command runs with an attacker-influenced current directory
//! (the repository under evaluation), so helpers must never be resolved
//! through OS executable-search semantics: on Windows the working directory
//! is part of the search order, letting a checkout containing a planted
//! `git.exe` win over the real tool installed later on `PATH`. Resolution
//! here is an explicit `PATH` scan that ignores empty entries (which mean
//! "the current directory" by convention) and relative entries (which
//! resolve against the current directory), returning an absolute path.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A helper binary that could not be resolved to an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedTool {
    /// Bare binary name that was requested (e.g. `"git"`).
    pub name: String,
}

impl std::fmt::Display for UnresolvedTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "helper `{}` not found via PATH lookup (empty and relative PATH \
             entries are ignored so a repository checkout can never supply \
             the binary); install it or check PATH",
            self.name
        )
    }
}

impl std::error::Error for UnresolvedTool {}

/// Resolve `name` to an absolute path via [`resolve_in`] over the live `PATH`.
pub fn resolve(name: &str) -> Result<PathBuf, UnresolvedTool> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    resolve_in(name, &path_var).ok_or_else(|| UnresolvedTool {
        name: name.to_string(),
    })
}

/// Scan one `PATH` value for `name`, skipping entries that resolve against
/// the current directory. Separated for hermetic testing (no env mutation).
fn resolve_in(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    // Windows executes `name` or `name.exe`; Unix uses `name` exactly.
    let mut candidates = vec![name.to_string()];
    if cfg!(windows) {
        candidates.push(format!("{name}.exe"));
    }
    for dir in std::env::split_paths(path_var) {
        // Empty entries mean CWD; relative entries resolve from the CWD.
        // Both would let the evaluated checkout supply the binary.
        if dir.as_os_str().is_empty() || dir.is_relative() {
            continue;
        }
        for candidate in &candidates {
            let full = dir.join(candidate);
            if is_executable_file(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// A regular file the OS would execute (executable bit on Unix).
fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake `PATH` root containing `name` (executable on Unix).
    fn bin_dir(name: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, b"fake").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        dir
    }

    fn join_path(dirs: &[&Path]) -> std::ffi::OsString {
        std::env::join_paths(dirs.iter()).unwrap()
    }

    #[test]
    fn finds_binary_in_absolute_path_entry() {
        let bin = bin_dir("git");
        let found = resolve_in("git", &join_path(&[bin.path()])).unwrap();
        assert_eq!(found, bin.path().join("git"));
    }

    #[test]
    fn skips_empty_and_relative_path_entries() {
        let bin = bin_dir("git");
        // Empty entries mean the CWD by convention; relative entries resolve
        // from the CWD. Both are skipped before any filesystem check, so a
        // PATH of only untrusted entries resolves nothing even though an
        // absolute entry finds the binary.
        let path_var = join_path(&[Path::new(""), Path::new("rel-bin"), bin.path()]);
        assert_eq!(resolve_in("git", &path_var), Some(bin.path().join("git")));
        let untrusted = join_path(&[Path::new(""), Path::new("rel-bin")]);
        assert_eq!(resolve_in("git", &untrusted), None);
    }

    #[test]
    fn missing_binary_resolves_to_none() {
        let bin = bin_dir("git");
        assert_eq!(resolve_in("curl", &join_path(&[bin.path()])), None);
    }

    #[test]
    fn non_executable_file_is_not_a_match() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("git"), b"not executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(dir.path().join("git"))
                .unwrap()
                .permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(dir.path().join("git"), perms).unwrap();
        }
        #[cfg(not(unix))]
        {
            // On non-Unix every regular file counts; just assert the positive
            // path instead of fighting platform semantics here.
            assert!(is_executable_file(&dir.path().join("git")));
            return;
        }
        assert_eq!(resolve_in("git", &join_path(&[dir.path()])), None);
    }

    #[test]
    fn unresolved_error_names_the_tool() {
        let err = UnresolvedTool {
            name: "git".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("`git`") && msg.contains("PATH"), "{msg}");
    }
}
