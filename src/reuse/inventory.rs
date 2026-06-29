//! License-text inventory and offline materialization into `LICENSES/` (FR-014, FR-017).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::spdx;

/// Referenced/present/missing/bundled inventory over `LICENSES/` and the config.
#[derive(Debug, Default)]
pub struct LicenseTextInventory {
    pub referenced: BTreeSet<String>,
    pub present: BTreeSet<String>,
    pub missing: BTreeSet<String>,
    pub bundled_available: BTreeSet<String>,
}

impl LicenseTextInventory {
    /// Compute the inventory for a set of referenced identifiers at `root`.
    pub fn compute(root: &Path, referenced: &BTreeSet<String>) -> Self {
        let present = present_texts(root);
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
        LicenseTextInventory {
            referenced: referenced.clone(),
            present,
            missing,
            bundled_available,
        }
    }

    /// True when every referenced text is present (REUSE text-existence check).
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Identifiers whose text files exist under `LICENSES/`. REUSE recognizes the license
/// text with a `.txt` or `.md` suffix or no suffix at all, so all three spellings map to
/// the same identifier (matching `reuse`'s own discovery — FR-014).
fn present_texts(root: &Path) -> BTreeSet<String> {
    let mut present = BTreeSet::new();
    let dir = root.join("LICENSES");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if let Some(name) = e.path().file_name().and_then(|s| s.to_str())
                && let Some(id) = license_id_from_filename(name)
            {
                present.insert(id);
            }
        }
    }
    present
}

/// Map a `LICENSES/` filename to its SPDX identifier: strip a trailing `.txt`/`.md`
/// (case-insensitive). Any other filename *is* the id — an extension-less `LicenseRef-Acme-1.0`
/// must not have its `.0` treated as an extension, so we never split on the last dot.
fn license_id_from_filename(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    for ext in [".txt", ".md"] {
        if lower.ends_with(ext) {
            return Some(name[..name.len() - ext.len()].to_string());
        }
    }
    Some(name.to_string())
}

/// Materialize referenced-but-missing texts into `LICENSES/` from the offline bundle.
/// Returns ids written and ids still missing (FR-017).
///
/// **Never scaffolds a placeholder.** A stub file would satisfy REUSE's text-existence
/// check while leaving the repo non-compliant, so a missing text is surfaced as
/// `still_missing` for the caller to fetch (opt-in) or error on — never papered over.
pub fn materialize(
    root: &Path,
    referenced: &BTreeSet<String>,
) -> std::io::Result<MaterializeResult> {
    let dir = root.join("LICENSES");
    let inv = LicenseTextInventory::compute(root, referenced);
    let mut written = Vec::new();
    let mut still_missing = Vec::new();

    if inv.missing.is_empty() {
        return Ok(MaterializeResult {
            written,
            still_missing,
        });
    }
    std::fs::create_dir_all(&dir)?;
    for id in &inv.missing {
        if let Some(text) = spdx::bundled_text(id) {
            std::fs::write(dir.join(format!("{id}.txt")), text)?;
            written.push(id.clone());
        } else {
            still_missing.push(id.clone());
        }
    }
    Ok(MaterializeResult {
        written,
        still_missing,
    })
}

/// Outcome of [`materialize`].
pub struct MaterializeResult {
    pub written: Vec<String>,
    pub still_missing: Vec<String>,
}

/// Direct SPDX raw-text URL for a standard (non-`LicenseRef`) license id.
pub fn spdx_text_url(id: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/spdx/license-list-data/refs/heads/main/text/{id}.txt"
    )
}

/// Actionable guidance for a missing license text — never mentions an unavailable network
/// flag. For a `LicenseRef-*` we can only name the path to create; for a standard SPDX id we
/// link the exact upstream text to drop into `LICENSES/`.
pub fn missing_text_guidance(id: &str) -> String {
    if spdx::is_license_ref(id) {
        format!("`{id}` is missing — add its full text at LICENSES/{id}.txt")
    } else {
        format!(
            "`{id}` is missing — download {url} and save it to LICENSES/{id}.txt",
            url = spdx_text_url(id)
        )
    }
}

