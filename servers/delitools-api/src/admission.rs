// SPDX-License-Identifier: FSL-1.1-ALv2
//! Per-tenant admission control for new paid executions: a fixed-minute request count and a
//! running-call concurrency bound. Limits come from the live operator settings on every call.
//!
//! State is in memory by design: the service runs as one instance, and a restart only resets
//! counters (it never loses billing data). The execution layer calls [`Admission::admit`] after
//! authorization and before creating a request record.
use crate::{
    error::{Fault, Result},
    settings::Settings,
    telemetry::{LimitKind, Metrics},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Default)]
struct State {
    /// tenant -> (minute index, requests admitted in that minute)
    windows: HashMap<Uuid, (i64, u32)>,
    /// tenant -> running calls
    running: HashMap<Uuid, u32>,
}

#[derive(Clone, Default)]
pub struct Admission {
    state: Arc<Mutex<State>>,
}

/// Holds one concurrency slot. Dropping it, including on cancellation or panic unwinding, frees it.
pub struct Permit {
    state: Arc<Mutex<State>>,
    tenant: Uuid,
    /// The minute window whose request count this permit incremented.
    minute: i64,
}
impl Permit {
    /// Give back the request-rate budget this permit consumed, for a call that did no work (a
    /// duplicate that lost the atomic idempotency claim). Only the window the permit counted in
    /// is adjusted; the concurrency slot is released when the permit is dropped.
    pub fn refund_rate(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((window, count)) = state.windows.get_mut(&self.tenant)
            && *window == self.minute
        {
            *count = count.saturating_sub(1);
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = state.running.get_mut(&self.tenant) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.running.remove(&self.tenant);
            }
        }
    }
}

impl Admission {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a call at `unix_seconds`. The minute window is `unix_seconds / 60`; expired windows
    /// of other tenants are dropped so memory stays bounded by active tenants.
    pub fn admit_at(
        &self,
        tenant: Uuid,
        settings: &Settings,
        unix_seconds: i64,
        metrics: &Metrics,
    ) -> Result<Permit> {
        let minute = unix_seconds.div_euclid(60);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.windows.retain(|_, (window, _)| *window >= minute);
        if state.running.get(&tenant).copied().unwrap_or(0) >= settings.tenant_concurrency {
            metrics.limit(LimitKind::TenantConcurrency);
            return Err(Fault::limit("tenant concurrency limit reached"));
        }
        let window = state.windows.entry(tenant).or_insert((minute, 0));
        if window.0 != minute {
            *window = (minute, 0);
        }
        if window.1 >= settings.tenant_requests_per_minute {
            metrics.limit(LimitKind::TenantRate);
            return Err(Fault::limit("tenant request rate limit reached"));
        }
        window.1 += 1;
        *state.running.entry(tenant).or_insert(0) += 1;
        Ok(Permit {
            state: self.state.clone(),
            tenant,
            minute,
        })
    }

    pub fn admit(&self, tenant: Uuid, settings: &Settings, metrics: &Metrics) -> Result<Permit> {
        self.admit_at(tenant, settings, chrono::Utc::now().timestamp(), metrics)
    }

    pub fn running(&self, tenant: Uuid) -> u32 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.running.get(&tenant).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::fixture;

    fn limited(rate: u32, concurrency: u32) -> Settings {
        Settings {
            tenant_requests_per_minute: rate,
            tenant_concurrency: concurrency,
            ..fixture()
        }
    }

    #[test]
    fn fixed_minute_window_counts_and_resets() {
        let admission = Admission::new();
        let metrics = Metrics::new();
        let settings = limited(2, 10);
        let tenant = Uuid::now_v7();
        let t = 6000;
        drop(admission.admit_at(tenant, &settings, t, &metrics).unwrap());
        drop(
            admission
                .admit_at(tenant, &settings, t + 59, &metrics)
                .unwrap(),
        );
        let error = admission
            .admit_at(tenant, &settings, t + 59, &metrics)
            .err()
            .unwrap();
        assert_eq!(
            error.reason,
            delino_protocol::delitools::v1::ErrorReason::ERROR_REASON_LIMIT
        );
        drop(
            admission
                .admit_at(tenant, &settings, t + 60, &metrics)
                .unwrap(),
        );
        assert!(metrics.encode().contains("tenant_rate"));
    }

    #[test]
    fn a_refunded_permit_gives_back_its_rate_budget_only_in_its_own_window() {
        let admission = Admission::new();
        let metrics = Metrics::new();
        let settings = limited(1, 10);
        let tenant = Uuid::now_v7();
        let t = 6000;
        let duplicate = admission.admit_at(tenant, &settings, t, &metrics).unwrap();
        assert!(admission.admit_at(tenant, &settings, t, &metrics).is_err());
        duplicate.refund_rate();
        drop(duplicate);
        // The refund restored the budget, so the next distinct call is admitted.
        let next = admission.admit_at(tenant, &settings, t, &metrics).unwrap();
        // A refund after the minute rolled over must not reduce the new window's count.
        let stale = admission
            .admit_at(tenant, &limited(5, 10), t + 60, &metrics)
            .unwrap();
        next.refund_rate();
        let tight = limited(1, 10);
        assert!(
            admission
                .admit_at(tenant, &tight, t + 60, &metrics)
                .is_err(),
            "the new window still counts the call admitted in it"
        );
        drop((next, stale));
    }

    #[test]
    fn concurrency_is_bounded_and_released_on_drop() {
        let admission = Admission::new();
        let metrics = Metrics::new();
        let settings = limited(100, 2);
        let tenant = Uuid::now_v7();
        let a = admission.admit_at(tenant, &settings, 0, &metrics).unwrap();
        let b = admission.admit_at(tenant, &settings, 0, &metrics).unwrap();
        assert_eq!(admission.running(tenant), 2);
        assert!(admission.admit_at(tenant, &settings, 0, &metrics).is_err());
        drop(a);
        let c = admission.admit_at(tenant, &settings, 0, &metrics).unwrap();
        drop((b, c));
        assert_eq!(admission.running(tenant), 0);
        assert!(admission.state.lock().unwrap().running.is_empty());
    }

    #[test]
    fn tenants_are_isolated_and_live_settings_apply() {
        let admission = Admission::new();
        let metrics = Metrics::new();
        let (one, two) = (Uuid::now_v7(), Uuid::now_v7());
        let tight = limited(1, 1);
        let _held = admission.admit_at(one, &tight, 0, &metrics).unwrap();
        assert!(admission.admit_at(one, &tight, 0, &metrics).is_err());
        let _other = admission.admit_at(two, &tight, 0, &metrics).unwrap();
        let loose = limited(100, 100);
        let _raised = admission.admit_at(one, &loose, 0, &metrics).unwrap();
        let _more = admission.admit_at(one, &loose, 61, &metrics).unwrap();
    }
}
