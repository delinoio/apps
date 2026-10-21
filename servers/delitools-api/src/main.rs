// SPDX-License-Identifier: FSL-1.1-ALv2
use delitools_api::{
    Server,
    authority::TenantAuthority,
    config::Config,
    platform::Platform,
    store::{
        self, Store,
        audit::{self, EventCategory, Severity},
    },
    worker::Worker,
};
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        tracing::error!(
            category = "startup_or_runtime_failure",
            "delitools-api stopped"
        );
        std::process::exit(1);
    }
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
    let store = tokio::time::timeout(config.timeout(), Store::open(&config.database_url)).await??;
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
    let authority: Arc<dyn TenantAuthority> = authority(&config)?;
    let platform = Platform::new(store.clone(), config.clone(), verifier, authority, settings);
    let server = Arc::new(Server::new(platform.clone()));
    let worker = Arc::new(Worker::new(platform.clone()));
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
    let served = server_common::transport::serve(
        listener,
        app,
        server_common::transport::Limits {
            header: config.timeout(),
            idle: Duration::from_secs(60),
            write: config.timeout(),
            // Sized for the longest live execution timeout, not the startup value.
            shutdown: config.drain_ceiling(),
        },
        shutdown,
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

/// The delcore-backed tenant authority is installed by the execution layer, once delcore exposes
/// the consumer authorization modes it needs. Until then the service fails closed: every
/// authorization question is answered "unavailable", so key management and paid execution are
/// refused while operator RPCs, health, readiness and metrics keep working.
fn authority(_config: &Config) -> anyhow::Result<Arc<dyn TenantAuthority>> {
    tracing::warn!(
        category = "authority_not_installed",
        "no tenant authority adapter is installed; tenant-scoped RPCs fail closed"
    );
    Ok(Arc::new(delitools_api::authority::FailClosedAuthority))
}

#[cfg(test)]
mod tests {
    use super::*;

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
