// SPDX-License-Identifier: Apache-2.0
//! The AnyDash CLI and shared collector. Modules are exported for integration tests, not as a
//! public library API.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "These executable internals are exported only for integration tests, not as a public library API."
)]
pub mod error;
pub mod keys;
pub mod parse;
pub mod pb;
pub mod scan;
pub mod settings;
pub mod store;
pub mod time;
