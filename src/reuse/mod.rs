//! REUSE-compatibility surface: out-of-band metadata, license-text inventory, and atomic
//! file writes (FR-014..FR-017, FR-024).

pub mod inventory;
pub mod oob;

use std::path::Path;

/// Write `content` to `path` atomically: write a sibling temp file, fsync, then rename
/// over the original so an interruption never leaves a half-written file (FR-024, SC-010).
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "tmp".to_string());
    let tmp = dir.join(format!(".{file_name}.licet.tmp"));

    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_replaces_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "old").unwrap();
        atomic_write(&p, "new content").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new content");
        // No temp file left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("licet.tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
