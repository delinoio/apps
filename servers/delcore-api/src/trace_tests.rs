// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use std::{collections::BTreeMap, sync::Mutex};
use tracing::{
    Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*, registry::LookupSpan};
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);
#[derive(Default)]
struct Fields(BTreeMap<String, String>);
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
}
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Capture {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        fields
            .0
            .insert("name".into(), attrs.metadata().name().into());
        ctx.span(id).unwrap().extensions_mut().insert(fields);
    }
    fn on_record(&self, id: &Id, record: &Record<'_>, ctx: Context<'_, S>) {
        record.record(
            ctx.span(id)
                .unwrap()
                .extensions_mut()
                .get_mut::<Fields>()
                .unwrap(),
        );
    }
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        self.0.lock().unwrap().push(
            ctx.span(&id)
                .unwrap()
                .extensions()
                .get::<Fields>()
                .unwrap()
                .0
                .clone(),
        );
    }
}
#[tokio::test]
async fn event_spans_include_authentication_rejections() {
    use axum::{Json, Router, routing::get};
    use tracing::instrument::WithSubscriber;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let discovery = json!({"issuer":issuer,"jwks_uri":format!("{issuer}/jwks")});
    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || async { Json(discovery) }),
        )
        .route("/jwks", get(|| async { Json(json!({"keys":[]})) }));
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut values: std::collections::HashMap<_, _> =
        dotenvy::from_read_iter(include_bytes!("../.env.example").as_slice())
            .map(|v| v.unwrap())
            .collect();
    values.insert("ISSUER".into(), issuer);
    let config =
        config::Config::from_getter(|key| values.get(key).cloned().unwrap_or_default()).unwrap();
    let auth = auth::Verifier::new(&config).await.unwrap();
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@127.0.0.1:1/test")
        .unwrap();
    let server = Server::new(db, auth, config);
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    async {
        let ctx = || RequestContext::new(Default::default());
        assert!(
            server
                .handle::<_, IngestEventResponse>(
                    ctx(),
                    Op::IngestEvent,
                    IngestEventRequest::default()
                )
                .await
                .is_err()
        );
        assert!(
            server
                .handle::<_, CorrectEventResponse>(
                    ctx(),
                    Op::CorrectEvent,
                    CorrectEventRequest::default()
                )
                .await
                .is_err()
        );
    }
    .with_subscriber(subscriber)
    .await;
    let spans = capture.0.lock().unwrap();
    for name in ["delcore.event.ingest", "delcore.event.correct"] {
        let span = spans
            .iter()
            .find(|s| s.get("name").is_some_and(|v| v == name))
            .unwrap();
        assert_eq!(span["delcore.outcome"], "rejected");
        assert_eq!(span["delcore.error_reason"], "unauthenticated");
        assert!(server_common::canonical_id(&span["delcore.correlation_id"]));
        assert!(
            !span.keys().any(|k| k.contains("tenant")
                || k.contains("service_id")
                || k.contains("attributes"))
        );
    }
    task.abort();
}
