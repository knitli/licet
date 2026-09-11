//! License-text inventory and offline materialization into `LICENSES/` (FR-014, FR-017).
//!
//! Every identifier is validated before any filesystem mutation: known SPDX
//! license/exception ids (canonicalized spelling) are materialized from the
//! embedded bundle offline; valid-but-unbundled standard ids may be fetched with
//! the system `curl` binary only under explicit `--allow-network` consent (into
//! owned temporary storage, never through the destination); syntactically valid
//! `LicenseRef-*` ids are reported as required local texts, never downloaded
//! and never scaffolded with placeholder prose. Anything else is a usage error
//! (exit 2) raised before any directory is created or any subprocess is spawned.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use crate::reuse::atomic_write;
use crate::spdx;

/// Maximum accepted downloaded license-text size (4 MiB).
pub const MAX_DOWNLOAD_BYTES: usize = 4 * 1024 * 1024;

/// Canonical download location for a standard SPDX id.
pub fn download_url(id: &str) -> String {
    format!("https://spdx.org/licenses/{id}.txt")
}

/// A `LICENSES/` entry that names no recognizable license.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrecognizedText {
    /// Snapshot-relative path (`LICENSES/Unknown-Thing.txt`).
    pub path: String,
    pub reason: String,
}

/// A recognized `LICENSES/` text that could not be validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableText {
    /// Snapshot-relative path.
    pub path: String,
    pub reason: String,
}

/// Why a text inventory cannot be computed at all.
#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    /// The same license id is claimed by several filenames (the reference
    /// tool aborts on this too — the inventory is ambiguous).
    #[error("duplicate license texts for `{id}`: {paths:?}")]
    DuplicateIds { id: String, paths: Vec<String> },
    /// The snapshot could not supply a text (merge state or prefetch gap).
    #[error("cannot read license text `{path}` from the evaluated snapshot: {reason}")]
    Snapshot { path: String, reason: String },
}

/// Referenced/present/missing/bundled inventory over `LICENSES/` and the config.
#[derive(Debug, Default)]
pub struct LicenseTextInventory {
    pub referenced: BTreeSet<String>,
    pub present: BTreeSet<String>,
    pub missing: BTreeSet<String>,
    pub bundled_available: BTreeSet<String>,
    /// Present but never referenced (project-wide lint finding only).
    pub unused: Vec<String>,
    /// Entries naming no recognizable license id (project-wide lint finding).
    pub unrecognized: Vec<UnrecognizedText>,
    /// Recognized ids kept in an extensionless file: they satisfy
    /// materialization and policy presence, but strict REUSE lint reports
    /// their missing extension (the spec requires one even though the
    /// reference tool accepts some extensionless names).
    pub missing_extension: Vec<String>,
    /// Recognized texts that could not be read as UTF-8: validation is
    /// incomplete for them, never a pass and never a proven violation.
    pub unreadable: Vec<UnreadableText>,
}

