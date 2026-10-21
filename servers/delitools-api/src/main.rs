// SPDX-License-Identifier: FSL-1.1-ALv2
use delitools_api::{
    Server,
    authority::TenantAuthority,
    config::Config,
    delcore::DelcoreAuthority,
    exec::Executor,
    gateway::Gateway,
    platform::Platform,
    store::{
        self, Store,
        audit::{self, EventCategory, Severity},
    },
    usage::{DeliveryTask, PriceSyncTask, ReconciliationTask, UsageService},
    worker::Worker,
};
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        report_failure(&error, tracing::dispatcher::has_been_set());
        std::process::exit(1);
    }
}

/// Report the error that stopped the process.
///
/// Every error reaching this point was built from operator-safe text: configuration errors name
/// the variable but never its value, database failures are reduced to "database unavailable", and
/// configured URLs cannot carry credentials. The alternate format keeps the cause chain so
/// operators can tell configuration, database, identity-provider, bind and serving failures apart.
///
/// Logging setup itself can fail (an unsupported `LOG_FORMAT`, an OTLP exporter that cannot be
/// built). No subscriber exists then, so a `tracing` event would vanish and the process would exit
/// without a diagnostic. That case writes the same fields as one JSON line to stderr instead.
fn report_failure(error: &anyhow::Error, logging_installed: bool) {
    if logging_installed {
        tracing::error!(
            category = FAILURE_CATEGORY,
            error = %failure_chain(error),
            "delitools-api stopped"
        );
    } else {
        eprintln!("{}", unlogged_failure_line(error));
    }
}

/// The error and its causes joined like the alternate `anyhow` format, with each HTTP client
/// error's request URL reduced to its origin. A configured or discovered endpoint can carry a
/// signed query parameter or a path token, and `reqwest` repeats the full URL in its message.
fn failure_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| {
            let text = cause.to_string();
            match cause
                .downcast_ref::<reqwest::Error>()
                .and_then(reqwest::Error::url)
            {
                Some(url) => text.replace(url.as_str(), &url.origin().ascii_serialization()),
                None => text,
            }
        })
        .collect::<Vec<_>>()
        .join(": ")
}

const FAILURE_CATEGORY: &str = "startup_or_runtime_failure";

fn unlogged_failure_line(error: &anyhow::Error) -> String {
    serde_json::json!({
        "level": "ERROR",
        "message": "delitools-api stopped",
        "category": FAILURE_CATEGORY,
        "error": failure_chain(error),
    })
    .to_string()
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve,
    Migrate,
    CheckSchema,
}

fn command(args: impl Iterator<Item = String>) -> anyhow::Result<Command> {
    let mut command = None;
    for arg in args {
        // Accepted for launcher compatibility with the other Rust APIs; the working-directory
        // `.env` is always read and never overrides process variables.
        if arg == "--dev-env" {
            continue;
        }
        anyhow::ensure!(command.is_none(), "unexpected command argument");
        command = Some(match arg.as_str() {
            "serve" => Command::Serve,
            "migrate" | "-migrate" | "--migrate" => Command::Migrate,
            "check-schema" | "--check-schema" => Command::CheckSchema,
            _ => anyhow::bail!("unknown command"),
        });
    }
    Ok(command.unwrap_or(Command::Serve))
}

