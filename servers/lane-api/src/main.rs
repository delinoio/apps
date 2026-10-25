// SPDX-License-Identifier: FSL-1.1-ALv2
use lane_api::{Server, auth::Auth, config::Config, push::Push, webhooks::WebhookConfig};
use std::{sync::Arc, time::Duration};
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        tracing::error!(
            category = "startup",
            "Lane stopped; check configuration and dependencies"
        );
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
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&server_common::config::required("DATABASE_URL")?)
        .await?;
    if command == "migrate" {
        lane_api::migration::migrate(&db).await?;
        return Ok(());
    }
    lane_api::migration::check(&db).await?;
    if command == "check-schema" {
        return Ok(());
    }
    let config = Config::from_env()?;
    let attachments = lane_api::capture::ObjectStore::from_env()?;
    let auth = Arc::new(Auth::from_env(db.clone()).await?);
    let push = Arc::new(Push::from_env(db.clone())?);
    let webhooks = Arc::new(WebhookConfig::from_env()?);
    let server = Arc::new(Server {
        db: db.clone(),
        auth,
        config: Arc::new(config.clone()),
        metrics: Arc::default(),
        push,
        webhooks,
    });
    let app = lane_api::http::router(server.clone());
    let worker = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            if lane_api::capture::tick(&server.db, &attachments)
                .await
                .is_err()
            {
                tracing::warn!(category = "attachments", "attachment work remains pending");
            }
            if server.push.tick().await.is_err() {
                tracing::warn!(category = "push", "push work remains pending");
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    tracing::info!(address=%config.listen,"Lane listening");
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
