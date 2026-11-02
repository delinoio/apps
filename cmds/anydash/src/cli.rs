// SPDX-License-Identifier: Apache-2.0
//! Command-line parsing, execution through the running collector or directly, and human and
//! JSON output. Exit code 0 is success, 1 an operation failure and 2 an invalid invocation.
use crate::{
    collector,
    core::{Core, Options, Owner},
    error::{Failure, Result},
    ipc,
    keys::{DataKey, SecureStore},
    pb::{
        self, AddConnectionCredentialRequest, AddConnectionCredentialResponse,
        AddSourcePathRequest, AddSourcePathResponse, CollectorStatus, DisableSourcePathRequest,
        DisableSourcePathResponse, EnableSourcePathRequest, EnableSourcePathResponse,
        EnrollmentState, ErrorReason, GetCollectorStatusRequest, GetCollectorStatusResponse,
        KeySource, ListSourcePathsRequest, ListSourcePathsResponse, LocalTool, LogoutRequest,
        LogoutResponse, Provider, RemoveConnectionCredentialRequest,
        RemoveConnectionCredentialResponse, RequestLocalConnectionRefreshRequest,
        RequestLocalConnectionRefreshResponse, RequestLocalSyncRequest, RequestLocalSyncResponse,
        RotateKeyRequest, RotateKeyResponse, SourcePath, StartLoginRequest, StartLoginResponse,
    },
    providers::{self, Endpoints},
    scan, service,
    settings::Settings,
    time,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
    path::PathBuf,
    sync::Arc,
};
pub const USAGE: &str = "Usage: anydash [--json] [--color auto|always|never] <command>

Commands:
  login [--device-name NAME]       Connect this device with a code and browser approval
  logout                           Delete the device credentials and stop collection
  run [--service | --owner foreground|service|desktop]
                                   Run the collector in the foreground
  service install | uninstall      Install or remove the per-user service
  status                           Show collector, device, connection and parser status
  connect <codex|claude-code|grok|vercel|supabase|cloudflare>
          [--connection-id ID] [--name NAME] [--token-stdin | --token-env VAR] [--team-id ID]
                                   Add an external connection or register this device for one
  connection list                  List connections registered for this device
  disconnect <connection-id>       Remove this device's credential for a connection
  refresh [<connection-id>]        Request a manual refresh or a local usage sync
  key rotate [--new-key-env VAR]   Rotate the local encryption key
  paths list | add | disable | enable <codex|claude-code> <path>
                                   Manage session paths
  --version                        Print the executable version
