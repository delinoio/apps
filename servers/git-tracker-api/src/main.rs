// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{
    Router,
    body::Bytes,
    extract::DefaultBodyLimit,
    http::{HeaderMap, StatusCode},
    middleware,
    routing::{get, post},
};
use git_tracker_api::{Server, auth::Actions, github::GitHub};
use server_common::{
    authority::Authority,
    config::{duration, required},
    http::Policy,
    telemetry,
};
use std::{sync::Arc, time::Duration};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        tracing::error!(%error,"git-tracker-api stopped");
        eprintln!("git-tracker-api: {error}");
        std::process::exit(1)
    }
}
async fn run() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--dev-env") {
        server_common::config::load_dev_env()?
    }
    let provider = telemetry::init()?;
    let command = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    if command == "alerts" {
        print!(
            "{}",
            git_tracker_api::alerts::rules(|name| std::env::var(name).unwrap_or_default())?
        );
        return Ok(());
    }
    anyhow::ensure!(
        matches!(command.as_str(), "serve" | "migrate"),
        "unknown command"
    );
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .connect(&required("DATABASE_URL")?)
        .await
        .map_err(|_| anyhow::anyhow!("database unavailable"))?;
    if command == "migrate" {
        git_tracker_api::schema::migrate(&db).await?;
        tracing::info!("migrations complete");
        return Ok(());
    }
    required("AUDIT_RETENTION")?;
    let retention = duration("AUDIT_RETENTION", Duration::from_secs(1))?;
    let origin = required("WEB_ORIGIN")?;
    let server = Arc::new(Server {
        db: db.clone(),
        authority: Authority::from_env(Duration::from_secs(30))?,
        github: GitHub::from_env()?,
        actions: Actions::discover().await?,
        audit_retention: chrono::Duration::from_std(retention)?,
        webhook_secret: required("WEBHOOK_SECRET")?,
        metrics: Arc::new(telemetry::Metrics::new(telemetry::Service::GitTracker)),
    });
    let rpc = connectrpc::Router::new()
        .add_service(server.clone())
        .into_axum_service()
        .with_limits(
            connectrpc::Limits::default()
                .with_max_message_size(8 << 20)
                .with_max_request_body_size(8 << 20),
        );
    let webhook = server.clone();
    // Wakes the apply task as soon as a delivery is stored; the per-minute cycle is the fallback.
    let stored = Arc::new(tokio::sync::Notify::new());
    let webhook_stored = stored.clone();
    let ready = db.clone();
    let metrics = server.metrics.clone();
    let app = Router::new()
        .route("/healthz", get(|| async { StatusCode::OK }))
        .route(
            "/readyz",
            get(move || {
                let db = ready.clone();
                async move {
                    // Ready only when the database holds exactly this binary's migrations.
                    match git_tracker_api::schema::check(&db).await {
                        Ok(()) => StatusCode::OK,
                        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
                    }
                }
            }),
        )
        .route(
            "/metrics",
            get(move || {
                let metrics = metrics.clone();
                async move {
                    (
                        [("content-type", "text/plain; version=0.0.4")],
                        metrics.encode(),
                    )
                }
            }),
        )
        .route(
            "/github/events",
            post(move |headers: HeaderMap, body: Bytes| {
                let server = webhook.clone();
                let stored = webhook_stored.clone();
                async move {
                    let event = headers
                        .get("X-GitHub-Event")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    let signature = headers
                        .get("X-Hub-Signature-256")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    let delivery = headers
                        .get("X-GitHub-Delivery")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    match server.webhook(event, delivery, signature, &body).await {
                        Ok(()) => {
                            stored.notify_one();
                            StatusCode::NO_CONTENT
                        }
                        Err(f) => {
                            server.record_failure(&f);
                            match f.code {
                                connectrpc::ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
                                connectrpc::ErrorCode::InvalidArgument => StatusCode::BAD_REQUEST,
                                _ => StatusCode::SERVICE_UNAVAILABLE,
                            }
                        }
                    }
                }
            })
            .layer(DefaultBodyLimit::max(2 << 20)),
        )
        .fallback_service(rpc)
        .layer(middleware::from_fn_with_state(
            Policy {
                origin,
                timeout: Duration::from_secs(60),
                service: telemetry::Service::GitTracker,
                metrics: server.metrics.clone(),
                procedure: None,
            },
            server_common::http::middleware,
        ))
        // Outermost layer: stamp the request before any body bytes are consumed so the optional
        // publication cutoff covers body delivery as well as handler work.
        .layer(middleware::from_fn(git_tracker_api::capture_ingress));
    let listen = std::env::var("LISTEN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1:46321".into());
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(address = listen, "git-tracker-api listening");
    let applier = server.clone();
    let apply = tokio::spawn(async move {
        loop {
            stored.notified().await;
            if let Err(fault) = applier.apply_webhooks().await {
                applier.record_failure(&fault);
                tracing::error!(category = "persistence", "webhook apply failed")
            }
        }
    });
    let maintenance = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(fault) = server.maintain().await {
                server.record_failure(&fault);
                tracing::error!(category = "persistence", "maintenance failed")
            }
        }
    });
    server_common::transport::serve(
        listener,
        app,
        server_common::transport::Limits {
            header: Duration::from_secs(10),
            idle: Duration::from_secs(60),
            write: Duration::from_secs(60),
            shutdown: Duration::from_secs(60),
        },
        server_common::shutdown(),
    )
    .await?;
    maintenance.abort();
    let _ = maintenance.await;
    apply.abort();
    let _ = apply.await;
    db.close().await;
    if let Some(provider) = provider {
        provider.shutdown()?
    }
    Ok(())
}
