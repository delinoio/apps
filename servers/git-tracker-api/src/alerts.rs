// SPDX-License-Identifier: FSL-1.1-ALv2
pub fn rules(get: impl Fn(&str) -> String) -> anyhow::Result<String> {
    let mut out = "groups:\n  - name: git-tracker\n    rules:\n".to_owned();
    let number = regex::Regex::new(r"^[0-9]+(\.[0-9]+)?$")?;
    let duration = regex::Regex::new(r"^[1-9][0-9]*[smhd]$")?;
    for (name, key, metric) in [
        (
            "GitTrackerAPIErrors",
            "API_ERRORS",
            r#"sum(rate(git_tracker_http_requests_total{outcome="server_error"}[WINDOW])) / clamp_min(sum(rate(git_tracker_http_requests_total[WINDOW])), 0.000001)"#,
        ),
        (
            "GitTrackerPersistenceFailures",
            "PERSISTENCE",
            r#"sum(increase(git_tracker_events_total{kind="persistence",category="failure"}[WINDOW]))"#,
        ),
        (
            "GitTrackerGitHubFailures",
            "GITHUB",
            r#"sum(increase(git_tracker_events_total{kind="github",category="failure"}[WINDOW]))"#,
        ),
        (
            "GitTrackerInterruptedExecutions",
            "INTERRUPTED",
            r#"sum(increase(git_tracker_events_total{kind="execution",category="interrupted"}[WINDOW]))"#,
        ),
    ] {
        let threshold = get(&format!("ALERT_{key}_THRESHOLD"));
        let window = get(&format!("ALERT_{key}_WINDOW"));
        let hold = get(&format!("ALERT_{key}_FOR"));
        anyhow::ensure!(
            number.is_match(&threshold)
                && threshold.parse::<f64>().is_ok_and(f64::is_finite)
                && duration.is_match(&window)
                && duration.is_match(&hold),
            "valid threshold, observation window and hold duration required for {key}"
        );
        out.push_str(&format!("      - alert: {name}\n        expr: '{} > {threshold}'\n        for: {hold}\n        labels:\n          service: git-tracker\n        annotations:\n          summary: {name} exceeded the configured threshold\n",metric.replace("WINDOW",&window)));
    }
    Ok(out)
}
