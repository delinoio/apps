// SPDX-License-Identifier: FSL-1.1-ALv2
use anyhow::{Result, bail};
use std::{env, path::Path, time::Duration};
pub fn required(key: &str) -> Result<String> {
    let v = env::var(key).unwrap_or_default();
    if v.is_empty() {
        bail!("{key} is required")
    }
    Ok(v)
}
pub fn positive(key: &str, default: i64) -> Result<i64> {
    let n = env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(default);
    if n <= 0 {
        bail!("{key} must be positive")
    }
    Ok(n)
}
pub fn duration(key: &str, default: Duration) -> Result<Duration> {
    let Some(s) = env::var(key).ok().filter(|v| !v.is_empty()) else {
        return Ok(default);
    };
    parse_duration(key, &s)
}
// Pure parser behind `duration()`; it reads no process environment so tests stay isolated.
fn parse_duration(key: &str, s: &str) -> Result<Duration> {
    let mut remaining = s;
    let mut seconds = 0.0;
    while !remaining.is_empty() {
        let n = remaining
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .ok_or_else(|| anyhow::anyhow!("{key} must be a duration"))?;
        let value: f64 = remaining[..n].parse()?;
        remaining = &remaining[n..];
        let (unit, scale) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(u, _)| remaining.starts_with(u))
        .ok_or_else(|| anyhow::anyhow!("{key} must be a duration"))?;
        remaining = &remaining[unit.len()..];
        seconds += value * scale;
    }
    if !seconds.is_finite() || seconds <= 0.0 {
        bail!("{key} must be positive")
    }
    let parsed = Duration::try_from_secs_f64(seconds)?;
    // Positive fractional-nanosecond inputs (below 0.5ns) round to Duration::ZERO. Consumers such
    // as tokio::time::interval reject a zero period, so reject the setting at startup instead.
    if parsed.is_zero() {
        bail!("{key} must be at least 1ns after conversion to nanosecond precision")
    }
    Ok(parsed)
}
pub fn safe_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none()
            && (u.scheme() == "https"
                || (u.scheme() == "http"
                    && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
    })
}
pub fn valid_origin(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https") && u.origin().ascii_serialization() == value
    })
}

// Development-only dotenv loading retains injected and explicitly empty process values.
// Normal serving and operator commands never read a local environment file.
// The file is opened by exact path in the working directory. `dotenvy::from_filename` searches
// ancestor directories when the file is absent, which would import unrelated configuration into
// unset variables or fail startup on an unrelated malformed file.
pub fn load_dev_env() -> Result<()> {
    load_env_file(Path::new(".env"))
}

