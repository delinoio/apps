// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::measurement::{Record, State};
use delino_protocol::gittracker::v1::{
    ComparisonStatus::COMPARISON_STATUS_REGRESSION, Context, Repository,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use server_common::{Fault, Result, authority::bounded_json, integer, string};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
/// Default wall-clock budget for one pull request membership check, covering the REST fast path
/// and the GraphQL traversal. Expiry fails closed as unavailable; it never proves non-membership.
pub const MEMBERSHIP_DEADLINE: Duration = Duration::from_secs(120);
/// Most first-parent ancestors stored for a push execution. Older history is not needed to find a
/// recent baseline; rows without a measured ancestor inside this window are comparison-unavailable.
pub const MAX_ANCESTORS: usize = 1000;
/// Default wall-clock budget for collecting the ancestors of one push commit. Expiry fails closed
/// as unavailable and persists nothing.
pub const ANCESTRY_DEADLINE: Duration = Duration::from_secs(20);
/// Most commit-list pages read for one ancestry collection; `MAX_ANCESTORS` linear commits need
/// ten. Parents still missing after these pages are fetched one commit at a time.
const MAX_ANCESTRY_PAGES: usize = 12;
/// Page size for the GraphQL pull request commit connection (the GitHub maximum).
const COMMIT_PAGE: usize = 100;
const COMMITS_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!,$after:String){repository(owner:$owner,name:$name){databaseId pullRequest(number:$number){id number state baseRefName headRefOid commits(first:100,after:$after){totalCount nodes{commit{oid}} pageInfo{hasNextPage endCursor}}}}}";
pub struct GitHub {
    pub http: reqwest::Client,
    pub base: String,
    pub app_id: String,
    pub membership_deadline: Duration,
    pub ancestry_deadline: Duration,
    key: EncodingKey,
}
pub fn escape(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}
pub fn path(r: &Repository) -> String {
    let (owner, name) = r.full_name.split_once('/').unwrap_or(("", ""));
    format!("/repos/{}/{}", escape(owner), escape(name))
}
impl GitHub {
    pub fn new(http: reqwest::Client, base: String, app_id: String, key: EncodingKey) -> Self {
        Self {
            http,
            base,
            app_id,
            membership_deadline: MEMBERSHIP_DEADLINE,
            ancestry_deadline: ANCESTRY_DEADLINE,
            key,
        }
    }
    pub fn from_env() -> anyhow::Result<Self> {
        use server_common::config::required;
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("delino-git-tracker")
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base: "https://api.github.com".into(),
            app_id: required("GITHUB_APP_ID")?,
            membership_deadline: MEMBERSHIP_DEADLINE,
            ancestry_deadline: ANCESTRY_DEADLINE,
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
        self.call_inner(method, path, token, body)
            .await
            .map_err(github_failure)
    }
    async fn call_inner(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(
                method.parse().map_err(|_| Fault::unavailable())?,
                format!("{}{path}", self.base),
            )
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(&body)
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(Fault::unavailable());
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
    pub async fn installation(&self, id: &str) -> Result<String> {
        let v = self
            .call(
                "POST",
                &format!("/app/installations/{}/access_tokens", escape(id)),
                &self.app_token()?,
                Some(json!({})),
            )
            .await?;
        let token = string(&v, "token");
        if token.is_empty() {
            return Err(Fault::unavailable());
        }
        Ok(token.into())
    }
    pub async fn user(&self, token: &str) -> Result<String> {
        let v = self.call("GET", "/user", token, None).await?;
        let id = integer(&v, "id");
        if id == 0 {
            return Err(Fault::denied());
        }
        Ok(id.to_string())
    }
    /// Confirms administrator access and discovers the App installation for `name`.
    pub async fn connect(&self, name: &str, token: &str) -> Result<Repository> {
        let parts: Vec<_> = name.split('/').collect();
        if parts.len() != 2 || parts.iter().any(|v| v.is_empty()) {
            return Err(Fault::invalid("owner/repository required"));
        }
        let p = format!("/repos/{}/{}", escape(parts[0]), escape(parts[1]));
        let v = self.call("GET", &p, token, None).await?;
        if v["permissions"]["admin"] != true {
            return Err(Fault::denied());
        }
        let install = self
            .call(
                "GET",
                &format!("{p}/installation"),
                &self.app_token()?,
                None,
            )
            .await?;
        // Discovery only: the returned identity carries no visibility. A visibility or name
        // observed here predates the connection transaction, so `connect_repository` must refresh
        // it with `metadata` while holding the writer locks instead of persisting this snapshot.
        Ok(Repository {
            github_repository_id: integer(&v, "id").to_string(),
            installation_id: integer(&install, "id").to_string(),
            connected: true,
            ..Default::default()
        })
    }
    pub async fn metadata(&self, r: &Repository) -> Result<Value> {
        self.call(
            "GET",
            &format!("/repositories/{}", escape(&r.github_repository_id)),
            &self.installation(&r.installation_id).await?,
            None,
        )
        .await
    }
    pub async fn pull(&self, r: &Repository, n: i64) -> Result<Value> {
        self.call(
            "GET",
            &format!("{}/pulls/{n}", path(r)),
            &self.installation(&r.installation_id).await?,
            None,
        )
        .await
    }
    async fn commit(&self, r: &Repository, sha: &str, token: &str) -> Result<Value> {
        self.call(
            "GET",
            &format!("{}/commits/{}", path(r), escape(sha)),
            token,
            None,
        )
        .await
    }
    /// Loads the commit at the exact head of `branch`.
    ///
    /// The generic `/commits/{ref}` lookup also resolves SHAs and tag names, so a
    /// caller-supplied branch must first be resolved through the exact
    /// `/git/ref/heads/{branch}` endpoint. The returned ref name and object type are
    /// verified because a provider fallback must never turn a non-branch value into
    /// a branch head.
    async fn branch_head(&self, r: &Repository, branch: &str, token: &str) -> Result<Value> {
        if branch.is_empty()
            || branch
                .split('/')
                .any(|segment| matches!(segment, "" | "." | ".."))
        {
            return Err(Fault::unavailable());
        }
        let encoded = branch.split('/').map(escape).collect::<Vec<_>>().join("/");
        let head = self
            .call(
                "GET",
                &format!("{}/git/ref/heads/{encoded}", path(r)),
                token,
                None,
            )
            .await?;
        let sha = string(&head["object"], "sha");
        if head["ref"] != format!("refs/heads/{branch}")
            || head["object"]["type"] != "commit"
            || !crate::valid_sha(sha)
        {
            return Err(Fault::unavailable());
        }
        let commit = self.commit(r, sha, token).await?;
        if string(&commit, "sha") != sha {
            return Err(Fault::unavailable());
        }
        Ok(commit)
    }
    pub async fn branch(
        &self,
        r: &Repository,
        branch: &str,
        mut needed: HashSet<String>,
    ) -> Result<Vec<String>> {
        let token = self.installation(&r.installation_id).await?;
        let mut c = self.branch_head(r, branch, &token).await?;
        let mut result = vec![string(&c, "sha").to_owned()];
        needed.remove(&result[0]);
        while !needed.is_empty() {
            let Some(sha) = c["parents"][0]["sha"].as_str().map(str::to_owned) else {
                break;
            };
            result.push(sha.clone());
            needed.remove(&sha);
            if needed.is_empty() {
                break;
            }
            c = self.commit(r, &sha, &token).await?;
        }
        Ok(result)
    }
    /// Decides whether a non-head commit belongs to the pull request. The capped REST list is
    /// only a positive fast path: GitHub returns at most 250 commits in total, so a miss (or a
    /// short final page) never proves absence. Every miss falls through to the GraphQL commit
    /// connection, which needs a complete, consistent traversal before reporting a negative.
    async fn pull_contains(&self, r: &Repository, c: &Context, token: &str) -> Result<bool> {
        let mut page = 1;
        loop {
            let rows = self
                .call(
                    "GET",
                    &format!(
                        "{}/pulls/{}/commits?per_page=100&page={page}",
                        path(r),
                        c.pull_request
                    ),
                    token,
                    None,
                )
                .await?;
            let rows = rows.as_array().ok_or_else(Fault::unavailable)?;
            if rows.iter().any(|v| v["sha"] == c.commit) {
                return Ok(true);
            }
            if rows.len() < COMMIT_PAGE {
                break;
            }
            page += 1;
        }
        self.graphql_contains(r, c, token).await
    }
    /// Walks `repository.pullRequest.commits` until the commit is found (positive) or the
    /// traversal is provably complete (negative). Any inconsistency returns unavailable.
    async fn graphql_contains(&self, r: &Repository, c: &Context, token: &str) -> Result<bool> {
        let fail = |category: &'static str| {
            tracing::warn!(
                category,
                pull_request = c.pull_request,
                "pull request commit traversal failed closed"
            );
            Fault::unavailable()
        };
        let (owner, name) = r
            .full_name
            .split_once('/')
            .ok_or_else(|| fail("repository"))?;
        let mut after = Value::Null;
        let mut cursors = HashSet::new();
        let mut seen = HashSet::new();
        let mut snapshot: Option<(Value, i64)> = None;
        loop {
            let response = self
                .call(
                    "POST",
                    "/graphql",
                    token,
                    Some(json!({"query":COMMITS_QUERY,"variables":{
                        "owner":owner,"name":name,"number":c.pull_request,"after":after
                    }})),
                )
                .await?;
            if response.get("errors").is_some_and(|e| !e.is_null()) {
                return Err(fail("graphql_errors"));
            }
            let repository = &response["data"]["repository"];
            let pull = &repository["pullRequest"];
            let connection = &pull["commits"];
            if !pull.is_object() || !connection.is_object() {
                return Err(fail("missing_pull_request"));
            }
            if repository["databaseId"]
                .as_i64()
                .map(|v| v.to_string())
                .as_deref()
                != Some(r.github_repository_id.as_str())
                || pull["number"].as_i64() != Some(c.pull_request)
                || pull["baseRefName"] != c.branch
            {
                return Err(fail("identity_mismatch"));
            }
            let total = connection["totalCount"]
                .as_i64()
                .filter(|v| *v >= 0)
                .ok_or_else(|| fail("total_count"))?;
            let metadata = json!([
                pull["id"],
                pull["state"],
                pull["baseRefName"],
                pull["headRefOid"]
            ]);
            if !pull["id"].as_str().is_some_and(|v| !v.is_empty())
                || !pull["state"].as_str().is_some_and(|v| !v.is_empty())
                || !pull["headRefOid"].as_str().is_some_and(crate::valid_sha)
            {
                return Err(fail("metadata"));
            }
            match &snapshot {
                None => snapshot = Some((metadata, total)),
                Some((m, t)) if *m == metadata && *t == total => {}
                Some(_) => return Err(fail("snapshot_changed")),
            }
            let nodes = connection["nodes"]
                .as_array()
                .filter(|v| v.len() <= COMMIT_PAGE)
                .ok_or_else(|| fail("nodes"))?;
            for node in nodes {
                let oid = node["commit"]["oid"]
                    .as_str()
                    .filter(|v| crate::valid_sha(v))
                    .ok_or_else(|| fail("oid"))?;
                if oid == c.commit {
                    return Ok(true);
                }
                if !seen.insert(oid.to_owned()) {
                    return Err(fail("duplicate_oid"));
                }
            }
            if seen.len() as i64 > total {
                return Err(fail("count_mismatch"));
            }
            let info = &connection["pageInfo"];
            match info["hasNextPage"].as_bool() {
                Some(false) => {
                    if seen.len() as i64 != total {
                        return Err(fail("count_mismatch"));
                    }
                    return Ok(false);
                }
                Some(true) => {
                    let cursor = info["endCursor"]
                        .as_str()
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| fail("cursor"))?;
                    if !cursors.insert(cursor.to_owned()) {
                        return Err(fail("cursor"));
                    }
                    after = json!(cursor);
                }
                None => return Err(fail("page_info")),
            }
        }
    }
    pub async fn resolve(&self, r: &Repository, c: &Context) -> Result<(Vec<String>, String)> {
        self.resolve_inner(r, c).await.map_err(github_failure)
    }
    async fn resolve_inner(&self, r: &Repository, c: &Context) -> Result<(Vec<String>, String)> {
        let token = self.installation(&r.installation_id).await?;
        let commit = self.commit(r, &c.commit, &token).await?;
        if string(&commit, "sha") != c.commit {
            return Err(Fault::unavailable());
        }
        if c.pull_request > 0 {
            let p = self.pull(r, c.pull_request).await?;
            if p["base"]["ref"] != c.branch {
                // The pull request was read successfully, so the mismatch is a verdict on the
                // caller's context rather than a provider failure.
                return Err(Fault::precondition(
                    "pull request base does not match branch",
                ));
            }
            if p["head"]["sha"] != c.commit {
                match tokio::time::timeout(
                    self.membership_deadline,
                    self.pull_contains(r, c, &token),
                )
                .await
                {
                    Ok(Ok(true)) => {}
                    // `pull_contains` reports false only after a complete, consistent traversal.
                    Ok(Ok(false)) => {
                        return Err(Fault::precondition(
                            "commit is not part of the pull request",
                        ));
                    }
                    Ok(Err(fault)) => return Err(fault),
                    Err(_) => {
                        tracing::warn!(
                            category = "membership_deadline",
                            pull_request = c.pull_request,
                            "pull request membership check exceeded its deadline"
                        );
                        return Err(Fault::unavailable());
                    }
                }
            }
            let base = if c.baseline_commit.is_empty() {
                let v = self
                    .call(
                        "GET",
                        &format!(
                            "{}/compare/{}...{}",
                            path(r),
                            string(&p["base"], "sha"),
                            c.commit
                        ),
                        &token,
                        None,
                    )
                    .await?;
                string(&v["merge_base_commit"], "sha").to_owned()
            } else {
                c.baseline_commit.clone()
            };
            return Ok((vec![], base));
        }
        let branch = self
            .branch(r, &c.branch, HashSet::from([c.commit.clone()]))
            .await?;
        // `branch` walks the first-parent path until it finds the commit or reaches the root, so
        // a different tail proves the commit is not on the path.
        if branch.last() != Some(&c.commit) {
            return Err(Fault::precondition(
                "commit is not on the branch first-parent path",
            ));
        }
        let parents =
            match tokio::time::timeout(self.ancestry_deadline, self.ancestors(r, &commit, &token))
                .await
            {
                Ok(parents) => parents?,
                Err(_) => {
                    tracing::warn!(
                        category = "ancestry_deadline",
                        "first-parent ancestry collection exceeded its deadline"
                    );
                    return Err(Fault::unavailable());
                }
            };
        Ok((parents, c.baseline_commit.clone()))
    }
    /// Collects at most `MAX_ANCESTORS` first-parent ancestors of `tip`, nearest first.
    ///
    /// The commit list (100 per page) fills a map from each commit to its first parent, which
    /// replaces one request per ancestor. GitHub orders that list by date rather than by
    /// first-parent path, so a first parent that the pages read so far do not cover (typical in
    /// merge-heavy history) is fetched individually after the page budget is spent.
    async fn ancestors(&self, r: &Repository, tip: &Value, token: &str) -> Result<Vec<String>> {
        let tip_sha = string(tip, "sha").to_owned();
        let mut current = first_parent(tip)?;
        let mut parents: HashMap<String, Option<String>> = HashMap::new();
        let mut pages = 0;
        let mut exhausted = false;
        let mut seen = HashSet::from([tip_sha.clone()]);
        let mut ancestors = Vec::new();
        while let Some(sha) = current {
            if !seen.insert(sha.clone()) {
                tracing::warn!(
                    category = "ancestry_cycle",
                    "first-parent path repeats a commit"
                );
                return Err(Fault::unavailable());
            }
            ancestors.push(sha.clone());
            if ancestors.len() >= MAX_ANCESTORS {
                // The cap is checked before the parent lookup so the last stored ancestor never
                // costs a request. Its parent may be unknown, so any end that is not a known
                // root is reported as truncated.
                if parents.get(&sha) != Some(&None) {
                    tracing::warn!(
                        category = "ancestry_truncated",
                        max_ancestors = MAX_ANCESTORS,
                        "first-parent ancestry was cut at the depth limit"
                    );
                }
                break;
            }
            let known = loop {
                if let Some(parent) = parents.get(&sha) {
                    break parent.clone();
                }
                if !exhausted && pages < MAX_ANCESTRY_PAGES {
                    pages += 1;
                    exhausted = self
                        .collect_page(r, &tip_sha, pages, token, &mut parents)
                        .await?;
                    continue;
                }
                let commit = self.commit(r, &sha, token).await?;
                if string(&commit, "sha") != sha {
                    return Err(Fault::unavailable());
                }
                let parent = first_parent(&commit)?;
                parents.insert(sha.clone(), parent.clone());
                break parent;
            };
            current = known;
        }
        Ok(ancestors)
    }
    /// Reads one page of `GET /commits?sha={tip}` into `parents` and reports whether it was the
    /// last page.
    async fn collect_page(
        &self,
        r: &Repository,
        tip: &str,
        page: usize,
        token: &str,
        parents: &mut HashMap<String, Option<String>>,
    ) -> Result<bool> {
        let rows = self
            .call(
                "GET",
                &format!(
                    "{}/commits?sha={}&per_page={COMMIT_PAGE}&page={page}",
                    path(r),
                    escape(tip)
                ),
                token,
                None,
            )
            .await?;
        let rows = rows
            .as_array()
            .filter(|v| v.len() <= COMMIT_PAGE)
            .ok_or_else(Fault::unavailable)?;
        for row in rows {
            let sha = string(row, "sha");
            if !crate::valid_sha(sha) {
                return Err(Fault::unavailable());
            }
            parents.insert(sha.to_owned(), first_parent(row)?);
        }
        Ok(rows.len() < COMMIT_PAGE)
    }
    pub async fn verify_merge(&self, r: &Repository, merge: &str, head: &str) -> Result<()> {
        if !crate::valid_sha(merge) || !crate::valid_sha(head) {
            return Err(Fault::denied());
        }
        let v = self
            .commit(r, merge, &self.installation(&r.installation_id).await?)
            .await?;
        if v["sha"] != merge
            || v["parents"].as_array().map(Vec::len) != Some(2)
            || v["parents"][1]["sha"] != head
        {
            return Err(Fault::denied());
        }
        Ok(())
    }
    pub async fn comment(&self, r: &Repository, state: &State, record: &Record) -> Result<()> {
        self.comment_inner(r, state, record)
            .await
            .map_err(github_failure)
    }
    async fn comment_inner(&self, r: &Repository, state: &State, record: &Record) -> Result<()> {
        let c = &record.execution.context;
        if c.pull_request == 0 {
            return Ok(());
        }
        let p = self.pull(r, c.pull_request).await?;
        if p["head"]["sha"] != c.commit {
            return Ok(());
        }
        if state.records.values().any(|v| {
            v.execution.context.pull_request == c.pull_request
                && v.execution.context.commit == c.commit
                && v.execution.registration_order > record.execution.registration_order
        }) {
            return Ok(());
        }
        let comparisons = state.comparisons(record, None);
        let regressions = comparisons
            .iter()
            .filter(|v| v.status == COMPARISON_STATUS_REGRESSION)
            .count();
        let status = serde_json::to_value(record.execution.state)?;
        let mut body = format!(
            "<!-- git-tracker -->\n### Git Tracker\n\nState: {}. Regressions: {regressions}.\n\n[Execution details](https://git-tracker.delino.io/?repository={}&execution={})\n\n| Dataset / item / column | Value | Change | Result |\n| --- | ---: | ---: | --- |\n",
            status.as_str().unwrap_or(""),
            escape(&r.id),
            escape(&c.execution_id)
        );
        for v in comparisons {
            let status = serde_json::to_value(v.status)?;
            let line = comparison_row(&v, status.as_str().unwrap_or(""));
            if body.len() + line.len() > 60000 {
                body.push_str("\nMore rows are available in the execution details.\n");
                break;
            }
            body.push_str(&line);
        }
        let token = self.installation(&r.installation_id).await?;
        let mut page = 1;
        let mut comment = None;
        loop {
            let rows = self
                .call(
                    "GET",
                    &format!(
                        "{}/issues/{}/comments?per_page=100&page={page}",
                        path(r),
                        c.pull_request
                    ),
                    &token,
                    None,
                )
                .await?;
            let rows = rows.as_array().ok_or_else(Fault::unavailable)?;
            for v in rows {
                if integer(&v["performed_via_github_app"], "id").to_string() == self.app_id
                    && string(v, "body").starts_with("<!-- git-tracker -->")
                {
                    comment = Some(integer(v, "id"))
                }
            }
            if rows.len() < 100 {
                break;
            }
            page += 1;
        }
        let (method, url) = match comment {
            Some(id) => ("PATCH", format!("{}/issues/comments/{id}", path(r))),
            None => (
                "POST",
                format!("{}/issues/{}/comments", path(r), c.pull_request),
            ),
        };
        self.call(method, &url, &token, Some(json!({"body":body})))
            .await?;
        Ok(())
    }
}

