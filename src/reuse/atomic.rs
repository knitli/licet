//! Byte-safe atomic file replacement confined to an allowed root (F01, FR-024).
//!
//! Every mutation in the tool (source headers, sidecars, `REUSE.toml`, generated
//! configs, license texts) goes through [`atomic_write`], which validates the
//! destination **inside** the helper so no caller can forget a check:
//!
//! - the relative path cannot escape the allowed root (no absolute paths, no `..`),
//! - no ancestor (and never the destination itself) may be a symlink / reparse point,
//! - the destination must be a regular file (directories, FIFOs, sockets rejected),
//! - the caller states what it expects to find (`None` = must not exist, `Some` =
//!   exact bytes); anything else aborts before touching the destination,
//! - replacement uses an exclusively-created sibling temp file
//!   ([`tempfile::NamedTempFile`]) — never a predictable name — preserves the
//!   existing file's permissions, fsyncs content, re-verifies the destination,
//!   then renames; directory durability is synced where supported.
//!
//! The helper is portable containment, not an OS-level compare-and-swap against a
//! hostile process concurrently replacing ancestors: it detects changes observed
//! between planning and replacement but makes no stronger race guarantee.

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// Failure of [`atomic_write`].
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub struct WriteError {
    /// The underlying I/O failure.
    pub source: std::io::Error,
    /// True when the replacement bytes were committed (rename succeeded) and the
    /// failure happened afterward (durability sync). Callers must count such a
    /// write as applied even though an error is reported.
    pub replacement_completed: bool,
}

impl WriteError {
    fn before(source: io::Error) -> Self {
        WriteError {
            source,
            replacement_completed: false,
        }
    }

    fn after(source: io::Error) -> Self {
        WriteError {
            source,
            replacement_completed: true,
        }
    }

    /// A destination read failure (permission, I/O) surfaced as a write error
    /// that never committed anything.
    pub fn for_read_failure(source: io::Error) -> Self {
        WriteError::before(source)
    }
}

impl From<WriteError> for io::Error {
    fn from(w: WriteError) -> io::Error {
        w.source
    }
}

/// Library-level seam forcing the next [`atomic_write`] to a given (canonical)
/// destination to fail its post-persist durability sync — after the replacement
/// bytes are committed. Path-scoped (and consumed one-shot) so parallel tests
/// using other destinations are unaffected. Hidden test hook, not a CLI switch.
static POST_PERSIST_FAILURE_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Arm the post-persist sync failure for one write to `canonical_dest`.
/// Pass the canonical destination path (e.g. `dir.canonicalize()?.join(rel)`).
#[doc(hidden)]
pub fn __licet_test_force_post_persist_sync_failure_for(canonical_dest: &Path) {
    *POST_PERSIST_FAILURE_PATH.lock().unwrap() = Some(canonical_dest.to_path_buf());
}

/// Take the armed failure iff it targets `dest`.
fn take_post_persist_failure_for(dest: &Path) -> bool {
    let mut guard = POST_PERSIST_FAILURE_PATH.lock().unwrap();
    if guard.as_deref() == Some(dest) {
        *guard = None;
        true
    } else {
        false
    }
}