// Exact-path loader behind `load_dev_env()`; the path is never searched upward.
fn load_env_file(path: &Path) -> Result<()> {
    match dotenvy::from_path(path) {
        Ok(()) => Ok(()),
        Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("could not load development environment"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_positive_inputs_that_round_to_zero() {
        for input in ["0.1ns", "0.49ns", "0.0000000001s", "0.0001us"] {
            let err = parse_duration("CLEANUP_INTERVAL", input).unwrap_err();
            assert!(
                err.to_string().contains("CLEANUP_INTERVAL"),
                "{input}: {err}"
            );
        }
    }

    #[test]
    fn preserves_valid_durations() {
        let cases = [
            ("1ns", Duration::from_nanos(1)),
            ("0.5ns", Duration::from_nanos(1)),
            ("0.6ns", Duration::from_nanos(1)),
            ("3s", Duration::from_secs(3)),
            ("5m", Duration::from_secs(300)),
            ("1m30s", Duration::from_secs(90)),
            ("2160h", Duration::from_secs(7_776_000)),
            ("9999999999h", Duration::from_secs(35_999_999_996_400)),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_duration("K", input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn unset_or_empty_value_uses_default() {
        let default = Duration::from_secs(300);
        // These keys are private to this test, so mutating them cannot race other tests.
        assert_eq!(
            duration("SERVER_COMMON_TEST_UNSET_DURATION", default).unwrap(),
            default
        );
        // SAFETY: the key is read only by this test.
        unsafe { env::set_var("SERVER_COMMON_TEST_EMPTY_DURATION", "") };
        assert_eq!(
            duration("SERVER_COMMON_TEST_EMPTY_DURATION", default).unwrap(),
            default
        );
    }

    #[test]
    fn env_adapter_rejects_zero_rounding_values() {
        // SAFETY: the key is read only by this test.
        unsafe { env::set_var("SERVER_COMMON_TEST_ZERO_DURATION", "0.1ns") };
        let err = duration("SERVER_COMMON_TEST_ZERO_DURATION", Duration::from_secs(1)).unwrap_err();
        assert!(err.to_string().contains("SERVER_COMMON_TEST_ZERO_DURATION"));
    }

    #[test]
    fn keeps_rejecting_invalid_inputs() {
        for input in [
            "0ns",
            "-1ns",
            "garbage",
            "NaNs",
            "infs",
            "99999999999999999999999999h",
        ] {
            assert!(parse_duration("K", input).is_err(), "{input}");
        }
    }

    #[tokio::test]
    async fn one_nanosecond_period_reaches_first_tick() {
        let period = parse_duration("K", "1ns").unwrap();
        tokio::time::timeout(Duration::from_secs(5), async move {
            tokio::time::interval(period).tick().await;
        })
        .await
        .expect("first tick");
    }

    // Child-process probe for `load_dev_env()`. It only runs when the parent test selects a case,
    // so it is a no-op during an ordinary test run and never mutates the shared test process.
    #[test]
    fn dev_env_child_probe() {
        let Ok(case) = env::var("SERVER_COMMON_TEST_DEV_ENV_CASE") else {
            return;
        };
        let result = load_dev_env();
        println!("CHILD_CASE={case}");
        match result {
            Ok(()) => println!("CHILD_RESULT=ok"),
            Err(e) => println!("CHILD_RESULT=err:{e}"),
        }
        for key in [
            "DEVENV_PROBE",
            "DEVENV_DERIVED",
            "DEVENV_EMPTY",
            "DEVENV_SET",
        ] {
            println!("CHILD_VAR {key}={:?}", env::var(key).ok());
        }
    }

    struct Probe {
        result: String,
        vars: std::collections::HashMap<String, String>,
    }

    // Builds `<root>/parent/service`, writes the optional files and runs the probe with the
    // service directory as its working directory and a minimal process environment.
    fn run_probe(
        name: &str,
        ancestor: Option<&str>,
        local: Option<&str>,
        injected: &[(&str, &str)],
    ) -> Probe {
        let root = env::temp_dir().join(format!(
            "server-common-dev-env-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let service = root.join("parent").join("service");
        std::fs::create_dir_all(&service).unwrap();
        if let Some(content) = ancestor {
            std::fs::write(root.join("parent").join(".env"), content).unwrap();
        }
        if let Some(content) = local {
            std::fs::write(service.join(".env"), content).unwrap();
        }
        let mut cmd = std::process::Command::new(env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "config::tests::dev_env_child_probe",
            "--nocapture",
        ])
        .current_dir(&service)
        .env_clear()
        .env("SERVER_COMMON_TEST_DEV_ENV_CASE", name);
        for (k, v) in injected {
            cmd.env(k, v);
        }
        let output = cmd.output().unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(output.status.success(), "{name}: {output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let mut result = String::new();
        let mut vars = std::collections::HashMap::new();
        for line in stdout.lines() {
            if let Some(r) = line.strip_prefix("CHILD_RESULT=") {
                result = r.to_string();
            } else if let Some(v) = line.strip_prefix("CHILD_VAR ") {
                let (k, val) = v.split_once('=').unwrap();
                vars.insert(k.to_string(), val.to_string());
            }
        }
        assert!(!result.is_empty(), "{name}: probe did not run: {stdout}");
        Probe { result, vars }
    }

    #[test]
    fn absent_local_file_ignores_valid_ancestor_file() {
        let p = run_probe("ancestor-valid", Some("DEVENV_PROBE=ancestor\n"), None, &[]);
        assert_eq!(p.result, "ok");
        assert_eq!(p.vars["DEVENV_PROBE"], "None");
    }

    #[test]
    fn absent_local_file_ignores_malformed_ancestor_file() {
        let p = run_probe(
            "ancestor-malformed",
            Some("DEVENV_PROBE=\"unterminated\n"),
            None,
            &[],
        );
        assert_eq!(p.result, "ok");
        assert_eq!(p.vars["DEVENV_PROBE"], "None");
    }

    #[test]
    fn absent_local_and_ancestor_files_are_optional() {
        let p = run_probe("none", None, None, &[]);
        assert_eq!(p.result, "ok");
        assert_eq!(p.vars["DEVENV_PROBE"], "None");
    }

    #[test]
    fn local_file_supplies_unset_values_with_expansion_and_wins_over_ancestor() {
        let p = run_probe(
            "local-valid",
            Some("DEVENV_PROBE=ancestor\nDEVENV_SET=ancestor\n"),
            Some("DEVENV_PROBE=local\nDEVENV_DERIVED=${DEVENV_PROBE}-derived\n"),
            &[],
        );
        assert_eq!(p.result, "ok");
        assert_eq!(p.vars["DEVENV_PROBE"], "Some(\"local\")");
        assert_eq!(p.vars["DEVENV_DERIVED"], "Some(\"local-derived\")");
        assert_eq!(p.vars["DEVENV_SET"], "None");
    }

    #[test]
    fn local_file_preserves_nonempty_and_explicitly_empty_process_values() {
        let p = run_probe(
            "preserve-process",
            None,
            Some("DEVENV_PROBE=local\nDEVENV_EMPTY=local\nDEVENV_SET=local\n"),
            &[("DEVENV_PROBE", "injected"), ("DEVENV_EMPTY", "")],
        );
        assert_eq!(p.result, "ok");
        assert_eq!(p.vars["DEVENV_PROBE"], "Some(\"injected\")");
        assert_eq!(p.vars["DEVENV_EMPTY"], "Some(\"\")");
        assert_eq!(p.vars["DEVENV_SET"], "Some(\"local\")");
    }

    #[test]
    fn malformed_local_file_fails_with_sanitized_error() {
        let p = run_probe(
            "local-malformed",
            Some("DEVENV_PROBE=ancestor\n"),
            Some("DEVENV_PROBE=\"super-secret-unterminated\n"),
            &[],
        );
        assert_eq!(p.result, "err:could not load development environment");
        assert!(!p.result.contains("super-secret"));
    }

    #[test]
    fn unreadable_local_path_fails_with_sanitized_error() {
        // A directory at the exact path is present but cannot be read as a file.
        let dir = env::temp_dir().join(format!("server-common-dev-env-{}-dir", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".env")).unwrap();
        let err = load_env_file(&dir.join(".env")).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(err.to_string(), "could not load development environment");
    }

    #[test]
    fn missing_exact_path_is_optional() {
        let missing = env::temp_dir().join(format!(
            "server-common-dev-env-{}-missing/.env",
            std::process::id()
        ));
        assert!(load_env_file(&missing).is_ok());
    }
}