/// Longest uploaded text, in characters, that the pull request comment displays. The execution
/// details page keeps the full value.
const DISPLAY_LIMIT: usize = 120;

/// Renders uploaded text for a pull request comment table cell so that Markdown and HTML in it
/// stay literal. The text is truncated to `DISPLAY_LIMIT` characters (plus an ellipsis), line
/// breaks become spaces and the result is one inline code span: the backtick fence is one
/// longer than the longest backtick run inside, a space pads text that starts or ends with a
/// backtick (or that a code span would trim), and every pipe is escaped for the table. Empty
/// text renders as nothing.
pub fn code_span(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut shown: String = text
        .chars()
        .take(DISPLAY_LIMIT)
        .map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c })
        .collect();
    if text.chars().nth(DISPLAY_LIMIT).is_some() {
        shown.push('\u{2026}');
    }
    let longest = shown
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest + 1);
    // A code span strips one space from both ends when both are spaces (unless all spaces).
    let trimmed =
        shown.starts_with(' ') && shown.ends_with(' ') && !shown.trim_matches(' ').is_empty();
    let pad = if shown.starts_with('`') || shown.ends_with('`') || trimmed {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{}{pad}{fence}", escape_pipes(&shown))
}

/// Escapes every pipe as `\|`. A table splitter pairs backslashes from the left, so a pipe that
/// follows an odd number of literal backslashes would stay unescaped after one more backslash
/// (an even run) and split the cell. One extra backslash keeps such a pipe escaped and the row
/// at exactly four cells, at the cost of one additional displayed backslash.
fn escape_pipes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut backslashes = 0;
    for c in text.chars() {
        if c == '|' {
            if backslashes % 2 == 1 {
                out.push('\\');
            }
            out.push('\\');
        }
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
        out.push(c);
    }
    out
}

