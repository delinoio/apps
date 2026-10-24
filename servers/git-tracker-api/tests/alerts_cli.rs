// SPDX-License-Identifier: FSL-1.1-ALv2
//! Runs the built `git-tracker-api alerts` binary with an isolated environment. No database or
//! provider is needed to generate rules.
use std::process::{Command, Output};

const KEYS: [&str; 4] = ["API_ERRORS", "PERSISTENCE", "GITHUB", "INTERRUPTED"];

fn run(override_name: &str, override_value: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-tracker-api"));
    command.arg("alerts").env_clear();
    for key in KEYS {
        command
            .env(format!("ALERT_{key}_THRESHOLD"), "1")
            .env(format!("ALERT_{key}_WINDOW"), "5m")
            .env(format!("ALERT_{key}_FOR"), "1m");
    }
    command.env(override_name, override_value);
    command.output().expect("run git-tracker-api alerts")
}

#[test]
fn prints_rules_for_maximum_representable_durations() {
    let output = run("ALERT_GITHUB_FOR", "106751d");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("for: 106751d"));
}

#[test]
fn exits_nonzero_without_rules_for_out_of_range_durations() {
    for (name, value) in [
        ("ALERT_API_ERRORS_FOR", "10000000000s"),
        ("ALERT_PERSISTENCE_WINDOW", "10000000000s"),
        ("ALERT_GITHUB_WINDOW", "2562048h"),
        ("ALERT_INTERRUPTED_FOR", "106752d"),
    ] {
        let output = run(name, value);
        assert!(!output.status.success(), "{name}={value}");
        assert!(output.stdout.is_empty(), "{name}={value}");
    }
}