";
const VALUE_FLAGS: [&str; 8] = [
    "color",
    "device-name",
    "connection-id",
    "name",
    "token-env",
    "team-id",
    "new-key-env",
    "owner",
];
const SWITCHES: [&str; 5] = ["json", "token-stdin", "service", "help", "version"];
/// Process inputs. Tests replace the endpoints, the secure store and the service runner.
pub struct Env {
    pub settings: Settings,
    pub endpoints: Endpoints,
    pub os: Arc<dyn SecureStore>,
    pub exe: PathBuf,
    pub runner: Box<dyn service::Runner + Send>,
    pub platform: service::Platform,
    /// Open the approval page in a browser when a GUI session exists.
    pub open_browser: bool,
}
pub struct Io<'a> {
    pub out: &'a mut (dyn Write + Send),
    pub err: &'a mut (dyn Write + Send),
    pub input: &'a mut (dyn BufRead + Send),
    pub tty: bool,
}
#[derive(Default, Debug)]
pub struct Parsed {
    pub json: bool,
    pub color: Option<String>,
    pub words: Vec<String>,
    pub flags: BTreeMap<String, String>,
    pub switches: Vec<String>,
}
impl Parsed {
    fn flag(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }
    fn switch(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
}
fn usage(message: &str) -> Failure {
    Failure::invalid("command", message)
}
pub fn parse(args: &[String]) -> Result<Parsed> {
    let mut parsed = Parsed::default();
    let mut i = 0;
    while i < args.len() {
        let arg = if args[i] == "-h" {
            "--help"
        } else {
            args[i].as_str()
        };
        if let Some(flag) = arg.strip_prefix("--") {
            let (name, inline) = flag
                .split_once('=')
                .map_or((flag, None), |(k, v)| (k, Some(v.to_owned())));
            if SWITCHES.contains(&name) && inline.is_none() {
                parsed.switches.push(name.to_owned());
            } else if VALUE_FLAGS.contains(&name) {
                let value = if let Some(v) = inline {
                    v
                } else {
                    i += 1;
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| usage(&format!("--{name} requires a value.")))?
                };
                parsed.flags.insert(name.to_owned(), value);
            } else {
                return Err(usage(&format!("Unknown option --{name}.")));
            }
        } else {
            parsed.words.push(arg.to_owned());
        }
        i += 1;
    }
    parsed.json = parsed.switch("json");
    parsed.color = parsed.flags.remove("color");
    if let Some(c) = &parsed.color
        && !matches!(c.as_str(), "auto" | "always" | "never")
    {
        return Err(usage("--color must be auto, always or never."));
    }
    Ok(parsed)
}
/// Whether human output uses ANSI color.
#[must_use]
pub fn output_color(parsed: &Parsed, settings: &Settings, tty: bool) -> bool {
    match parsed.color.as_deref() {
        Some("always") => !parsed.json,
        Some("never") => false,
        _ => tty && !parsed.json && !settings.color_disabled(),
    }
}
/// Whether logs use ANSI color: on by default, off with `--color=never`, `NO_COLOR` or
/// `ANYDASH_LOG_COLOR=never`.
#[must_use]
pub fn log_color(parsed: &Parsed, settings: &Settings) -> bool {
    parsed.color.as_deref() != Some("never") && !settings.color_disabled()
}
enum Backend {
    Ipc(ipc::Client),
    /// Direct execution holds the collector lock file until the command ends.
    Direct(
        Arc<Core>,
        #[expect(dead_code, reason = "Held only to keep the collector lock.")] std::fs::File,
    ),
}
impl Backend {
    const fn running(&self) -> bool {
        matches!(self, Self::Ipc(_))
    }
    async fn call<Req: prost::Message, Resp: prost::Message + Default>(
        &self,
        method: &str,
        request: &Req,
    ) -> Result<Resp> {
        match self {
            Self::Ipc(client) => client.call(method, request).await,
            Self::Direct(core, _) => {
                let bytes = ipc::dispatch(core, method, &request.encode_to_vec(), false)
                    .await
                    .ok_or_else(|| Failure::internal("Unknown method."))??;
                Resp::decode(bytes.as_slice())
                    .map_err(|_| Failure::internal("Unreadable response."))
            }
        }
    }
}
fn options(env: &Env, owner: Owner) -> Options {
    Options {
        settings: env.settings.clone(),
        endpoints: env.endpoints.clone(),
        os: env.os.clone(),
        owner,
    }
}
async fn backend(env: &Env) -> Result<Backend> {
    let dirs = env.settings.dirs();
    for _ in 0..10 {
        if let Some(client) = ipc::Client::connect(&dirs).await {
            return Ok(Backend::Ipc(client));
        }
        if let Some(lock) = collector::try_lock(&dirs)? {
            return Ok(Backend::Direct(
                Core::open(options(env, Owner::Direct))?,
                lock,
            ));
        }
        // A collector holds the lock but is still binding its IPC endpoint.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err(Failure::local(
        ErrorReason::DependencyUnavailable,
        "A collector is running but its local IPC service is unreachable.",
    ))
}
struct Out<'a, 'b> {
    io: &'a mut Io<'b>,
    color: bool,
}
impl Out<'_, '_> {
    fn line(&mut self, text: &str) {
        let _ = writeln!(self.io.out, "{text}");
    }
    fn field(&mut self, label: &str, value: &str) {
        let label = if self.color {
            format!("\u{1b}[1m{label}:\u{1b}[0m")
        } else {
            format!("{label}:")
        };
        let _ = writeln!(self.io.out, "{label} {value}");
    }
    fn note(&mut self, text: &str) {
        let _ = writeln!(self.io.err, "{text}");
    }
}
fn show_time(t: Option<&prost_types::Timestamp>) -> String {
    t.map_or_else(|| "-".to_owned(), |t| time::rfc3339(t.seconds))
}
fn enum_name<E: TryFrom<i32>>(value: i32, name: impl Fn(&E) -> &'static str) -> String {
    E::try_from(value).map_or_else(|_| value.to_string(), |e| name(&e).to_owned())
}
fn path_json(p: &SourcePath) -> Value {
    pb::to_json(p, "SourcePath")
}
fn print_paths(out: &mut Out<'_, '_>, paths: &[SourcePath]) {
    for p in paths {
        out.line(&format!(
            "{}\t{}\t{}\t{}\t{}",
            scan::tool_name(LocalTool::try_from(p.tool).unwrap_or(LocalTool::Unspecified)),
            enum_name(p.origin, pb::SourcePathOrigin::as_str_name)
                .trim_start_matches("SOURCE_PATH_ORIGIN_")
                .to_ascii_lowercase(),
            if p.enabled { "enabled" } else { "disabled" },
            if p.exists { "exists" } else { "missing" },
            p.path
        ));
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "One function prints every documented status field in order."
)]
fn print_status(out: &mut Out<'_, '_>, s: &CollectorStatus, running: bool) {
    out.field(
        "Collector",
        &if running {
            format!(
                "running ({})",
                enum_name(s.owner_kind, pb::CollectorKind::as_str_name)
            )
        } else {
            "not running".to_owned()
        },
    );
    out.field(
        "Run state",
        &enum_name(s.run_state, pb::CollectorRunState::as_str_name),
    );
    out.field(
        "Device ID",
        if s.device_id.is_empty() {
            "-"
        } else {
            &s.device_id
        },
    );
    out.field("Collector version", &s.collector_version);
    out.field(
        "Key source",
        &enum_name(s.key_source, KeySource::as_str_name),
    );
    out.field(
        "Autostart",
        if s.autostart_enabled {
            "enabled"
        } else {
            "disabled"
        },
    );
    out.field("Last local sync", &show_time(s.last_local_sync_at.as_ref()));
    out.field("Last upload", &show_time(s.last_upload_at.as_ref()));
    out.field(
        "Pending buckets",
        &format!(
            "{} (oldest {})",
            s.pending_bucket_count,
            show_time(s.oldest_pending_bucket_start.as_ref())
        ),
    );
    if s.last_error_correlation_id.is_empty() {
        out.field("Last error", "-");
    } else {
        out.field(
            "Last error",
            &format!(
                "{} correlation ID {} at {}",
                enum_name(s.last_error_reason, ErrorReason::as_str_name),
                s.last_error_correlation_id,
                show_time(s.last_error_at.as_ref())
            ),
        );
    }
    out.field("Connections", &s.connections.len().to_string());
    for c in &s.connections {
        out.line(&format!(
            "  {} {} \"{}\" credential={} lease={} epoch={} status={} failure={} last_success={} next_attempt={} last_error={}",
            c.connection_id,
            providers::provider_name(Provider::try_from(c.provider).unwrap_or(Provider::Unspecified)),
            c.display_name,
            if c.credential_present { "present" } else { "missing" },
            if c.lease_held { "held" } else { "not-held" },
            c.lease_epoch,
            enum_name(c.last_status, pb::CollectionStatus::as_str_name),
            enum_name(c.failure_class, pb::FailureClass::as_str_name),
            show_time(c.last_success_at.as_ref()),
            show_time(c.next_attempt_at.as_ref()),
            if c.last_error_correlation_id.is_empty() { "-" } else { &c.last_error_correlation_id },
        ));
    }
    out.field("Parsers", "");
    for p in &s.parsers {
        out.line(&format!(
            "  {} {} skipped={} paths={} version={}",
            scan::tool_name(LocalTool::try_from(p.tool).unwrap_or(LocalTool::Unspecified)),
            enum_name(p.state, pb::ParserState::as_str_name),
            p.skipped_record_count,
            p.source_path_count,
            p.parser_version
        ));
    }
    if let Some(l) = &s.login {
        out.field(
            "Login",
            &format!(
                "{} code {} {}",
                enum_name(l.state, EnrollmentState::as_str_name),
                l.user_code,
                l.verification_url
            ),
        );
    }
    for n in &s.notices {
        let text = match pb::LocalNotice::try_from(*n) {
            Ok(pb::LocalNotice::UpdateAvailable) => {
                "A new collector version is available. Update through the install script, npm or the desktop app."
            }
            Ok(pb::LocalNotice::UpdateRequired) => {
                "Update required: this collector version is no longer accepted. Local usage keeps aggregating for up to 90 days."
            }
            Ok(pb::LocalNotice::UnsupportedRecords) => {
                "Some session records could not be read. Update the collector."
            }
            _ => continue,
        };
        out.field("Notice", text);
    }
    if !running {
        out.line("The collector is not running.");
    }
}
fn read_token(parsed: &Parsed, env: &Env, io: &mut Io<'_>) -> Result<String> {
    match (parsed.switch("token-stdin"), parsed.flag("token-env")) {
        (true, Some(_)) => Err(usage("Use either --token-stdin or --token-env.")),
        (true, None) => {
            let mut line = String::new();
            io.input.read_line(&mut line)?;
            Ok(line.trim().to_owned())
        }
        (false, Some(var)) => env
            .settings
            .var(var)
            .map(str::to_owned)
            .ok_or_else(|| usage(&format!("The environment variable {var} is not set."))),
        (false, None) => Ok(String::new()),
    }
}
fn open_browser(settings: &Settings, url: &str) {
    let gui = if cfg!(target_os = "macos") || cfg!(windows) {
        settings.var("SSH_CONNECTION").is_none()
    } else {
        settings.var("DISPLAY").is_some() || settings.var("WAYLAND_DISPLAY").is_some()
    };
    if !gui || !url.starts_with("https://") {
        return;
    }
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = command
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
fn tool_arg(words: &[String], index: usize) -> Result<LocalTool> {
    words
        .get(index)
        .and_then(|t| scan::parse_tool(t))
        .ok_or_else(|| usage("Choose codex or claude-code."))
}
async fn login(parsed: &Parsed, env: &Env, b: &Backend, out: &mut Out<'_, '_>) -> Result<Value> {
    let r: StartLoginResponse = b
        .call(
            "StartLogin",
            &StartLoginRequest {
                device_name: parsed.flag("device-name").unwrap_or("").to_owned(),
            },
        )
        .await?;
    let login = r.login.unwrap_or_default();
    let message = format!(
        "Open {} and confirm the code {}. Waiting for approval...",
        login.verification_url, login.user_code
    );
    if parsed.json {
        out.note(&message);
    } else {
        out.line(&message);
    }
    if env.open_browser {
        open_browser(&env.settings, &login.verification_url);
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let s: GetCollectorStatusResponse = b
            .call("GetCollectorStatus", &GetCollectorStatusRequest {})
            .await?;
        let status = s.status.unwrap_or_default();
        let state = status
            .login
            .as_ref()
            .map_or(EnrollmentState::Unspecified as i32, |l| l.state);
        match EnrollmentState::try_from(state).unwrap_or(EnrollmentState::Unspecified) {
            EnrollmentState::Pending => {}
            EnrollmentState::Consumed | EnrollmentState::Approved => {
                if !parsed.json {
                    out.line(&format!("Logged in. Device ID: {}", status.device_id));
                }
                return Ok(json!({"deviceId": status.device_id}));
            }
            EnrollmentState::Expired => {
                return Err(Failure::local(
                    ErrorReason::EnrollmentExpired,
                    "The connection code expired; run anydash login again.",
                ));
            }
            _ => {
                return Err(Failure::local(
                    ErrorReason::EnrollmentInvalidated,
                    "The enrollment was rejected or invalidated; run anydash login again.",
                ));
            }
        }
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "One dispatch table maps every documented command."
)]
async fn execute(parsed: &Parsed, env: &mut Env, io: &mut Io<'_>, color: bool) -> Result<Value> {
    let words: Vec<&str> = parsed.words.iter().map(String::as_str).collect();
    let mut out = Out { io, color };
    match words.as_slice() {
        ["run"] => {
            let owner = match (parsed.switch("service"), parsed.flag("owner")) {
                (true, None | Some("service")) | (false, Some("service")) => Owner::Service,
                (false, None | Some("foreground")) => Owner::Foreground,
                (false, Some("desktop")) => Owner::Desktop,
                _ => return Err(usage("--owner must be foreground, service or desktop.")),
            };
            collector::run(options(env, owner)).await?;
            Ok(json!({}))
        }
        ["service", action @ ("install" | "uninstall")] => {
            let dirs = env.settings.dirs();
            if *action == "install" {
                if let Some(client) = ipc::Client::connect(&dirs).await {
                    let s: GetCollectorStatusResponse = client
                        .call("GetCollectorStatus", &GetCollectorStatusRequest {})
                        .await?;
                    if !s.status.is_some_and(|s| s.autostart_enabled) {
                        return Err(Failure::local(
                            ErrorReason::Internal,
                            "A foreground or desktop collector is running; stop it before installing the service.",
                        ));
                    }
                }
                let message =
                    service::install(&env.settings, &env.exe, env.platform, env.runner.as_mut())?;
                out.note(&message);
            } else {
                let message = service::uninstall(&env.settings, env.platform, env.runner.as_mut())?;
                out.note(&message);
            }
            Ok(json!({"autostartEnabled": *action == "install"}))
        }
        ["login"] => {
            let b = backend(env).await?;
            login(parsed, env, &b, &mut out).await
        }
        ["logout"] => {
            let b = backend(env).await?;
            let _: LogoutResponse = b.call("Logout", &LogoutRequest {}).await?;
            out.note("Logged out. The device registration and its data remain on the server.");
            Ok(json!({}))
        }
        ["status"] => {
            let b = backend(env).await?;
            let r: GetCollectorStatusResponse = b
                .call("GetCollectorStatus", &GetCollectorStatusRequest {})
                .await?;
            let status = r.status.unwrap_or_default();
            if !parsed.json {
                print_status(&mut out, &status, b.running());
            }
            let mut v = pb::to_json(&status, "CollectorStatus");
            v["collectorRunning"] = b.running().into();
            Ok(v)
        }
        ["connect", provider] => {
            let provider = providers::parse_provider(provider).ok_or_else(|| {
                usage("Choose codex, claude-code, grok, vercel, supabase or cloudflare.")
            })?;
            let secret = read_token(parsed, env, out.io)?;
            if !providers::uses_local_cli(provider) && secret.is_empty() {
                return Err(usage(
                    "Supply the token with --token-stdin or --token-env VAR.",
                ));
            }
            if providers::uses_local_cli(provider) && !secret.is_empty() {
                return Err(usage(
                    "This provider uses its CLI login; do not supply a token.",
                ));
            }
            if provider == Provider::Vercel && parsed.flag("team-id").is_none() {
                return Err(usage("Vercel connections require --team-id."));
            }
            let b = backend(env).await?;
            let r: AddConnectionCredentialResponse = b
                .call(
                    "AddConnectionCredential",
                    &AddConnectionCredentialRequest {
                        provider: provider.into(),
                        connection_id: parsed.flag("connection-id").unwrap_or("").to_owned(),
                        display_name: parsed.flag("name").unwrap_or("").to_owned(),
                        secret,
                        scope_id: parsed.flag("team-id").unwrap_or("").to_owned(),
                    },
                )
                .await?;
            let c = r.connection.unwrap_or_default();
            if provider == Provider::Vercel {
                out.note("Vercel tokens can write. AnyDash calls only read endpoints.");
            }
            if !parsed.json {
                out.field("Connection ID", &c.id);
            }
            Ok(pb::to_json(&c, "Connection"))
        }
        ["connection", "list"] => {
            let b = backend(env).await?;
            let r: GetCollectorStatusResponse = b
                .call("GetCollectorStatus", &GetCollectorStatusRequest {})
                .await?;
            let status = r.status.unwrap_or_default();
            if !parsed.json {
                for c in &status.connections {
                    out.line(&format!(
                        "{}\t{}\t{}\tlease={}\tstatus={}\tlast_success={}",
                        c.connection_id,
                        providers::provider_name(
                            Provider::try_from(c.provider).unwrap_or(Provider::Unspecified)
                        ),
                        c.display_name,
                        if c.lease_held { "held" } else { "not-held" },
                        enum_name(c.last_status, pb::CollectionStatus::as_str_name),
                        show_time(c.last_success_at.as_ref()),
                    ));
                }
            }
            Ok(
                json!({"connections": status.connections.iter().map(|c| pb::to_json(c, "LocalConnectionStatus")).collect::<Vec<_>>()}),
            )
        }
        ["disconnect", id] => {
            let b = backend(env).await?;
            let _: RemoveConnectionCredentialResponse = b
                .call(
                    "RemoveConnectionCredential",
                    &RemoveConnectionCredentialRequest {
                        connection_id: (*id).to_owned(),
                    },
                )
                .await?;
            out.note(
                "Removed this device's credential. Delete the connection itself in the app or web.",
            );
            Ok(json!({}))
        }
        ["refresh"] => {
            let b = backend(env).await?;
            let r: RequestLocalSyncResponse = b
                .call("RequestLocalSync", &RequestLocalSyncRequest {})
                .await?;
            if !parsed.json {
                out.line("Local usage sync requested.");
            }
            Ok(pb::to_json(&r, "RequestLocalSyncResponse"))
        }
        ["refresh", id] => {
            // A running collector alone uses the device credentials; direct execution runs
            // the same handler under this command's collector lock. An older collector without
            // the method answers `unimplemented`, which asks the user to update it.
            let b = backend(env).await?;
            let r: RequestLocalConnectionRefreshResponse = b
                .call(
                    "RequestLocalConnectionRefresh",
                    &RequestLocalConnectionRefreshRequest {
                        connection_id: (*id).to_owned(),
                    },
                )
                .await?;
            if !parsed.json {
                out.line("Refresh requested.");
            }
            let next = r.next_allowed_at.map_or(0, |t| t.seconds);
            Ok(json!({"nextAllowedAt": time::rfc3339(next)}))
        }
        ["key", "rotate"] => {
            let b = backend(env).await?;
            let r: RotateKeyResponse = match (parsed.flag("new-key-env"), &b) {
                (Some(var), Backend::Direct(core, _)) => {
                    let key = DataKey::parse(env.settings.var(var).ok_or_else(|| {
                        usage(&format!("The environment variable {var} is not set."))
                    })?)?;
                    ipc::rotate_with(core, Some(key))?
                }
                (Some(_), Backend::Ipc(_)) => {
                    return Err(Failure::invalid(
                        "new-key-env",
                        "Stop the running collector before rotating an environment key.",
                    ));
                }
                (None, _) => b.call("RotateKey", &RotateKeyRequest {}).await?,
            };
            out.note(&format!(
                "Rotated the encryption key for {} secrets.",
                r.secret_count
            ));
            if parsed.flag("new-key-env").is_some() {
                out.note("Update ANYDASH_ENCRYPTION_KEY to the new key before the next start.");
            }
            Ok(pb::to_json(&r, "RotateKeyResponse"))
        }
        ["paths", "list"] => {
            let b = backend(env).await?;
            let r: ListSourcePathsResponse = b
                .call("ListSourcePaths", &ListSourcePathsRequest {})
                .await?;
            if !parsed.json {
                print_paths(&mut out, &r.paths);
            }
            Ok(json!({"paths": r.paths.iter().map(path_json).collect::<Vec<_>>()}))
        }
        ["paths", action @ ("add" | "disable" | "enable"), _, path] => {
            let tool = tool_arg(&parsed.words, 2)?;
            let b = backend(env).await?;
            let p = match *action {
                "add" => {
                    b.call::<_, AddSourcePathResponse>(
                        "AddSourcePath",
                        &AddSourcePathRequest {
                            tool: tool.into(),
                            path: (*path).to_owned(),
                        },
                    )
                    .await?
                    .path
                }
                "disable" => {
                    b.call::<_, DisableSourcePathResponse>(
                        "DisableSourcePath",
                        &DisableSourcePathRequest {
                            tool: tool.into(),
                            path: (*path).to_owned(),
                        },
                    )
                    .await?
                    .path
                }
                _ => {
                    b.call::<_, EnableSourcePathResponse>(
                        "EnableSourcePath",
                        &EnableSourcePathRequest {
                            tool: tool.into(),
                            path: (*path).to_owned(),
                        },
                    )
                    .await?
                    .path
                }
            }
            .unwrap_or_default();
            if !parsed.json {
                print_paths(&mut out, std::slice::from_ref(&p));
            }
            Ok(path_json(&p))
        }
        _ => Err(usage("Unknown command. Run anydash --help.")),
    }
}
/// Run the CLI and return the exit code.
pub async fn run(args: &[String], env: &mut Env, io: &mut Io<'_>) -> i32 {
    let parsed = match parse(args) {
        Ok(p) => p,
        Err(f) => {
            let json = args.iter().any(|a| a == "--json");
            return fail(io, &f, json, 2);
        }
    };
    if parsed.switch("version") {
        let _ = writeln!(io.out, "anydash {}", crate::core::VERSION);
        return 0;
    }
    if parsed.switch("help") {
        let _ = write!(io.out, "{USAGE}");
        return 0;
    }
    if parsed.words.is_empty() {
        let _ = write!(io.err, "{USAGE}");
        return 2;
    }
    let color = output_color(&parsed, &env.settings, io.tty);
    match execute(&parsed, env, io, color).await {
        Ok(result) => {
            if parsed.json {
                let _ = writeln!(io.out, "{}", json!({"ok": true, "result": result}));
            }
            0
        }
        Err(f) => {
            let code = if f.reason == ErrorReason::InvalidArgument && f.detail.field == "command" {
                2
            } else {
                1
            };
            fail(io, &f, parsed.json, code)
        }
    }
}
fn fail(io: &mut Io<'_>, f: &Failure, json: bool, code: i32) -> i32 {
    if json {
        let _ = writeln!(io.out, "{}", json!({"ok": false, "error": f.to_json()}));
    } else {
        let wait = f.retry_after.map_or_else(String::new, |w| {
            format!(" Try again in {} seconds.", w.as_secs().max(1))
        });
        let _ = writeln!(
            io.err,
            "{}{wait} (correlation ID {})",
            f.message, f.correlation_id
        );
    }
    code
}