/// One four-cell table row of the pull request comment. Every uploaded string goes through
/// `code_span`.
fn comparison_row(v: &delino_protocol::gittracker::v1::Comparison, status: &str) -> String {
    let value: Vec<String> = [&v.value, &v.unit]
        .into_iter()
        .map(|part| code_span(part))
        .filter(|part| !part.is_empty())
        .collect();
    format!(
        "| {} / {} / {} | {} | {} | {} |\n",
        code_span(&v.dataset_id),
        code_span(&v.row_id),
        code_span(&v.column_id),
        value.join(" "),
        code_span(&v.delta),
        status.trim_start_matches("COMPARISON_STATUS_")
    )
}

/// Returns the first parent of a commit object, `None` for a root commit, and fails closed on a
/// malformed parent list.
fn first_parent(commit: &Value) -> Result<Option<String>> {
    match commit["parents"].as_array() {
        Some(parents) => match parents.first() {
            None => Ok(None),
            Some(parent) => parent["sha"]
                .as_str()
                .filter(|v| crate::valid_sha(v))
                .map(|v| Some(v.to_owned()))
                .ok_or_else(Fault::unavailable),
        },
        None if commit["parents"].is_null() => Ok(None),
        None => Err(Fault::unavailable()),
    }
}
/// Attributes a failure to GitHub so it counts in `git_tracker_events_total{kind="github"}`.
/// Failed-precondition faults are definitive verdicts about the request (for example, the commit
/// is provably outside the pull request) and keep the application source: they are neither
/// provider failures nor retryable.
fn github_failure(mut fault: Fault) -> Fault {
    if fault.code != connectrpc::ErrorCode::FailedPrecondition {
        fault.source = server_common::error::FaultSource::GitHub;
    }
    fault
}

