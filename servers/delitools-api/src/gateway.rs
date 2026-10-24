// SPDX-License-Identifier: FSL-1.1-ALv2
//! Vercel AI Gateway client (OpenAI-compatible chat completions).
//!
//! One call is one provider attempt. The client never logs request or response bodies. Dropping
//! the returned future aborts the HTTP request, which is how timeouts and caller cancellation
//! cancel the provider call. Provider usage is interpreted by [`crate::usage`].

use crate::{
    config::GatewaySettings,
    retry::{Failure, classify_status, parse_retry_after},
    usage::{self, UsageReading},
};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};

const MAX_RESPONSE_BYTES: usize = 8 << 20;

/// One chat completion request.
#[derive(Clone, Debug)]
pub struct ChatRequest {
    pub model: String,
    pub system: String,
    pub user: String,
    pub response_format: Value,
    pub max_tokens: u32,
}

/// A parsed 2xx completion.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    /// Assistant message text; `None` when absent, refused or not text.
    pub content: Option<String>,
    /// The provider stopped because it reached the token limit.
    pub truncated: bool,
    pub usage: UsageReading,
}

pub struct Gateway {
    http: reqwest::Client,
    url: String,
    key: crate::config::Secret,
}

impl Gateway {
    pub fn new(settings: &GatewaySettings) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            url: format!(
                "{}/chat/completions",
                settings.base_url.trim_end_matches('/')
            ),
            key: settings.api_key.clone(),
        })
    }

    /// Builds the JSON body. Exposed for tests.
    pub fn body(request: &ChatRequest) -> Value {
        json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": request.system},
                {"role": "user", "content": request.user},
            ],
            "temperature": 0,
            "max_tokens": request.max_tokens,
            "stream": false,
            "response_format": request.response_format,
        })
    }

    /// Performs one attempt. The error is the retry classification of the failure.
    pub async fn complete(&self, request: &ChatRequest) -> Result<Completion, Failure> {
        let response = self
            .http
            .post(&self.url)
            .bearer_auth(self.key.expose())
            .json(&Self::body(request))
            .send()
            .await
            .map_err(|_| Failure::Transient { retry_after: None })?;
        let received_at = SystemTime::now();
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| parse_retry_after(v, received_at));
        if !(200..300).contains(&status) {
            return Err(classify_status(status, retry_after));
        }
        let mut bytes = Vec::new();
        let mut response = response;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                        return Ok(Completion {
                            content: None,
                            truncated: false,
                            usage: UsageReading::Unknown(usage::UnknownUsage::Missing),
                        });
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => return Err(Failure::Transient { retry_after: None }),
            }
        }
        Ok(parse_completion(&bytes))
    }
}

/// Parses a completion body. An unreadable body has no content and unknown usage.
pub fn parse_completion(bytes: &[u8]) -> Completion {
    let Ok(body) = serde_json::from_slice::<Value>(bytes) else {
        return Completion {
            content: None,
            truncated: false,
            usage: UsageReading::Unknown(usage::UnknownUsage::Missing),
        };
    };
    let choice = &body["choices"][0];
    Completion {
        content: choice["message"]["content"].as_str().map(str::to_owned),
        truncated: choice["finish_reason"].as_str() == Some("length"),
        usage: usage::interpret(&body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{TokenUsage, UnknownUsage};

    #[test]
    fn parses_content_usage_and_truncation() {
        let ok = parse_completion(
            br#"{"choices":[{"message":{"content":"[\"x\"]"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
        );
        assert_eq!(ok.content.as_deref(), Some("[\"x\"]"));
        assert!(!ok.truncated);
        assert_eq!(
            ok.usage,
            UsageReading::Known(TokenUsage {
                input: 7,
                output: 3
            })
        );

        let cut = parse_completion(
            br#"{"choices":[{"message":{"content":"{"},"finish_reason":"length"}],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
        );
        assert!(cut.truncated);
    }

    #[test]
    fn unreadable_or_contentless_bodies_are_handled() {
        let garbage = parse_completion(b"<html>");
        assert_eq!(garbage.content, None);
        assert_eq!(garbage.usage, UsageReading::Unknown(UnknownUsage::Missing));
        let refusal = parse_completion(br#"{"choices":[{"message":{"content":null,"refusal":"no"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#);
        assert_eq!(refusal.content, None);
        assert!(matches!(refusal.usage, UsageReading::Known(_)));
        assert_eq!(parse_completion(br#"{"choices":[]}"#).content, None);
    }

    #[test]
    fn request_body_has_the_openai_shape_and_no_caller_identity() {
        let body = Gateway::body(&ChatRequest {
            model: "m".into(),
            system: "s".into(),
            user: "u".into(),
            response_format: json!({"type":"json_object"}),
            max_tokens: 100,
        });
        assert_eq!(body["model"], "m");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "u");
        assert_eq!(body["max_tokens"], 100);
        assert_eq!(body["stream"], false);
        assert!(body.get("user").is_none());
    }
}