/// Read a mutation destination, distinguishing absence from failure (F13).
///
/// `Ok(None)` only on `NotFound`; permission errors, I/O failures, and (via the
/// caller) invalid UTF-8 are errors, never silent absence.
pub fn read_expected_for_write(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Destination states distinguished without following symlinks.
enum Dest {
    Absent,
    Present { permissions: std::fs::Permissions },
}

/// Classify the destination without following symlinks; permission/read
/// failures are errors, never absence (F13). Every refusal (symlink,
/// non-regular file, unexpected presence/absence/content) aborts before
/// any byte is staged or written.
fn classify_destination(
    dest: &Path,
    relative: &Path,
    expected: Option<&[u8]>,
) -> Result<Dest, WriteError> {
    match std::fs::symlink_metadata(dest) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if expected.is_some() {
                return Err(WriteError::before(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "destination {} disappeared or was never read; refusing to write",
                        relative.display()
                    ),
                )));
            }
            Ok(Dest::Absent)
        }
        Err(e) => Err(WriteError::before(io::Error::new(
            e.kind(),
            format!("cannot stat destination {}: {e}", relative.display()),
        ))),
        Ok(meta) => {
            let ft = meta.file_type();
            if is_link(&meta) {
                return Err(WriteError::before(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "refusing to write through symlink destination {}",
                        relative.display()
                    ),
                )));
            }
            if !ft.is_file() {
                return Err(WriteError::before(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "refusing to replace non-regular destination {}",
                        relative.display()
                    ),
                )));
            }
            match expected {
                None => Err(WriteError::before(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "destination {} already exists; refusing to create",
                        relative.display()
                    ),
                ))),
                Some(exp) => {
                    let current = std::fs::read(dest).map_err(|e| {
                        WriteError::before(io::Error::new(
                            e.kind(),
                            format!("cannot re-read destination {}: {e}", relative.display()),
                        ))
                    })?;
                    if current.as_slice() != exp {
                        return Err(WriteError::before(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "destination {} changed since it was read; refusing to replace",
                                relative.display()
                            ),
                        )));
                    }
                    Ok(Dest::Present {
                        permissions: meta.permissions(),
                    })
                }
            }
        }
    }
}

/// Atomically replace (or create) `relative` under `root`.
///
/// - `expected = None`: the destination must not exist (creation).
/// - `expected = Some(bytes)`: the destination must be a regular file whose
///   current bytes equal `bytes`; anything else aborts with no write.
/// - `replacement`: exact bytes to install.
///
/// See the module docs for the containment checks applied to every call.
pub fn atomic_write(
    root: &Path,
    relative: &Path,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> Result<(), WriteError> {
    let rel = join_checked(relative)?;
    let canon_root = root.canonicalize().map_err(|e| {
        WriteError::before(io::Error::new(
            e.kind(),
            format!("cannot resolve allowed root {}: {e}", root.display()),
        ))
    })?;
    let dest = canon_root.join(&rel);
    let parent = dest.parent().ok_or_else(|| {
        WriteError::before(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("destination has no parent: {}", relative.display()),
        ))
    })?;

    check_ancestors(&canon_root, parent, relative)?;

    let dest_state = classify_destination(&dest, relative, expected)?;

    // Exclusively-created temp file in the destination directory: random name, so
    // a pre-existing predictable `<name>.licet.tmp` (regular file or symlink) is
    // never touched.
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
        WriteError::before(io::Error::new(
            e.kind(),
            format!("cannot create temp file for {}: {e}", relative.display()),
        ))
    })?;
    if let Err(e) = (|| {
        tmp.write_all(replacement)?;
        tmp.flush()?;
        match &dest_state {
            Dest::Present { permissions } => {
                tmp.as_file().set_permissions(permissions.clone())?;
            }
            #[cfg(unix)]
            Dest::Absent => {
                use std::os::unix::fs::PermissionsExt;
                tmp.as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o644))?;
            }
            #[cfg(not(unix))]
            Dest::Absent => {}
        }
        tmp.as_file().sync_all()?;
        Ok::<(), io::Error>(())
    })() {
        return Err(WriteError::before(io::Error::new(
            e.kind(),
            format!("cannot stage replacement for {}: {e}", relative.display()),
        )));
    }

    // Re-verify the destination immediately before replacing it.
    recheck(&dest, relative, expected)?;

    // Commit. Creation uses no-clobber persistence so a concurrently created file
    // is an error, not a silent overwrite.
    let persisted = match &dest_state {
        Dest::Absent => tmp.persist_noclobber(&dest),
        Dest::Present { .. } => tmp.persist(&dest),
    }
    .map_err(|e| {
        WriteError::before(io::Error::new(
            e.error.kind(),
            format!("cannot replace {}: {}", relative.display(), e.error),
        ))
    })?;

    // Post-commit durability. Any failure here still leaves the replacement bytes
    // on disk, so it is reported with `replacement_completed = true`.
    if take_post_persist_failure_for(&dest) {
        return Err(WriteError::after(io::Error::other(format!(
            "injected post-persist sync failure for {}",
            relative.display()
        ))));
    }
    if let Err(e) = persisted.sync_all() {
        return Err(WriteError::after(io::Error::new(
            e.kind(),
            format!(
                "replacement of {} committed but sync failed: {e}",
                relative.display()
            ),
        )));
    }
    if let Err(e) = sync_dir(parent) {
        return Err(WriteError::after(io::Error::new(
            e.kind(),
            format!(
                "replacement of {} committed but directory sync failed: {e}",
                relative.display()
            ),
        )));
    }
    Ok(())
}