#[cfg(test)]
mod tests {
    use super::*;
    use delino_protocol::gittracker::v1::Comparison;

    #[test]
    fn code_span_keeps_ordinary_text_literal() {
        assert_eq!(code_span("size"), "`size`");
        assert_eq!(code_span("a b"), "`a b`");
        assert_eq!(code_span("@org/team"), "`@org/team`");
        assert_eq!(code_span(""), "");
    }

    #[test]
    fn code_span_fences_backtick_runs() {
        // The fence is one longer than the longest inner run.
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(code_span("a```b``c"), "````a```b``c````");
        // Text that starts or ends with a backtick is padded so the fence stays separate.
        assert_eq!(code_span("`a"), "`` `a ``");
        assert_eq!(code_span("a`"), "`` a` ``");
        assert_eq!(code_span("`"), "`` ` ``");
        // Padding keeps spaces that a code span would otherwise trim.
        assert_eq!(code_span(" a "), "`  a  `");
        assert_eq!(code_span("  "), "`  `");
    }

    #[test]
    fn code_span_flattens_line_breaks_and_escapes_pipes() {
        assert_eq!(code_span("a\r\nb\nc"), "`a  b c`");
        assert_eq!(code_span("a|b"), "`a\\|b`");
        assert_eq!(code_span("||"), "`\\|\\|`");
        // A pipe after an odd backslash run gets one more backslash so it cannot split the cell.
        assert_eq!(code_span("a\\|b"), "`a\\\\\\|b`");
        assert_eq!(code_span("a\\\\|b"), "`a\\\\\\|b`");
        // A trailing backslash is literal inside a code span.
        assert_eq!(code_span("x\\"), "`x\\`");
    }

