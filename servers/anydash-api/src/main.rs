// SPDX-License-Identifier: FSL-1.1-ALv2
use anydash_api::{Server, config::Config, migration};
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        tracing::error!(category = "startup", error = %e, "AnyDash stopped; check configuration and dependencies");
        eprintln!("anydash-api: {e}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--dev-env") {
        server_common::config::load_dev_env()?;
    }
    let telemetry = server_common::telemetry::init()?;
    let command = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .unwrap_or_else(|| "serve".into());
    anyhow::ensure!(
        matches!(command.as_str(), "serve" | "migrate" | "check-schema"),
        "use serve, migrate or check-schema"
    );
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(32)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&server_common::config::required("DATABASE_URL")?)
        .await?;
    if command == "migrate" {
        migration::migrate(&db).await?;
        tracing::info!("AnyDash migrations applied");
        return Ok(());
    }
    migration::check(&db).await?;
    if command == "check-schema" {
        return Ok(());
    }
    let config = Config::from_env()?;
    let listen = config.listen.clone();
    let server = Server::new(db.clone(), config)?;
    let worker = anydash_api::worker::spawn(server.clone());
    let app = anydash_api::http::router(Arc::new(server));
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(address = %listen, "AnyDash listening");
    server_common::transport::serve(
        listener,
        app,
        server_common::transport::Limits {
            header: Duration::from_secs(5),
            idle: Duration::from_secs(60),
            write: Duration::from_secs(65),
            shutdown: Duration::from_secs(60),
        },
        server_common::shutdown(),
    )
    .await?;
    worker.abort();
    let _ = worker.await;
    db.close().await;
    if let Some(telemetry) = telemetry {
        telemetry.shutdown()?;
    }
    Ok(())
}
