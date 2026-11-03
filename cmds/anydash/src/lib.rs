// SPDX-License-Identifier: Apache-2.0
//! The AnyDash CLI and shared collector. Modules are exported for integration tests, not as a
//! public library API.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "These executable internals are exported only for integration tests, not as a public library API."
)]
pub mod api;
pub mod cli;
pub mod collector;
pub mod connect;
pub mod core;
pub mod error;
pub mod ipc;
pub mod keys;
pub mod logging;
pub mod parse;
pub mod pb;
pub mod providers;
pub mod scan;
pub mod schedule;
pub mod service;
pub mod settings;
pub mod store;
pub mod time;
#[cfg(windows)]
mod windows;
