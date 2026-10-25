// SPDX-License-Identifier: FSL-1.1-ALv2
/// Generate deployable Prometheus rules from required operator choices.
pub fn rules(read: impl Fn(&str) -> String) -> anyhow::Result<String> {
    fn positive(read: &impl Fn(&str) -> String, key: &str) -> anyhow::Result<u64> {
        let n = read(key).parse::<u64>()?;
        anyhow::ensure!(n > 0, "{key} must be positive");
        Ok(n)
    }
    let window = positive(&read, "ALERT_WINDOW_SECONDS")?;
    let failures = positive(&read, "ALERT_FAILURE_THRESHOLD")?;
    let age = positive(&read, "ALERT_CLEANUP_AGE_SECONDS")?;
    Ok(format!(
        "groups:\n  - name: perfine\n    rules:\n      - alert: PerfineRequestFailures\n        expr: increase(perfine_requests_failed_total[{window}s]) >= {failures}\n        for: {window}s\n        labels:\n          severity: warning\n        annotations:\n          summary: Inspect Perfine dependency and authority failures\n      - alert: PerfineCleanupBacklog\n        expr: perfine_cleanup_oldest_seconds >= {age}\n        for: {window}s\n        labels:\n          severity: critical\n        annotations:\n          summary: Restore R2 access; preserve tombstones and retry jobs\n      - alert: PerfinePublicationUncertain\n        expr: perfine_publication_reconciling > 0\n        for: {window}s\n        labels:\n          severity: warning\n        annotations:\n          summary: Reconcile approved draft content against GitHub before recovery\n"
    ))
}
#[cfg(test)]
mod tests {
    #[test]
    fn thresholds_are_operator_selected() {
        let out = super::rules(|_| "60".into()).unwrap();
        assert!(out.contains("[60s]"));
        assert!(super::rules(|_| "0".into()).is_err());
        assert!(super::rules(|_| "1; invalid".into()).is_err());
    }
}
