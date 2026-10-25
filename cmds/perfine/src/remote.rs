// SPDX-License-Identifier: Apache-2.0
use crate::{
    App,
    local::{atomic, git, now, request},
};
use anyhow::{Context, Result, bail, ensure};
use perfine_core::{
    adapters::ArtifactStore,
    publication::{
        ApprovalBinding, PublicationContent, PublicationState, PublishAuthority, Reconciliation,
    },
    types::{CommitSha, Fingerprint, Name, RequestId, ResourceId, Version, digest, fingerprint},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command, time::Duration};

pub fn gh(args: &[&str]) -> Result<Value> {
    let output = Command::new("gh").args(args).output()?;
    ensure!(
        output.status.success(),
        "GitHub operation failed; verify authentication and authority, then retry"
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
#[derive(Clone, Serialize, Deserialize)]
struct Draft {
    schema_version: Version,
    target_repository: String,
    base_ref: String,
    head_repository: String,
    head_ref: String,
    title: String,
    body: String,
}
#[derive(Serialize, Deserialize)]
struct PublicationRecord {
    approval: ResourceId,
    binding: ApprovalBinding,
    draft: Draft,
}
fn remote_commit(repository: &str, reference: &str) -> Result<String> {
    ensure!(
        !reference.starts_with('-')
            && reference
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._/".contains(&b)),
        "invalid GitHub ref"
    );
    let value = gh(&["api", &format!("repos/{repository}/commits/{reference}")])?;
    Ok(value["sha"]
        .as_str()
        .context("GitHub commit SHA")?
        .to_owned())
}
fn repository(value: &str) -> Result<()> {
    ensure!(
        value.split('/').count() == 2
            && value.split('/').all(|p| !p.is_empty()
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))),
        "owner/repository required"
    );
    Ok(())
}
impl App {
    #[expect(
        clippy::too_many_lines,
        reason = "Approval, dispatch and reconciliation form one ordered transaction boundary"
    )]
    pub(crate) fn publish(
        &self,
        experiment: &str,
        candidate: &str,
        req: &RequestId,
        path: &Path,
        approve: Option<&str>,
        judgment: Option<String>,
    ) -> Result<Value> {
        let id = ResourceId::new(experiment)?;
        let cid = ResourceId::new(candidate)?;
        let engine = self.engine()?;
        let exp = engine.status(&id)?;
        let c = exp.candidates.get(&cid).context("unknown candidate")?;
        self.compare(experiment, candidate)?;
        let draft: Draft = serde_json::from_slice(&fs::read(path)?)?;
        repository(&draft.target_repository)?;
        repository(&draft.head_repository)?;
        let directory = Path::new(c.workspace.directory.as_str());
        ensure!(
            git(directory, &["rev-parse", "HEAD"])? == c.head.as_str(),
            "candidate source changed; remeasure"
        );
        ensure!(
            git(
                directory,
                &["status", "--porcelain", "--untracked-files=all"]
            )?
            .is_empty(),
            "candidate is dirty; remeasure intended changes"
        );
        let base = exp.config.config().incremental_base.as_str();
        let changed = git(directory, &["diff", "--name-only", base, c.head.as_str()])?;
        let mut paths = std::collections::BTreeSet::new();
        for line in changed.lines() {
            ensure!(
                !line.starts_with('.')
                    && !exp
                        .config
                        .config()
                        .conditions
                        .inputs
                        .contains_key(&Name::new(line)?),
                "publication includes infrastructure or workload input"
            );
            paths.insert(line.to_owned());
        }
        ensure!(!paths.is_empty(), "empty production diff");
        let diff = git(
            directory,
            &["diff", "--binary", base, c.head.as_str(), "--"],
        )?;
        let evaluation = c
            .evaluation
            .as_ref()
            .context("candidate evaluation required")?;
        let binding = ApprovalBinding {
            experiment: id.clone(),
            candidate: cid.clone(),
            content: PublicationContent {
                target_repository: Name::new(&draft.target_repository)?,
                target_base: Name::new(&draft.base_ref)?,
                base_sha: exp.config.config().incremental_base.clone(),
                head_sha: c.head.clone(),
                title: Name::new(&draft.title)?,
                body: draft.body.clone(),
                production_diff: digest(diff.as_bytes()),
            },
            evaluation: fingerprint(evaluation)?,
        };
        // The token also seals the head repository/ref, which core's binding does not store.
        let token = fingerprint(&(&binding, &draft))?;
        if approve != Some(token.as_str()) {
            return Ok(
                json!({"approval_required":true,"approval_sha256":token,"draft":draft,"binding":binding,"evaluation":evaluation,"diff":diff,"instruction":"Review exact content and evidence, push the retained candidate branch explicitly, then repeat with --approve equal to approval_sha256. Publication creates a draft only."}),
            );
        }
        ensure!(
            remote_commit(&draft.target_repository, &draft.base_ref)? == base,
            "target base moved; remeasure/reapprove"
        );
        ensure!(
            remote_commit(&draft.head_repository, &draft.head_ref)? == c.head.as_str(),
            "remote head must equal measured SHA; push exact retained candidate explicitly"
        );
        let permission = gh(&["api", &format!("repos/{}", draft.head_repository)])?;
        ensure!(
            permission["permissions"]["push"] == true,
            "current push authority to head repository required"
        );
        let user = gh(&["api", "user"])?;
        let authority = PublishAuthority {
            currently_authorized: true,
        };
        let record_path = self
            .root()?
            .join(id.as_str())
            .join(format!("publication-{}.json", cid.as_str()));
        let publication_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(record_path.with_extension("lock"))?;
        fs2::FileExt::try_lock_exclusive(&publication_lock)
            .context("publication already active")?;
        let record = if record_path.exists() {
            let record: PublicationRecord = serde_json::from_slice(&fs::read(&record_path)?)?;
            if fingerprint(&(&record.binding, &record.draft))? == token {
                record
            } else {
                ensure!(
                    engine.approval(&id, &record.approval).is_ok_and(|a| a.state
                        == PublicationState::Approved
                        && a.dispatch_attempts == 0),
                    "publication binding changed after dispatch; reconcile retained publication first"
                );
                let replacement = PublicationRecord {
                    approval: ResourceId::generate(),
                    binding: binding.clone(),
                    draft: draft.clone(),
                };
                atomic(&record_path, &replacement)?;
                replacement
            }
        } else {
            let record = PublicationRecord {
                approval: ResourceId::generate(),
                binding: binding.clone(),
                draft: draft.clone(),
            };
            // Save intent before creating an approval. Repeated calls use the same identifier.
            atomic(&record_path, &record)?;
            record
        };
        if engine.approval(&id, &record.approval).is_err() {
            engine.approve(
                req,
                record.approval.clone(),
                binding.clone(),
                Name::new(user["login"].as_str().context("current GitHub user")?)?,
                judgment.map(Name::new).transpose()?,
                authority,
                now(),
            )?;
        }
        let approval = engine.approval(&id, &record.approval)?;
        if approval.state == PublicationState::Published {
            return Ok(serde_json::to_value(approval)?);
        }
        if matches!(
            approval.state,
            PublicationState::Dispatching | PublicationState::Uncertain
        ) {
            let owner = draft
                .head_repository
                .split('/')
                .next()
                .context("head owner")?;
            let found = gh(&[
                "api",
                &format!(
                    "repos/{}/pulls?state=all&head={}:{}&base={}",
                    draft.target_repository, owner, draft.head_ref, draft.base_ref
                ),
            ])?;
            let rows = found.as_array().context("GitHub pull list")?;
            let matches = rows
                .iter()
                .filter(|v| {
                    v["head"]["sha"] == c.head.as_str()
                        && v["title"] == draft.title
                        && v["body"] == draft.body
                        && v["base"]["sha"] == base
                })
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                let found = matches[0];
                ensure!(
                    found["draft"] == true,
                    "matching publication is no longer a draft; manual reconciliation required"
                );
                engine.reconcile_publication(
                    &request(),
                    &record.approval,
                    &binding,
                    Reconciliation::DraftFound {
                        pull_request: Name::new(found["html_url"].as_str().context("PR URL")?)?,
                    },
                    authority,
                    now(),
                )?;
                return Ok(serde_json::to_value(
                    engine.approval(&id, &record.approval)?,
                )?);
            }
            ensure!(
                rows.is_empty(),
                "ambiguous publication; reconcile GitHub state manually before retry"
            );
            engine.reconcile_publication(
                &request(),
                &record.approval,
                &binding,
                Reconciliation::Absent,
                authority,
                now(),
            )?;
        }
        engine.begin_publication(&request(), &record.approval, &binding, authority, now())?;
        let outcome = match create_pull_request(&draft, &mut Command::new("gh")) {
            Ok(created) => {
                let url = created["html_url"]
                    .as_str()
                    .context("created PR URL")?
                    .to_owned();
                ensure!(
                    url.starts_with(&format!(
                        "https://github.com/{}/pull/",
                        draft.target_repository
                    )),
                    "unexpected PR response; reconcile before retry"
                );
                // A successful create response alone does not establish the exact
                // submitted source: refs can move while GitHub processes the request.
                match gh(&[
                    "pr",
                    "view",
                    &url,
                    "--repo",
                    &draft.target_repository,
                    "--json",
                    "isDraft,headRefOid,baseRefOid,title,body",
                ]) {
                    Ok(actual)
                        if actual["isDraft"] == true
                            && actual["headRefOid"] == c.head.as_str()
                            && actual["baseRefOid"] == base
                            && actual["title"] == draft.title
                            && actual["body"] == draft.body =>
                    {
                        Reconciliation::DraftFound {
                            pull_request: Name::new(url)?,
                        }
                    }
                    _ => Reconciliation::Unknown,
                }
            }
            _ => Reconciliation::Unknown,
        };
        engine.reconcile_publication(
            &request(),
            &record.approval,
            &binding,
            outcome,
            authority,
            now(),
        )?;
        Ok(serde_json::to_value(
            engine.approval(&id, &record.approval)?,
        )?)
    }
}

