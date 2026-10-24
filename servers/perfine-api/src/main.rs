// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use delino_protocol::perfine::v1::*;
use perfine_api::{Server, auth::Actions, github::GitHub, storage::R2};
use server_common::{authority::Authority, config::required};
use std::{sync::Arc, time::Duration};
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        tracing::error!("Perfine stopped; verify service configuration and dependencies");
        std::process::exit(1);
    }
}
fn positive(name: &str) -> anyhow::Result<i64> {
    let n: i64 = required(name)?.parse()?;
    anyhow::ensure!(n > 0, "{name} must be positive");
    Ok(n)
}
async fn run() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--dev-env") {
        server_common::config::load_dev_env()?;
    }
    let provider = server_common::telemetry::init()?;
    let command = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .unwrap_or("serve".into());
    if command == "alerts" {
        print!(
            "{}",
            perfine_api::alerts::rules(|k| std::env::var(k).unwrap_or_default())?
        );
        return Ok(());
    }
    anyhow::ensure!(
        matches!(command.as_str(), "serve" | "migrate" | "check-schema"),
        "use serve, migrate or check-schema"
    );
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .connect(&required("DATABASE_URL")?)
        .await
        .map_err(|_| anyhow::anyhow!("database unavailable"))?;
    if command == "migrate" {
        sqlx::raw_sql(include_str!("../migrations/00001_perfine.sql"))
            .execute(&db)
            .await?;
        return Ok(());
    }
    let ready: bool = sqlx::query_scalar("SELECT to_regclass('perfine_cleanup') IS NOT NULL")
        .fetch_one(&db)
        .await?;
    anyhow::ensure!(ready, "run explicit migrations before serving");
    if command == "check-schema" {
        return Ok(());
    }
    let quota = positive("SPACE_STORAGE_QUOTA_BYTES")?;
    let upload_limit = positive("SINGLE_UPLOAD_LIMIT_BYTES")? as u64;
    let rate = positive("REQUESTS_PER_MINUTE")?;
    positive("CONCURRENT_CLOUD_EXECUTIONS")?;
    positive("ALERT_WINDOW_SECONDS")?;
    positive("ALERT_FAILURE_THRESHOLD")?;
    positive("ALERT_CLEANUP_AGE_SECONDS")?;
    let origin = required("WEB_ORIGIN")?;
    let _: HeaderValue = origin.parse()?;
    let server = Arc::new(Server {
        metrics: Default::default(),
        db: db.clone(),
        authority: Authority::from_env(Duration::from_secs(15))?,
        actions: Actions::discover().await?,
        github: GitHub::from_env()?,
        storage: R2::from_env()?,
        quota,
        upload_limit,
        rate,
    });
    let rpc = connectrpc::Router::new()
        .add_service::<_, AccessServiceRegisterMarker>(server.clone())
        .add_service::<_, ConnectionServiceRegisterMarker>(server.clone())
        .add_service::<_, ExperimentServiceRegisterMarker>(server.clone())
        .add_service::<_, EvidenceServiceRegisterMarker>(server.clone())
        .add_service::<_, ManagementServiceRegisterMarker>(server.clone())
        .add_service::<_, PublicationServiceRegisterMarker>(server.clone())
        .add_service::<_, CloudServiceRegisterMarker>(server.clone())
        .into_axum_service()
        .with_limits(
            connectrpc::Limits::default()
                .with_max_message_size(8 << 20)
                .with_max_request_body_size(8 << 20),
        );
    let ready = db.clone();
    let metrics = db.clone();
    let counters = server.clone();
    let app=Router::new().route("/healthz",get(||async{StatusCode::OK}))
 .route("/readyz",get(move||{let db=ready.clone();async move{if sqlx::query("SELECT 1").execute(&db).await.is_ok(){StatusCode::OK}else{StatusCode::SERVICE_UNAVAILABLE}}}))
 .route("/metrics",get(move||{let db=metrics.clone();let counters=counters.clone();async move{
  let counts=sqlx::query_as::<_,(i64,i64,i64,i64)>("SELECT (SELECT count(*) FROM perfine_cleanup WHERE data->>'state'<>'DELETION_STATE_COMPLETE'),(SELECT COALESCE(sum(reserved_bytes),0)::bigint FROM perfine_uploads),(SELECT count(*) FROM perfine_approvals WHERE publication->>'state'='PUBLICATION_STATE_RECONCILING'),(SELECT COALESCE(extract(epoch FROM now()-min(created_at)),0)::bigint FROM perfine_cleanup WHERE data->>'state'<>'DELETION_STATE_COMPLETE')").fetch_one(&db).await;
  match counts {Ok((cleanup,storage,publish,age))=>(StatusCode::OK,format!("{}perfine_cleanup_oldest_seconds {age}\nperfine_cleanup_pending {cleanup}\nperfine_storage_reserved_bytes {storage}\nperfine_publication_reconciling {publish}\n",counters.metrics.encode())),Err(_)=>(StatusCode::SERVICE_UNAVAILABLE,String::new())}
 }})).fallback_service(rpc).layer(middleware::from_fn_with_state(origin,http_policy));
    let maintenance = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if server.maintain().await.is_err() {
                tracing::error!(
                    category = "maintenance",
                    "Perfine maintenance failed; work remains durable"
                );
            }
        }
    });
    let listen = std::env::var("LISTEN").unwrap_or("127.0.0.1:46323".into());
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(address = listen, "Perfine listening");
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
    maintenance.abort();
    let _ = maintenance.await;
    db.close().await;
    if let Some(provider) = provider {
        provider.shutdown()?;
    }
    Ok(())
}
async fn http_policy(State(origin): State<String>, req: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::now_v7().to_string();
    let allowed = req
        .headers()
        .get("origin")
        .is_none_or(|v| v == origin.as_str());
    let mut response = if !allowed {
        StatusCode::FORBIDDEN.into_response()
    } else if req.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        match tokio::time::timeout(Duration::from_secs(60), next.run(req)).await {
            Ok(r) => r,
            Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
        }
    };
    let headers = response.headers_mut();
    headers
        .entry("x-request-id")
        .or_insert(request_id.parse().unwrap());
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    if allowed {
        headers.insert("access-control-allow-origin", origin.parse().unwrap());
        headers.insert("vary", HeaderValue::from_static("Origin"));
        headers.insert(
            "access-control-allow-methods",
            HeaderValue::from_static("POST, OPTIONS"),
        );
        headers.insert(
            "access-control-allow-headers",
            HeaderValue::from_static(
                "authorization, content-type, connect-protocol-version, x-github-token",
            ),
        );
        headers.insert(
            "access-control-expose-headers",
            HeaderValue::from_static("x-request-id"),
        );
    }
    response
}
