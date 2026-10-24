// SPDX-License-Identifier: FSL-1.1-ALv2
//! Per-tenant request rate and concurrency admission.
//!
//! Both limits are in-memory because exactly one instance serves requests (the PostgreSQL
//! instance lock enforces it). A [`Permit`] holds a concurrency slot until it is dropped, which
//! happens when the execution task ends, including after the caller canceled.

use crate::{config::Limits, error::ApiError};
use server_common::limit::Limiter;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct TenantGate {
    rate: Limiter,
    concurrency: usize,
    active: Arc<Mutex<HashMap<String, usize>>>,
}

/// An execution slot of one tenant, released on drop.
pub struct Permit {
    tenant: String,
    active: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = active.get_mut(&self.tenant) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.tenant);
            }
        }
    }
}

impl TenantGate {
    pub fn new(limits: &Limits) -> Self {
        Self {
            rate: Limiter::new(limits.tenant_rate, limits.tenant_burst),
            concurrency: limits.tenant_concurrency,
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Consumes one rate token and one concurrency slot of `tenant`, or rejects the call.
    pub fn admit(&self, tenant: &str) -> Result<Permit, ApiError> {
        if let Err(wait) = self.rate.allow(tenant) {
            return Err(ApiError::rate_limited(
                Some(wait),
                "the tenant request rate limit was exceeded",
            ));
        }
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        let count = active.entry(tenant.to_owned()).or_insert(0);
        if *count >= self.concurrency {
            return Err(ApiError::rate_limited(
                None,
                "the tenant concurrency limit was exceeded",
            ));
        }
        *count += 1;
        Ok(Permit {
            tenant: tenant.to_owned(),
            active: self.active.clone(),
        })
    }

    /// Slots currently held by a tenant. Exposed for tests.
    pub fn in_flight(&self, tenant: &str) -> usize {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use delino_protocol::delitools::v1::ErrorReason;

    fn limits(rate: f64, burst: u32, concurrency: usize) -> Limits {
        Limits {
            max_input_bytes: 1,
            max_translation_items: 1,
            max_schema_bytes: 1,
            execution_timeout: std::time::Duration::from_secs(1),
            tenant_rate: rate,
            tenant_burst: burst,
            tenant_concurrency: concurrency,
        }
    }

    #[test]
    fn concurrency_is_per_tenant_and_released_on_drop() {
        let gate = TenantGate::new(&limits(1000.0, 1000, 2));
        let a = gate.admit("a").unwrap();
        let _b = gate.admit("a").unwrap();
        let error = gate.admit("a").err().unwrap();
        assert_eq!(error.reason, ErrorReason::ERROR_REASON_RATE_LIMITED);
        assert!(gate.admit("other").is_ok());
        assert_eq!(gate.in_flight("a"), 2);
        drop(a);
        assert_eq!(gate.in_flight("a"), 1);
        assert!(gate.admit("a").is_ok());
    }

    #[test]
    fn rate_limit_rejects_with_a_retry_hint() {
        let gate = TenantGate::new(&limits(0.5, 2, 100));
        let _p1 = gate.admit("t").unwrap();
        let _p2 = gate.admit("t").unwrap();
        let error = gate.admit("t").err().unwrap();
        assert_eq!(error.reason, ErrorReason::ERROR_REASON_RATE_LIMITED);
        assert!(error.retry_after.is_some());
        assert!(gate.admit("isolated").is_ok());
    }

    #[test]
    fn smallest_supported_rate_rejects_with_a_positive_retry_hint_and_keeps_other_tenants() {
        let smallest_supported = f64::from_bits(2f64.powi(-64).to_bits() + 1);
        let gate = TenantGate::new(&limits(smallest_supported, 1, 100));
        let _permit = gate.admit("t").unwrap();
        for _ in 0..2 {
            let error = gate.admit("t").err().unwrap();
            assert_eq!(error.reason, ErrorReason::ERROR_REASON_RATE_LIMITED);
            assert!(error.retry_after.is_some_and(|wait| !wait.is_zero()));
        }
        assert!(gate.admit("other").is_ok());
    }

    #[test]
    fn released_slots_leave_no_residue() {
        let gate = TenantGate::new(&limits(1000.0, 1000, 1));
        drop(gate.admit("t").unwrap());
        assert_eq!(gate.in_flight("t"), 0);
        assert!(gate.active.lock().unwrap().is_empty());
    }
}
