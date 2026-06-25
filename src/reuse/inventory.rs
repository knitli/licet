//! License-text inventory and offline materialization into `LICENSES/` (FR-014, FR-017).

use std::collections::BTreeSet;
use std::path::Path;

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

/// Identifiers whose text files exist under `LICENSES/`.
fn present_texts(root: &Path) -> BTreeSet<String> {
    let mut present = BTreeSet::new();
    let dir = root.join("LICENSES");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().map(|x| x == "txt").unwrap_or(false)
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                present.insert(stem.to_string());
            }
        }
    }
    present
}

/// Materialize referenced-but-missing texts into `LICENSES/` from the offline bundle;
/// scaffold `LicenseRef-*` placeholders (FR-017). Returns ids written and ids still missing.
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
        let dest = dir.join(format!("{id}.txt"));
        if let Some(text) = spdx::bundled_text(id) {
            std::fs::write(&dest, text)?;
            written.push(id.clone());
        } else if spdx::is_license_ref(id) {
            // Scaffold a placeholder for the maintainer to fill.
            let placeholder = format!(
                "{id}\n\nTODO: provide the full text of the custom license `{id}`.\n\
                 This placeholder satisfies REUSE's text-existence check but is flagged by\n\
                 `licet lint` until filled.\n"
            );
            std::fs::write(&dest, placeholder)?;
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
    fn scaffolds_license_ref_placeholder() {
        let dir = tempdir().unwrap();
        let mut refs = BTreeSet::new();
        refs.insert("LicenseRef-Marque-1.0".to_string());
        let res = materialize(dir.path(), &refs).unwrap();
        assert_eq!(res.written.len(), 1);
        let text =
            std::fs::read_to_string(dir.path().join("LICENSES/LicenseRef-Marque-1.0.txt")).unwrap();
        assert!(text.contains("TODO"));
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
