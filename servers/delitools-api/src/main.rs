// SPDX-License-Identifier: FSL-1.1-ALv2
//! `delitools-api serve|migrate|check-schema`.
//!
//! `migrate` and `check-schema` need only `DATABASE_URL`. `serve` validates every setting,
//! checks the schema, takes the single-instance advisory lock and then listens. Serving never
//! migrates.

use delitools_api::{Server, config::Config, http, store};
use server_common::{config::required, telemetry};
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        tracing::error!(%error, "delitools-api stopped");
        eprintln!("delitools-api: {error}");
        std::process::exit(1)
    }
}

async fn run() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--dev-env") {
        server_common::config::load_dev_env()?
    }
    let provider = telemetry::init()?;
    let command = std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with("--"))
        .unwrap_or_else(|| "serve".into());
    anyhow::ensure!(
        matches!(command.as_str(), "serve" | "migrate" | "check-schema"),
        "unknown command; use serve, migrate or check-schema"
    );
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .connect(&required("DATABASE_URL")?)
        .await
        .map_err(|_| anyhow::anyhow!("database unavailable"))?;
    match command.as_str() {
        "migrate" => {
            store::migrate(&db).await?;
            tracing::info!("migrations complete");
            return Ok(());
        }
        "check-schema" => {
            store::check(&db).await?;
            tracing::info!("schema is compatible");
            return Ok(());
        }
        _ => {}
    }
    let config = Config::load()?;
    store::check(&db).await?;
    // The dedicated connection is detached so dropping it closes the session lock. One process
    // owns API execution and usage delivery.
    let mut lock = db.acquire().await?.detach();
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(store::INSTANCE_LOCK)
        .fetch_one(&mut lock)
        .await?;
    anyhow::ensure!(acquired, "delitools instance lock unavailable");
    let listen = config.listen.clone();
    let shutdown_grace = config.limits.execution_timeout;
    let dependency_timeout = config.dependency_timeout;
    let server = Server::new(db.clone(), config)?;
    let app = http::router(server.clone());
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(address = listen, "delitools-api listening");
    let delivery = tokio::spawn(server.delivery().run());
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Losing the lock session means a replacement may already own the database. The monitor
    // therefore fences the service and stops delivery itself, immediately and without waiting for
    // the transport drain that an ordinary SIGTERM shutdown performs. The drain that follows
    // ends as soon as the canceled jobs finish; confirmed usage is still stored, bounded by
    // `DEPENDENCY_TIMEOUT`. See the instance-lock rules in the server contract.
    let monitor = {
        let lost = lost.clone();
        let server = server.clone();
        let delivery = delivery.abort_handle();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                let ping = tokio::time::timeout(
                    Duration::from_secs(1),
                    sqlx::query("SELECT 1").execute(&mut lock),
                )
                .await;
                if !matches!(ping, Ok(Ok(_))) {
                    lost.store(true, std::sync::atomic::Ordering::SeqCst);
                    server.fence();
                    delivery.abort();
                    tracing::error!(
                        category = "instance_lock",
                        alert = true,
                        "instance lock session lost"
                    );
                    let _ = stop_tx.send(());
                    break;
                }
            }
        })
    };
    let shutdown = async move {
        tokio::select! {
            _ = server_common::shutdown() => {},
            _ = stop_rx => {},
        }
    };
    server_common::transport::serve(
        listener,
        app,
        server_common::transport::Limits {
            header: Duration::from_secs(5),
            idle: Duration::from_secs(60),
            write: shutdown_grace + Duration::from_secs(5),
            shutdown: shutdown_grace,
        },
        shutdown,
    )
    .await?;
    delivery.abort();
    let _ = delivery.await;
    monitor.abort();
    let _ = monitor.await;
    if lost.load(std::sync::atomic::Ordering::SeqCst) {
        // Closing waits for checked-out connections; a stuck accounting statement must not keep a
        // fenced process alive after its replacement took over.
        let _ = tokio::time::timeout(dependency_timeout, db.close()).await;
    } else {
        db.close().await;
    }
    if let Some(provider) = provider {
        provider.shutdown()?
    }
    anyhow::ensure!(
        !lost.load(std::sync::atomic::Ordering::SeqCst),
        "instance lock session lost"
    );
    Ok(())
}
