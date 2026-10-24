// SPDX-License-Identifier: FSL-1.1-ALv2
//! Audit expiration boundaries. Maintenance of empty repository tables makes no provider calls, so
//! the clients below point at an unreachable loopback address and are never used.
use chrono::{DateTime, Utc};
use git_tracker_api::{Server, auth::Actions, github::GitHub};
use jsonwebtoken::{EncodingKey, jwk::JwkSet};
use serde_json::json;
use server_common::{
    authority::Authority,
    telemetry::{Metrics, Service},
};
use std::{sync::Arc, time::Duration};
const KEY: &[u8] = include_bytes!("fixtures/github-test-key.pem");

struct Fixture {
    server: Server,
    db: sqlx::PgPool,
    admin: sqlx::PgPool,
    schema: String,
}

async fn fixture(retention: chrono::Duration) -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("tracker_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |c, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {search}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/00001_git_tracker.sql"))
        .execute(&db)
        .await
        .unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let base = "http://127.0.0.1:9".to_string();
    let jwks: JwkSet = serde_json::from_value(json!({"keys": []})).unwrap();
    let server = Server {
        db: db.clone(),
        authority: Authority::new(
            http.clone(),
            base.clone(),
            format!("{base}/token"),
            "resource".into(),
            "client".into(),
            "fixture".into(),
        ),
        github: GitHub::new(
            http.clone(),
            base.clone(),
            "1".into(),
            EncodingKey::from_rsa_pem(KEY).unwrap(),
        ),
        actions: Actions::new(http, format!("{base}/jwks"), jwks),
        audit_retention: retention,
        webhook_secret: "fixture-only".into(),
        metrics: Arc::new(Metrics::new(Service::GitTracker)),
    };
    Fixture {
        server,
        db,
        admin,
        schema,
    }
}

impl Fixture {
    async fn insert_audit(&self, created_at: DateTime<Utc>) -> uuid::Uuid {
        let id = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO git_tracker_audit VALUES($1,$2,'fixture',$3,$4)")
            .bind(id)
            .bind(uuid::Uuid::now_v7())
            .bind(uuid::Uuid::now_v7())
            .bind(created_at)
            .execute(&self.db)
            .await
            .unwrap();
        id
    }
    async fn present(&self, id: uuid::Uuid) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM git_tracker_audit WHERE id=$1")
            .bind(id)
            .fetch_one(&self.db)
            .await
            .unwrap()
            == 1
    }
    async fn close(self) {
        let Fixture {
            server,
            db,
            admin,
            schema,
        } = self;
        drop(server);
        db.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}

fn at(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339).unwrap().to_utc()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn unrepresentable_retention_keeps_audit_rows_across_cycles() {
    let retention = chrono::Duration::from_std(Duration::from_secs(9_999_999_999 * 3600)).unwrap();
    let f = fixture(retention).await;
    let ancient = f.insert_audit(at("2000-01-01T00:00:00Z")).await;
    let recent = f.insert_audit(server_common::now()).await;
    f.server.maintain().await.unwrap();
    f.server.maintain().await.unwrap();
    assert!(f.present(ancient).await);
    assert!(f.present(recent).await);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn ninety_day_retention_deletes_strictly_older_rows_at_a_fixed_instant() {
    let f = fixture(chrono::Duration::from_std(Duration::from_secs(2160 * 3600)).unwrap()).await;
    let cutoff = at("2026-07-24T00:00:00Z");
    let micro = chrono::Duration::microseconds(1);
    let before = f.insert_audit(cutoff - micro).await;
    let exact = f.insert_audit(cutoff).await;
    let after = f.insert_audit(cutoff + micro).await;
    f.server
        .maintain_at(at("2026-10-22T00:00:00Z"))
        .await
        .unwrap();
    assert!(!f.present(before).await);
    assert!(f.present(exact).await);
    assert!(f.present(after).await);
    f.close().await;
}
