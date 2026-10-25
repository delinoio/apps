// SPDX-License-Identifier: Apache-2.0
use crate::{
    error::{Failure, Result, invalid},
    protocol::{hash, strv, valid_id},
    store::{Store, lock, meta},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
pub const LOOPBACK: &str = "http://127.0.0.1:46327/auth/callback";
pub trait Vault: Send + Sync {
    fn get(&self, key: &str) -> Result<Value>;
    fn set(&self, key: &str, value: &Value) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}
pub struct OsVault;
fn credential_error() -> Failure {
    Failure::new(
        "UNAUTHENTICATED",
        "OS credentials are unavailable. Run lane login and approve the connection again.",
    )
}
fn approval_error() -> Failure {
    Failure::new(
        "UNAUTHENTICATED",
        "Browser approval is invalid or expired. Begin a new connection.",
    )
}
fn mismatch() -> Failure {
    Failure::new(
        "FORBIDDEN",
        "The credential does not belong to this account space.",
    )
}
fn os_entry(key: &str) -> Result<keyring::Entry> {
    // Preserve the Go helper's credential identity and UTF-8 blob representation.
    #[cfg(windows)]
    let entry =
        keyring::Entry::new_with_target(&format!("io.delino.lane:{key}"), "io.delino.lane", key);
    #[cfg(not(windows))]
    let entry = keyring::Entry::new("io.delino.lane", key);
    entry.map_err(|_| credential_error())
}
impl Vault for OsVault {
    fn get(&self, key: &str) -> Result<Value> {
        let raw = os_entry(key)?
            .get_secret()
            .map_err(|_| credential_error())?;
        serde_json::from_slice(&raw).map_err(|_| credential_error())
    }
    fn set(&self, key: &str, v: &Value) -> Result<()> {
        os_entry(key)?
            .set_secret(v.to_string().as_bytes())
            .map_err(|_| credential_error())
    }
    fn delete(&self, key: &str) -> Result<()> {
        match os_entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            _ => Err(credential_error()),
        }
    }
}
#[derive(Clone)]
pub struct Auth {
    pub vault: Arc<dyn Vault>,
    pub http: reqwest::blocking::Client,
    pub cancelled: Arc<AtomicBool>,
}
impl Default for Auth {
    fn default() -> Self {
        Self {
            vault: Arc::new(OsVault),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("HTTP client"),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}
pub fn endpoint(raw: &str) -> Result<String> {
    let u = url::Url::parse(raw).map_err(|_| invalid("Invalid API endpoint."))?;
    if u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || !matches!(u.path(), "" | "/")
        || !(u.scheme() == "https"
            || (u.scheme() == "http"
                && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))))
    {
        return Err(invalid("API endpoint requires HTTPS or loopback HTTP."));
    }
    Ok(u.to_string().trim_end_matches('/').into())
}
fn redirect(raw: &str) -> Result<()> {
    let u = url::Url::parse(raw).map_err(|_| approval_error())?;
    if !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(approval_error());
    }
    if matches!(u.scheme(), "lane" | "lane-dev")
        && u.host_str() == Some("auth")
        && u.path() == "/callback"
    {
        return Ok(());
    }
    if u.scheme() == "http"
        && matches!(u.host_str(), Some("127.0.0.1" | "[::1]"))
        && u.port().is_some()
        && u.path() == "/auth/callback"
    {
        return Ok(());
    }
    Err(approval_error())
}
pub fn key(s: &Store, c: &Value) -> String {
    format!(
        "credential:{}",
        hash(format!(
            "{}\0{}\0{}\0{}\0{}",
            s.space,
            s.device,
            strv(c, "issuer"),
            strv(c, "account_id"),
            strv(c, "tenant_id")
        ))
    )
}
fn expiry(v: &Value, key: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(strv(v, key)).is_ok_and(|t| t > Utc::now())
}
fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
#[must_use]
pub fn reconnect_error() -> Failure {
    Failure::new(
        "STALE_EPOCH",
        "Reconnection requires confirmation: run lane sync connect --confirm-epoch-change for a new browser approval.",
    )
}
pub fn approval_snapshot(s: &Store) -> Result<(String, String, bool)> {
    s.with(false,|c|Ok((meta(c,"account_epoch")?,c.query_row::<String,_,_>("SELECT CAST(coalesce(max(sequence),0) AS TEXT) FROM changes WHERE change_kind IN(1,2)",[],|r|r.get(0))?,meta(c,"reconnect_required")?=="1")))
}
impl Auth {
    pub fn credential_lock(&self, s: &Store, k: &str) -> Result<std::fs::File> {
        let _ = s;
        #[cfg(target_os = "macos")]
        let base =
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Caches"));
        #[cfg(target_os = "windows")]
        let base = Some(crate::windows::data_root()?);
        #[cfg(all(unix, not(target_os = "macos")))]
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache"))
            });
        let base = base.ok_or_else(credential_error)?;
        lock(
            &base
                .join("io.delino.lane/credential-locks")
                .join(format!("{}.lock", hash(k))),
            true,
        )
    }

    pub fn load(&self, s: &Store) -> Result<Value> {
        let identity = s.identity()?;
        let expected = json!({"issuer":identity.issuer,"account_id":identity.account,"tenant_id":identity.tenant});
        let c = self.vault.get(&key(s, &expected))?;
        if c["issuer"] != expected["issuer"]
            || c["account_id"] != expected["account_id"]
            || c["tenant_id"] != expected["tenant_id"]
            || c["device_id"] != s.device
        {
            return Err(mismatch());
        }
        endpoint(strv(&c, "endpoint"))?;
        Ok(c)
    }
    pub fn save(&self, s: &Store, c: &Value) -> Result<()> {
        if c["device_id"] != s.device
            || strv(c, "account_id").is_empty()
            || strv(c, "tenant_id").is_empty()
            || strv(c, "issuer").is_empty()
            || strv(c, "access_token").is_empty()
        {
            return Err(mismatch());
        }
        endpoint(strv(c, "endpoint"))?;
        let k = key(s, c);
        let _lock = self.credential_lock(s, &k)?;
        self.vault.set(&k, c)
    }
    pub fn delete(&self, s: &Store) -> Result<()> {
        let identity = s.identity()?;
        let k = key(
            s,
            &json!({"issuer":identity.issuer,"account_id":identity.account,"tenant_id":identity.tenant}),
        );
        let _lock = self.credential_lock(s, &k)?;
        self.vault.delete(&k)
    }
    pub fn token(&self, s: &Store) -> Result<Value> {
        let original = self.load(s)?;
        let k = key(s, &original);
        let _lock = self.credential_lock(s, &k)?;
        let c = self.load(s)?;
        if chrono::DateTime::parse_from_rfc3339(strv(&c, "expires_at"))
            .is_ok_and(|t| t > Utc::now() + Duration::seconds(30))
        {
            return Ok(c);
        }
        if strv(&c, "refresh_token").is_empty() {
            return Err(credential_error());
        }
        let mut fresh = self.exchange(
            strv(&c, "endpoint"),
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", strv(&c, "refresh_token")),
                ("client_id", "lane-cli"),
            ],
        )?;
        for f in [
            "issuer",
            "account_id",
            "tenant_id",
            "device_id",
            "session_id",
        ] {
            if fresh[f] != c[f] {
                return Err(mismatch());
            }
        }
        fresh["redirect_uri"] = c["redirect_uri"].clone();
        if strv(&fresh, "refresh_token").is_empty() {
            fresh["refresh_token"] = c["refresh_token"].clone();
        }
        self.vault.set(&k, &fresh)?;
        Ok(fresh)
    }
    pub fn exchange(&self, endpoint: &str, form: &[(&str, &str)]) -> Result<Value> {
        let endpoint = crate::auth::endpoint(endpoint)?;
        let response = self
            .http
            .post(format!("{endpoint}/oauth/token"))
            .form(form)
            .send()
            .map_err(|_| credential_error())?;
        let mut c = read_response(response, 1 << 20)?;
        if strv(&c, "access_token").is_empty()
            || c["token_type"] != "Bearer"
            || !c["expires_in"]
                .as_i64()
                .is_some_and(|n| n > 0 && n <= 31_536_000)
            || [
                "issuer",
                "account_id",
                "tenant_id",
                "device_id",
                "session_id",
            ]
            .iter()
            .any(|k| strv(&c, k).is_empty())
        {
            return Err(credential_error());
        }
        for k in [
            "redirect_uri",
            "approval_sequence",
            "approval_space_id",
            "approval_epoch",
            "approval_expires_at",
        ] {
            c[k] = "".into();
        }
        c["reconnect_confirmed"] = false.into();
        c["endpoint"] = endpoint.into();
        c["expires_at"] = (Utc::now() + Duration::seconds(c["expires_in"].as_i64().unwrap()))
            .to_rfc3339()
            .into();
        Ok(c)
    }
    pub fn begin(
        &self,
        s: &Store,
        endpoint_url: &str,
        redirect_uri: &str,
        account: &str,
        confirm: bool,
    ) -> Result<Value> {
        let endpoint = endpoint(endpoint_url)?;
        redirect(redirect_uri)?;
        let (epoch, sequence, required) = approval_snapshot(s)?;
        if (required && !confirm) || (confirm && (!required || account.is_empty())) {
            return Err(reconnect_error());
        }
        let state = random_token();
        let verifier = random_token();
        let pending = json!({"Sequence":sequence,"SpaceID":s.space,"Epoch":epoch,"ReconnectConfirmed":confirm,"ExpectedAccountID":account,"State":state,"Verifier":verifier,"RedirectURI":redirect_uri,"Endpoint":endpoint,"DeviceID":s.device,"ExpiresAt":(Utc::now()+Duration::minutes(10)).to_rfc3339()});
        self.vault
            .set(&format!("approval:{}:{state}", s.space), &pending)?;
        let mut u = url::Url::parse(&format!("{endpoint}/oauth/authorize"))
            .map_err(|_| approval_error())?;
        u.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", "lane-cli"),
            ("redirect_uri", redirect_uri),
            ("state", &state),
            (
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            ),
            ("code_challenge_method", "S256"),
            ("device_id", &s.device),
            ("device_name", "Lane CLI"),
        ]);
        Ok(json!({"authorizationUrl":u.to_string(),"challengeId":state}))
    }
    pub fn complete(&self, store: &Store, callback: &str) -> Result<Value> {
        let (mut url, query) = callback_query(callback)?;
        let state = one(&query, "state")?;
        let code = one(&query, "code")?;
        if query.contains_key("error") {
            return Err(approval_error());
        }
        let key = format!("approval:{}:{state}", store.space);
        let _lock = self.credential_lock(store, &key)?;
        let pending = self.vault.get(&key)?;
        if pending["State"] != state || !expiry(&pending, "ExpiresAt") {
            return Err(approval_error());
        }
        url.set_query(None);
        if url.as_str() != strv(&pending, "RedirectURI") {
            return Err(approval_error());
        }
        let mut credential = self.exchange(
            strv(&pending, "Endpoint"),
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", "lane-cli"),
                ("redirect_uri", strv(&pending, "RedirectURI")),
                ("code_verifier", strv(&pending, "Verifier")),
            ],
        )?;
        if credential["device_id"] != pending["DeviceID"]
            || (!strv(&pending, "ExpectedAccountID").is_empty()
                && credential["account_id"] != pending["ExpectedAccountID"])
        {
            return Err(mismatch());
        }
        self.vault.delete(&key)?;
        for (key, f) in [
            ("redirect_uri", "RedirectURI"),
            ("approval_sequence", "Sequence"),
            ("approval_space_id", "SpaceID"),
            ("approval_epoch", "Epoch"),
            ("reconnect_confirmed", "ReconnectConfirmed"),
            ("approval_expires_at", "ExpiresAt"),
        ] {
            credential[key] = pending[f].clone();
        }
        let (epoch, sequence, required) = approval_snapshot(store)?;
        if credential["approval_space_id"] != store.space
            || credential["approval_epoch"] != epoch
            || credential["approval_sequence"] != sequence
            || credential["reconnect_confirmed"] != required
        {
            return Err(reconnect_error());
        }
        Ok(credential)
    }
    pub fn revoke(&self, c: &Value) -> Result<()> {
        let token = if strv(c, "refresh_token").is_empty() {
            strv(c, "access_token")
        } else {
            strv(c, "refresh_token")
        };
        let response = self
            .http
            .post(format!("{}/oauth/revoke", endpoint(strv(c, "endpoint"))?))
            .form(&[("token", token), ("client_id", "lane-cli")])
            .send()
            .map_err(|_| credential_error())?;
        if !response.status().is_success() {
            return Err(credential_error());
        }
        Ok(())
    }
    pub fn store_deletion(&self, s: &Store, c: &Value, mode: &str, challenge: &str) -> Result<()> {
        if !valid_id(challenge)
            || !matches!(
                mode,
                "DATA_DELETION_MODE_DELETE_EVERYWHERE"
                    | "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"
            )
        {
            return Err(approval_error());
        }
        redirect(strv(c, "redirect_uri"))?;
        let k = format!("deletion:{}:{challenge}", s.space);
        let _lock = self.credential_lock(s, &k)?;
        if let Ok(p) = self.vault.get(&k) {
            check_deletion(s, c, &p, mode, challenge, false)?;
            return Ok(());
        }
        self.vault.set(&k,&json!({"SpaceID":s.space,"DeviceID":s.device,"Issuer":c["issuer"],"AccountID":c["account_id"],"TenantID":c["tenant_id"],"SessionID":c["session_id"],"Endpoint":c["endpoint"],"ChallengeID":challenge,"RedirectURI":c["redirect_uri"],"Mode":if mode=="DATA_DELETION_MODE_DELETE_EVERYWHERE"{1}else{2},"ExpiresAt":(Utc::now()+Duration::minutes(5)).to_rfc3339(),"Proof":"","Used":false}))
    }
    pub fn complete_deletion(
        &self,
        store: &Store,
        credential: &Value,
        callback: &str,
    ) -> Result<String> {
        let (mut url, query) = callback_query(callback)?;
        let challenge = one(&query, "reauth_challenge")?;
        if !valid_id(challenge) {
            return Err(approval_error());
        }
        let key = format!("deletion:{}:{challenge}", store.space);
        let _lock = self.credential_lock(store, &key)?;
        let mut pending = self.vault.get(&key)?;
        check_deletion(
            store,
            credential,
            &pending,
            deletion_mode(&pending),
            challenge,
            true,
        )?;
        url.set_query(None);
        if url.as_str() != strv(&pending, "RedirectURI") {
            return Err(approval_error());
        }
        if strv(&pending, "Proof").is_empty() {
            let response = self
                .http
                .post(format!(
                    "{}/auth/reauth/proof",
                    endpoint(strv(credential, "endpoint"))?
                ))
                .bearer_auth(strv(credential, "access_token"))
                .json(&json!({"challenge_id":challenge}))
                .send()
                .map_err(|_| credential_error())?;
            let proof = read_response(response, 1 << 20)?;
            if strv(&proof, "reauth_proof").is_empty() {
                return Err(approval_error());
            }
            pending["Proof"] = proof["reauth_proof"].clone();
            self.vault.set(&key, &pending)?;
        }
        Ok(challenge.into())
    }
    pub fn consume_deletion(
        &self,
        s: &Store,
        c: &Value,
        mode: &str,
        challenge: &str,
        cancel: bool,
    ) -> Result<String> {
        let k = format!("deletion:{}:{challenge}", s.space);
        let _lock = self.credential_lock(s, &k)?;
        let mut p = self.vault.get(&k)?;
        check_deletion(
            s,
            c,
            &p,
            if cancel { deletion_mode(&p) } else { mode },
            challenge,
            !cancel,
        )?;
        let proof = strv(&p, "Proof").to_owned();
        if !cancel && proof.is_empty() {
            return Err(approval_error());
        }
        p["Proof"] = "".into();
        p["Used"] = true.into();
        self.vault.set(&k, &p)?;
        Ok(proof)
    }
    pub fn listen(&self) -> Result<TcpListener> {
        let listener = TcpListener::bind("127.0.0.1:46327").map_err(|_| {
            Failure::new(
                "STORAGE_BUSY",
                "Browser callback port 46327 is unavailable. Close the other approval and retry.",
            )
        })?;
        listener.set_nonblocking(true)?;
        Ok(listener)
    }
    pub fn wait_callback<T>(
        &self,
        listener: TcpListener,
        mut complete: impl FnMut(&str) -> Result<T>,
    ) -> Result<T> {
        let until = Instant::now() + std::time::Duration::from_secs(600);
        while Instant::now() < until && !self.cancelled.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
                    let mut bytes = vec![0; 8192];
                    let n = stream.read(&mut bytes)?;
                    let line = std::str::from_utf8(&bytes[..n])
                        .map_err(|_| approval_error())?
                        .lines()
                        .next()
                        .unwrap_or("");
                    let parts: Vec<_> = line.split_whitespace().collect();
                    if parts.len() < 2
                        || parts[0] != "GET"
                        || !parts[1].starts_with("/auth/callback?")
                    {
                        let _=stream.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    }
                    let result = complete(&format!("http://127.0.0.1:46327{}", parts[1]));
                    let body = if result.is_ok() {
                        "Approval received. Return to Lane."
                    } else {
                        "Approval failed. Return to Lane."
                    };
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )?;
                    return result;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(approval_error())
    }
}
pub fn read_response(response: reqwest::blocking::Response, limit: u64) -> Result<Value> {
    if !response.status().is_success() {
        return Err(credential_error());
    }
    let mut bytes = vec![];
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| credential_error())?;
    if bytes.len() as u64 > limit {
        return Err(credential_error());
    }
    serde_json::from_slice(&bytes).map_err(|_| credential_error())
}
fn callback_query(callback: &str) -> Result<(url::Url, BTreeMap<String, Vec<String>>)> {
    let u = url::Url::parse(callback).map_err(|_| approval_error())?;
    if u.fragment().is_some() {
        return Err(approval_error());
    }
    let mut q: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (k, v) in u.query_pairs() {
        q.entry(k.into()).or_default().push(v.into());
    }
    Ok((u, q))
}
fn one<'a>(q: &'a BTreeMap<String, Vec<String>>, k: &str) -> Result<&'a str> {
    let v = q
        .get(k)
        .filter(|v| v.len() == 1 && !v[0].is_empty())
        .ok_or_else(approval_error)?;
    Ok(&v[0])
}
fn deletion_mode(p: &Value) -> &str {
    if p["Mode"] == 1 {
        "DATA_DELETION_MODE_DELETE_EVERYWHERE"
    } else {
        "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"
    }
}
fn check_deletion(
    s: &Store,
    c: &Value,
    p: &Value,
    mode: &str,
    challenge: &str,
    live: bool,
) -> Result<()> {
    if p["SpaceID"] != s.space
        || p["DeviceID"] != s.device
        || p["DeviceID"] != c["device_id"]
        || p["Issuer"] != c["issuer"]
        || p["AccountID"] != c["account_id"]
        || p["TenantID"] != c["tenant_id"]
        || p["SessionID"] != c["session_id"]
        || p["Endpoint"] != c["endpoint"]
        || p["RedirectURI"] != c["redirect_uri"]
        || p["ChallengeID"] != challenge
        || deletion_mode(p) != mode
        || (live && (p["Used"] == true || !expiry(p, "ExpiresAt")))
    {
        return Err(approval_error());
    }
    Ok(())
}
