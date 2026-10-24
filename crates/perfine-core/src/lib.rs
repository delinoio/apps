// SPDX-License-Identifier: Apache-2.0
//! Shared v1 contracts. Adapters supply observations; this crate validates them.
//! No operation in this crate executes candidate code or publishes a pull request.
#![forbid(unsafe_code)]
pub mod evidence;
pub mod interfaces;
pub mod lifecycle;
pub mod policy;
pub mod publication;
pub mod storage;
pub mod types;

pub use types::{Error, Result};