impl LicenseTextInventory {
    /// Compute the inventory for referenced identifiers through `snapshot`, so
    /// staged checks observe staged texts.
    ///
    /// Only the root `LICENSES/` directory's top level is inventoried.
    /// Recognized filename forms are `<id>.txt`, `<id>.md`, and the bare
    /// `<id>` (extensionless, flagged for lint). Anything else is
    /// unrecognized. Symlinked entries are ignored: a symlink is not a text
    /// the tool can vouch for (F02). Duplicate ids under several filenames
    /// are an error, never a silent pick.
    pub fn compute(
        snapshot: &crate::walk::Snapshot,
        referenced: &BTreeSet<String>,
    ) -> Result<Self, InventoryError> {
        // id -> snapshot-relative paths claiming it.
        let mut by_id: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        let mut unrecognized = Vec::new();
        let mut unreadable = Vec::new();
        let mut missing_extension = Vec::new();
        for rel in snapshot.license_text_candidates() {
            let display = rel.to_string_lossy().replace('\\', "/");
            let name = match rel.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => {
                    unrecognized.push(UnrecognizedText {
                        path: display,
                        reason: "filename is not valid UTF-8".to_string(),
                    });
                    continue;
                }
            };
            let (stem, suffixed) = match classify_text_filename(&name) {
                TextName::Recognized { id, suffixed } => (id, suffixed),
                TextName::Unrecognized { reason } => {
                    unrecognized.push(UnrecognizedText {
                        path: display,
                        reason,
                    });
                    continue;
                }
            };
            // A recognized text must be readable UTF-8 to count; undecodable
            // bytes are incomplete validation, never presence. A vanishing
            // file (worktree race) simply does not count. Index-snapshot
            // failures are snapshot inconsistencies, not file findings.
            // Presence is about naming and readability, never content
            // judgment: an empty but correctly named text counts (the
            // reference tool likewise reports no finding for it), because
            // licet never claims to prove legal correctness of file
            // contents.
            match snapshot.read(&rel) {
                Ok(Some(bytes)) => {
                    if std::str::from_utf8(&bytes).is_err() {
                        unreadable.push(UnreadableText {
                            path: display,
                            reason: "license text is not valid UTF-8".to_string(),
                        });
                        continue;
                    }
                }
                Ok(None) => continue,
                Err(e) if matches!(snapshot.source(), crate::domain::ContentSource::Worktree) => {
                    unreadable.push(UnreadableText {
                        path: display,
                        reason: format!("cannot read license text: {e}"),
                    });
                    continue;
                }
                Err(e) => {
                    return Err(InventoryError::Snapshot {
                        path: display,
                        reason: e.to_string(),
                    });
                }
            }
            if !suffixed {
                missing_extension.push(stem.clone());
            }
            by_id.entry(stem).or_default().push(display);
        }
        for (id, paths) in &by_id {
            if paths.len() > 1 {
                return Err(InventoryError::DuplicateIds {
                    id: id.clone(),
                    paths: paths.clone(),
                });
            }
        }
        let present: BTreeSet<String> = by_id.keys().cloned().collect();
        let mut missing = BTreeSet::new();
        let mut bundled_available = BTreeSet::new();
        for id in referenced {
            if !present.contains(id) {
                missing.insert(id.clone());
            }
            if spdx::bundled_text(id).is_some() {
                bundled_available.insert(id.clone());
            }
        }
        let unused: Vec<String> = present
            .iter()
            .filter(|id| !referenced.contains(*id))
            .cloned()
            .collect();
        missing_extension.retain(|id| present.contains(id));
        missing_extension.sort();
        missing_extension.dedup();
        Ok(LicenseTextInventory {
            referenced: referenced.clone(),
            present,
            missing,
            bundled_available,
            unused,
            unrecognized,
            missing_extension,
            unreadable,
        })
    }

    /// True when every referenced text is present **and** no text-level
    /// finding (unrecognized, missing extension, unreadable, unused) remains.
    /// `check` uses only `missing`; full lint uses this.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
            && self.unused.is_empty()
            && self.unrecognized.is_empty()
            && self.missing_extension.is_empty()
            && self.unreadable.is_empty()
    }
}

/// How a `LICENSES/` filename resolves to a license id.
enum TextName {
    Recognized { id: String, suffixed: bool },
    Unrecognized { reason: String },
}

