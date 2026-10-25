// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::storage::escape;
use crate::{Fault, Result, integer, string};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use server_common::authority::bounded_json;
pub struct PublicationTokens {
    pub head: String,
    pub target: String,
}
#[derive(Debug)]
pub enum GitHubCallError {
    Response {
        status: reqwest::StatusCode,
        rate_limited: bool,
    },
    Fault(Fault),
    RepositoryMismatch,
}
impl GitHubCallError {
    pub fn is_definitive_denial(&self) -> bool {
        match self {
            Self::Response {
                status,
                rate_limited,
            } => {
                !rate_limited
                    && matches!(
                        *status,
                        reqwest::StatusCode::UNAUTHORIZED
                            | reqwest::StatusCode::FORBIDDEN
                            | reqwest::StatusCode::NOT_FOUND
                    )
            }
            Self::RepositoryMismatch => true,
            Self::Fault(_) => false,
        }
    }

    pub fn into_fault(self) -> Fault {
        match self {
            Self::Response {
                status,
                rate_limited,
            } if status.is_client_error()
                && !rate_limited
                && status != reqwest::StatusCode::REQUEST_TIMEOUT =>
            {
                Fault::denied()
            }
            Self::Response { .. } => Fault::unavailable(),
            Self::Fault(fault) => fault,
            Self::RepositoryMismatch => Fault::denied(),
        }
    }
}
pub struct GitHub {
    pub http: reqwest::Client,
    pub base: String,
    pub app_id: String,
    pub key: EncodingKey,
}
impl GitHub {
    pub fn from_env() -> anyhow::Result<Self> {
        use server_common::config::required;
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("delino-perfine")
                .build()?,
            base: "https://api.github.com".into(),
            app_id: required("GITHUB_APP_ID")?,
            key: EncodingKey::from_rsa_pem(&std::fs::read(required("GITHUB_KEY_FILE")?)?)?,
        })
    }
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.call_classified(method, path, token, body)
            .await
            .map_err(GitHubCallError::into_fault)
    }
    pub async fn call_classified(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> std::result::Result<Value, GitHubCallError> {
        let mut req = self
            .http
            .request(
                method
                    .parse()
                    .map_err(|_| GitHubCallError::Fault(Fault::invalid("method")))?,
                format!("{}{path}", self.base),
            )
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req
            .send()
            .await
            .map_err(|error| GitHubCallError::Fault(error.into()))?;
        if !response.status().is_success() {
            return Err(GitHubCallError::Response {
                status: response.status(),
                rate_limited: response.status() == reqwest::StatusCode::FORBIDDEN
                    && (response.headers().get("retry-after").is_some()
                        || response
                            .headers()
                            .get("x-ratelimit-remaining")
                            .is_some_and(|value| value == "0")),
            });
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({}));
        }
        bounded_json(response, 8 << 20)
            .await
            .map_err(GitHubCallError::Fault)
    }
    pub fn app_token(&self) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        jsonwebtoken::encode(
            &Header::new(Algorithm::RS256),
            &json!({"iss":self.app_id,"iat":now-60,"exp":now+480}),
            &self.key,
        )
        .map_err(|_| Fault::unavailable())
    }
    pub async fn token(&self, connection: &Value) -> Result<String> {
        self.token_classified(connection)
            .await
            .map_err(GitHubCallError::into_fault)
    }
    pub async fn token_classified(
        &self,
        connection: &Value,
    ) -> std::result::Result<String, GitHubCallError> {
        let v=self.call_classified("POST",&format!("/app/installations/{}/access_tokens",integer(connection,"githubInstallationId")),&self.app_token().map_err(GitHubCallError::Fault)?,Some(json!({"repository_ids":[integer(&connection["repository"],"githubRepositoryId")]}))).await?;
        if string(&v, "token").is_empty() {
            return Err(GitHubCallError::Fault(Fault::unavailable()));
        }
        Ok(string(&v, "token").into())
    }
    pub async fn current(&self, c: &Value) -> Result<String> {
        self.current_classified(c)
            .await
            .map_err(GitHubCallError::into_fault)
    }
    pub async fn current_classified(
        &self,
        c: &Value,
    ) -> std::result::Result<String, GitHubCallError> {
        let token = self.token_classified(c).await?;
        let v = self
            .call_classified(
                "GET",
                &format!(
                    "/repositories/{}",
                    integer(&c["repository"], "githubRepositoryId")
                ),
                &token,
                None,
            )
            .await?;
        if integer(&v, "id") != integer(&c["repository"], "githubRepositoryId")
            || string(&v, "full_name")
                != format!(
                    "{}/{}",
                    string(&c["repository"], "owner"),
                    string(&c["repository"], "name")
                )
        {
            return Err(GitHubCallError::RepositoryMismatch);
        }
        Ok(token)
    }
    pub async fn publication_tokens(&self, c: &Value, d: &Value) -> Result<PublicationTokens> {
        if d["headRepository"] != c["repository"] {
            return Err(Fault::denied());
        }
        let head = self.current(c).await?;
        let target = if d["targetRepository"] == d["headRepository"] {
            head.clone()
        } else {
            let repository = &d["targetRepository"];
            if integer(repository, "githubRepositoryId") == 0 {
                return Err(Fault::denied());
            }
            // A fork's repository-scoped token cannot authorize a private upstream.
            // Resolve its installation as the App and mint a separate exact-repository token.
            let installation = self
                .call(
                    "GET",
                    &format!("{}/installation", path(repository)),
                    &self.app_token()?,
                    None,
                )
                .await?;
            let installation_id = integer(&installation, "id");
            if installation_id == 0 {
                return Err(Fault::denied());
            }
            self.current(
                &json!({"repository": repository, "githubInstallationId": installation_id}),
            )
            .await?
        };
        Ok(PublicationTokens { head, target })
    }
    pub async fn admin(&self, c: &Value, user_token: &str) -> Result<()> {
        if user_token.is_empty() {
            return Err(Fault::denied());
        }
        let v = self
            .call(
                "GET",
                &format!(
                    "/repositories/{}",
                    integer(&c["repository"], "githubRepositoryId")
                ),
                user_token,
                None,
            )
            .await?;
        if v["permissions"]["admin"] != true {
            return Err(Fault::denied());
        }
        self.current(c).await?;
        Ok(())
    }
    pub async fn production_diff(
        &self,
        d: &Value,
        candidate: &Value,
        tokens: &PublicationTokens,
    ) -> Result<()> {
        if candidate["_publicationAllowed"] != true
            || candidate["baseSha"] != d["baseSha"]
            || candidate["headSha"] != d["headSha"]
            || candidate["evidenceManifestSha256"] != d["evidenceManifestSha256"]
        {
            return Err(Fault::precondition("measured production evidence required"));
        }
        let comparison = self
            .call(
                "GET",
                &format!(
                    "{}/compare/{}...{}",
                    path(&d["targetRepository"]),
                    escape(string(d, "baseSha")),
                    escape(&format!(
                        "{}:{}",
                        string(&d["headRepository"], "owner"),
                        string(d, "headSha")
                    ))
                ),
                &tokens.target,
                None,
            )
            .await?;
        let files = comparison["files"]
            .as_array()
            .ok_or_else(Fault::unavailable)?;
        let paths = candidate["_measuredPaths"]
            .as_array()
            .ok_or_else(|| Fault::precondition("measured source paths missing"))?;
        let commits = comparison["commits"]
            .as_array()
            .ok_or_else(Fault::unavailable)?;
        if comparison["status"] != "ahead"
            || comparison["merge_base_commit"]["sha"] != d["baseSha"]
            || comparison["base_commit"]["sha"] != d["baseSha"]
            || commits.is_empty()
            || commits.len() >= 250
            || integer(&comparison, "total_commits") != commits.len() as u64
            || commits.last().map(|c| &c["sha"]) != Some(&d["headSha"])
            || !production_paths(files, paths)
        {
            return Err(Fault::precondition(
                "complete measured production diff required",
            ));
        }
        // Inspect every commit, not just the net diff: adding and then reverting unrelated
        // infrastructure must not smuggle those commits into a production PR. Exact parent
        // linkage also excludes merged histories, comparison truncation and unrelated bases.
        let mut parent = d["baseSha"].clone();
        for commit in commits {
            let sha = string(commit, "sha");
            if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Fault::precondition("invalid production commit"));
            }
            let detail = self
                .call(
                    "GET",
                    &format!("{}/commits/{}", path(&d["headRepository"]), escape(sha)),
                    &tokens.head,
                    None,
                )
                .await?;
            let parents = detail["parents"]
                .as_array()
                .ok_or_else(Fault::unavailable)?;
            let files = detail["files"].as_array().ok_or_else(Fault::unavailable)?;
            if detail["sha"] != commit["sha"] || parents.len() != 1 || parents[0]["sha"] != parent
                || !production_paths(files, paths)
                // The commit endpoint defaults to 30 files per page. Reject the boundary
                // rather than interpreting a truncated commit as a complete production diff.
                || files.len() >= 30
            {
                return Err(Fault::precondition(
                    "isolated measured production commits required",
                ));
            }
            parent = commit["sha"].clone();
        }
        Ok(())
    }
    pub async fn binding(&self, d: &Value, tokens: &PublicationTokens) -> Result<()> {
        for (repository, reference, sha, token) in [
            ("targetRepository", "baseRef", "baseSha", &tokens.target),
            ("headRepository", "headRef", "headSha", &tokens.head),
        ] {
            let r = &d[repository];
            let v = self
                .call(
                    "GET",
                    &format!("{}/commits/{}", path(r), escape(string(d, reference))),
                    token,
                    None,
                )
                .await?;
            if v["sha"] != d[sha] {
                return Err(crate::error::domain(
                    Fault::precondition("approval stale; revalidate measured source"),
                    delino_protocol::perfine::v1::ErrorReason::ApprovalStale,
                ));
            }
            let metadata = self.call("GET", &path(r), token, None).await?;
            if integer(&metadata, "id") != integer(r, "githubRepositoryId") {
                return Err(Fault::denied());
            }
        }
        Ok(())
    }
}
pub fn path(repository: &Value) -> String {
    format!(
        "/repos/{}/{}",
        escape(string(repository, "owner")),
        escape(string(repository, "name"))
    )
}

fn production_paths(files: &[Value], paths: &[Value]) -> bool {
    !files.is_empty()
        && files.len() < 300
        && files.iter().all(|f| {
            let name = string(f, "filename");
            paths.contains(&json!(name))
                && !name.starts_with(".github/")
                && !name.starts_with(".perfine/")
                && !name.contains("..")
                && !name.starts_with('/')
                && !name.contains('\\')
                && matches!(string(f, "status"), "added" | "modified" | "removed")
        })
}
