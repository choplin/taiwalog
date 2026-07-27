//! The durable local shell for Agentlog's first MVP slice.
//!
//! This package owns only Agentlog's configuration and `SQLite` catalog. Provider
//! collectors are added in later commits; no provider-owned path is written by
//! this crate.

pub mod app;
pub mod cli;
pub mod paths;
pub mod providers;
pub mod storage;

// Preserve the initial public module paths while implementation and internal
// dependencies use the explicit provider boundary.
pub use providers::{claude, codex, cursor, gemini, kimi, opencode};
