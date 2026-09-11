//! REUSE-compatibility surface: out-of-band metadata, license-text inventory, and atomic
//! file writes (FR-014..FR-017, FR-024).

pub mod atomic;
pub mod inventory;
pub mod oob;

pub use atomic::{WriteError, atomic_write, read_expected_for_write};
