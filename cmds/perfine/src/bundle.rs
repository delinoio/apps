// SPDX-License-Identifier: Apache-2.0
//! Lossless local-to-hosted evidence mapping; evaluation stays in perfine-core.
use crate::{
    App,
    config::Settings,
    local::{Journal, load},
    remote::{Artifact, ArtifactKind, Completion, Envelope, Manifest},
};
use anyhow::{Context, Result, ensure};
use buffa::Message;
use perfine_core::{
    adapters::ArtifactStore,
    evidence,
    interfaces::Language,
    policy::{Policy, ReviewEvidence},
    types::{ResourceId, Version, digest, fingerprint},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs};

#[derive(Serialize, Deserialize)]
pub struct Bundle {
    pub schema_version: Version,
    pub config: evidence::Config,
    pub incremental_baseline: [evidence::Manifest; 2],
    pub cumulative_baseline: Option<[evidence::Manifest; 2]>,
    pub first: Option<Round>,
    pub repeat: Option<Round>,
    pub review: ReviewEvidence,
}
#[derive(Serialize, Deserialize)]
pub struct Round {
    pub base: evidence::Manifest,
    pub head: evidence::Manifest,
    pub cumulative_base: Option<evidence::Manifest>,
}
impl App {
    pub(crate) fn retained_bundle(
        &self,
        id: &ResourceId,
        candidate: Option<&str>,
    ) -> Result<Bundle> {
        let exp = self.engine()?.status(id)?;
        let journal: Journal = serde_json::from_slice(&fs::read(
            self.root()?.join(id.as_str()).join("journal.json"),
        )?)?;
        let store = ArtifactStore::new(self.root()?.join("artifacts"))?;
        let run = |label: &str| -> Result<evidence::Manifest> {
            let record = journal.runs.get(label).with_context(|| {
                format!("incomplete evidence: missing {label}; local files retained")
            })?;
            Ok(load(&exp.config, record, &store)?.manifest().clone())
        };
        let cumulative = exp.config.config().initial_base != exp.config.config().incremental_base;
        let mut bundle = Bundle {
            schema_version: Version,
            config: exp.config.config().clone(),
            incremental_baseline: [run("baseline-a")?, run("baseline-b")?],
            cumulative_baseline: cumulative
                .then(|| Ok::<_, anyhow::Error>([run("initial-a")?, run("initial-b")?]))
                .transpose()?,
            first: None,
            repeat: None,
            review: ReviewEvidence::default(),
        };
        if let Some(candidate) = candidate {
            let cid = ResourceId::new(candidate)?;
            let c = exp.candidates.get(&cid).context("unknown candidate")?;
            ensure!(
                c.state == perfine_core::types::ExecutionState::Completed,
                "candidate incomplete; no hosted pass possible"
            );
            let label = format!("candidate-{candidate}");
            bundle.first = Some(Round {
                base: run("baseline-a")?,
                head: run(&label)?,
                cumulative_base: cumulative.then(|| run("initial-a")).transpose()?,
            });
            if journal.runs.contains_key(&format!("{label}-repeat-head")) {
                bundle.repeat = Some(Round {
                    base: run(&format!("{label}-repeat-base"))?,
                    head: run(&format!("{label}-repeat-head"))?,
                    cumulative_base: cumulative
                        .then(|| run(&format!("{label}-repeat-initial")))
                        .transpose()?,
                });
            }
        } else {
            ensure!(
                exp.candidates.is_empty(),
                "select --candidate explicitly when candidates exist"
            );
        }
        if let Some(candidate) = candidate {
            let path = self
                .root()?
                .join(id.as_str())
                .join(format!("review-{candidate}.json"));
            if path.exists() {
                let record: ReviewRecord = serde_json::from_slice(&fs::read(path)?)?;
                ensure!(
                    record.measurements == fingerprint(&bundle)?,
                    "review measurements changed"
                );
                bundle.review = record.input.review;
            }
        }
        Ok(bundle)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep lossless artifact and protocol mapping together"
    )]
    pub(crate) fn upload_bundle(
        &self,
        id: &ResourceId,
        candidate: Option<&str>,
        consent_revision: u64,
    ) -> Result<Envelope> {
        let exp = self.engine()?.status(id)?;
        if let Some(candidate) = candidate {
            self.compare(id.as_str(), candidate)?;
        }
        let mut bundle = self.retained_bundle(id, candidate)?;
        let store = ArtifactStore::new(self.root()?.join("artifacts"))?;
        // Hosted normalized observations supplement originals; local manifests remain untouched.
        let mut normalized = BTreeMap::new();
        let mut normalize = |run: &mut evidence::Manifest| -> Result<()> {
            for (label, kind, hosted, value) in [
                (
                    "measurements",
                    evidence::ArtifactKind::Measurement,
                    ArtifactKind::Measurements,
                    serde_json::to_value(&run.measurements)?,
                ),
                (
                    "profile",
                    evidence::ArtifactKind::Profile,
                    ArtifactKind::ProfileSummary,
                    serde_json::to_value(&run.profile_paths)?,
                ),
                (
                    "correctness",
                    evidence::ArtifactKind::Correctness,
                    ArtifactKind::CorrectnessResults,
                    serde_json::to_value(&run.correctness)?,
                ),
            ] {
                let bytes = serde_json::to_vec(&value)?;
                let hash = store.put(&bytes)?;
                let name = perfine_core::types::Name::new(format!("{label}-{}", hash.as_str()))?;
                run.artifacts.insert(
                    name.clone(),
                    evidence::Artifact {
                        kind,
                        sha256: hash,
                        size: bytes.len() as u64,
                    },
                );
                normalized.insert(name, hosted);
            }
            Ok(())
        };
        for run in &mut bundle.incremental_baseline {
            normalize(run)?;
        }
        if let Some(runs) = &mut bundle.cumulative_baseline {
            for run in runs {
                normalize(run)?;
            }
        }
        for round in [&mut bundle.first, &mut bundle.repeat]
            .into_iter()
            .flatten()
        {
            normalize(&mut round.base)?;
            normalize(&mut round.head)?;
            if let Some(run) = &mut round.cumulative_base {
                normalize(run)?;
            }
        }
        let cfg = &bundle.config;
        ensure!(
            cfg.conditions.environment.variables.is_empty(),
            "environment variables are not eligible for hosting; immutable local evidence retained"
        );
        let settings = Settings::read(&self.config)?;
        let specification = specification(cfg, settings.adapter.language, exp.created_at)?;
        let store = ArtifactStore::new(self.root()?.join("artifacts"))?;
        let mut artifacts = BTreeMap::new();
        let mut runs: Vec<&evidence::Manifest> = bundle.incremental_baseline.iter().collect();
        if let Some(b) = &bundle.cumulative_baseline {
            runs.extend(b);
        }
        for r in [&bundle.first, &bundle.repeat].into_iter().flatten() {
            runs.extend([&r.base, &r.head]);
            runs.extend(r.cumulative_base.as_ref());
        }
        for run in runs {
            for (name, a) in &run.artifacts {
                let bytes = store.get(&a.sha256)?;
                screen(&bytes)?;
                let kind = normalized.get(name).copied().unwrap_or(match a.kind {
                    evidence::ArtifactKind::Profile => ArtifactKind::RawProfile,
                    evidence::ArtifactKind::Measurement | evidence::ArtifactKind::Correctness => {
                        ArtifactKind::RawTestLog
                    }
                });
                let artifact = Artifact {
                    name: name.as_str().into(),
                    kind,
                    media_type: "application/octet-stream".into(),
                    size_bytes: a.size.to_string(),
                    sha256: a.sha256.clone(),
                    completion: Completion::Complete,
                    contains_source_snippets: !normalized.contains_key(name),
                    secrets_removed: true,
                    required_for_evaluation: true,
                };
                if let Some(previous) = artifacts.insert(name.clone(), artifact) {
                    ensure!(previous.sha256 == a.sha256, "artifact name collision");
                }
            }
        }
        let bytes = serde_json::to_vec(&bundle)?;
        screen(&bytes)?;
        let hash = store.put(&bytes)?;
        let engine_name = "engine.json".to_owned();
        let mut artifacts: Vec<_> = artifacts.into_values().collect();
        artifacts.push(Artifact {
            name: engine_name.clone(),
            kind: ArtifactKind::EngineEvidence,
            media_type: "application/vnd.perfine.engine.v1+json".into(),
            size_bytes: bytes.len().to_string(),
            sha256: hash,
            completion: Completion::Complete,
            contains_source_snippets: false,
            secrets_removed: true,
            required_for_evaluation: true,
        });
        let value = serde_json::to_value(&specification)?;
        let mut manifest = Manifest {
            schema_version: Version,
            experiment_id: id.clone(),
            candidate_id: candidate.map(ResourceId::new).transpose()?,
            source_base_sha: cfg.incremental_base.clone(),
            source_head_sha: bundle.first.as_ref().map_or_else(
                || cfg.incremental_base.clone(),
                |r| r.head.identity.source.clone(),
            ),
            harness_sha: cfg.conditions.harness.clone(),
            environment: serde_json::from_value(value["environment"].clone())?,
            inputs: serde_json::from_value(value["inputs"].clone())?,
            policy: serde_json::from_value(value["policy"].clone())?,
            metrics: vec![serde_json::from_value(value["policy"]["metric"].clone())?],
            artifacts,
            completion: Completion::Complete,
            experiment_fingerprint_sha256: exp.config.fingerprint().clone(),
            engine_artifact_name: engine_name,
            checksum: String::new(),
            run_started_at: timestamp(exp.created_at)?,
            consent_revision: consent_revision.to_string(),
        };
        // Hash canonical protobuf bytes, with the checksum empty. JSON is transport only.
        let wire: crate::protocol::perfine::v1::EvidenceManifest =
            serde_json::from_value(serde_json::to_value(&manifest)?)?;
        manifest.checksum = digest(&wire.encode_to_vec()).as_str().into();
        Ok(Envelope {
            specification,
            manifest,
        })
    }
}
fn timestamp(seconds: u64) -> Result<String> {
    Ok(chrono::DateTime::from_timestamp(i64::try_from(seconds)?, 0)
        .context("timestamp range")?
        .to_rfc3339())
}
fn specification(
    cfg: &evidence::Config,
    language: Language,
    created_at: u64,
) -> Result<crate::wire::Specification> {
    let c = &cfg.conditions;
    let objective = match &cfg.policy {
        Policy::Declared(p) if p.objective == perfine_core::policy::Objective::Maximize => {
            "METRIC_OBJECTIVE_MAXIMIZE"
        }
        _ => "METRIC_OBJECTIVE_MINIMIZE",
    };
    let metric = json!({"name":serde_json::to_value(c.metric.metric)?.as_str().context("metric")?,"unit":c.metric.unit,"objective":objective,"engineKind":format!("ENGINE_METRIC_KIND_{}",serde_json::to_value(c.metric.metric)?.as_str().context("metric")?.to_uppercase())});
    let mut p = json!({"schemaVersion":1,"metric":metric,"policySha256":fingerprint(&cfg.policy)?});
    match &cfg.policy {
        Policy::Ir(ir) => {
            p["kind"] = json!("POLICY_KIND_CALLGRIND_IR");
            p["minimumImprovement"] = json!(ir.minimum_improvement);
            p["maximumCaseRegression"] = json!(ir.maximum_case_regression);
            p["repetitions"] = json!(1);
            p["uncertainty"] = json!({"engineSchemaVersion":1,"repeatTolerance":ir.repeat_tolerance,"nearThreshold":ir.near_threshold});
            let mut exceptions = vec![];
            if ir.allow_simple_exception {
                exceptions.push("POLICY_EXCEPTION_BELOW_MINIMUM_IMPROVEMENT");
            }
            if ir.allow_single_case_exception {
                exceptions.push("POLICY_EXCEPTION_SINGLE_UNPROTECTED_REGRESSION");
            }
            p["allowedExceptions"] = json!(exceptions);
        }
        Policy::Declared(d) => {
            p["kind"] = json!("POLICY_KIND_DECLARED");
            p["minimumImprovement"] = json!(d.minimum_improvement);
            p["maximumCaseRegression"] = json!(d.maximum_case_regression);
            p["repetitions"] = json!(d.repetitions);
            p["uncertainty"] = json!({"engineSchemaVersion":1,"repeatTolerance":d.uncertainty.repeat_tolerance,"nearThreshold":d.near_threshold,"maximumRelativeRange":d.uncertainty.maximum_relative_range,"decisionMargin":d.uncertainty.decision_margin});
        }
        Policy::MeasurementsOnly => {
            p["kind"] = json!("POLICY_KIND_MEASUREMENTS_ONLY");
        }
    }
    let lang = match language {
        Language::Rust => "RUST",
        Language::C => "C",
        Language::Cpp => "CPP",
        Language::Zig => "ZIG",
        Language::Go => "GO",
        Language::JavaScript => "JAVASCRIPT",
        Language::Extension => anyhow::bail!("unsupported hosted language"),
    };
    Ok(serde_json::from_value(
        json!({"schemaVersion":1,"baseSha":cfg.incremental_base,"initialBaseSha":cfg.initial_base,"harnessSha":c.harness,"workflowSha":c.workflow,"language":format!("LANGUAGE_{lang}"),
            "environment":{"operatingSystem":c.environment.os,"architecture":c.environment.architecture,"runner":c.environment.runner,"runtime":c.toolchain,"hardware":c.environment.hardware,"tools":c.tools.iter().map(|(name,version)|json!({"name":name,"version":version})).collect::<Vec<_>>(),"buildOptions":c.build_options,"runtimeFlags":c.runtime_flags,"environmentSha256":fingerprint(&c.environment)?},
            "inputs":c.inputs.iter().map(|(name,sha256)|json!({"name":name,"sha256":sha256})).collect::<Vec<_>>(),"cases":c.cases.iter().map(|(id,case)|json!({"id":id,"family":case.family,"frequency":case.weight(),"frequencySource":if case.frequency.is_some(){"FREQUENCY_SOURCE_DECLARED"}else{"FREQUENCY_SOURCE_ASSUMED_ONE"},"protectedCase":case.protected})).collect::<Vec<_>>(),"policy":p,
            "limits":{"candidateCount":cfg.limits.candidate_count,"simultaneousCandidates":cfg.limits.simultaneous_candidates,"candidateTimeout":format!("{}s",cfg.limits.candidate_duration_seconds),"startDeadline":timestamp(created_at.checked_add(cfg.limits.start_duration_seconds).context("deadline overflow")?)?},"startDuration":format!("{}s",cfg.limits.start_duration_seconds),"fingerprintSha256":cfg.clone().freeze()?.fingerprint(),"workloadRepresentativenessConfirmed":true,"workloadConfirmation":cfg.workload_confirmation,"correctnessChecks":c.correctness_checks
        }),
    )?)
}
/// Reject known credential markers before any network write. Raw evidence still requires server consent and inspection.
fn screen(bytes: &[u8]) -> Result<()> {
    use std::io::Read;
    const LIMIT: u64 = 64 << 20;
    ensure!(
        bytes.len() as u64 <= LIMIT,
        "artifact exceeds privacy inspection limit"
    );
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .take(LIMIT + 1)
            .read_to_end(&mut decoded)?;
        ensure!(
            decoded.len() as u64 <= LIMIT,
            "decoded artifact exceeds privacy inspection limit"
        );
        return screen_text(&String::from_utf8_lossy(&decoded));
    }
    screen_text(&String::from_utf8_lossy(bytes))?;
    // Inspect escaped JSON strings too, without discarding duplicate object keys.
    if matches!(
        bytes.iter().find(|b| !b.is_ascii_whitespace()),
        Some(b'{' | b'[')
    ) {
        use serde::de::DeserializeSeed;
        PrivacyScan.deserialize(&mut serde_json::Deserializer::from_slice(bytes))?;
    }
    Ok(())
}
fn screen_text(text: &str) -> Result<()> {
    let lower = text.to_ascii_lowercase();
    for marker in [
        "-----begin",
        "private key",
        "authorization:",
        "bearer ",
        "ghp_",
        "ghs_",
        "github_pat_",
        "sk-ant-",
        "sk-proj-",
        "dlk_",
        "aws_secret_access_key",
        "client_secret",
        "access_token",
        "refresh_token",
        "api_key",
        "password",
        "diff --git",
        "@@ -",
    ] {
        ensure!(
            !lower.contains(marker),
            "artifact contains prohibited source or credential content; local evidence retained"
        );
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewInput {
    schema_version: Version,
    statement: perfine_core::types::Name,
    review: ReviewEvidence,
}
#[derive(Serialize, Deserialize)]
struct ReviewRecord {
    input: ReviewInput,
    confirmation: perfine_core::types::Fingerprint,
    measurements: perfine_core::types::Fingerprint,
    request: perfine_core::types::RequestId,
}
impl App {
    #[expect(
        clippy::too_many_lines,
        reason = "Retain the ordered confirmation, evidence verification and durable review transaction"
    )]
    pub(crate) fn review(
        &self,
        experiment: &str,
        candidate: &str,
        request: &perfine_core::types::RequestId,
        path: &std::path::Path,
        confirmation: Option<&str>,
    ) -> Result<Value> {
        let id = ResourceId::new(experiment)?;
        let cid = ResourceId::new(candidate)?;
        let directory = self.root()?.join(experiment);
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("runner.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock).context("experiment active")?;
        let input: ReviewInput = serde_json::from_slice(&fs::read(path)?)?;
        let mut bundle = self.retained_bundle(&id, Some(candidate))?;
        // Bind confirmation to the immutable runs, excluding the prior review assertion.
        bundle.review = ReviewEvidence::default();
        let measurements = fingerprint(&bundle)?;
        let token = fingerprint(&(&id, &cid, &measurements, &input))?;
        if confirmation != Some(token.as_str()) {
            return Ok(
                json!({"confirmation_required":true,"confirm_review":token,"measurements":measurements,"input":input,"instruction":"Review the retained artifact hashes and statement, then repeat with --confirm-review equal to confirm_review."}),
            );
        }
        bundle.review = input.review.clone();
        let store = ArtifactStore::new(self.root()?.join("artifacts"))?;
        let frozen = bundle.config.clone().freeze()?;
        let verify = |m: &evidence::Manifest| {
            perfine_core::adapters::measurement::verify_local(
                &frozen,
                &m.identity,
                m.clone(),
                &store,
            )
        };
        let a = verify(&bundle.incremental_baseline[0])?;
        let b = verify(&bundle.incremental_baseline[1])?;
        let baseline = evidence::validate_baseline(&frozen, &a, &b)?;
        let initial = bundle
            .cumulative_baseline
            .as_ref()
            .map(|m| Ok::<_, anyhow::Error>((verify(&m[0])?, verify(&m[1])?)))
            .transpose()?;
        let initial_validation = initial
            .as_ref()
            .map(|(a, b)| evidence::validate_baseline(&frozen, a, b))
            .transpose()?;
        let first = bundle
            .first
            .as_ref()
            .context("candidate measurements required")?;
        let base = verify(&first.base)?;
        let head = verify(&first.head)?;
        let cumulative = first.cumulative_base.as_ref().map(verify).transpose()?;
        let first = round(
            &base,
            &head,
            cumulative.as_ref(),
            &baseline,
            initial_validation.as_ref(),
        );
        let repeat_runs = bundle
            .repeat
            .as_ref()
            .map(|r| {
                Ok::<_, anyhow::Error>((
                    verify(&r.base)?,
                    verify(&r.head)?,
                    r.cumulative_base.as_ref().map(verify).transpose()?,
                ))
            })
            .transpose()?;
        let repeat = repeat_runs.as_ref().map(|(base, head, cumulative)| {
            round(
                base,
                head,
                cumulative.as_ref(),
                &baseline,
                initial_validation.as_ref(),
            )
        });
        // Validate the attestations against retained artifact hashes before saving intent.
        perfine_core::policy::evaluate(&frozen, &first, repeat.as_ref(), &input.review)?;
        let record = ReviewRecord {
            input,
            confirmation: token,
            measurements,
            request: request.clone(),
        };
        let review_path = directory.join(format!("review-{candidate}.json"));
        crate::local::atomic(&review_path.with_extension("pending.json"), &record)?;
        self.engine()?.finish_candidate(
            request,
            &id,
            &cid,
            &first,
            repeat.as_ref(),
            &record.input.review,
            crate::local::now(),
        )?;
        crate::local::atomic(&review_path, &record)?;
        crate::local::atomic(
            &directory.join(format!(
                "review-{candidate}-{}.json",
                record.confirmation.as_str()
            )),
            &record,
        )?;
        self.compare(experiment, candidate)
    }
}

fn round<'a>(
    base: &'a evidence::VerifiedRun,
    head: &'a evidence::VerifiedRun,
    cumulative: Option<&'a evidence::VerifiedRun>,
    baseline: &'a evidence::BaselineValidation,
    initial: Option<&'a evidence::BaselineValidation>,
) -> perfine_core::policy::Round<'a> {
    perfine_core::policy::Round {
        incremental: perfine_core::policy::Comparison {
            base,
            head,
            baseline,
        },
        cumulative: cumulative.zip(initial).map(|(base, baseline)| {
            perfine_core::policy::Comparison {
                base,
                head,
                baseline,
            }
        }),
    }
}

struct PrivacyScan;
impl<'de> serde::de::DeserializeSeed<'de> for PrivacyScan {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> serde::de::Visitor<'de> for PrivacyScan {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("screened artifact JSON")
    }
    fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<(), E> {
        screen_text(s).map_err(E::custom)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut a: A,
    ) -> std::result::Result<(), A::Error> {
        while let Some(key) = a.next_key::<String>()? {
            screen_text(&key).map_err(serde::de::Error::custom)?;
            a.next_value_seed(Self)?;
        }
        Ok(())
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut a: A,
    ) -> std::result::Result<(), A::Error> {
        while a.next_element_seed(Self)?.is_some() {}
        Ok(())
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<(), E> {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn privacy_checks_escaped_duplicate_fields_and_compressed_bytes() {
        assert!(super::screen(br#"{"value":"\u0067hp_hidden","value":"safe"}"#).is_err());
        use std::io::Write;
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(b"pprof string table: authorization: bearer fixture")
            .unwrap();
        assert!(super::screen(&gzip.finish().unwrap()).is_err());
    }
}
