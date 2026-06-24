//! `licet` — declarative, SPDX/REUSE-compatible license-header management.
//!
//! The library exposes the full engine (config → projection → drift → reconcile) so the
//! CLI is a thin shell and the engine is independently testable/embeddable.

pub mod cli;
pub mod comment;
pub mod config;
pub mod detect;
pub mod domain;
pub mod engine;
pub mod error;
pub mod reconcile;
pub mod report;
pub mod reuse;
pub mod rules;
pub mod spdx;
pub mod walk;

pub use error::{ExitCode, LicetError, Result};
