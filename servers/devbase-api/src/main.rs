// SPDX-License-Identifier: FSL-1.1-ALv2
use devbase_api::{Server, config::Config, store};
use server_common::{authority::Authority, config::required, telemetry};
use sqlx::Row;
use std::{sync::Arc, time::Duration};
#[tokio::main]
async fn main() {
    let result = run().await;
    if let Err(error) = result {
        tracing::error!(%error,"devbase-api stopped");
        eprintln!("devbase-api: {error}");
        std::process::exit(1)
    }
}
async fn run() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--dev-env") {
        server_common::config::load_dev_env()?
    }
    let provider = telemetry::init()?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("serve");
    anyhow::ensure!(
        matches!(command, "serve" | "migrate" | "check-schema" | "audit"),
        "unknown command"
    );
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .connect(&required("DATABASE_URL")?)
        .await
        .map_err(|_| anyhow::anyhow!("database unavailable"))?;
    match command {
        "migrate" => {
            store::migrate(&db).await?;
            tracing::info!("migrations complete");
            return Ok(());
        }
        "check-schema" => {
            store::check(&db).await?;
            return Ok(());
        }
        "audit" => return audit(&db, &args[1..]).await,
        _ => {}
    }
    let config = Config::load()?;
    store::check(&db).await?;
    // The dedicated connection is detached so dropping it closes the session lock.
    let mut lock = db.acquire().await?.detach();
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(41830002)")
        .fetch_one(&mut lock)
        .await?;
    anyhow::ensure!(acquired, "devbase instance lock unavailable");
    let authority = Authority::from_env(config.dependency_timeout)?;
    let server = Arc::new(Server::new(db.clone(), authority, config));
    let app = devbase_api::http::router(server.clone());
    let listener = tokio::net::TcpListener::bind(&server.config.listen).await?;
    tracing::info!(address = server.config.listen, "devbase-api listening");
    let worker = server.clone();
    let maintenance = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(worker.config.cleanup_interval);
        loop {
            ticker.tick().await;
            if worker.maintain().await.is_err() {
                worker.metrics.cleanup_failures.inc();
                tracing::warn!(category = "maintenance", "maintenance failed");
            }
        }
    });
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let monitor = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let ping = tokio::time::timeout(
                Duration::from_secs(1),
                sqlx::query("SELECT 1").execute(&mut lock),
            )
            .await;
            if !matches!(ping, Ok(Ok(_))) {
                let _ = stop_tx.send(());
                break;
            }
        }
    });
    let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lost_signal = lost.clone();
    let shutdown = async move {
        tokio::select! {_=server_common::shutdown()=>{},_=stop_rx=>{lost_signal.store(true,std::sync::atomic::Ordering::Relaxed);tracing::error!("instance lock session lost");}}
    };
    server_common::transport::serve(
        listener,
        app,
        server_common::transport::Limits {
            header: Duration::from_secs(5),
            idle: Duration::from_secs(60),
            write: server.config.request_timeout + Duration::from_secs(5),
            shutdown: server.config.request_timeout,
        },
        shutdown,
    )
    .await?;
    maintenance.abort();
    let _ = maintenance.await;
    monitor.abort();
    let _ = monitor.await;
    db.close().await;
    if let Some(provider) = provider {
        provider.shutdown()?
    }
    anyhow::ensure!(
        !lost.load(std::sync::atomic::Ordering::Relaxed),
        "instance lock session lost"
    );
    Ok(())
}
async fn audit(db: &sqlx::PgPool, args: &[String]) -> anyhow::Result<()> {
    let mut from = chrono::Utc::now() - chrono::Duration::hours(24);
    let mut target = String::new();
    let mut actor = String::new();
    let mut limit = 100_i64;
    let mut i = 0;
    while i < args.len() {
        let (flag, value, step) = if let Some((flag, value)) = args[i].split_once('=') {
            (flag, value, 1)
        } else {
            (
                args[i].as_str(),
                args.get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("missing flag value"))?
                    .as_str(),
                2,
            )
        };
        match flag.trim_start_matches('-') {
            "since" => {
                from = chrono::DateTime::parse_from_rfc3339(value)?.with_timezone(&chrono::Utc)
            }
            "target" => {
                uuid::Uuid::parse_str(value)?;
                target = value.into()
            }
            "actor" => actor = value.into(),
            "limit" => limit = value.parse()?,
            _ => anyhow::bail!("unknown audit flag"),
        };
        i += step;
    }
    anyhow::ensure!((1..=10000).contains(&limit), "--limit must be 1 to 10000");
    let rows=sqlx::query("SELECT occurred_at,action,actor,target_id,trace_id FROM audit_events WHERE occurred_at>=$1 AND ($2='' OR target_id=NULLIF($2,'')::uuid) AND ($3='' OR actor=$3) ORDER BY occurred_at DESC,id DESC LIMIT $4").bind(from).bind(target).bind(actor).bind(limit).fetch_all(db).await?;
    for row in rows {
        println!(
            "{}",
            serde_json::json!({"occurred_at":row.get::<chrono::DateTime<chrono::Utc>,_>("occurred_at"),"action":row.get::<String,_>("action"),"actor":row.get::<String,_>("actor"),"target_id":row.get::<uuid::Uuid,_>("target_id"),"trace_id":row.get::<String,_>("trace_id")})
        );
    }
    Ok(())
}