/// Fetch a standard SPDX license text via the system `curl` into `LICENSES/<id>.txt`.
///
/// Opt-in only: licet ships nothing that can reach the network (the offline-guard forbids
/// any network crate in the dependency tree). This shells out to a `curl` binary the user
/// already has, only when they have explicitly permitted it. Errors — writing nothing — for
/// `LicenseRef-*`, when `curl` is absent, or on any non-success/empty response. Returns the
/// written path on success.
pub fn fetch_via_curl(root: &Path, id: &str) -> Result<PathBuf, String> {
    if spdx::is_license_ref(id) {
        return Err(format!("{id} is a custom LicenseRef and cannot be fetched"));
    }
    let dir = root.join("LICENSES");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("{id}.txt"));
    let url = spdx_text_url(id);
    let status = std::process::Command::new("curl")
        .arg("-fsSL")
        .arg(&url)
        .arg("-o")
        .arg(&dest)
        .status()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&dest);
        return Err(format!("curl failed for {url} ({status})"));
    }
    // Guard against an empty body (e.g. a 404 page suppressed by `-f`) passing as success.
    match std::fs::metadata(&dest) {
        Ok(m) if m.len() > 0 => Ok(dest),
        _ => {
            let _ = std::fs::remove_file(&dest);
            Err(format!("curl produced no text for {id}"))
        }
    }
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
    fn license_ref_is_reported_missing_never_scaffolded() {
        let dir = tempdir().unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("LicenseRef-Marque-1.0".to_string());
        let res = materialize(dir.path(), &refs).unwrap();
        // No placeholder is written; the id is surfaced for the caller to act on.
        assert!(res.written.is_empty());
        assert_eq!(res.still_missing, vec!["LicenseRef-Marque-1.0".to_string()]);
        assert!(
            !dir.path()
                .join("LICENSES/LicenseRef-Marque-1.0.txt")
                .exists()
        );
    }

    #[test]
    fn present_texts_recognizes_md_and_extensionless() {
        let dir = tempdir().unwrap();
        let licenses = dir.path().join("LICENSES");
        std::fs::create_dir_all(&licenses).unwrap();
        std::fs::write(licenses.join("MIT.md"), "x").unwrap();
        std::fs::write(licenses.join("Apache-2.0"), "x").unwrap();
        std::fs::write(licenses.join("LicenseRef-Acme-1.0"), "x").unwrap();
        let present = present_texts(dir.path());
        assert!(present.contains("MIT"), "{present:?}");
        assert!(present.contains("Apache-2.0"), "{present:?}");
        // An extension-less LicenseRef keeps its trailing version segment intact.
        assert!(present.contains("LicenseRef-Acme-1.0"), "{present:?}");
    }

    #[test]
    fn missing_guidance_links_spdx_text_and_names_license_ref_path() {
        let std = missing_text_guidance("MIT");
        assert!(
            std.contains("license-list-data") && std.contains("MIT.txt"),
            "{std}"
        );
        assert!(!std.contains("--allow"), "no network-flag hint: {std}");
        let lref = missing_text_guidance("LicenseRef-Acme");
        assert!(lref.contains("LICENSES/LicenseRef-Acme.txt"), "{lref}");
        assert!(
            !lref.contains("http"),
            "LicenseRef has no upstream URL: {lref}"
        );
    }

    #[test]
    fn inventory_reports_missing() {
        let dir = tempdir().unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("MIT".to_string());
        let inv = LicenseTextInventory::compute(dir.path(), &refs);
        assert!(inv.missing.contains("MIT"));
        assert!(inv.bundled_available.contains("MIT"));
        assert!(!inv.is_complete());
    }
}
