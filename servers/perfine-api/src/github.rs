// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::storage::escape;
use crate::{Fault, Result, integer, string};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use server_common::authority::bounded_json;
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
        let mut req = self
            .http
            .request(
                method.parse().map_err(|_| Fault::invalid("method"))?,
                format!("{}{path}", self.base),
            )
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req.send().await?;
        if !response.status().is_success() {
            return Err(if response.status().is_client_error() {
                Fault::denied()
            } else {
                Fault::unavailable()
            });
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({}));
        }
        bounded_json(response, 8 << 20).await
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
        let v=self.call("POST",&format!("/app/installations/{}/access_tokens",integer(connection,"githubInstallationId")),&self.app_token()?,Some(json!({"repository_ids":[integer(&connection["repository"],"githubRepositoryId")]}))).await?;
        if string(&v, "token").is_empty() {
            return Err(Fault::unavailable());
        }
        Ok(string(&v, "token").into())
    }
    pub async fn current(&self, c: &Value) -> Result<String> {
        let token = self.token(c).await?;
        let v = self
            .call(
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
            return Err(Fault::denied());
        }
        Ok(token)
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
    pub async fn production_diff(&self, d: &Value, candidate: &Value, token: &str) -> Result<()> {
        if candidate["_publicationAllowed"] != true
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
                token,
                None,
            )
            .await?;
        let files = comparison["files"]
            .as_array()
            .ok_or_else(Fault::unavailable)?;
        let paths = candidate["_measuredPaths"]
            .as_array()
            .ok_or_else(|| Fault::precondition("measured source paths missing"))?;
        if integer(&comparison, "total_commits") != 1
            || comparison["status"] != "ahead"
            || files.is_empty()
            || files.len() >= 300
            || files.iter().any(|f| {
                let name = string(f, "filename");
                !paths.contains(&json!(name))
                    || name.starts_with(".github/")
                    || name.starts_with(".perfine/")
                    || name.contains("..")
                    || f["status"] == "renamed"
            })
        {
            return Err(Fault::precondition(
                "production diff requires one isolated commit containing only measured paths; remeasure prepared submission",
            ));
        }
        Ok(())
    }
    pub async fn binding(&self, d: &Value, token: &str) -> Result<()> {
        for (repository, reference, sha) in [
            ("targetRepository", "baseRef", "baseSha"),
            ("headRepository", "headRef", "headSha"),
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
                return Err(Fault::precondition(
                    "approval stale; revalidate measured source",
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
