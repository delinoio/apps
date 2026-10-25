// SPDX-License-Identifier: Apache-2.0
//! Synchronous command adapters with cancellation through the entire HTTP body read.
use crate::{
    auth::Auth,
    error::{Failure, Result},
};
use std::{
    sync::{OnceLock, atomic::Ordering},
    time::Duration,
};
static HTTP_RUNTIME: OnceLock<std::io::Result<tokio::runtime::Runtime>> = OnceLock::new();
pub struct Response {
    pub status: reqwest::StatusCode,
    pub body: Vec<u8>,
}
impl Response {
    #[must_use]
    pub const fn status(&self) -> reqwest::StatusCode {
        self.status
    }
}
pub trait SendRequest {
    fn send_cancellable(self, auth: &Auth) -> Result<Response>;
}
impl SendRequest for reqwest::RequestBuilder {
    fn send_cancellable(self, auth: &Auth) -> Result<Response> {
        let unavailable = || Failure::new("DEPENDENCY_UNAVAILABLE", "Lane service is unavailable.");
        // Keep the reactor alive across requests so pooled HTTP connections and
        // concurrent MCP calls share a bounded number of I/O threads.
        let runtime = HTTP_RUNTIME
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
            })
            .as_ref()
            .map_err(|_| unavailable())?;
        runtime.block_on(async {
            let operation = async {
                let mut response = self.send().await.map_err(|_| unavailable())?;
                let status = response.status();
                let mut body = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                    if body.len() + chunk.len() > 16 << 20 {
                        return Err(Failure::new(
                            "INVALID_INPUT",
                            "Lane response exceeds the size limit.",
                        ));
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(Response { status, body })
            };
            let cancelled = async {
                while !auth.cancelled.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            };
            tokio::select! {
                biased;
                () = cancelled => Err(Failure::new("DEPENDENCY_UNAVAILABLE", "Request cancelled.")),
                result = operation => result,
            }
        })
    }
}
