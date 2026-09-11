//! Deterministic single-file write execution (FR-021).
//!
//! [`execute_writes`] carries out [`PlannedWrite`]s in deterministic
//! destination order through the shared safe writer, recording every attempt
//! and failure. Metadata-document batches (`REUSE.toml`) stay with the caller,
//! which groups them per document; everything here is one destination, one
//! atomic write. The same [`PlannedWrite`] records drive dry-run previews
//! (reported `Planned`, never executed) and real runs.

use std::path::Path;

use crate::domain::{ExecutedWrite, PlannedWrite, WriteStatus};

/// Execute planned single-file writes in deterministic destination order
/// (byte-wise path sort; stable within one path).
///
/// Every write is attempted independently: one destination's failure is
/// recorded on that write and never blocks the others. A committed
/// replacement that fails only its durability sync reports `Failed` with
/// `replacement_completed` set, so callers still count it as a change.
pub fn execute_writes(root: &Path, writes: &[PlannedWrite]) -> Vec<ExecutedWrite> {
    let mut order: Vec<usize> = (0..writes.len()).collect();
    order.sort_by(|&a, &b| writes[a].path.cmp(&writes[b].path));
    order
        .into_iter()
        .map(|i| execute_one(root, &writes[i]))
        .collect()
}

fn execute_one(root: &Path, write: &PlannedWrite) -> ExecutedWrite {
    // Create exactly the destination's own ancestor chain (e.g. `LICENSES/`);
    // nothing beyond what the write requires.
    if let Some(parent) = root.join(&write.path).parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return ExecutedWrite {
            write: write.clone(),
            status: WriteStatus::Failed,
            message: Some(format!(
                "cannot create parent directory {}: {e}",
                parent.display()
            )),
            replacement_completed: false,
        };
    }
    let after = match &write.after {
        Some(bytes) => bytes,
        None => {
            return ExecutedWrite::blocked(write.clone(), "no bytes were planned for this write");
        }
    };
    match crate::reuse::atomic_write(root, &write.path, write.before.as_deref(), after) {
        Ok(()) => ExecutedWrite {
            write: write.clone(),
            status: WriteStatus::Applied,
            message: None,
            replacement_completed: true,
        },
        Err(e) => ExecutedWrite {
            write: write.clone(),
            status: WriteStatus::Failed,
            message: Some(e.to_string()),
            replacement_completed: e.replacement_completed,
        },
    }
}

// REUSE-IgnoreStart — SPDX tags in the tests below are fixtures, not this file's licensing.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::WriteKind;

    fn write(rel: &str, before: Option<&[u8]>, after: &[u8]) -> PlannedWrite {
        PlannedWrite {
            path: std::path::PathBuf::from(rel),
            kind: WriteKind::Source,
            before: before.map(|b| b.to_vec()),
            after: Some(after.to_vec()),
            affected_files: vec![std::path::PathBuf::from(rel)],
        }
    }

    #[test]
    fn executes_in_destination_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let outcomes = execute_writes(
            root,
            &[
                write("b.txt", None, b"new-b"),
                write("a.txt", None, b"new-a"),
            ],
        );
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| o.status == WriteStatus::Applied));
        // Deterministic destination order regardless of plan order.
        let paths: Vec<_> = outcomes.iter().map(|o| o.write.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                std::path::PathBuf::from("a.txt"),
                std::path::PathBuf::from("b.txt")
            ]
        );
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"new-a");
        assert_eq!(std::fs::read(root.join("b.txt")).unwrap(), b"new-b");
    }

    #[test]
    fn expected_guard_mismatch_fails_without_touching_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), b"actual").unwrap();
        let outcomes = execute_writes(root, &[write("a.txt", Some(b"stale"), b"new")]);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, WriteStatus::Failed);
        assert!(!outcomes[0].replacement_completed);
        assert!(
            !outcomes[0]
                .message
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        );
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"actual");
    }

    #[test]
    fn destination_turned_directory_between_plan_and_execution_fails() {
        // Deterministic partial-failure repro without permission tricks: the
        // plan is computed against a file that becomes a directory before the
        // executor runs. No production test flags are involved.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("ok.txt"), b"old-ok").unwrap();
        std::fs::write(root.join("gone.txt"), b"old-gone").unwrap();
        let planned = vec![
            write("ok.txt", Some(b"old-ok"), b"new-ok"),
            write("gone.txt", Some(b"old-gone"), b"new-gone"),
        ];
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::create_dir(root.join("gone.txt")).unwrap();
        let outcomes = execute_writes(root, &planned);
        assert_eq!(outcomes.len(), 2);
        let ok = outcomes
            .iter()
            .find(|o| o.write.path == std::path::Path::new("ok.txt"))
            .unwrap();
        let gone = outcomes
            .iter()
            .find(|o| o.write.path == std::path::Path::new("gone.txt"))
            .unwrap();
        assert_eq!(ok.status, WriteStatus::Applied);
        assert_eq!(std::fs::read(root.join("ok.txt")).unwrap(), b"new-ok");
        assert_eq!(gone.status, WriteStatus::Failed);
        assert!(!gone.replacement_completed);
        assert!(root.join("gone.txt").is_dir(), "failed write left alone");
    }
}
// REUSE-IgnoreEnd