// REST supports organization-owned fork heads, including two repositories owned
// by the same organization. `gh pr create --head owner:branch` does not.
fn create_pull_request(draft: &Draft, command: &mut Command) -> Result<Value> {
    let (owner, repo) = draft
        .head_repository
        .split_once('/')
        .context("head repository")?;
    let payload = json!({"head":format!("{owner}:{}",draft.head_ref),"head_repo":repo,"base":draft.base_ref,"title":draft.title,"body":draft.body,"draft":true});
    let input = tempfile::NamedTempFile::new()?;
    fs::write(input.path(), serde_json::to_vec(&payload)?)?;
    let output = command
        .args([
            "api",
            &format!("repos/{}/pulls", draft.target_repository),
            "--method",
            "POST",
            "--input",
        ])
        .arg(input.path())
        .output()?;
    ensure!(
        output.status.success(),
        "GitHub draft creation failed; reconcile before retry"
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

// Connect JSON wire envelopes keep uint64 values as decimal strings. The caller's
// protocol envelope is verified against local content hashes before network access.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    #[serde(rename = "ARTIFACT_KIND_ENGINE_EVIDENCE")]
    EngineEvidence,
    #[serde(rename = "ARTIFACT_KIND_MEASUREMENTS")]
    Measurements,
    #[serde(rename = "ARTIFACT_KIND_PROFILE_SUMMARY")]
    ProfileSummary,
    #[serde(rename = "ARTIFACT_KIND_CORRECTNESS_RESULTS")]
    CorrectnessResults,
    #[serde(rename = "ARTIFACT_KIND_RAW_PROFILE")]
    RawProfile,
    #[serde(rename = "ARTIFACT_KIND_RAW_TEST_LOG")]
    RawTestLog,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum Completion {
    #[serde(rename = "COMPLETION_STATE_INCOMPLETE")]
    Incomplete,
    #[serde(rename = "COMPLETION_STATE_COMPLETE")]
    Complete,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub(crate) name: String,
    pub(crate) kind: ArtifactKind,
    pub(crate) media_type: String,
    pub(crate) size_bytes: String,
    pub(crate) sha256: Fingerprint,
    pub(crate) completion: Completion,
    pub(crate) contains_source_snippets: bool,
    pub(crate) secrets_removed: bool,
    pub(crate) required_for_evaluation: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub(crate) schema_version: Version,
    pub(crate) experiment_id: ResourceId,
    pub(crate) candidate_id: Option<ResourceId>,
    pub(crate) source_base_sha: CommitSha,
    pub(crate) source_head_sha: CommitSha,
    pub(crate) harness_sha: CommitSha,
    pub(crate) environment: crate::wire::Environment,
    pub(crate) inputs: Vec<crate::wire::Input>,
    pub(crate) policy: crate::wire::Policy,
    pub(crate) metrics: Vec<crate::wire::Metric>,
    pub(crate) artifacts: Vec<Artifact>,
    pub(crate) completion: Completion,
    pub(crate) experiment_fingerprint_sha256: Fingerprint,
    pub(crate) engine_artifact_name: String,
    #[serde(rename = "manifestSha256")]
    pub(crate) checksum: String,
    pub(crate) run_started_at: String,
    pub(crate) consent_revision: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub(crate) specification: crate::wire::Specification,
    pub(crate) manifest: Manifest,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EligibilityRequest<'a> {
    request_id: &'a RequestId,
    space_id: &'a str,
    connection_id: &'a str,
    manifest: &'a Manifest,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EligibilityResponse {
    request_id: String,
    #[serde(default)]
    decisions: Vec<ArtifactDecision>,
    eligible_for_complete_evaluation: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactDecision {
    artifact_name: String,
    eligibility: Eligibility,
    #[serde(default)]
    reason: String,
}
#[derive(Serialize, Deserialize)]
enum Eligibility {
    #[serde(rename = "ARTIFACT_ELIGIBILITY_ELIGIBLE")]
    Eligible,
    #[serde(rename = "ARTIFACT_ELIGIBILITY_CONSENT_REQUIRED")]
    ConsentRequired,
    #[serde(rename = "ARTIFACT_ELIGIBILITY_CONSENT_REVOKED")]
    ConsentRevoked,
    #[serde(rename = "ARTIFACT_ELIGIBILITY_SECRET_REJECTED")]
    SecretRejected,
    #[serde(rename = "ARTIFACT_ELIGIBILITY_SOURCE_REJECTED")]
    SourceRejected,
    #[serde(rename = "ARTIFACT_ELIGIBILITY_LIMIT_EXCEEDED")]
    LimitExceeded,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Begin<'a> {
    request_id: &'a RequestId,
    space_id: &'a str,
    connection_id: &'a str,
    experiment_id: &'a ResourceId,
    specification: &'a crate::wire::Specification,
    origin: &'static str,
    manifest: &'a Manifest,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    artifact_name: String,
    signed_put_url: String,
    #[serde(default)]
    required_headers: Vec<Header>,
}
#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Begun {
    request_id: String,
    upload: Upload,
    #[serde(default)]
    targets: Vec<Target>,
}
#[derive(Serialize, Deserialize)]
struct Upload {
    id: ResourceId,
    state: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    artifact_name: String,
    sha256: Fingerprint,
    size_bytes: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Complete<'a> {
    request_id: &'a RequestId,
    space_id: &'a str,
    upload_id: &'a ResourceId,
    receipts: Vec<Receipt>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Completed {
    request_id: String,
    upload: Upload,
    evidence_completion: Completion,
}
#[derive(Serialize, Deserialize)]
struct UploadJournal {
    binding: Fingerprint,
    completion_request: RequestId,
    begin_request: RequestId,
    upload: Option<ResourceId>,
    attempts: std::collections::BTreeMap<String, u8>,
    receipts: std::collections::BTreeMap<String, Receipt>,
}
fn secure_url(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        (url.scheme() == "https"
            || (url.scheme() == "http"
                && url.host_str().is_some_and(|h| h
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback()))))
            && url.username().is_empty()
            && url.password().is_none(),
        "HTTPS URL without userinfo required (HTTP permitted only for literal loopback addresses)"
    );
    Ok(url)
}
fn rpc<T: Serialize, R: serde::de::DeserializeOwned>(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    key: &str,
    method: &str,
    body: &T,
) -> Result<R> {
    let url = format!(
        "{}/perfine.v1.EvidenceService/{method}",
        endpoint.trim_end_matches('/')
    );
    secure_url(&url)?;
    for attempt in 0..2 {
        let response = client
            .post(&url)
            .bearer_auth(key)
            .header("Connect-Protocol-Version", "1")
            .json(body)
            .send();
        match response {
            Ok(r)
                if (r.status().as_u16() == 429 || r.status().is_server_error()) && attempt == 0 => {
            }
            Ok(r) => {
                ensure!(
                    r.status().is_success(),
                    "Connect {method} failed with HTTP {}; local results retained",
                    r.status()
                );
                return Ok(r.json()?);
            }
            Err(_) if attempt == 0 => (),
            Err(_) => bail!("Connect transport failed; local results retained"),
        }
    }
    unreachable!()
}
impl App {
    #[expect(
        clippy::too_many_lines,
        clippy::too_many_arguments,
        reason = "Upload phases preserve explicit authorization scope and ordered provenance"
    )]
    pub(crate) fn upload(
        &self,
        experiment: &str,
        req: &RequestId,
        candidate: Option<&str>,
        consent_revision: u64,
        endpoint: &str,
        space: &str,
        connection: &str,
    ) -> Result<Value> {
        let id = ResourceId::new(experiment)?;
        ResourceId::new(req.as_str()).context("hosted mutation request IDs must be UUID v7")?;
        let directory = self.root()?.join(id.as_str());
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("upload.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock).context("upload already active")?;
        let runner = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("runner.lock"))?;
        fs2::FileExt::try_lock_shared(&runner).context("experiment execution or review active")?;
        let exp = self.engine()?.status(&id)?;
        let envelope = self.upload_bundle(&id, candidate, consent_revision)?;
        ensure!(
            envelope.manifest.experiment_id == id
                && envelope.manifest.experiment_fingerprint_sha256 == *exp.config.fingerprint(),
            "upload provenance mismatch"
        );
        ensure!(
            envelope.manifest.source_base_sha == exp.config.config().incremental_base
                && envelope.manifest.harness_sha == exp.config.config().conditions.harness,
            "upload source/harness mismatch"
        );
        ensure!(
            envelope.specification.fingerprint_sha256 == *exp.config.fingerprint()
                && envelope.specification.base_sha == exp.config.config().incremental_base
                && envelope.specification.initial_base_sha == exp.config.config().initial_base
                && envelope.specification.harness_sha == exp.config.config().conditions.harness
                && envelope.specification.workflow_sha == exp.config.config().conditions.workflow,
            "prepared specification does not match local experiment"
        );
        let store = ArtifactStore::new(self.root()?.join("artifacts"))?;
        for artifact in &envelope.manifest.artifacts {
            ensure!(
                artifact.secrets_removed,
                "upload requires explicit secret screening"
            );
            ensure!(
                !artifact.contains_source_snippets
                    || matches!(
                        artifact.kind,
                        ArtifactKind::RawProfile | ArtifactKind::RawTestLog
                    ),
                "only consented raw profiles/test logs may contain source snippets"
            );
            let bytes = store.get(&artifact.sha256)?;
            ensure!(
                bytes.len() as u64 == artifact.size_bytes.parse::<u64>()?,
                "upload size mismatch"
            );
        }
        ensure!(
            envelope
                .manifest
                .artifacts
                .iter()
                .filter(|a| a.kind == ArtifactKind::EngineEvidence)
                .count()
                == 1
                && envelope
                    .manifest
                    .artifacts
                    .iter()
                    .any(|a| a.kind == ArtifactKind::EngineEvidence
                        && a.name == envelope.manifest.engine_artifact_name),
            "exactly one declared engine artifact required"
        );
        let key = std::env::var("DELCORE_API_KEY")
            .context("DELCORE_API_KEY required for explicit hosted upload")?;
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let binding = fingerprint(&(endpoint, space, connection, &envelope))?;
        let journal_path = directory.join(format!("upload-request-{}.json", req.as_str()));
        let mut journal: UploadJournal = if journal_path.exists() {
            let saved: UploadJournal = serde_json::from_slice(&fs::read(&journal_path)?)?;
            ensure!(saved.binding == binding, "upload request binding changed");
            saved
        } else {
            let saved = UploadJournal {
                binding,
                completion_request: request(),
                begin_request: req.clone(),
                upload: None,
                attempts: std::collections::BTreeMap::new(),
                receipts: std::collections::BTreeMap::new(),
            };
            atomic(&journal_path, &saved)?;
            atomic(
                &directory.join(format!("upload-envelope-{}.json", req.as_str())),
                &envelope,
            )?;
            saved
        };
        if let Some(upload) = &journal.upload {
            let status_request = request();
            let status: Value = rpc(
                &client,
                endpoint,
                &key,
                "GetUpload",
                &json!({"requestId":status_request,"spaceId":space,"uploadId":upload}),
            )?;
            ensure!(
                status["requestId"] == status_request.as_str()
                    && status["upload"]["id"] == upload.as_str(),
                "upload reconciliation identity mismatch"
            );
            if status["upload"]["state"] == "UPLOAD_STATE_COMPLETE" {
                return Ok(
                    json!({"reconciled":true,"upload":status["upload"],"local_evidence_preserved":true}),
                );
            }
            // BeginUpload deduplicates the envelope to the same upload resource. A new
            // operation ID refreshes expired signed URLs instead of replaying cached URLs.
            journal.begin_request = request();
            atomic(&journal_path, &journal)?;
        }
        let eligibility: EligibilityResponse = rpc(
            &client,
            endpoint,
            &key,
            "CheckUploadEligibility",
            &EligibilityRequest {
                request_id: req,
                space_id: space,
                connection_id: connection,
                manifest: &envelope.manifest,
            },
        )?;
        ensure!(
            eligibility.request_id == req.as_str(),
            "Connect eligibility request ID mismatch"
        );
        atomic(
            &directory.join(format!("upload-eligibility-{}.json", req.as_str())),
            &eligibility,
        )?;
        if !eligibility.eligible_for_complete_evaluation
            || eligibility
                .decisions
                .iter()
                .any(|d| !matches!(d.eligibility, Eligibility::Eligible))
        {
            return Ok(
                json!({"eligibility":eligibility,"uploaded":false,"evidence_completion":"incomplete","local_evidence_preserved":true}),
            );
        }
        let upload_request = Begin {
            request_id: &journal.begin_request,
            space_id: space,
            connection_id: connection,
            experiment_id: &id,
            specification: &envelope.specification,
            origin: "EXECUTION_ORIGIN_LOCAL",
            manifest: &envelope.manifest,
        };
        let begun: Begun = rpc(&client, endpoint, &key, "BeginUpload", &upload_request)?;
        ensure!(
            begun.request_id == journal.begin_request.as_str(),
            "Connect request ID mismatch"
        );
        if let Some(prior) = &journal.upload {
            ensure!(*prior == begun.upload.id, "upload resource changed");
        }
        journal.upload = Some(begun.upload.id.clone());
        atomic(&journal_path, &journal)?;
        if begun.upload.state == "UPLOAD_STATE_COMPLETE" {
            return Ok(json!({"reconciled":true,"upload":begun.upload}));
        }
        let mut targets = std::collections::BTreeSet::new();
        for target in begun.targets {
            ensure!(
                targets.insert(target.artifact_name.clone()),
                "duplicate upload target"
            );
            if journal.receipts.contains_key(&target.artifact_name) {
                continue;
            }
            let artifact = envelope
                .manifest
                .artifacts
                .iter()
                .find(|a| a.name == target.artifact_name)
                .context("server selected undeclared artifact")?;
            let url = secure_url(&target.signed_put_url)?;
            let mut put = client
                .put(url)
                .header("content-length", &artifact.size_bytes)
                .body(store.get(&artifact.sha256)?);
            for header in target.required_headers {
                ensure!(
                    matches!(
                        header.name.to_ascii_lowercase().as_str(),
                        "content-type" | "content-length" | "content-md5" | "x-amz-checksum-sha256"
                    ),
                    "unexpected storage header"
                );
                if header.name.eq_ignore_ascii_case("content-length") {
                    ensure!(
                        header.value.parse::<u64>()? == artifact.size_bytes.parse::<u64>()?,
                        "storage content length mismatch"
                    );
                }
                put = put.header(header.name, header.value);
            }
            loop {
                let attempts = journal.attempts.entry(artifact.name.clone()).or_default();
                ensure!(
                    *attempts < 2,
                    "storage retry exhausted; reconcile upload before a new explicit request; local evidence retained"
                );
                *attempts += 1;
                let attempt = *attempts;
                atomic(&journal_path, &journal)?;
                let response = put.try_clone().context("replayable upload body")?.send();
                match response {
                    Ok(r) if r.status().is_success() => break,
                    Ok(r)
                        if attempt < 2
                            && (r.status().as_u16() == 429 || r.status().is_server_error()) => {}
                    Err(_) if attempt < 2 => (),
                    _ => bail!("storage upload failed; local evidence retained"),
                }
            }
            journal.receipts.insert(
                artifact.name.clone(),
                Receipt {
                    artifact_name: artifact.name.clone(),
                    sha256: artifact.sha256.clone(),
                    size_bytes: artifact.size_bytes.clone(),
                },
            );
            atomic(&journal_path, &journal)?;
        }
        ensure!(
            journal.receipts.len() == envelope.manifest.artifacts.len(),
            "missing upload targets; incomplete evidence retained"
        );
        let complete_request = &journal.completion_request;
        let completed: Completed = rpc(
            &client,
            endpoint,
            &key,
            "CompleteUpload",
            &Complete {
                request_id: complete_request,
                space_id: space,
                upload_id: &begun.upload.id,
                receipts: journal.receipts.values().cloned().collect(),
            },
        )?;
        ensure!(
            completed.request_id == complete_request.as_str(),
            "Connect completion request mismatch"
        );
        atomic(
            &self
                .root()?
                .join(id.as_str())
                .join(format!("upload-{}.json", begun.upload.id.as_str())),
            &completed,
        )?;
        Ok(json!({"eligibility":eligibility,"completion":completed}))
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactKind, Draft, create_pull_request, secure_url};
    #[test]
    fn fork_creation_uses_structured_rest_payload_for_personal_and_organization_owners() {
        for owner in ["person", "organization"] {
            let draft = Draft {
                schema_version: perfine_core::types::Version,
                target_repository: "organization/upstream".into(),
                base_ref: "main".into(),
                head_repository: format!("{owner}/fork"),
                head_ref: "candidate".into(),
                title: "Exact title".into(),
                body: "Literal $() and `text`\nSecond line".into(),
            };
            let mut command = std::process::Command::new("python3");
            command.args([
                "-c",
                r"import sys,json
assert sys.argv[1:5] == ['api','repos/organization/upstream/pulls','--method','POST']
assert sys.argv[5] == '--input'
payload=json.load(open(sys.argv[6]))
assert payload['head'] in ['person:candidate','organization:candidate']
assert payload['head_repo']=='fork' and payload['base']=='main' and payload['draft'] is True
assert payload['body']=='Literal $() and `text`\nSecond line'
print(json.dumps({'html_url':'https://github.com/organization/upstream/pull/1'}))
",
            ]);
            assert_eq!(
                create_pull_request(&draft, &mut command).unwrap()["html_url"],
                "https://github.com/organization/upstream/pull/1"
            );
        }
    }
    #[test]
    fn upload_rejects_credential_urls_and_non_artifact_content() {
        assert!(secure_url("http://example.com/upload").is_err());
        assert!(secure_url("https://token@example.com/upload").is_err());
        assert!(secure_url("https://example.com/upload").is_ok());
        assert!(serde_json::from_str::<ArtifactKind>("\"ARTIFACT_KIND_SOURCE\"").is_err());
        assert!(serde_json::from_str::<ArtifactKind>("\"ARTIFACT_KIND_UNSPECIFIED\"").is_err());
    }
}