    #[test]
    fn code_span_truncates_on_character_boundaries() {
        let exact = "é".repeat(DISPLAY_LIMIT);
        assert_eq!(code_span(&exact), format!("`{exact}`"));
        let long = "é".repeat(300);
        assert_eq!(
            code_span(&long),
            format!("`{}\u{2026}`", "é".repeat(DISPLAY_LIMIT))
        );
        // The limit counts characters of the original text, before any escaping.
        let pipes = "|".repeat(DISPLAY_LIMIT + 1);
        assert_eq!(
            code_span(&pipes),
            format!("`{}\u{2026}`", "\\|".repeat(DISPLAY_LIMIT))
        );
    }

    #[test]
    fn rows_have_four_cells_and_neutralize_markdown() {
        let hostile = Comparison {
            dataset_id: "@org/team".into(),
            row_id: "[x](https://example.invalid/)![i](https://example.invalid/i.png) #1 :+1: x\\"
                .into(),
            column_id: "c|d".into(),
            value: "1".into(),
            unit: "B".into(),
            delta: "+1".into(),
            ..Default::default()
        };
        let row = comparison_row(&hostile, "COMPARISON_STATUS_REGRESSION");
        assert_eq!(
            row,
            "| `@org/team` / `[x](https://example.invalid/)![i](https://example.invalid/i.png) #1 :+1: x\\` / `c\\|d` | `1` `B` | `+1` | REGRESSION |\n"
        );
        // Outside the escaped code spans there are exactly five unescaped pipes (four cells).
        let outside: Vec<&str> = row.split('`').step_by(2).collect();
        assert_eq!(outside.concat().matches('|').count(), 5);

        let plain = Comparison {
            dataset_id: "d".into(),
            row_id: "r".into(),
            column_id: "c".into(),
            value: "10".into(),
            ..Default::default()
        };
        assert_eq!(
            comparison_row(&plain, "COMPARISON_STATUS_UNAVAILABLE"),
            "| `d` / `r` / `c` | `10` |  | UNAVAILABLE |\n"
        );
    }
}