/// Classify one `LICENSES/` filename: `<id>.txt` / `<id>.md` (suffixed),
/// bare `<id>` (recognized but extensionless), or unrecognized. Standard ids
/// match case-insensitively to canonical spelling; `LicenseRef-*` matches
/// exactly (custom ids keep their case).
fn classify_text_filename(name: &str) -> TextName {
    // Strip one recognized suffix; anything else must be the bare id itself.
    let (stem, suffixed) = if let Some(stem) = name.strip_suffix(".txt") {
        (stem, true)
    } else if let Some(stem) = name.strip_suffix(".md") {
        (stem, true)
    } else if name.contains('.') {
        return TextName::Unrecognized {
            reason: format!("unsupported license-text suffix in `{name}` (expected .txt or .md)"),
        };
    } else {
        (name, false)
    };
    if stem.is_empty() {
        return TextName::Unrecognized {
            reason: format!("empty license id in `{name}`"),
        };
    }
    if is_valid_license_ref(stem) {
        return TextName::Recognized {
            id: stem.to_string(),
            suffixed,
        };
    }
    // Legacy `GPL-2.0+`-style filenames are not normalized: the reference tool
    // treats them as a deprecated id that satisfies nothing, and this tool
    // rejects deprecated ids everywhere, so such a text can never satisfy a
    // reference. Point at the canonical `-only`/`-or-later` name instead.
    if stem.ends_with('+') {
        return TextName::Unrecognized {
            reason: format!(
                "`{stem}` uses a legacy `+` suffix; name the text with the canonical id instead"
            ),
        };
    }
    if let Some(canonical) = canonical_standard_id(stem) {
        return TextName::Recognized {
            id: canonical,
            suffixed,
        };
    }
    if let Some(exc) = ::spdx::exception_id(stem) {
        return TextName::Recognized {
            id: exc.name.to_string(),
            suffixed,
        };
    }
    TextName::Unrecognized {
        reason: format!("`{stem}` is not a recognized SPDX license id, exception, or LicenseRef-*"),
    }
}

/// A validated materialization target: what an id means and where its file lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedId {
    /// Standard SPDX license id with bundled text (canonical spelling).
    Bundled { canonical: String },
    /// Standard SPDX license/exception id absent from the bundle; fetchable over
    /// HTTPS under explicit network consent.
    Fetchable { canonical: String },
    /// Custom `LicenseRef-*`: the maintainer must supply the text locally.
    CustomRef { id: String },
}

/// Why an `add-license` identifier is rejected (usage error, exit 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidId {
    pub id: String,
    pub reason: String,
}

impl std::fmt::Display for InvalidId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid license id `{}`: {}", self.id, self.reason)
    }
}

impl std::error::Error for InvalidId {}

/// Validate one materialization id without touching the filesystem.
///
/// Canonicalizes standard-id spelling; rejects separators, absolute paths,
/// control bytes, leading dashes (option injection into subprocesses), and
/// compound expressions passed as a single id.
pub fn validate_materialize_id(id: &str) -> Result<ValidatedId, InvalidId> {
    let bad = |reason: &str| {
        Err(InvalidId {
            id: id.to_string(),
            reason: reason.to_string(),
        })
    };
    if id.is_empty() {
        return bad("identifier is empty");
    }
    if id.starts_with('-') {
        return bad("identifier must not start with `-`");
    }
    if id.contains(['/', '\\']) || Path::new(id).is_absolute() {
        return bad("identifier must not contain path separators or be absolute");
    }
    if id.chars().any(|c| c.is_control()) {
        return bad("identifier must not contain control characters");
    }
    if id.chars().any(|c| c.is_whitespace()) {
        return bad("compound SPDX expressions cannot be materialized as one id");
    }
    if let Some(canonical) = canonical_standard_id(id) {
        if spdx::bundled_text(&canonical).is_some() {
            return Ok(ValidatedId::Bundled { canonical });
        }
        return Ok(ValidatedId::Fetchable { canonical });
    }
    if let Some(exc) = ::spdx::exception_id(id) {
        return Ok(ValidatedId::Fetchable {
            canonical: exc.name.to_string(),
        });
    }
    if is_valid_license_ref(id) {
        return Ok(ValidatedId::CustomRef { id: id.to_string() });
    }
    bad("unknown SPDX license id, exception, or LicenseRef-*")
}

/// Canonical spelling of a standard license id: exact lookup first, then a
/// full-length imprecise match for the promised case tolerance (`mit` → `MIT`).
/// Prefix-only matches (`MITX`) are rejected by the length check.
fn canonical_standard_id(id: &str) -> Option<String> {
    if let Some(lic) = ::spdx::license_id(id) {
        return Some(lic.name.to_string());
    }
    if let Some((lic, matched)) = ::spdx::imprecise_license_id(id)
        && matched == id.len()
    {
        return Some(lic.name.to_string());
    }
    None
}

