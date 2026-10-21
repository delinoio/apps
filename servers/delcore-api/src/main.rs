// SPDX-License-Identifier: FSL-1.1-ALv2
use delcore_api::{Server, auth, config::Config, postgres, worker::Worker};
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        tracing::error!(category = "startup_or_runtime_failure", "delcore stopped");
        std::process::exit(1);
    }
}
async fn run() -> anyhow::Result<()> {
    let telemetry = server_common::telemetry::init()?;
    let command = command(std::env::args().skip(1))?;
    let config = Config::load().map_err(|error| {
        tracing::error!(category="invalid_configuration",%error,"configuration rejected");
        error
    })?;
    let store = tokio::time::timeout(
        config.timeout(),
        postgres::Store::open(&config.database_url),
    )
    .await??;
    match command {
        Command::Migrate => {
            store.migrate().await?;
            tracing::info!("migrations complete");
            return Ok(());
        }
        Command::CheckSchema => return store.check_schema().await,
        Command::Serve => {}
    }
    store.check_schema().await?;
    let mut lock = store.acquire_instance().await?;
    let metrics = Arc::new(delcore_api::telemetry::Metrics::default());
    if config.development {
        delcore_api::catalog::sync(
            &store.pool,
            std::path::Path::new(&config.catalog_dir),
            &metrics,
        )
        .await?;
    }
    // A registered DeliTools service needs the explicit switch and its reporting
    // windows. Check after catalog synchronization, before serving.
    {
        let mut connection = store.pool.acquire().await?;
        delcore_api::delitools::ensure_configured(&mut connection, &config).await?;
    }
    let verifier = auth::Verifier::new(&config).await?;
    let mut server = Server::new(store.pool.clone(), verifier, config.clone());
    server.metrics = metrics.clone();
    let server = Arc::new(server);
    let mut worker = Worker::new(store.pool.clone(), config.clone())?;
    worker.metrics = metrics.clone();
    let worker = Arc::new(worker);
    let app = delcore_api::http::router(server, worker.clone());
    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let mut worker_task = tokio::spawn(async move { worker.run(stopped).await });
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
    let sample_pool = store.pool.clone();
    let sample_config = config.clone();
    let sampler = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let _ = tokio::time::timeout(
                sample_config.timeout(),
                metrics.sample_jobs(
                    &sample_pool,
                    sample_config.backlog_alert,
                    sample_config.settlement_alert_seconds as f64,
                ),
            )
            .await;
        }
    });
    let mut failure = None;
    let mut worker_finished = false;
    let shutdown = async {
        tokio::select! {
            _=server_common::shutdown()=>{},
            _=lost_rx=>{tracing::error!(category="instance_lock_lost","instance lock session lost");failure=Some(anyhow::anyhow!("instance lock session lost"));},
            result=&mut worker_task=>{worker_finished=true;failure=Some(match result {Ok(Err(error))=>error,_=>anyhow::anyhow!("background worker stopped")});},
        }
        let _ = stop.send(true);
    };
    tracing::info!("delcore started");
    let served = server_common::transport::serve(
        listener,
        app,
        server_common::transport::Limits {
            header: config.timeout(),
            idle: Duration::from_secs(60),
            write: config.timeout(),
            shutdown: config.timeout(),
        },
        shutdown,
    )
    .await;
    let _ = stop.send(true);
    if !worker_finished {
        match tokio::time::timeout(config.timeout(), &mut worker_task).await {
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
    sampler.abort();
    let _ = sampler.await;
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
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve,
    Migrate,
    CheckSchema,
}
fn command(args: impl Iterator<Item = String>) -> anyhow::Result<Command> {
    let mut command = None;
    for arg in args {
        if arg == "--dev-env" {
            continue;
        }
        anyhow::ensure!(command.is_none(), "unexpected command argument");
        command = Some(match arg.as_str() {
            "serve" => Command::Serve,
            "migrate" | "-migrate" | "--migrate" => Command::Migrate,
            "check-schema" => Command::CheckSchema,
            _ => anyhow::bail!("unknown command"),
        });
    }
    Ok(command.unwrap_or(Command::Serve))
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
        assert!(command(["serve".into(), "migrate".into()].into_iter()).is_err());
        assert_eq!(
            command(["--dev-env".into()].into_iter()).unwrap(),
            Command::Serve
        );
    }
}
