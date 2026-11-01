// SPDX-License-Identifier: Apache-2.0
//! Human and JSON output, exit codes, color opt-out, token input and the user service.
mod support;
use anydash::{
    cli::{self, Env, Io},
    collector,
    core::Owner,
    keys::MemoryStore,
    providers::Endpoints,
    service::{self, Platform, Runner},
};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);
impl Runner for Recorder {
    fn run(&mut self, program: &str, args: &[String]) -> anydash::error::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(format!("{program} {}", args.join(" ")));
        Ok(())
    }
}
struct Output {
    code: i32,
    out: String,
    err: String,
}
impl Output {
    fn json(&self) -> Value {
        serde_json::from_str(self.out.trim()).unwrap_or_else(|_| panic!("not JSON: {}", self.out))
    }
}
fn env(home: &Path, extra: &[(&str, &str)], platform: Platform, recorder: &Recorder) -> Env {
    Env {
        settings: support::settings(home, extra),
        endpoints: Endpoints::mock("http://127.0.0.1:9", "grok"),
        os: Arc::new(MemoryStore::default()),
        exe: home.join("bin/anydash"),
        runner: Box::new(recorder.clone()),
        platform,
        open_browser: false,
    }
}
async fn run_with(env: &mut Env, args: &[&str], stdin: &str, tty: bool) -> Output {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let mut out = vec![];
    let mut err = vec![];
    let mut input = std::io::Cursor::new(stdin.as_bytes().to_vec());
    let mut io = Io {
        out: &mut out,
        err: &mut err,
        input: &mut input,
        tty,
    };
    let code = cli::run(&args, env, &mut io).await;
    Output {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}
async fn run(home: &Path, args: &[&str]) -> Output {
    run_with(
        &mut env(
            home,
            &[("ANYDASH_API_URL", "http://127.0.0.1:9")],
            Platform::current(),
            &Recorder::default(),
        ),
        args,
        "",
        false,
    )
    .await
}
#[tokio::test(flavor = "multi_thread")]
async fn version_help_and_invalid_invocations() {
    let home = tempfile::tempdir().unwrap();
    let o = run(home.path(), &["--version"]).await;
    assert_eq!(
        (o.code, o.out.trim()),
        (0, format!("anydash {}", env!("CARGO_PKG_VERSION")).as_str())
    );
    let o = run(home.path(), &["--help"]).await;
    assert!(o.code == 0 && o.out.contains("paths list"));
    assert_eq!(run(home.path(), &[]).await.code, 2);
    let o = run(home.path(), &["teleport"]).await;
    assert_eq!(o.code, 2);
    assert!(o.err.contains("Unknown command"));
    let o = run(home.path(), &["--json", "status", "--bogus"]).await;
    assert_eq!(o.code, 2);
    let v = o.json();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["reason"], "ERROR_REASON_INVALID_ARGUMENT");
    assert!(
        v["error"]["correlationId"]
            .as_str()
            .is_some_and(|c| c.len() == 36)
    );
    // Tokens are never accepted as command arguments.
    assert_eq!(
        run(home.path(), &["connect", "vercel", "--token", "secret"])
            .await
            .code,
        2
    );
    assert_eq!(
        run(home.path(), &["connect", "vercel", "--team-id", "t"])
            .await
            .code,
        2
    );
    assert_eq!(
        run(home.path(), &["paths", "add", "cursor", "/x"])
            .await
            .code,
        2
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn status_human_json_and_color_opt_out() {
    let home = tempfile::tempdir().unwrap();
    let o = run(home.path(), &["status"]).await;
    assert_eq!(o.code, 0);
    assert!(
        o.out.contains("Collector: not running")
            && o.out.contains("Device ID: -")
            && o.out.contains("Collector version:")
    );
    assert!(o.out.contains("The collector is not running."));
    let o = run(home.path(), &["--json", "status"]).await;
    let v = o.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["runState"], "COLLECTOR_RUN_STATE_NOT_LOGGED_IN");
    assert_eq!(v["result"]["collectorRunning"], false);
    assert_eq!(
        v["result"]["pendingBucketCount"], "0",
        "64-bit values are decimal strings"
    );
    assert!(!o.out.contains('\u{1b}'));
    let recorder = Recorder::default();
    let colored = run_with(
        &mut env(home.path(), &[], Platform::current(), &recorder),
        &["status"],
        "",
        true,
    )
    .await;
    assert!(colored.out.contains('\u{1b}'));
    for (vars, args) in [
        (vec![("NO_COLOR", "1")], vec!["status"]),
        (vec![("ANYDASH_LOG_COLOR", "never")], vec!["status"]),
        (vec![], vec!["--color=never", "status"]),
        (vec![], vec!["--json", "--color", "always", "status"]),
    ] {
        let o = run_with(
            &mut env(home.path(), &vars, Platform::current(), &recorder),
            &args,
            "",
            true,
        )
        .await;
        assert!(!o.out.contains('\u{1b}'), "{vars:?} {args:?}");
    }
    let forced = run_with(
        &mut env(home.path(), &[], Platform::current(), &recorder),
        &["--color=always", "status"],
        "",
        false,
    )
    .await;
    assert!(forced.out.contains('\u{1b}'));
    let parsed = cli::parse(&["--color=never".into(), "run".into()]).unwrap();
    assert!(!cli::log_color(
        &parsed,
        &support::settings(home.path(), &[])
    ));
    let parsed = cli::parse(&["run".into()]).unwrap();
    assert!(
        cli::log_color(&parsed, &support::settings(home.path(), &[])),
        "logs are colored by default"
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn paths_commands() {
    let home = tempfile::tempdir().unwrap();
    let custom = home.path().join("sessions");
    std::fs::create_dir_all(&custom).unwrap();
    let custom = custom.to_string_lossy().into_owned();
    let o = run(home.path(), &["paths", "add", "codex", &custom]).await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("user_added\tenabled\texists"));
    let o = run(
        home.path(),
        &["--json", "paths", "disable", "codex", &custom],
    )
    .await;
    assert_eq!(o.json()["result"]["enabled"], false);
    let detected = home
        .path()
        .join(".claude/projects")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        run(home.path(), &["paths", "disable", "claude-code", &detected])
            .await
            .code,
        0
    );
    let v = run(home.path(), &["--json", "paths", "list"]).await.json();
    let paths = v["result"]["paths"].as_array().unwrap();
    assert!(paths.iter().any(|p| p["path"] == detected
        && p["enabled"] == false
        && p["origin"] == "SOURCE_PATH_ORIGIN_DEFAULT"));
    assert!(paths.iter().any(|p| p["path"] == custom.as_str() && p["origin"] == "SOURCE_PATH_ORIGIN_USER_ADDED"));
    assert_eq!(
        run(home.path(), &["paths", "enable", "claude-code", &detected])
            .await
            .code,
        0
    );
    assert_eq!(
        run(home.path(), &["paths", "enable", "codex", "/not/listed"])
            .await
            .code,
        1
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn operation_failures_exit_one_and_manual_refresh_wait() {
    let home = tempfile::tempdir().unwrap();
    let o = run(home.path(), &["--json", "logout"]).await;
    assert_eq!(
        (o.code, o.json()["error"]["reason"].as_str()),
        (1, Some("ERROR_REASON_NOT_LOGGED_IN"))
    );
    let mut e = env(
        home.path(),
        &[("VERCEL_TOKEN", "v")],
        Platform::current(),
        &Recorder::default(),
    );
    let o = run_with(
        &mut e,
        &[
            "connect",
            "vercel",
            "--team-id",
            "team",
            "--token-env",
            "VERCEL_TOKEN",
        ],
        "",
        false,
    )
    .await;
    assert_eq!(o.code, 1);
    assert!(o.err.contains("anydash login"));
    let o = run_with(
        &mut e,
        &["connect", "supabase", "--token-stdin"],
        "sbp_secret\n",
        false,
    )
    .await;
    assert_eq!(o.code, 1, "stdin tokens are read, then login is required");
    assert!(!o.err.contains("sbp_secret"));
    assert_eq!(run(home.path(), &["refresh"]).await.code, 0);
    let o = run(home.path(), &["refresh"]).await;
    assert_eq!(o.code, 1);
    assert!(o.err.contains("Try again in"));
    let v = run(home.path(), &["--json", "refresh"]).await.json();
    assert!(
        v["error"]["retryAfterSeconds"]
            .as_u64()
            .is_some_and(|s| s <= 60)
    );
    let o = run(home.path(), &["--json", "key", "rotate"]).await;
    assert_eq!(
        (o.code, o.json()["result"]["secretCount"].as_u64()),
        (0, Some(0))
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn service_install_and_uninstall_per_platform() {
    for platform in [Platform::MacOs, Platform::Linux, Platform::Windows] {
        let home = tempfile::tempdir().unwrap();
        let recorder = Recorder::default();
        let mut e = env(home.path(), &[], platform, &recorder);
        let o = run_with(&mut e, &["service", "install"], "", false).await;
        assert_eq!(o.code, 0, "{}", o.err);
        let definition = service::definition(&e.settings, platform);
        assert!(definition.exists());
        let content = std::fs::read_to_string(&definition).unwrap();
        let commands = recorder.0.lock().unwrap().clone();
        match platform {
            Platform::MacOs => {
                assert!(
                    definition.ends_with("Library/LaunchAgents/io.delino.anydash.collector.plist")
                );
                assert!(
                    content.contains("<key>RunAtLoad</key><true/>")
                        && content.contains("<key>KeepAlive</key><true/>")
                );
                assert!(content.contains("<string>run</string><string>--service</string>"));
                assert!(content.contains("ANYDASH_HOME"));
                assert!(
                    commands
                        .iter()
                        .any(|c| c.starts_with("launchctl bootstrap gui/"))
                );
            }
            Platform::Linux => {
                assert!(definition.ends_with(".config/systemd/user/anydash-collector.service"));
                assert!(
                    content.contains("run --service")
                        && content.contains("WantedBy=default.target")
                );
                assert!(
                    commands
                        .iter()
                        .any(|c| c == "systemctl --user enable --now anydash-collector.service")
                );
                assert!(o.err.contains("enable-linger"));
            }
            Platform::Windows => {
                assert!(
                    commands[0].contains("/SC ONLOGON")
                        && commands[0].contains("/TN AnyDash Collector")
                        && commands[0].contains("run --service")
                );
            }
        }
        recorder.0.lock().unwrap().clear();
        assert_eq!(
            run_with(&mut e, &["service", "uninstall"], "", false)
                .await
                .code,
            0
        );
        assert!(!definition.exists());
        assert_ne!(recorder.0.lock().unwrap().len(), 0);
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn service_install_is_refused_while_a_foreground_collector_runs() {
    let home = tempfile::tempdir().unwrap();
    let recorder = Recorder::default();
    let mut e = env(
        home.path(),
        &[("ANYDASH_API_URL", "http://127.0.0.1:9")],
        Platform::current(),
        &recorder,
    );
    let running = collector::start(anydash::core::Options {
        settings: e.settings.clone(),
        endpoints: e.endpoints.clone(),
        os: e.os.clone(),
        owner: Owner::Foreground,
    })
    .unwrap();
    let core = running.core.clone();
    let task = tokio::spawn(running.run());
    let o = run_with(&mut e, &["service", "install"], "", false).await;
    assert_eq!(o.code, 1);
    assert!(o.err.contains("stop it before installing"));
    assert_eq!(recorder.0.lock().unwrap().len(), 0);
    // Commands use the running collector through local IPC.
    let v = run_with(&mut e, &["--json", "status"], "", false)
        .await
        .json();
    assert_eq!(v["result"]["collectorRunning"], true);
    assert_eq!(v["result"]["ownerKind"], "COLLECTOR_KIND_CLI");
    let o = run_with(&mut e, &["run"], "", false).await;
    assert_eq!(o.code, 1, "a second collector for the same user fails");
    core.quit().unwrap();
    task.await.unwrap().unwrap();
}