/// Syntactic shape of a custom reference: `LicenseRef-` plus a nonempty body of
/// alphanumerics, `.`, `-`, `+` (custom ids keep their exact case).
fn is_valid_license_ref(id: &str) -> bool {
    match id.strip_prefix("LicenseRef-") {
        Some(body) => {
            !body.is_empty()
                && body
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        }
        None => false,
    }
}

/// Failure to materialize license texts.
#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error("{0}")]
    InvalidId(#[from] InvalidId),
    #[error("{0}")]
    Inventory(#[from] InventoryError),
    #[error("cannot materialize license texts: {0}")]
    Io(#[from] io::Error),
}

impl From<crate::reuse::WriteError> for MaterializeError {
    fn from(w: crate::reuse::WriteError) -> Self {
        MaterializeError::Io(w.source)
    }
}

/// One required license text with no plannable write: the id plus why it is
/// blocked (a custom text nobody supplied, or a fetch `apply` never runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedText {
    pub id: String,
    pub message: String,
}

/// Plan `LICENSES/` installs without writing anything (FR-014, FR-017).
///
/// Validates the whole requested list first, exactly like [`materialize`]:
/// bundled-but-missing ids become creation-only [`PlannedWrite`]s
/// (`LicenseText`); valid-but-unbundled standard ids become explicit blockers
/// naming their download URL (a required fetch that has not occurred —
/// `apply`, dry-run or real, never fetches); custom `LicenseRef-*` ids with
/// no local text become explicit blockers naming the expected path. Texts
/// already present yield no records at all.
pub fn plan_text_writes(
    root: &Path,
    referenced: &BTreeSet<String>,
) -> Result<(Vec<crate::domain::PlannedWrite>, Vec<BlockedText>), MaterializeError> {
    use crate::domain::{PlannedWrite, WriteKind};
    let mut targets: Vec<ValidatedId> = Vec::with_capacity(referenced.len());
    for id in referenced {
        targets.push(validate_materialize_id(id)?);
    }
    let canonical_refs: BTreeSet<String> = targets
        .iter()
        .map(|t| match t {
            ValidatedId::Bundled { canonical } | ValidatedId::Fetchable { canonical } => {
                canonical.clone()
            }
            ValidatedId::CustomRef { id } => id.clone(),
        })
        .collect();
    let worktree = crate::walk::Snapshot::Worktree {
        root: root.to_path_buf(),
    };
    let inv = LicenseTextInventory::compute(&worktree, &canonical_refs)?;

    let mut writes = Vec::new();
    let mut blocked = Vec::new();
    for target in &targets {
        let (file_id, kind) = match target {
            ValidatedId::Bundled { canonical } => (canonical.clone(), "bundled"),
            ValidatedId::Fetchable { canonical } => (canonical.clone(), "fetchable"),
            ValidatedId::CustomRef { id } => (id.clone(), "custom"),
        };
        if !inv.missing.contains(&file_id) {
            continue;
        }
        match target {
            ValidatedId::Bundled { canonical } => match spdx::bundled_text(canonical) {
                Some(text) => writes.push(PlannedWrite {
                    path: Path::new("LICENSES").join(format!("{file_id}.txt")),
                    kind: WriteKind::LicenseText,
                    before: None,
                    after: Some(text.as_bytes().to_vec()),
                    affected_files: Vec::new(),
                }),
                None => blocked.push(BlockedText {
                    id: file_id.clone(),
                    message: format!(
                        "`{file_id}` ({kind}) validates but the bundle carries no text for it; \
                         add LICENSES/{file_id}.txt manually"
                    ),
                }),
            },
            ValidatedId::Fetchable { canonical } => blocked.push(BlockedText {
                id: file_id,
                message: format!(
                    "no offline text for `{canonical}` (fetch {} to LICENSES/{canonical}.txt \
                     with `add-license --allow-network`; dry-run and apply never fetch)",
                    download_url(canonical)
                ),
            }),
            ValidatedId::CustomRef { id } => blocked.push(BlockedText {
                id: id.clone(),
                message: format!(
                    "no local text for `{id}` (add LICENSES/{id}.txt manually); custom texts \
                     are never downloaded or scaffolded"
                ),
            }),
        }
    }
    Ok((writes, blocked))
}

/// Materialize referenced-but-missing texts into `LICENSES/` from the offline bundle.
///
/// Validates the **whole** requested list before creating any directory. Writes
/// go through the shared safe writer (creation only — an existing text is never
/// overwritten). `LicenseRef-*` and unbundled standard ids are left in
/// `still_missing` for the caller to report or fetch; no placeholder prose is
/// ever invented. Returns ids written and ids still missing.
pub fn materialize(
    root: &Path,
    referenced: &BTreeSet<String>,
) -> Result<MaterializeResult, MaterializeError> {
    let mut targets: Vec<ValidatedId> = Vec::with_capacity(referenced.len());
    for id in referenced {
        targets.push(validate_materialize_id(id)?);
    }
    // Inventory over canonical file ids (`mit` is satisfied by `MIT.txt`).
    let canonical_refs: BTreeSet<String> = targets
        .iter()
        .map(|t| match t {
            ValidatedId::Bundled { canonical } | ValidatedId::Fetchable { canonical } => {
                canonical.clone()
            }
            ValidatedId::CustomRef { id } => id.clone(),
        })
        .collect();
    let worktree = crate::walk::Snapshot::Worktree {
        root: root.to_path_buf(),
    };
    let inv = LicenseTextInventory::compute(&worktree, &canonical_refs)?;
    let mut written = Vec::new();
    let mut still_missing = Vec::new();

    if inv.missing.is_empty() {
        return Ok(MaterializeResult {
            written,
            still_missing,
        });
    }
    std::fs::create_dir_all(root.join("LICENSES"))?;
    for target in &targets {
        let (file_id, text) = match target {
            ValidatedId::Bundled { canonical } if inv.missing.contains(canonical) => {
                match spdx::bundled_text(canonical) {
                    Some(text) => (canonical.clone(), text),
                    None => {
                        still_missing.push(canonical.clone());
                        continue;
                    }
                }
            }
            ValidatedId::Fetchable { canonical } => {
                still_missing.push(canonical.clone());
                continue;
            }
            ValidatedId::CustomRef { id } => {
                still_missing.push(id.clone());
                continue;
            }
            _ => continue,
        };
        let rel = Path::new("LICENSES").join(format!("{file_id}.txt"));
        atomic_write(root, &rel, None, text.as_bytes())?;
        written.push(file_id);
    }
    Ok(MaterializeResult {
        written,
        still_missing,
    })
}

/// Outcome of [`materialize`].
#[derive(Debug)]
pub struct MaterializeResult {
    pub written: Vec<String>,
    pub still_missing: Vec<String>,
}

/// Fetch one standard license text over HTTPS with the system `curl` binary.
///
/// The download lands in owned temporary storage (never the destination): bounded
/// execution (`--connect-timeout 10`, `--max-time 60`, 4 MiB cap enforced in-code
/// as well as via `--max-filesize`), HTTPS-only initial and redirect protocols,
/// `--disable` first so user curl configuration cannot change behavior. The result
/// must be nonempty UTF-8 text. Nothing is deleted or installed on failure — the
/// caller installs successful bytes through the shared safe writer.
pub fn fetch_text_via_curl(id: &str) -> Result<Vec<u8>, FetchError> {
    let url = download_url(id);
    let tmp = tempfile::tempdir().map_err(FetchError::Launch)?;
    let out_path = tmp.path().join("license.txt");
    // Resolve without consulting the working directory ([`crate::tool`]): the
    // download runs for the repository under evaluation, which must never
    // supply the executable itself.
    let curl = crate::tool::resolve("curl").map_err(|e| {
        FetchError::Launch(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            e.to_string(),
        ))
    })?;
    let output = std::process::Command::new(curl)
        .arg("--disable")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--location")
        .arg("--proto")
        .arg("=https")
        .arg("--proto-redir")
        .arg("=https")
        .arg("--connect-timeout")
        .arg("10")
        .arg("--max-time")
        .arg("60")
        .arg("--max-filesize")
        .arg(MAX_DOWNLOAD_BYTES.to_string())
        .arg("--output")
        .arg(&out_path)
        .arg(&url)
        .output()
        .map_err(FetchError::Launch)?;
    if !output.status.success() {
        return Err(FetchError::Failed {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(500)
                .collect(),
        });
    }
    let bytes = std::fs::read(&out_path).map_err(FetchError::Launch)?;
    if bytes.is_empty() {
        return Err(FetchError::Empty);
    }
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(FetchError::TooLarge { bytes: bytes.len() });
    }
    if std::str::from_utf8(&bytes).is_err() {
        return Err(FetchError::InvalidUtf8);
    }
    Ok(bytes)
}

