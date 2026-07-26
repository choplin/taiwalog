//! The durable local shell for Agentlog's first MVP slice.
//!
//! This package owns only Agentlog's configuration and `SQLite` catalog. Provider
//! collectors are added in later commits; no provider-owned path is written by
//! this crate.

pub mod app;
pub mod paths;
pub mod storage;
