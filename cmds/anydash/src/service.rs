// SPDX-License-Identifier: Apache-2.0
//! The opt-in per-user service that starts `anydash run --service` at login: a launchd
//! LaunchAgent on macOS, a systemd user unit on Linux and a Task Scheduler logon task on Windows.
use crate::{
    error::{Failure, Result},
    pb::ErrorReason,
    settings::Settings,
};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};
pub const LABEL: &str = "io.delino.anydash.collector";
pub const UNIT: &str = "anydash-collector.service";
pub const TASK: &str = "AnyDash Collector";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}
impl Platform {
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}
/// Runs service manager commands; tests record them instead.
pub trait Runner {
    fn run(&mut self, program: &str, args: &[String]) -> Result<()>;
}
pub struct System;
impl Runner for System {
    fn run(&mut self, program: &str, args: &[String]) -> Result<()> {
        let status = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(Failure::local(
                ErrorReason::Internal,
                &format!("{program} failed."),
            ))
        }
    }
}
/// The definition file, or the marker file that records the Windows task.
#[must_use]
pub fn definition(settings: &Settings, platform: Platform) -> PathBuf {
    match platform {
        Platform::MacOs => settings
            .user_home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
        Platform::Linux => settings.config_home().join("systemd/user").join(UNIT),
        Platform::Windows => settings.dirs().config.join("service-task"),
    }
}
#[must_use]
pub fn installed(settings: &Settings) -> bool {
    definition(settings, Platform::current()).exists()
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn environment(settings: &Settings) -> Vec<(&'static str, String)> {
    ["ANYDASH_HOME", "ANYDASH_API_URL", "ANYDASH_KEY_SOURCE"]
        .into_iter()
        .filter_map(|k| settings.var(k).map(|v| (k, v.to_owned())))
        .collect()
}
#[must_use]
pub fn launchd(settings: &Settings, exe: &Path) -> String {
    let env = environment(settings);
    let env = if env.is_empty() {
        String::new()
    } else {
        format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n{}  </dict>\n",
            env.iter().fold(String::new(), |mut s, (k, v)| {
                let _ = writeln!(s, "    <key>{k}</key><string>{}</string>", xml(v));
                s
            })
        )
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{LABEL}</string>\n  <key>ProgramArguments</key>\n  <array><string>{}</string><string>run</string><string>--service</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n  <key>ProcessType</key><string>Background</string>\n{env}</dict>\n</plist>\n",
        xml(&exe.to_string_lossy())
    )
}
fn systemd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
#[must_use]
pub fn systemd(settings: &Settings, exe: &Path) -> String {
    let env = environment(settings)
        .iter()
        .fold(String::new(), |mut s, (k, v)| {
            let _ = writeln!(s, "Environment={}", systemd_quote(&format!("{k}={v}")));
            s
        });
    format!(
        "[Unit]\nDescription=AnyDash collector\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={} run --service\nRestart=on-failure\nRestartSec=30\n{env}\n[Install]\nWantedBy=default.target\n",
        systemd_quote(&exe.to_string_lossy())
    )
}
/// `schtasks` arguments that register the logon task.
#[must_use]
pub fn windows_create(exe: &Path) -> Vec<String> {
    [
        "/Create",
        "/F",
        "/SC",
        "ONLOGON",
        "/RL",
        "LIMITED",
        "/TN",
        TASK,
        "/TR",
        &format!("\"{}\" run --service", exe.to_string_lossy()),
    ]
    .map(str::to_owned)
    .to_vec()
}
fn uid() -> String {
    #[cfg(unix)]
    // SAFETY: getuid has no preconditions and cannot fail.
    let id = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let id = 0;
    id.to_string()
}
fn write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}
/// Install and start the service. Returns guidance for the user.
pub fn install(
    settings: &Settings,
    exe: &Path,
    platform: Platform,
    runner: &mut dyn Runner,
) -> Result<String> {
    let path = definition(settings, platform);
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    match platform {
        Platform::MacOs => {
            write(&path, &launchd(settings, exe))?;
            let domain = format!("gui/{}", uid());
            let _ = runner.run("launchctl", &s(&["bootout", &format!("{domain}/{LABEL}")]));
            runner.run(
                "launchctl",
                &s(&["bootstrap", &domain, &path.to_string_lossy()]),
            )?;
            Ok(format!("Installed the LaunchAgent {LABEL}."))
        }
        Platform::Linux => {
            write(&path, &systemd(settings, exe))?;
            runner.run("systemctl", &s(&["--user", "daemon-reload"]))?;
            runner.run("systemctl", &s(&["--user", "enable", "--now", UNIT]))?;
            Ok(format!(
                "Installed the systemd user unit {UNIT}. On servers without a login session, run `loginctl enable-linger {}` so it keeps running after logout.",
                settings.var("USER").unwrap_or("$USER")
            ))
        }
        Platform::Windows => {
            runner.run("schtasks", &windows_create(exe))?;
            runner.run("schtasks", &s(&["/Run", "/TN", TASK]))?;
            write(&path, TASK)?;
            Ok(format!("Installed the logon task {TASK}."))
        }
    }
}
/// Stop the service and remove its definition.
///
/// On macOS and Linux the stop signal lets the collector release its leases; on Windows,
/// ending the task stops the process and its leases expire after 5 minutes.
pub fn uninstall(
    settings: &Settings,
    platform: Platform,
    runner: &mut dyn Runner,
) -> Result<String> {
    let path = definition(settings, platform);
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    match platform {
        Platform::MacOs => {
            let _ = runner.run(
                "launchctl",
                &s(&["bootout", &format!("gui/{}/{LABEL}", uid())]),
            );
        }
        Platform::Linux => {
            let _ = runner.run("systemctl", &s(&["--user", "disable", "--now", UNIT]));
        }
        Platform::Windows => {
            let _ = runner.run("schtasks", &s(&["/End", "/TN", TASK]));
            let _ = runner.run("schtasks", &s(&["/Delete", "/F", "/TN", TASK]));
        }
    }
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    if platform == Platform::Linux {
        let _ = runner.run("systemctl", &s(&["--user", "daemon-reload"]));
    }
    Ok("Removed the AnyDash user service.".to_owned())
}