/// Validate `relative` and join it onto an already-canonical root.
///
/// Rejects absolute paths and any `..` component; `.` components are skipped
/// (they cannot escape the root).
fn join_checked(relative: &Path) -> Result<PathBuf, WriteError> {
    if relative.as_os_str().is_empty() {
        return Err(WriteError::before(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination path is empty",
        )));
    }
    let mut rel = PathBuf::new();
    let mut saw_normal = false;
    for comp in relative.components() {
        match comp {
            Component::Normal(c) => {
                rel.push(c);
                saw_normal = true;
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(WriteError::before(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "destination {} escapes its allowed root",
                        relative.display()
                    ),
                )));
            }
        }
    }
    if !saw_normal {
        return Err(WriteError::before(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("destination {} names no file", relative.display()),
        )));
    }
    Ok(rel)
}

/// Reject symlink/reparse ancestors and non-directory ancestors from `parent`
/// up to (and including) the canonical root.
fn check_ancestors(canon_root: &Path, parent: &Path, relative: &Path) -> Result<(), WriteError> {
    let mut anc = parent;
    loop {
        match std::fs::symlink_metadata(anc) {
            Ok(meta) => {
                if is_link(&meta) {
                    return Err(WriteError::before(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "refusing to write through symlink ancestor {} for {}",
                            anc.display(),
                            relative.display()
                        ),
                    )));
                }
                if !meta.file_type().is_dir() {
                    return Err(WriteError::before(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "ancestor {} of {} is not a directory",
                            anc.display(),
                            relative.display()
                        ),
                    )));
                }
            }
            Err(e) => {
                return Err(WriteError::before(io::Error::new(
                    e.kind(),
                    format!(
                        "cannot stat ancestor {} of {}: {e}",
                        anc.display(),
                        relative.display()
                    ),
                )));
            }
        }
        if anc == canon_root {
            break;
        }
        anc = anc.parent().ok_or_else(|| {
            WriteError::before(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "destination {} escapes its allowed root",
                    relative.display()
                ),
            ))
        })?;
    }
    Ok(())
}

/// Re-verify destination type/content immediately before the rename.
fn recheck(dest: &Path, relative: &Path, expected: Option<&[u8]>) -> Result<(), WriteError> {
    match std::fs::symlink_metadata(dest) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if expected.is_some() {
                return Err(WriteError::before(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "destination {} changed since it was read; refusing to replace",
                        relative.display()
                    ),
                )));
            }
            Ok(())
        }
        Err(e) => Err(WriteError::before(io::Error::new(
            e.kind(),
            format!("cannot re-stat destination {}: {e}", relative.display()),
        ))),
        Ok(meta) => {
            if is_link(&meta) || !meta.file_type().is_file() {
                return Err(WriteError::before(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "destination {} changed type since it was read; refusing to replace",
                        relative.display()
                    ),
                )));
            }
            match expected {
                None => Err(WriteError::before(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "destination {} was created concurrently; refusing to overwrite",
                        relative.display()
                    ),
                ))),
                Some(exp) => {
                    let current = std::fs::read(dest).map_err(|e| {
                        WriteError::before(io::Error::new(
                            e.kind(),
                            format!("cannot re-read destination {}: {e}", relative.display()),
                        ))
                    })?;
                    if current.as_slice() != exp {
                        return Err(WriteError::before(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "destination {} changed since it was read; refusing to replace",
                                relative.display()
                            ),
                        )));
                    }
                    Ok(())
                }
            }
        }
    }
}

