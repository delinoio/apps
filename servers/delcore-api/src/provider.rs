// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::config::Config;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::{Client as HttpClient, Method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Transient,
    Permanent,
    Ambiguous,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Transient => "provider transient failure",
            Self::Permanent => "provider rejected request",
            Self::Ambiguous => "payment outcome requires reconciliation",
        })
    }
}
impl std::error::Error for Error {}
#[derive(Clone)]
pub struct Client {
    pub config: Config,
    pub http: HttpClient,
}
impl Client {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let http = HttpClient::builder()
            .timeout(config.timeout())
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { config, http })
    }
    async fn bounded(&self, mut response: reqwest::Response) -> Result<Vec<u8>, Error> {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Ambiguous)? {
            if body.len().saturating_add(chunk.len()) > self.config.max_payload_bytes {
                return Err(Error::Ambiguous);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
    /// Sends one authenticated request and returns the decoded response envelope.
    async fn envelope(&self, method: Method, path: &str, input: Value) -> Result<Value, Error> {
        let mutation = method != Method::GET;
        let response = self
            .http
            .request(
                method,
                format!("{}{}", self.config.paddle_url.trim_end_matches('/'), path),
            )
            .bearer_auth(&self.config.paddle_token)
            .header("Paddle-Version", "1")
            .json(&input)
            .send()
            .await
            .map_err(|_| {
                if mutation {
                    Error::Ambiguous
                } else {
                    Error::Transient
                }
            })?;
        let status = response.status();
        let body = self.bounded(response).await?;
        if status.is_server_error() {
            return Err(if mutation {
                Error::Ambiguous
            } else {
                Error::Transient
            });
        }
        if status.as_u16() == 429 {
            return Err(Error::Transient);
        }
        if !status.is_success() {
            return Err(Error::Permanent);
        }
        serde_json::from_slice(&body).map_err(|_| Error::Ambiguous)
    }
    pub async fn request(&self, method: Method, path: &str, input: Value) -> Result<Value, Error> {
        self.envelope(method, path, input)
            .await?
            .get("data")
            .cloned()
            .ok_or(Error::Ambiguous)
    }
    /// Reads one page of a Paddle list route with a GET request.
    ///
    /// `path` is the list route with its filters. Pass the continuation cursor of the previous page
    /// as `after` to request the following page; the cursor is added as the `after` query
    /// parameter and the request always goes to the configured Paddle origin.
    ///
    /// The page keeps `data` usable even when its pagination block is invalid, so a caller can
    /// still act on a payment that the page contains. See [`Page::next`] and [`next_cursor`].
    pub async fn request_page(&self, path: &str, after: Option<&str>) -> Result<Page, Error> {
        let mut full = path.to_string();
        if let Some(after) = after {
            full.push(if path.contains('?') { '&' } else { '?' });
            full.push_str("after=");
            full.push_str(&escape_path(after));
        }
        let request = url::Url::parse(&format!(
            "{}{}",
            self.config.paddle_url.trim_end_matches('/'),
            full
        ))
        .map_err(|_| Error::Permanent)?;
        let envelope = self.envelope(Method::GET, &full, Value::Null).await?;
        let data = envelope.get("data").cloned().ok_or(Error::Ambiguous)?;
        let next = next_cursor(&self.config.paddle_url, &request, &data, &envelope);
        Ok(Page { data, next })
    }
    pub async fn send_email(&self, to: &str, subject: &str, text: &str) -> Result<(), Error> {
        let html = text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&#34;")
            .replace('\'', "&#39;");
        let response=self.http.post(format!("{}/accounts/{}/email/sending/send",self.config.email_url.trim_end_matches('/'),escape_path(&self.config.email_account))).bearer_auth(&self.config.email_token).json(&json!({"to":to,"from":{"address":self.config.email_sender},"subject":subject,"text":text,"html":format!("<p>{html}</p>")})).send().await.map_err(|_|Error::Transient)?;
        let status = response.status();
        if status.as_u16() == 429 || status.is_server_error() {
            return Err(Error::Transient);
        }
        if !status.is_success() {
            return Err(Error::Permanent);
        }
        let body = self.bounded(response).await.map_err(|_| Error::Transient)?;
        let value: Value = serde_json::from_slice(&body).map_err(|_| Error::Transient)?;
        let count = |key: &str| value["result"][key].as_array().map_or(0, Vec::len);
        if value["success"] != true || count("permanent_bounces") > 0 {
            return Err(Error::Permanent);
        }
        if count("delivered") + count("queued") == 0 {
            return Err(Error::Transient);
        }
        Ok(())
    }
}
/// One page of a Paddle list response.
#[derive(Debug)]
pub struct Page {
    pub data: Value,
    /// The cursor of the following page, `None` when the provider reports no more pages, or an
    /// error when the pagination block is malformed or not safe to follow. A caller must inspect
    /// `data` first and only then act on this value.
    pub next: Result<Option<String>, Error>,
}
/// Validates the continuation of a list response and returns the `after` cursor to resume from.
///
/// Paddle reports `meta.pagination.has_more` and a `next` URL. The URL is never requested as
/// given: the bearer token must only reach the configured Paddle origin, so the function accepts
/// the URL only when it has the same scheme, host and port as `base`, no credentials and the same
/// route as the `request` that produced the page, and copies only its `after` cursor. The remaining
/// query must repeat the requested filters and sort order (`per_page` may differ because the
/// provider caps it). The cursor must differ from the one that was requested and must identify the
/// last returned entity, which bounds a malicious or broken provider to a finite scan. A `next`
/// value with `has_more=false` is ignored.
fn next_cursor(
    base: &str,
    request: &url::Url,
    data: &Value,
    envelope: &Value,
) -> Result<Option<String>, Error> {
    let Some(pagination) = envelope.get("meta").and_then(|meta| meta.get("pagination")) else {
        return Ok(None);
    };
    match pagination.get("has_more") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => return Ok(None),
        Some(Value::Bool(true)) => {}
        Some(_) => return Err(Error::Ambiguous),
    }
    let next = pagination
        .get("next")
        .and_then(Value::as_str)
        .and_then(|next| url::Url::parse(next).ok())
        .ok_or(Error::Ambiguous)?;
    let base = url::Url::parse(base).map_err(|_| Error::Permanent)?;
    if next.scheme() != base.scheme()
        || next.host_str() != base.host_str()
        || next.port_or_known_default() != base.port_or_known_default()
        || !next.username().is_empty()
        || next.password().is_some()
        || next.fragment().is_some()
        || next.path() != request.path()
    {
        return Err(Error::Ambiguous);
    }
    let filters = |url: &url::Url| {
        let mut pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .filter(|(key, _)| key != "after" && key != "per_page")
            .collect();
        pairs.sort();
        pairs
    };
    let cursor = |url: &url::Url| {
        let mut values = url
            .query_pairs()
            .filter(|(key, _)| key == "after")
            .map(|(_, value)| value.into_owned());
        let first = values.next();
        (first, values.next().is_some())
    };
    let (after, repeated) = cursor(&next);
    let requested = cursor(request).0;
    let last = data
        .as_array()
        .and_then(|rows| rows.last())
        .and_then(|row| row.get("id"))
        .and_then(Value::as_str);
    match after {
        Some(after)
            if !repeated
                && !after.is_empty()
                && filters(&next) == filters(request)
                && Some(after.as_str()) != requested.as_deref()
                && Some(after.as_str()) == last =>
        {
            Ok(Some(after))
        }
        _ => Err(Error::Ambiguous),
    }
}
pub fn escape_path(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
pub fn verify_signature(secret: &str, header: &str, body: &[u8], now: DateTime<Utc>) -> bool {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            match k {
                "ts" => {
                    if timestamp.is_some() {
                        return false;
                    }
                    timestamp = Some(v)
                }
                "h1" => signatures.push(v),
                _ => {}
            }
        }
    }
    let Some(timestamp) = timestamp else {
        return false;
    };
    let Ok(seconds) = timestamp.parse::<i64>() else {
        return false;
    };
    let Some(at) = DateTime::from_timestamp(seconds, 0) else {
        return false;
    };
    let age = now.signed_duration_since(at);
    if age > chrono::Duration::minutes(5) || age < chrono::Duration::seconds(-30) {
        return false;
    }
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);
    signatures
        .into_iter()
        .any(|sig| hex::decode(sig).is_ok_and(|bytes| mac.clone().verify_slice(&bytes).is_ok()))
}
fn zero_time() -> DateTime<Utc> {
    DateTime::from_timestamp(-62_135_596_800, 0).expect("valid year one")
}
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    #[serde(rename = "event_id")]
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(rename = "event_type")]
    #[serde(deserialize_with = "null_default")]
    pub r#type: String,
    #[serde(default = "zero_time")]
    pub occurred_at: DateTime<Utc>,
    pub data: Value,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomData {
    #[serde(rename = "delcore_job_id")]
    #[serde(deserialize_with = "null_default")]
    pub job_id: String,
    #[serde(rename = "delcore_tenant_id")]
    #[serde(deserialize_with = "null_default")]
    pub tenant_id: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Price {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub product_id: String,
    #[serde(rename = "custom_data")]
    #[serde(deserialize_with = "null_default")]
    pub custom: CustomData,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    #[serde(deserialize_with = "null_default")]
    pub price: Price,
    #[serde(deserialize_with = "null_default")]
    pub quantity: i64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Totals {
    #[serde(deserialize_with = "null_default")]
    pub subtotal: String,
    #[serde(deserialize_with = "null_default")]
    pub total: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LineItem {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub price_id: String,
    #[serde(deserialize_with = "null_default")]
    pub totals: Totals,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Details {
    #[serde(deserialize_with = "null_default")]
    pub line_items: Vec<LineItem>,
    #[serde(deserialize_with = "null_default")]
    pub totals: Totals,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Checkout {
    #[serde(deserialize_with = "null_default")]
    pub url: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BillingPeriod {
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Transaction {
    #[serde(deserialize_with = "null_default")]
    pub origin: String,
    #[serde(deserialize_with = "null_default")]
    pub items: Vec<Item>,
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    #[serde(deserialize_with = "null_default")]
    pub customer_id: String,
    #[serde(deserialize_with = "null_default")]
    pub subscription_id: String,
    #[serde(rename = "currency_code")]
    #[serde(deserialize_with = "null_default")]
    pub currency: String,
    #[serde(rename = "custom_data")]
    #[serde(deserialize_with = "null_default")]
    pub custom: CustomData,
    #[serde(deserialize_with = "null_default")]
    pub details: Details,
    #[serde(deserialize_with = "null_default")]
    pub checkout: Checkout,
    pub billing_period: Option<BillingPeriod>,
    pub billed_at: Option<DateTime<Utc>>,
}
impl Transaction {
    pub fn confirmed_at(&self, processed: DateTime<Utc>) -> DateTime<Utc> {
        self.billed_at
            .filter(|at| *at <= processed && at.timestamp() != -62135596800)
            .unwrap_or(processed)
    }
    pub fn attribute_charge(mut self, product: &str) -> Self {
        if self.items.len() == 1
            && self.items[0].quantity == 1
            && self.items[0].price.product_id == product
            && !self.items[0].price.custom.job_id.is_empty()
        {
            self.custom = self.items[0].price.custom.clone()
        }
        self
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures_reject_replays_duplicates_and_tampering() {
        let now = DateTime::from_timestamp(1700000000, 0).unwrap();
        let body = b"{\"event_id\":\"evt_1\"}";
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(b"1700000000:");
        mac.update(body);
        let header = format!(
            "ts=1700000000;h1={}",
            hex::encode(mac.finalize().into_bytes())
        );
        assert!(verify_signature("secret", &header, body, now));
        assert!(!verify_signature(
            "secret",
            &format!("{header};ts=1700000000"),
            body,
            now
        ));
        assert!(!verify_signature("secret", &header, b"tampered", now));
        assert!(!verify_signature(
            "secret",
            &header,
            body,
            now + chrono::Duration::minutes(6)
        ));
        assert!(!verify_signature(
            "secret",
            &header,
            body,
            now - chrono::Duration::seconds(31)
        ));
    }
    #[test]
    fn nullable_paddle_fields_keep_empty_defaults() {
        let t: Transaction = serde_json::from_value(json!({"id":"txn_1", "subscription_id":null, "customer_id":null, "custom_data":null, "checkout":null, "items":null})).unwrap();
        assert_eq!(t.id, "txn_1");
        assert!(t.subscription_id.is_empty());
        assert!(t.custom.job_id.is_empty());
        let e: Event = serde_json::from_value(json!({"event_id":"evt_1"})).unwrap();
        assert_eq!(e.occurred_at, zero_time());
    }
    #[test]
    fn payment_time_is_bounded() {
        let now = Utc::now();
        let mut t = Transaction::default();
        assert_eq!(t.confirmed_at(now), now);
        t.billed_at = Some(now + chrono::Duration::seconds(1));
        assert_eq!(t.confirmed_at(now), now);
        t.billed_at = Some(now - chrono::Duration::hours(1));
        assert_eq!(t.confirmed_at(now), t.billed_at.unwrap());
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    async fn server(status: u16, body: &'static str) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(move |headers: axum::http::HeaderMap| async move {
            assert_eq!(headers.get("authorization").unwrap(), "Bearer fixture");
            (axum::http::StatusCode::from_u16(status).unwrap(), body)
        });
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), task)
    }
    #[tokio::test]
    async fn provider_failure_classes_and_response_bounds() {
        for (method, status, body, want) in [
            (Method::POST, 400, "", Error::Permanent),
            (Method::POST, 429, "", Error::Transient),
            (Method::POST, 500, "", Error::Ambiguous),
            (Method::GET, 500, "", Error::Transient),
            (Method::POST, 200, "{}", Error::Ambiguous),
        ] {
            let (url, task) = server(status, body).await;
            let config = Config {
                paddle_url: url,
                paddle_token: "fixture".into(),
                timeout_seconds: 1,
                max_payload_bytes: 1024,
                ..Default::default()
            };
            let c = Client::new(config).unwrap();
            assert_eq!(
                c.request(method, "/transactions", json!({}))
                    .await
                    .unwrap_err(),
                want
            );
            task.abort();
        }
    }
    /// Serves `render(base_url)` for every request so a response can point at its own origin.
    async fn paged_server(
        render: impl Fn(&str) -> String + Clone + Send + Sync + 'static,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let served = base.clone();
        let app = axum::Router::new().fallback(move || {
            let body = render(&served);
            async move { body }
        });
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, task)
    }
    fn page_client(url: String) -> Client {
        Client::new(Config {
            paddle_url: url,
            paddle_token: "fixture".into(),
            timeout_seconds: 1,
            max_payload_bytes: 4096,
            ..Default::default()
        })
        .unwrap()
    }
    const LIST: &str = "/transactions?per_page=200&order_by=created_at[DESC]&customer_id=ctm_1";
    #[tokio::test]
    async fn request_page_follows_only_trusted_continuations() {
        let next = |tail: &'static str| {
            move |base: &str| {
                format!(
                    r#"{{"data":[{{"id":"txn_1"}},{{"id":"txn_2"}}],"meta":{{"pagination":{{"per_page":30,"has_more":true,"next":"{}"}}}}}}"#,
                    tail.replace("CREDS", &base.replace("http://", "http://user:pw@"))
                        .replace("BASE", base)
                )
            }
        };
        let filters = "per_page=30&order_by=created_at%5BDESC%5D&customer_id=ctm_1";
        for (tail, want) in [
            (
                format!("BASE/transactions?{filters}&after=txn_2"),
                Ok(Some("txn_2".to_string())),
            ),
            // A next URL of another origin, scheme, port, route or with credentials is not safe.
            (
                format!("http://127.0.0.1:1/transactions?{filters}&after=txn_2"),
                Err(Error::Ambiguous),
            ),
            (
                format!("https://evil.test/transactions?{filters}&after=txn_2"),
                Err(Error::Ambiguous),
            ),
            (
                format!("BASE/subscriptions?{filters}&after=txn_2"),
                Err(Error::Ambiguous),
            ),
            (
                format!("CREDS/transactions?{filters}&after=txn_2"),
                Err(Error::Ambiguous),
            ),
            // The cursor must identify the last returned entity and must be unique and present.
            (
                format!("BASE/transactions?{filters}&after=txn_1"),
                Err(Error::Ambiguous),
            ),
            (
                format!("BASE/transactions?{filters}&after=txn_2&after=txn_2"),
                Err(Error::Ambiguous),
            ),
            (
                format!("BASE/transactions?{filters}"),
                Err(Error::Ambiguous),
            ),
            // The filters and the sort order must be preserved.
            (
                "BASE/transactions?per_page=30&order_by=created_at%5BDESC%5D&after=txn_2".to_string(),
                Err(Error::Ambiguous),
            ),
            (
                "BASE/transactions?per_page=30&order_by=created_at%5BASC%5D&customer_id=ctm_1&after=txn_2".to_string(),
                Err(Error::Ambiguous),
            ),
            (
                format!("BASE/transactions?{filters}&status=billed&after=txn_2"),
                Err(Error::Ambiguous),
            ),
            ("not a url".to_string(), Err(Error::Ambiguous)),
        ] {
            let tail: &'static str = Box::leak(tail.into_boxed_str());
            let (url, task) = paged_server(next(tail)).await;
            let page = page_client(url).request_page(LIST, None).await.unwrap();
            assert_eq!(page.data.as_array().unwrap().len(), 2, "{tail}");
            assert_eq!(page.next, want, "{tail}");
            task.abort();
        }
    }
    #[tokio::test]
    async fn request_page_rejects_a_repeated_cursor_and_ignores_terminal_next() {
        let (url, task) = paged_server(|base| {
            format!(
                r#"{{"data":[{{"id":"txn_2"}}],"meta":{{"pagination":{{"has_more":true,"next":"{base}/transactions?per_page=30&order_by=created_at%5BDESC%5D&customer_id=ctm_1&after=txn_2"}}}}}}"#
            )
        })
        .await;
        let page = page_client(url)
            .request_page(LIST, Some("txn_2"))
            .await
            .unwrap();
        assert_eq!(page.next, Err(Error::Ambiguous));
        task.abort();
        for body in [
            r#"{"data":[{"id":"txn_2"}],"meta":{"pagination":{"has_more":false,"next":"https://evil.test/transactions?after=txn_2"}}}"#,
            r#"{"data":[{"id":"txn_2"}],"meta":{"pagination":{"has_more":false}}}"#,
            r#"{"data":[{"id":"txn_2"}]}"#,
        ] {
            let (url, task) = paged_server(move |_| body.to_string()).await;
            let page = page_client(url).request_page(LIST, None).await.unwrap();
            assert_eq!(page.next, Ok(None), "{body}");
            task.abort();
        }
        for body in [
            r#"{"data":[{"id":"txn_2"}],"meta":{"pagination":{"has_more":"yes"}}}"#,
            r#"{"data":[{"id":"txn_2"}],"meta":{"pagination":{"has_more":true}}}"#,
            r#"{"data":[],"meta":{"pagination":{"has_more":true,"next":"http://127.0.0.1/transactions"}}}"#,
        ] {
            let (url, task) = paged_server(move |_| body.to_string()).await;
            let page = page_client(url).request_page(LIST, None).await.unwrap();
            assert_eq!(page.next, Err(Error::Ambiguous), "{body}");
            task.abort();
        }
    }
    #[tokio::test]
    async fn email_checks_delivery_results() {
        for (body, want) in [
            (
                r#"{"success":true,"result":{"queued":["recipient@example.test"]}}"#,
                Ok(()),
            ),
            (
                r#"{"success":true,"result":{"permanent_bounces":["recipient@example.test"]}}"#,
                Err(Error::Permanent),
            ),
            (r#"{"success":true,"result":{}}"#, Err(Error::Transient)),
        ] {
            let (url, task) = server(200, body).await;
            let c = Client::new(Config {
                email_url: url,
                email_token: "fixture".into(),
                email_account: "account".into(),
                email_sender: "sender@example.test".into(),
                timeout_seconds: 1,
                max_payload_bytes: 1024,
                ..Default::default()
            })
            .unwrap();
            assert_eq!(
                c.send_email("recipient@example.test", "Test", "Text").await,
                want
            );
            task.abort();
        }
    }
}