async fn run() -> anyhow::Result<()> {
    // Telemetry reads the process environment directly, so publish the optional `.env` first. A
    // read error is reported once logging exists.
    let local_environment = delitools_api::config::export_local_environment();
    let telemetry = server_common::telemetry::init()?;
    local_environment.inspect_err(|error| {
        tracing::error!(category = "invalid_configuration", %error, "configuration rejected");
    })?;
    let command = command(std::env::args().skip(1))?;
    // The operator commands need only DATABASE_URL.
    if command != Command::Serve {
        let url = Config::database_url_only().inspect_err(|error| {
            tracing::error!(category = "invalid_configuration", %error, "configuration rejected");
        })?;
        let store = Store::open(&url).await?;
        let result = match command {
            Command::Migrate => store.migrate().await.map(|()| {
                tracing::info!("migrations complete");
            }),
            _ => store.check_schema().await,
        };
        store.pool.close().await;
        return result;
    }
    let config = Arc::new(Config::load().inspect_err(|error| {
        tracing::error!(category = "invalid_configuration", %error, "configuration rejected");
    })?);
    let store = tokio::time::timeout(config.timeout(), Store::open(&config.database_url))
        .await
        .map_err(|_| anyhow::anyhow!("database connection timed out"))??;
    store.check_schema().await?;
    let mut lock = store.acquire_instance().await?;

    // Seed the stored settings from the environment exactly once; afterwards the database copy is
    // authoritative and changes only through the audited UpdateSettings RPC.
    let now = store::now();
    let seeded = store::settings::seed(&store.pool, &config.initial_settings, now)
        .await
        .map_err(|e| anyhow::anyhow!("settings could not be initialized: {e}"))?;
    let settings = store::settings::load(&store.pool)
        .await
        .map_err(|e| anyhow::anyhow!("settings could not be loaded: {e}"))?;
    anyhow::ensure!(
        settings.validate().is_ok(),
        "stored settings are invalid; correct them through UpdateSettings after a manual repair"
    );
    let correlation = uuid::Uuid::now_v7();
    if seeded {
        audit::record_event(
            &store.pool,
            EventCategory::SettingsSeeded,
            Severity::Info,
            None,
            correlation,
            serde_json::json!({}),
        )
        .await;
    }
    // A processing row at startup belongs to a process that no longer exists. Mark it unknown so
    // deduplication reports an unknown outcome and an AI prompt is never repeated automatically.
    let interrupted = store::recovery::mark_interrupted_unknown(&store.pool, now).await?;
    if interrupted > 0 {
        tracing::warn!(interrupted, "interrupted requests marked unknown");
        audit::record_event(
            &store.pool,
            EventCategory::InterruptedRequestsRecovered,
            Severity::Warn,
            None,
            correlation,
            serde_json::json!({"requests": interrupted}),
        )
        .await;
    }

    let verifier = delitools_api::auth::Verifier::new(
        &config.issuer,
        &config.audience,
        &config.operator_role,
        config.timeout(),
    )
    .await?;
    // Tenant authorization, billing checks and usage reporting all go through delcore with the
    // service's machine-to-machine identity. An unreachable delcore fails new paid work closed.
    let delcore = Arc::new(DelcoreAuthority::new(&config, config.timeout())?);
    let authority: Arc<dyn TenantAuthority> = delcore.clone();
    let platform = Platform::new(store.clone(), config.clone(), verifier, authority, settings);
    let gateway = Arc::new(Gateway::new(
        &config.gateway_url,
        &config.gateway_key,
        config.max_gateway_response_bytes,
        config.timeout(),
    )?);
    let usage = UsageService::new(
        platform.clone(),
        gateway.clone(),
        delcore.clone(),
        config.service_id,
    );
    let executor = Executor::new(platform.clone(), gateway.clone(), usage.clone());
    let server = Arc::new(Server::with_execution(platform.clone(), executor));
    let mut worker = Worker::new(platform.clone());
    worker.register(Arc::new(DeliveryTask(usage.clone())));
    worker.register(Arc::new(ReconciliationTask(usage)));
    worker.register(Arc::new(PriceSyncTask { gateway }));
    let worker = Arc::new(worker);
    let app = delitools_api::http::router(server);
    let listener = tokio::net::TcpListener::bind(&config.listen).await?;

    let (stop, stopped) = tokio::sync::watch::channel(false);
    let mut worker_task = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run(stopped).await }
    });
    let (lost_tx, lost_rx) = tokio::sync::oneshot::channel();
    let monitor = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if !matches!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    sqlx::query("SELECT 1").execute(&mut lock)
                )
                .await,
                Ok(Ok(_))
            ) {
                let _ = lost_tx.send(());
                break;
            }
        }
    });
    audit::record_event(
        &store.pool,
        EventCategory::Startup,
        Severity::Info,
        None,
        correlation,
        serde_json::json!({"schemaVersion": store::SCHEMA_VERSION}),
    )
    .await;

    let mut failure = None;
    let mut worker_finished = false;
    let shutdown = async {
        tokio::select! {
            _ = server_common::shutdown() => {},
            _ = lost_rx => {
                tracing::error!(category = "instance_lock_lost", "instance lock session lost");
                failure = Some(anyhow::anyhow!("instance lock session lost"));
            },
            result = &mut worker_task => {
                worker_finished = true;
                failure = Some(match result {
                    Ok(Err(error)) => error,
                    _ => anyhow::anyhow!("background worker stopped"),
                });
            },
        }
        let _ = stop.send(true);
    };
    tracing::info!(address = %config.listen, "delitools-api listening");
    let served = delitools_api::worker::stop_after(
        &stop,
        server_common::transport::serve(
            listener,
            app,
            // Sized for the longest live execution timeout, not the startup value.
            config.transport_limits(),
            shutdown,
        ),
    )
    .await;
    // Graceful shutdown: connections drained above; now let background runs finish.
    if !worker_finished {
        let deadline = config.timeout()
            + Duration::from_secs(u64::from(platform.settings().execution_timeout_seconds));
        match tokio::time::timeout(deadline, &mut worker_task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => failure = Some(error),
            Ok(Err(error)) => failure = Some(error.into()),
            Err(_) => {
                worker_task.abort();
                let _ = worker_task.await;
                failure = Some(anyhow::anyhow!("worker shutdown deadline exceeded"));
            }
        }
    }
    monitor.abort();
    let _ = monitor.await;
    store.pool.close().await;
    if let Some(provider) = telemetry {
        provider.shutdown()?
    }
    served?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_before_logging_exists_is_written_as_a_json_line() {
        let error = anyhow::anyhow!("LOG_FORMAT must be text or json").context("telemetry");
        let line: serde_json::Value = serde_json::from_str(&unlogged_failure_line(&error)).unwrap();
        assert_eq!(line["category"], FAILURE_CATEGORY);
        assert_eq!(line["level"], "ERROR");
        assert_eq!(line["error"], "telemetry: LOG_FORMAT must be text or json");
    }

    /// A failed identity-provider request must not write the endpoint's query or path to the log.
    #[tokio::test]
    async fn failure_reports_never_repeat_request_urls() {
        let client = reqwest::Client::new();
        let url = "http://127.0.0.1:9/oidc/jwks?X-Signature=top-secret-signature";
        let cause = client.get(url).send().await.unwrap_err();
        assert!(cause.to_string().contains("top-secret-signature"));
        let error = anyhow::Error::new(cause).context("startup");
        let text = failure_chain(&error);
        assert!(text.starts_with("startup: "), "{text}");
        assert!(text.contains("http://127.0.0.1:9"), "{text}");
        assert!(!text.contains("top-secret-signature"), "{text}");
        assert!(!text.contains("/oidc/jwks"), "{text}");
        let line = unlogged_failure_line(&error);
        assert!(!line.contains("top-secret-signature"), "{line}");
    }

    #[test]
    fn operator_command_compatibility() {
        for flag in ["migrate", "-migrate", "--migrate"] {
            assert_eq!(
                command([flag.to_owned()].into_iter()).unwrap(),
                Command::Migrate
            );
        }
        for flag in ["check-schema", "--check-schema"] {
            assert_eq!(
                command([flag.to_owned()].into_iter()).unwrap(),
                Command::CheckSchema
            );
        }
        assert!(command(["serve".into(), "migrate".into()].into_iter()).is_err());
        assert!(command(["unknown".into()].into_iter()).is_err());
        assert_eq!(command(std::iter::empty()).unwrap(), Command::Serve);
        assert_eq!(
            command(["--dev-env".into()].into_iter()).unwrap(),
            Command::Serve
        );
    }
}