/// True for symlinks and (on Windows) any reparse point such as junctions.
fn is_link(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileAttributesExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Fsync a directory handle where supported; no-op elsewhere.
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let f = std::fs::File::open(dir)?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn replaces_content_and_leaves_no_temp() {
        let dir = root();
        let rel = Path::new("f.txt");
        std::fs::write(dir.path().join(rel), "old").unwrap();
        atomic_write(dir.path(), rel, Some(b"old"), b"new content").unwrap();
        assert_eq!(std::fs::read(dir.path().join(rel)).unwrap(), b"new content");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name().to_string_lossy().contains(".licet.tmp")
                    || e.file_name().to_string_lossy().starts_with(".tmp")
            })
            .collect();
        assert!(leftovers.is_empty(), "temp file leaked: {leftovers:?}");
    }

    #[test]
    fn creates_missing_destination_only_when_expected_none() {
        let dir = root();
        let rel = Path::new("sub/new.txt");
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        atomic_write(dir.path(), rel, None, b"created").unwrap();
        assert_eq!(std::fs::read(dir.path().join(rel)).unwrap(), b"created");
        // Creating over an existing file is refused.
        let err = atomic_write(dir.path(), rel, None, b"again").unwrap_err();
        assert!(!err.replacement_completed);
        assert_eq!(err.source.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(dir.path().join(rel)).unwrap(), b"created");
        // Overwriting without the expected bytes is refused.
        let err = atomic_write(dir.path(), rel, Some(b"stale"), b"x").unwrap_err();
        assert_eq!(err.source.kind(), io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(dir.path().join(rel)).unwrap(), b"created");
        // Overwriting a missing file with expected bytes is refused.
        let err =
            atomic_write(dir.path(), Path::new("sub/gone.txt"), Some(b"old"), b"x").unwrap_err();
        assert!(!err.replacement_completed);
    }

    #[cfg(unix)]
    #[test]
    fn preserves_mode_bits() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o600, 0o700, 0o755] {
            let dir = root();
            let rel = Path::new("f.sh");
            let abs = dir.path().join(rel);
            std::fs::write(&abs, b"old").unwrap();
            std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(mode)).unwrap();
            atomic_write(dir.path(), rel, Some(b"old"), b"new").unwrap();
            let got = std::fs::symlink_metadata(&abs)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(got, mode, "mode {mode:o} must survive replacement");
            assert_eq!(std::fs::read(&abs).unwrap(), b"new");
        }
    }

    #[test]
    fn rejects_escape_and_absolute_paths() {
        let dir = root();
        for bad in [
            "../outside",
            "a/../../outside",
            "..",
            "/absolute/path",
            "",
            ".",
        ] {
            let err = atomic_write(dir.path(), Path::new(bad), None, b"x").unwrap_err();
            assert!(!err.replacement_completed, "{bad}");
            assert_eq!(err.source.kind(), io::ErrorKind::InvalidInput, "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_destination_and_ancestors() {
        use std::os::unix::fs::symlink;
        let dir = root();
        let outside = root();
        let sentinel = outside.path().join("sentinel");
        std::fs::write(&sentinel, b"KEEP").unwrap();

        // Symlink destination.
        symlink(&sentinel, dir.path().join("link.txt")).unwrap();
        let err = atomic_write(dir.path(), Path::new("link.txt"), None, b"x").unwrap_err();
        assert_eq!(err.source.kind(), io::ErrorKind::InvalidInput);
        let err = atomic_write(dir.path(), Path::new("link.txt"), Some(b"KEEP"), b"x").unwrap_err();
        assert_eq!(err.source.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"KEEP");

        // Symlink parent directory.
        std::fs::create_dir(outside.path().join("real")).unwrap();
        symlink(outside.path().join("real"), dir.path().join("linkdir")).unwrap();
        let err = atomic_write(dir.path(), Path::new("linkdir/f.txt"), None, b"x").unwrap_err();
        assert_eq!(err.source.kind(), io::ErrorKind::InvalidInput);
        assert!(!outside.path().join("real/f.txt").exists());
    }

    #[test]
    fn rejects_non_regular_destination() {
        let dir = root();
        std::fs::create_dir(dir.path().join("adir")).unwrap();
        let err = atomic_write(dir.path(), Path::new("adir"), None, b"x").unwrap_err();
        assert_eq!(err.source.kind(), io::ErrorKind::InvalidInput);
        // Directory mistaken for an expected file is also refused.
        let err = atomic_write(dir.path(), Path::new("adir"), Some(b""), b"x").unwrap_err();
        assert_eq!(err.source.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn fifo_destination_is_refused() {
        let dir = root();
        let fifo = dir.path().join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo available");
        assert!(status.success());
        let err = atomic_write(dir.path(), Path::new("pipe"), None, b"x").unwrap_err();
        assert_eq!(err.source.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn preserves_existing_predictable_temp_contents() {
        let dir = root();
        std::fs::write(dir.path().join("a.rs"), b"old").unwrap();
        std::fs::write(dir.path().join(".a.rs.licet.tmp"), b"PREDICTABLE").unwrap();
        atomic_write(dir.path(), Path::new("a.rs"), Some(b"old"), b"new").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join(".a.rs.licet.tmp")).unwrap(),
            b"PREDICTABLE"
        );
        assert_eq!(std::fs::read(dir.path().join("a.rs")).unwrap(), b"new");
    }

    #[test]
    fn two_destinations_do_not_collide() {
        let dir = root();
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
        atomic_write(dir.path(), Path::new("a.txt"), Some(b"a"), b"A").unwrap();
        atomic_write(dir.path(), Path::new("b.txt"), Some(b"b"), b"B").unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"A");
        assert_eq!(std::fs::read(dir.path().join("b.txt")).unwrap(), b"B");
    }

    #[test]
    fn post_persist_sync_failure_reports_committed_write() {
        let dir = root();
        let rel = Path::new("f.txt");
        std::fs::write(dir.path().join(rel), b"old").unwrap();
        let canon_dest = dir.path().canonicalize().unwrap().join(rel);
        __licet_test_force_post_persist_sync_failure_for(&canon_dest);
        let err = atomic_write(dir.path(), rel, Some(b"old"), b"new").unwrap_err();
        assert!(
            err.replacement_completed,
            "post-persist failure must flag the committed write"
        );
        assert_eq!(std::fs::read(dir.path().join(rel)).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_destination_is_an_error_not_absence() {
        use std::os::unix::fs::PermissionsExt;
        let dir = root();
        let rel = Path::new("locked.txt");
        let abs = dir.path().join(rel);
        std::fs::write(&abs, b"secret").unwrap();
        std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o000)).unwrap();
        let res = atomic_write(dir.path(), rel, None, b"x");
        // Restore so the temp dir can be cleaned up.
        std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o600)).unwrap();
        if res.is_ok() {
            // Running as root: permission bits do not restrict us; nothing to assert.
            return;
        }
        let err = res.unwrap_err();
        assert!(!err.replacement_completed);
    }

    #[test]
    fn read_expected_distinguishes_absence_from_failure() {
        let dir = root();
        assert_eq!(
            read_expected_for_write(&dir.path().join("missing")).unwrap(),
            None
        );
        std::fs::write(dir.path().join("f"), b"data").unwrap();
        assert_eq!(
            read_expected_for_write(&dir.path().join("f")).unwrap(),
            Some(b"data".to_vec())
        );
    }
}
