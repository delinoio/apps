// SPDX-License-Identifier: Apache-2.0
//! Shared test helpers: a local HTTP mock that records every request, settings in a temporary
//! home and Connect encoding helpers for the mocked AnyDash API.
#![allow(
    dead_code,
    reason = "Each integration test binary uses a different subset of helpers."
)]
use anydash::{
    core::{Core, Options, Owner},
    keys::MemoryStore,
    pb::{ErrorDetail, ErrorReason},
    providers::{Ctx, Endpoints},
    settings::Settings,
};
use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use prost::Message;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or("")
    }
}
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay: Option<std::time::Duration>,
}
impl Reply {
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: value.to_string().into_bytes(),
            delay: None,
        }
    }
    pub fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![],
            body: body.as_bytes().to_vec(),
            delay: None,
        }
    }
    pub fn proto<M: Message>(message: &M) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".into(), "application/proto".into())],
            body: message.encode_to_vec(),
            delay: None,
        }
    }
    /// A Connect error carrying one `ErrorDetail`.
    pub fn connect_error(status: u16, code: &str, detail: ErrorDetail) -> Self {
        let value = serde_json::json!({
            "code": code,
            "message": "mock error",
            "details": [{"type": "anydash.v1.ErrorDetail", "value": base64::engine::general_purpose::STANDARD_NO_PAD.encode(detail.encode_to_vec())}],
        });
        let mut reply = Self::json(status, &value);
        reply
            .headers
            .push(("X-AnyDash-Correlation-Id".into(), detail.correlation_id));
        reply
    }
    pub fn reason(status: u16, code: &str, reason: ErrorReason) -> Self {
        Self::connect_error(
            status,
            code,
            ErrorDetail {
                reason: reason.into(),
                correlation_id: "01890000-0000-7000-8000-000000000001".into(),
                ..ErrorDetail::default()
            },
        )
    }
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    pub const fn delayed(mut self, delay: std::time::Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}
pub type Handler = Arc<dyn Fn(&Recorded) -> Reply + Send + Sync>;
#[derive(Clone)]
pub struct Mock {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    handler: Arc<Mutex<Handler>>,
}
impl Mock {
    pub async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mock = Self {
            base,
            requests: Arc::default(),
            handler: Arc::new(Mutex::new(Arc::new(handler))),
        };
        let server = mock.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let server = server.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            let server = server.clone();
                            async move {
                                let method = request.method().to_string();
                                let path = request
                                    .uri()
                                    .path_and_query()
                                    .map_or_else(String::new, ToString::to_string);
                                let headers = request
                                    .headers()
                                    .iter()
                                    .map(|(k, v)| {
                                        (k.to_string(), v.to_str().unwrap_or("").to_owned())
                                    })
                                    .collect();
                                let body = request
                                    .into_body()
                                    .collect()
                                    .await
                                    .map_or_else(|_| Vec::new(), |b| b.to_bytes().to_vec());
                                let recorded = Recorded {
                                    method,
                                    path,
                                    headers,
                                    body,
                                };
                                server.requests.lock().unwrap().push(recorded.clone());
                                let handler = server.handler.lock().unwrap().clone();
                                let reply = handler(&recorded);
                                if let Some(delay) = reply.delay {
                                    tokio::time::sleep(delay).await;
                                }
                                let mut response = hyper::Response::builder().status(reply.status);
                                for (k, v) in &reply.headers {
                                    response = response.header(k, v);
                                }
                                Ok::<_, std::convert::Infallible>(
                                    response.body(Full::new(Bytes::from(reply.body))).unwrap(),
                                )
                            }
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        mock
    }
    pub fn set(&self, handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) {
        *self.handler.lock().unwrap() = Arc::new(handler);
    }
    pub fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
    pub fn clear(&self) {
        self.requests.lock().unwrap().clear();
    }
    pub fn count(&self, route: &str) -> usize {
        self.recorded()
            .iter()
            .filter(|r| r.route() == route)
            .count()
    }
    /// No request ever targets a token refresh endpoint or carries a refresh token.
    pub fn assert_no_refresh_requests(&self) {
        for r in self.recorded() {
            let path = r.path.to_ascii_lowercase();
            let body = String::from_utf8_lossy(&r.body).to_ascii_lowercase();
            assert!(
                !path.contains("oauth/token")
                    && !path.contains("/token")
                    && !path.contains("refresh"),
                "refresh request {}",
                r.path
            );
            assert!(
                !body.contains("refresh_token") && !body.contains("grant_type"),
                "refresh body for {}",
                r.path
            );
        }
    }
}
pub fn decode<M: Message + Default>(r: &Recorded) -> M {
    M::decode(r.body.as_slice()).unwrap()
}
/// Settings for an isolated home with the key in a 0600 file.
pub fn settings(home: &Path, extra: &[(&str, &str)]) -> Settings {
    let anydash_home = home.join("anydash");
    let codex = home.join(".codex");
    let mut vars: Vec<(&str, String)> = vec![
        ("ANYDASH_HOME", anydash_home.to_string_lossy().into_owned()),
        ("ANYDASH_KEY_SOURCE", "file".into()),
        ("CODEX_HOME", codex.to_string_lossy().into_owned()),
        ("USER", "tester".into()),
    ];
    for (k, v) in extra {
        vars.retain(|(key, _)| key != k);
        vars.push((k, (*v).to_owned()));
    }
    let borrowed: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    Settings::with_vars(home.to_path_buf(), &borrowed)
}
pub fn ctx(home: &Path, mock: &Mock, grok: &str) -> Ctx {
    Ctx::new(
        Endpoints::mock(&mock.base, grok),
        settings(home, &[]),
        Arc::new(MemoryStore::default()),
    )
    .unwrap()
}
pub fn core(home: &Path, api: &Mock, owner: Owner, extra: &[(&str, &str)]) -> Arc<Core> {
    let mut vars = vec![("ANYDASH_API_URL", api.base.as_str())];
    vars.extend_from_slice(extra);
    Core::open(Options {
        settings: settings(home, &vars),
        endpoints: Endpoints::mock(&api.base, "grok-not-installed-for-tests"),
        os: Arc::new(MemoryStore::default()),
        owner,
    })
    .unwrap()
}
pub fn write(path: &Path, content: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
    path.to_path_buf()
}
/// An unsigned JWT with the given `exp`.
pub fn jwt(exp: i64) -> String {
    let e = |v: &serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
    };
    format!(
        "{}.{}.sig",
        e(&serde_json::json!({"alg": "none"})),
        e(&serde_json::json!({"exp": exp, "sub": "user"}))
    )
}
