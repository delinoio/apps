// SPDX-License-Identifier: FSL-1.1-ALv2
/// Largest duration Prometheus accepts, in nanoseconds. `model.ParseDuration` stores durations as a
/// signed 64-bit nanosecond count, so larger syntactically valid values fail in rule files and
/// PromQL range selectors.
const MAX_DURATION_NANOS: u128 = i64::MAX as u128;

/// Checks that `value` (already matching `^[1-9][0-9]*[smhd]$`) is representable as a Prometheus
/// duration. Digit strings that overflow `u128` are rejected rather than wrapped or panicking.
fn representable(value: &str) -> bool {
    let (digits, unit) = value.split_at(value.len() - 1);
    let nanos_per_unit: u128 = match unit {
        "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        "d" => 86_400_000_000_000,
        _ => return false,
    };
    digits
        .parse::<u128>()
        .ok()
        .and_then(|count| count.checked_mul(nanos_per_unit))
        .is_some_and(|nanos| nanos <= MAX_DURATION_NANOS)
}

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
        anyhow::ensure!(
            representable(&window) && representable(&hold),
            "observation window and hold duration for {key} must not exceed 9223372036854775807 nanoseconds"
        );
        out.push_str(&format!("      - alert: {name}\n        expr: '{} > {threshold}'\n        for: {hold}\n        labels:\n          service: git-tracker\n        annotations:\n          summary: {name} exceeded the configured threshold\n",metric.replace("WINDOW",&window)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::rules;

    const KEYS: [&str; 4] = ["API_ERRORS", "PERSISTENCE", "GITHUB", "INTERRUPTED"];

    fn generate(key: &str, field: &str, value: &str) -> anyhow::Result<String> {
        rules(|name| {
            if name == format!("ALERT_{key}_{field}") {
                value.to_owned()
            } else if name.ends_with("_THRESHOLD") {
                "1".to_owned()
            } else if name.ends_with("_WINDOW") {
                "5m".to_owned()
            } else {
                "1m".to_owned()
            }
        })
    }

    #[test]
    fn accepts_maximum_representable_durations_for_every_key_and_field() {
        for key in KEYS {
            for field in ["WINDOW", "FOR"] {
                for value in [
                    "9223372036s",
                    "153722867m",
                    "2562047h",
                    "106751d",
                    "5m",
                    "1s",
                ] {
                    let document = generate(key, field, value)
                        .unwrap_or_else(|error| panic!("{key} {field} {value}: {error}"));
                    assert!(document.contains(value));
                }
            }
        }
    }

    #[test]
    fn rejects_durations_beyond_signed_nanoseconds_for_every_key_and_field() {
        let overlong = format!("{}s", "9".repeat(400));
        for key in KEYS {
            for field in ["WINDOW", "FOR"] {
                for value in [
                    "9223372037s",
                    "153722868m",
                    "2562048h",
                    "106752d",
                    "10000000000s",
                    "18446744073709551616s",
                    "340282366920938463463374607431768211456s",
                    overlong.as_str(),
                ] {
                    let error = generate(key, field, value).expect_err(value).to_string();
                    assert!(error.contains(key) && error.contains("9223372036854775807"));
                }
            }
        }
    }

    #[test]
    fn keeps_existing_syntax_rejections() {
        for value in ["0s", "-1s", "5", "5w", "1h30m", "1.5h", "", "05m"] {
            assert!(generate("API_ERRORS", "WINDOW", value).is_err(), "{value}");
            assert!(generate("API_ERRORS", "FOR", value).is_err(), "{value}");
        }
    }
}
