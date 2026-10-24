// SPDX-License-Identifier: FSL-1.1-ALv2
//! Server-owned v1 evidence validation and policy evaluation.
//! Preserve the versioned JSON field order and canonical hashes when evolving this module.
//! Local execution, adapters, and filesystem storage are not part of the server boundary.
pub mod evidence;
pub mod policy;
pub mod types;
use serde::{Deserialize, Serialize};
use types::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub candidate_count: u32,
    pub simultaneous_candidates: u8,
    pub start_duration_seconds: u64,
    pub candidate_duration_seconds: u64,
}
impl Limits {
    pub fn validate(&self) -> Result<()> {
        if self.candidate_count == 0
            || !(1..=5).contains(&self.simultaneous_candidates)
            || self.start_duration_seconds == 0
            || self.candidate_duration_seconds == 0
        {
            return Err(Error::Invalid("mandatory execution limits"));
        }
        Ok(())
    }
}
