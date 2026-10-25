// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Fault, Result, hash};
use delino_protocol::perfine::v1::ErrorReason;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::Duration;
pub struct R2 {
    pub endpoint: String,
    pub bucket: String,
    pub access: String,
    pub secret: String,
    pub http: reqwest::Client,
}
fn transient(fault: Fault) -> Fault {
    crate::error::domain(fault, ErrorReason::TransientFailure)
}
fn mac(key: &[u8], value: &str) -> Vec<u8> {
    let mut h = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key size");
    h.update(value.as_bytes());
    h.finalize().into_bytes().to_vec()
}
pub fn escape(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
impl R2 {
    pub fn from_env() -> anyhow::Result<Self> {
        use server_common::config::{required, safe_url};
        let endpoint = required("R2_ENDPOINT")?;
        anyhow::ensure!(safe_url(&endpoint), "invalid R2 endpoint");
        let url = reqwest::Url::parse(&endpoint)?;
        anyhow::ensure!(
            url.path() == "/" && url.query().is_none() && url.username().is_empty(),
            "R2 endpoint must be an origin"
        );
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').into(),
            bucket: required("R2_BUCKET")?,
            access: required("R2_ACCESS_KEY_ID")?,
            secret: required("R2_SECRET_ACCESS_KEY")?,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
    pub fn signed(&self, method: &str, key: &str) -> Result<String> {
        self.sign(method, key, None)
    }
    pub fn signed_upload(&self, key: &str, size: u64) -> Result<String> {
        self.sign("PUT", key, Some(size))
    }
    fn sign(&self, method: &str, key: &str, size: Option<u64>) -> Result<String> {
        let now = chrono::Utc::now();
        let date = now.format("%Y%m%d").to_string();
        let time = now.format("%Y%m%dT%H%M%SZ").to_string();
        let scope = format!("{date}/auto/s3/aws4_request");
        let host =
            reqwest::Url::parse(&self.endpoint).map_err(|_| transient(Fault::unavailable()))?;
        let host = match host.port() {
            Some(port) => format!(
                "{}:{port}",
                host.host_str()
                    .ok_or_else(|| transient(Fault::unavailable()))?
            ),
            None => host
                .host_str()
                .ok_or_else(|| transient(Fault::unavailable()))?
                .to_owned(),
        };
        let path = format!(
            "/{}/{}",
            escape(&self.bucket),
            key.split('/').map(escape).collect::<Vec<_>>().join("/")
        );
        let signed_headers = if size.is_some() {
            "content-length;host"
        } else {
            "host"
        };
        let canonical_headers = match size {
            Some(size) => format!("content-length:{size}\nhost:{host}\n"),
            None => format!("host:{host}\n"),
        };
        let query = format!(
            "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={}&X-Amz-Date={time}&X-Amz-Expires=900&X-Amz-SignedHeaders={}",
            escape(&format!("{}/{scope}", self.access)),
            escape(signed_headers)
        );
        let canonical = format!(
            "{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\nUNSIGNED-PAYLOAD"
        );
        let signing = mac(
            &mac(
                &mac(
                    &mac(format!("AWS4{}", self.secret).as_bytes(), &date),
                    "auto",
                ),
                "s3",
            ),
            "aws4_request",
        );
        let signature = hex::encode(mac(
            &signing,
            &format!(
                "AWS4-HMAC-SHA256\n{time}\n{scope}\n{}",
                hash(canonical.as_bytes())
            ),
        ));
        Ok(format!(
            "{}{path}?{query}&X-Amz-Signature={signature}",
            self.endpoint
        ))
    }
    pub async fn read(&self, key: &str, limit: u64) -> Result<Vec<u8>> {
        let mut response = self
            .http
            .get(self.signed("GET", key)?)
            .send()
            .await
            .map_err(|error| transient(error.into()))?;
        if !response.status().is_success() {
            return Err(Fault::precondition("artifact is unavailable"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| transient(error.into()))?
        {
            if bytes.len() as u64 + chunk.len() as u64 > limit {
                return Err(Fault::quota());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub async fn write(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let response = self
            .http
            .put(self.signed("PUT", key)?)
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|error| transient(error.into()))?;
        if !response.status().is_success() {
            return Err(transient(Fault::unavailable()));
        }
        Ok(())
    }
    pub async fn delete(&self, key: &str) -> Result<()> {
        let response = self
            .http
            .delete(self.signed("DELETE", key)?)
            .send()
            .await
            .map_err(|error| transient(error.into()))?;
        if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(transient(Fault::unavailable()))
        }
    }
}
pub fn key(experiment: &str, upload: &str, name: &str) -> String {
    format!("{experiment}/{upload}/{name}")
}