/// A failed license-text download. The destination is always untouched.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("could not run curl for the download: {0}")]
    Launch(#[source] io::Error),
    #[error("curl exited with status {status:?}: {stderr}")]
    Failed { status: Option<i32>, stderr: String },
    #[error("downloaded license text is empty")]
    Empty,
    #[error("downloaded license text is {bytes} bytes (limit {MAX_DOWNLOAD_BYTES})")]
    TooLarge { bytes: usize },
    #[error("downloaded license text is not valid UTF-8")]
    InvalidUtf8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn materializes_bundled_text() {
        let dir = tempdir().unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("MIT".to_string());
        let res = materialize(dir.path(), &refs).unwrap();
        assert_eq!(res.written, vec!["MIT".to_string()]);
        assert!(dir.path().join("LICENSES/MIT.txt").exists());
    }

    #[test]
    fn canonicalizes_standard_id_spelling() {
        let dir = tempdir().unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("mit".to_string());
        let res = materialize(dir.path(), &refs).unwrap();
        assert_eq!(res.written, vec!["MIT".to_string()]);
        assert!(dir.path().join("LICENSES/MIT.txt").exists());
    }

    #[test]
    fn license_ref_reports_required_text_without_scaffolding() {
        let dir = tempdir().unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("LicenseRef-Acme-1.0".to_string());
        let res = materialize(dir.path(), &refs).unwrap();
        assert!(res.written.is_empty());
        assert_eq!(res.still_missing, vec!["LicenseRef-Acme-1.0".to_string()]);
        // No placeholder prose is invented for custom licenses.
        assert!(!dir.path().join("LICENSES/LicenseRef-Acme-1.0.txt").exists());
    }

    #[test]
    fn invalid_ids_fail_before_any_directory_exists() {
        for bad in [
            "../victim",
            "/absolute",
            "a/b",
            "MIT OR Apache-2.0",
            "Definitely-Not-A-License-9.9",
            "",
            "-n",
            "MIT\x07",
        ] {
            let dir = tempdir().unwrap();
            let mut refs = BTreeSet::new();
            refs.insert(bad.to_string());
            let err = materialize(dir.path(), &refs).unwrap_err();
            assert!(
                matches!(err, MaterializeError::InvalidId(_)),
                "{bad:?} must be a validation error, got {err:?}"
            );
            assert!(
                !dir.path().join("LICENSES").exists(),
                "{bad:?} must not create any directory"
            );
        }
    }

    fn worktree_snapshot(dir: &tempfile::TempDir) -> crate::walk::Snapshot {
        crate::walk::Snapshot::Worktree {
            root: dir.path().to_path_buf(),
        }
    }

    #[test]
    fn inventory_reports_missing() {
        let dir = tempdir().unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("MIT".to_string());
        let snap = worktree_snapshot(&dir);
        let inv = LicenseTextInventory::compute(&snap, &refs).unwrap();
        assert!(inv.missing.contains("MIT"));
        assert!(inv.bundled_available.contains("MIT"));
        assert!(!inv.is_complete());
    }

    #[test]
    fn symlinked_text_is_not_present() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("LICENSES")).unwrap();
        let outside = tempdir().unwrap();
        let target = outside.path().join("MIT.txt");
        std::fs::write(&target, "external").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dir.path().join("LICENSES/MIT.txt")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, dir.path().join("LICENSES/MIT.txt")).unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("MIT".to_string());
        let snap = worktree_snapshot(&dir);
        let inv = LicenseTextInventory::compute(&snap, &refs).unwrap();
        assert!(
            inv.missing.contains("MIT"),
            "symlinked text must not count as present"
        );
    }

    #[test]
    fn duplicate_ids_are_an_error() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("LICENSES")).unwrap();
        std::fs::write(dir.path().join("LICENSES/MIT.txt"), "text").unwrap();
        std::fs::write(dir.path().join("LICENSES/MIT.md"), "text").unwrap();
        let snap = worktree_snapshot(&dir);
        let err = LicenseTextInventory::compute(&snap, &BTreeSet::new()).unwrap_err();
        assert!(err.to_string().contains("MIT"), "names the id: {err}");
    }

    #[test]
    fn extensionless_known_id_counts_but_is_flagged() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("LICENSES")).unwrap();
        std::fs::write(dir.path().join("LICENSES/MIT"), "text").unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("MIT".to_string());
        let snap = worktree_snapshot(&dir);
        let inv = LicenseTextInventory::compute(&snap, &refs).unwrap();
        assert!(
            inv.missing.is_empty(),
            "extensionless text satisfies presence"
        );
        assert_eq!(inv.missing_extension, vec!["MIT".to_string()]);
        assert!(
            !inv.is_complete(),
            "lint still reports the missing extension"
        );
    }

    #[test]
    fn dotted_licenseref_round_trips() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("LICENSES")).unwrap();
        std::fs::write(
            dir.path().join("LICENSES/LicenseRef-Acme-1.0.txt"),
            "custom",
        )
        .unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("LicenseRef-Acme-1.0".to_string());
        let snap = worktree_snapshot(&dir);
        let inv = LicenseTextInventory::compute(&snap, &refs).unwrap();
        assert!(inv.missing.is_empty());
        assert!(inv.present.contains("LicenseRef-Acme-1.0"));
    }

    #[test]
    fn unknown_and_bad_suffixes_are_unrecognized() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("LICENSES")).unwrap();
        std::fs::write(dir.path().join("LICENSES/Unknown-Thing.txt"), "x").unwrap();
        std::fs::write(dir.path().join("LICENSES/MIT.txt.txt"), "x").unwrap();
        std::fs::write(dir.path().join("LICENSES/GPL-2.0+.txt"), "x").unwrap();
        let snap = worktree_snapshot(&dir);
        let inv = LicenseTextInventory::compute(&snap, &BTreeSet::new()).unwrap();
        assert_eq!(inv.unrecognized.len(), 3);
        assert!(
            inv.unused.is_empty(),
            "unrecognized entries are not licenses"
        );
        assert!(!inv.is_complete());
    }

    #[test]
    fn non_utf8_text_is_unreadable_not_present() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("LICENSES")).unwrap();
        std::fs::write(dir.path().join("LICENSES/MIT.txt"), [0xffu8, 0xfe]).unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("MIT".to_string());
        let snap = worktree_snapshot(&dir);
        let inv = LicenseTextInventory::compute(&snap, &refs).unwrap();
        assert!(
            inv.missing.contains("MIT"),
            "undecodable bytes prove nothing"
        );
        assert_eq!(inv.unreadable.len(), 1);
        assert!(!inv.is_complete());
    }
}
