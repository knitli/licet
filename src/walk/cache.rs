//! Warm-scan classification cache (SC-006, FR-023, SC-011).
//!
//! The cache key folds in file content hash **+** an effective-config fingerprint **+**
//! the tool version, so a hit is observationally identical to a cold run and can never
//! produce a stale `Compliant` (SC-011).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Lowercase-hex encode a digest's bytes, independent of the array type `finalize`
/// returns (newer `sha2` yields a `hybrid-array` `Array` that does not impl `LowerHex`).
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// On-disk cache mapping `path → (content+config+version key, drift label)`.
#[derive(Default)]
pub struct ScanCache {
    entries: HashMap<String, CacheEntry>,
    config_fingerprint: String,
    dirty: bool,
    path: Option<PathBuf>,
}

struct CacheEntry {
    key: String,
    drift: String,
}

impl ScanCache {
    /// Tool version embedded at build time (folds into the cache key).
    fn tool_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Open (or create) a cache at `path` for the given config fingerprint.
    pub fn open(path: &Path, config_fingerprint: &str) -> Self {
        let mut cache = ScanCache {
            entries: HashMap::new(),
            config_fingerprint: config_fingerprint.to_string(),
            dirty: false,
            path: Some(path.to_path_buf()),
        };
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                // format: <path>\t<key>\t<drift>
                let mut parts = line.splitn(3, '\t');
                if let (Some(p), Some(k), Some(d)) = (parts.next(), parts.next(), parts.next()) {
                    cache.entries.insert(
                        p.to_string(),
                        CacheEntry {
                            key: k.to_string(),
                            drift: d.to_string(),
                        },
                    );
                }
            }
        }
        cache
    }

    /// A disabled (`--no-cache`) cache that never hits and never persists.
    pub fn disabled() -> Self {
        ScanCache::default()
    }

    /// Compute the content-hash component for a file's bytes.
    pub fn content_hash(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        hex(h.finalize().as_ref())
    }

    /// The full cache key folding content + config fingerprint + tool version.
    fn full_key(&self, content_hash: &str) -> String {
        let mut h = Sha256::new();
        h.update(content_hash.as_bytes());
        h.update(b"\0");
        h.update(self.config_fingerprint.as_bytes());
        h.update(b"\0");
        h.update(Self::tool_version().as_bytes());
        hex(h.finalize().as_ref())
    }

    /// Look up a cached drift label for a path+content, honoring the full key.
    pub fn get(&self, rel_path: &str, content_hash: &str) -> Option<String> {
        self.path.as_ref()?;
        let key = self.full_key(content_hash);
        self.entries
            .get(rel_path)
            .filter(|e| e.key == key)
            .map(|e| e.drift.clone())
    }

    /// Record a classification result.
    pub fn put(&mut self, rel_path: &str, content_hash: &str, drift: &str) {
        if self.path.is_none() {
            return;
        }
        let key = self.full_key(content_hash);
        self.entries.insert(
            rel_path.to_string(),
            CacheEntry {
                key,
                drift: drift.to_string(),
            },
        );
        self.dirty = true;
    }

    /// Persist the cache to disk if modified.
    pub fn flush(&self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(path) = &self.path {
            let mut out = String::new();
            for (p, e) in &self.entries {
                out.push_str(&format!("{p}\t{}\t{}\n", e.key, e.drift));
            }
            std::fs::write(path, out)?;
        }
        Ok(())
    }
}

/// Compute an effective-config fingerprint for the cache key (FR-023).
pub fn config_fingerprint(config_text: &str) -> String {
    let mut h = Sha256::new();
    h.update(config_text.as_bytes());
    hex(h.finalize().as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hit_only_with_matching_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cache");
        let mut c = ScanCache::open(&path, "fp1");
        let ch = ScanCache::content_hash(b"hello");
        c.put("a.rs", &ch, "compliant");
        assert_eq!(c.get("a.rs", &ch).as_deref(), Some("compliant"));
        // Different content → miss.
        assert!(c.get("a.rs", &ScanCache::content_hash(b"world")).is_none());
    }

    #[test]
    fn config_change_invalidates() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cache");
        let ch = ScanCache::content_hash(b"hello");
        {
            let mut c = ScanCache::open(&path, "fp1");
            c.put("a.rs", &ch, "compliant");
            c.flush().unwrap();
        }
        // Reopen with a different fingerprint → stale entry must not hit.
        let c2 = ScanCache::open(&path, "fp2");
        assert!(c2.get("a.rs", &ch).is_none());
    }

    #[test]
    fn disabled_never_hits() {
        let mut c = ScanCache::disabled();
        let ch = ScanCache::content_hash(b"x");
        c.put("a", &ch, "compliant");
        assert!(c.get("a", &ch).is_none());
    }
}
