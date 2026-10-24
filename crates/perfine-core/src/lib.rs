// SPDX-License-Identifier: Apache-2.0
//! Shared v1 contracts. Adapters supply observations; this crate validates them.
//! Concrete adapters execute isolated candidate code; publication remains caller-owned.
#![forbid(unsafe_code)]
pub mod adapters;
pub mod evidence;
pub mod interfaces;
pub mod lifecycle;
pub mod policy;
pub mod publication;
pub mod storage;
pub mod types;

pub use types::{Error, Result};
