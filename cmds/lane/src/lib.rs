// SPDX-License-Identifier: Apache-2.0
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "These executable internals are exported only for integration tests, not as a public library API."
)]
#![allow(
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    reason = "Keep transactional dispatch together; JSON constructors borrow their logically owned inputs."
)]
pub mod error;
pub mod protocol;

pub mod auth;
pub mod cli;
pub mod locallog;
pub mod mcp;
pub mod remote;
pub mod runtime;
pub mod store;
pub mod syncclient;
#[cfg(windows)]
mod windows;

mod watch;

pub mod input;

pub mod network;
