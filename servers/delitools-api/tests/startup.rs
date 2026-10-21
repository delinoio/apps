// SPDX-License-Identifier: FSL-1.1-ALv2
//! Process-level startup behavior that needs no database: the working-directory `.env` supplies
//! telemetry settings, and process variables keep precedence over it.
use std::{path::PathBuf, process::Command};

/// Run the binary in a scratch working directory whose `.env` is `dotenv`. The configuration is
/// intentionally incomplete, so startup logs a rejection and exits without touching any service.
fn rejected_startup_log(dotenv: &str, process_env: &[(&str, &str)]) -> String {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("delitools-startup-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(".env"), dotenv).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_delitools-api"))
        .current_dir(&dir)
        .env_clear()
        .envs(process_env.iter().copied())
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!output.status.success());
    String::from_utf8(output.stderr).unwrap()
}

fn is_json_log(log: &str) -> bool {
    log.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .is_ok_and(|v| v["fields"]["category"] == "invalid_configuration")
    })
}

#[test]
fn log_format_from_the_dotenv_file_configures_logging() {
    let log = rejected_startup_log("LOG_FORMAT=json\n", &[]);
    assert!(is_json_log(&log), "{log}");
}

#[test]
fn process_variables_take_precedence_over_the_dotenv_file() {
    let log = rejected_startup_log("LOG_FORMAT=json\n", &[("LOG_FORMAT", "text")]);
    assert!(
        !is_json_log(&log) && log.contains("configuration rejected"),
        "{log}"
    );
    let explicit_empty = rejected_startup_log("LOG_FORMAT=json\n", &[("LOG_FORMAT", "")]);
    assert!(!is_json_log(&explicit_empty), "{explicit_empty}");
}

#[test]
fn an_unreadable_dotenv_file_is_reported_and_stops_startup() {
    let log = rejected_startup_log("this line is not valid\n", &[]);
    assert!(
        log.contains("configuration rejected") && log.contains("could not read .env configuration"),
        "{log}"
    );
}
