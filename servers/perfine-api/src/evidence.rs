// SPDX-License-Identifier: FSL-1.1-ALv2
//! Lossless hosted bundle. Each run retains the versioned client evidence manifest; all referenced bytes
//! must be in the upload. Hosted summaries are derived only after shared verification.
use crate::evaluation::{evidence as engine, policy, types};
use crate::{Fault, Result, hash, integer, string};
use delino_protocol::perfine::v1::ErrorReason;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub schema_version: types::Version,
    pub config: engine::Config,
    pub incremental_baseline: [engine::Manifest; 2],
    pub cumulative_baseline: Option<[engine::Manifest; 2]>,
    pub first: Option<Round>,
    pub repeat: Option<Round>,
    pub review: policy::ReviewEvidence,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Round {
    pub base: engine::Manifest,
    pub head: engine::Manifest,
    pub cumulative_base: Option<engine::Manifest>,
}
fn engine_error(_: types::Error) -> Fault {
    Fault::precondition("engine evidence verification failed")
}
pub fn content_allowed(bytes: &[u8], kind: &str) -> Result<()> {
    crate::privacy::content_allowed(bytes, kind)
}
struct VerifiedRound {
    base: engine::VerifiedRun,
    head: engine::VerifiedRun,
    cumulative: Option<engine::VerifiedRun>,
}
pub fn evaluate(e: &Value, m: &Value, bytes: &BTreeMap<String, Vec<u8>>) -> Result<Option<Value>> {
    if m["completion"] != "COMPLETION_STATE_COMPLETE"
        || m["artifacts"].as_array().is_none_or(|a| {
            a.iter()
                .any(|v| v["completion"] != "COMPLETION_STATE_COMPLETE")
        })
    {
        return Err(crate::error::domain(
            Fault::precondition("evidence incomplete"),
            ErrorReason::EvidenceIncomplete,
        ));
    }
    let raw = bytes.get(string(m, "engineArtifactName")).ok_or_else(|| {
        crate::error::domain(
            Fault::precondition("engine artifact missing"),
            ErrorReason::EvidenceIncomplete,
        )
    })?;
    let bundle: Bundle = serde_json::from_slice(raw)?;
    // Reject unmodeled nested JSON too: the core allows additive fields for local compatibility,
    // but the hosted privacy boundary cannot accept uninspected extensions.
    if serde_json::from_slice::<Value>(raw)? != serde_json::to_value(&bundle)? {
        return Err(crate::error::domain(
            Fault::precondition("unmodeled engine fields are not eligible for hosting"),
            ErrorReason::ArtifactIneligible,
        ));
    }
    if !bundle.config.conditions.environment.variables.is_empty() {
        return Err(crate::error::domain(
            Fault::precondition("environment variables must not be uploaded"),
            ErrorReason::ArtifactIneligible,
        ));
    }
    let frozen = bundle.config.clone().freeze().map_err(engine_error)?;
    let cfg = frozen.config();
    let spec = &e["specification"];
    let (owner, name) = cfg
        .repository
        .as_str()
        .split_once('/')
        .ok_or_else(|| Fault::invalid("repository identity required"))?;
    let expected = crate::preparation::verify_spec(spec, &json!({"owner":owner,"name":name}))?;
    if expected != *cfg
        || crate::preparation::policy(&m["policy"])
            .map_err(|_| Fault::precondition("policy mapping mismatch"))?
            != cfg.policy
    {
        return Err(Fault::precondition(
            "complete engine configuration mapping mismatch",
        ));
    }

    if frozen.fingerprint().as_str() != string(m, "experimentFingerprintSha256") {
        return Err(crate::error::domain(
            Fault::precondition("engine provenance mismatch"),
            ErrorReason::FingerprintMismatch,
        ));
    }
    if cfg.incremental_base.as_str() != string(m, "sourceBaseSha")
        || cfg.initial_base.as_str() != string(spec, "initialBaseSha")
        || cfg.conditions.harness.as_str() != string(m, "harnessSha")
    {
        return Err(Fault::precondition("engine provenance mismatch"));
    }
    if types::fingerprint(&cfg.policy)
        .map_err(engine_error)?
        .as_str()
        != string(&m["policy"], "policySha256")
        || types::fingerprint(&cfg.conditions.environment)
            .map_err(engine_error)?
            .as_str()
            != string(&m["environment"], "environmentSha256")
    {
        return Err(Fault::precondition("policy or environment digest mismatch"));
    }
    let env = &m["environment"];
    if string(env, "operatingSystem") != cfg.conditions.environment.os.as_str()
        || string(env, "architecture") != cfg.conditions.environment.architecture.as_str()
        || string(env, "runner") != cfg.conditions.environment.runner.as_str()
        || string(env, "runtime") != cfg.conditions.toolchain.as_str()
        || env.get("buildOptions").cloned().unwrap_or(json!([]))
            != json!(cfg.conditions.build_options)
        || env.get("runtimeFlags").cloned().unwrap_or(json!([]))
            != json!(cfg.conditions.runtime_flags)
    {
        return Err(Fault::precondition("environment mapping mismatch"));
    }
    let inputs: BTreeMap<_, _> = m["inputs"]
        .as_array()
        .ok_or_else(|| Fault::invalid("inputs required"))?
        .iter()
        .map(|v| (string(v, "name"), string(v, "sha256")))
        .collect();
    if inputs.len() != cfg.conditions.inputs.len()
        || cfg
            .conditions
            .inputs
            .iter()
            .any(|(n, h)| inputs.get(n.as_str()) != Some(&h.as_str()))
    {
        return Err(Fault::precondition("input mapping mismatch"));
    }
    let cases = spec["cases"]
        .as_array()
        .ok_or_else(|| Fault::invalid("cases required"))?;
    if cases.len() != cfg.conditions.cases.len() {
        return Err(Fault::precondition("case mapping mismatch"));
    }
    for case in cases {
        let c = cfg
            .conditions
            .cases
            .iter()
            .find(|(n, _)| n.as_str() == string(case, "id"))
            .map(|(_, c)| c)
            .ok_or_else(|| Fault::precondition("case mismatch"))?;
        let frequency = case["frequency"]
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| Fault::precondition("case mapping mismatch"))?;
        let protected = match case.get("protectedCase") {
            None | Some(Value::Null) => false,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| Fault::precondition("case mapping mismatch"))?,
        };
        if c.family.as_str() != string(case, "family")
            || c.weight() != frequency
            || c.protected != protected
            || ((c.frequency.is_none())
                != (case["frequencySource"] == "FREQUENCY_SOURCE_ASSUMED_ONE"))
        {
            return Err(Fault::precondition("case mapping mismatch"));
        }
    }
    let tools = env["tools"]
        .as_array()
        .ok_or_else(|| Fault::invalid("tools required"))?;
    if tools.len() != cfg.conditions.tools.len()
        || tools.iter().any(|v| {
            cfg.conditions
                .tools
                .iter()
                .find(|(n, _)| n.as_str() == string(v, "name"))
                .is_none_or(|(_, version)| version.as_str() != string(v, "version"))
        })
    {
        return Err(Fault::precondition("tools mismatch"));
    }
    if cfg.limits.candidate_count as u64 != integer(&spec["limits"], "candidateCount")
        || cfg.limits.simultaneous_candidates as u64
            != integer(&spec["limits"], "simultaneousCandidates")
    {
        return Err(Fault::precondition("limits mismatch"));
    }
    let policy = &m["policy"];
    let (kind, min, max) = match &cfg.policy {
        policy::Policy::Ir(p) => (
            "POLICY_KIND_CALLGRIND_IR",
            p.minimum_improvement,
            p.maximum_case_regression,
        ),
        policy::Policy::Declared(p) => (
            "POLICY_KIND_DECLARED",
            p.minimum_improvement,
            p.maximum_case_regression,
        ),
        policy::Policy::MeasurementsOnly => ("POLICY_KIND_MEASUREMENTS_ONLY", 0.0, 0.0),
    };
    if string(policy, "kind") != kind
        || crate::preparation::optional_finite_number(policy, "minimumImprovement")
            .map_err(|_| Fault::precondition("policy mapping mismatch"))?
            != min
        || crate::preparation::optional_finite_number(policy, "maximumCaseRegression")
            .map_err(|_| Fault::precondition("policy mapping mismatch"))?
            != max
        || string(&policy["metric"], "unit") != cfg.conditions.metric.unit.as_str()
    {
        return Err(Fault::precondition("policy mapping mismatch"));
    }
    let metrics = m["metrics"].as_array();
    if metrics.is_none_or(|metrics| {
        metrics.len() != 1
            || metrics[0] != policy["metric"]
            || crate::preparation::metric(&json!({ "metric": metrics[0] })).ok()
                != Some(cfg.conditions.metric.metric)
    }) {
        return Err(Fault::precondition("metric mapping mismatch"));
    }
    let mut runs = BTreeMap::new();
    let mut inspected = BTreeSet::from([string(m, "engineArtifactName").to_owned()]);
    let mut verify = |manifest: &engine::Manifest,
                      source: &str,
                      candidate: bool|
     -> Result<engine::VerifiedRun> {
        let identity = &manifest.identity;
        // A validated baseline may also be the comparison base. Reuse only exact
        // manifests; one run identity must never describe different observations.
        let digest = types::fingerprint(manifest).map_err(engine_error)?;
        if runs
            .insert(identity.run.clone(), digest.clone())
            .is_some_and(|prior| prior != digest)
            || identity.experiment.as_str() != string(e, "id")
            || identity.source.as_str() != source
            || identity.candidate.as_ref().map(|v| v.as_str())
                != if candidate {
                    Some(string(m, "candidateId"))
                } else {
                    None
                }
        {
            return Err(Fault::precondition("run identity mismatch"));
        }
        if e["origin"] == "EXECUTION_ORIGIN_GITHUB_ACTIONS" {
            let exec = identity
                .external_execution
                .as_ref()
                .ok_or_else(Fault::denied)?;
            if exec.provider.as_str() != "github"
                || exec.workflow_run.as_str()
                    != integer(&e["githubExecution"], "githubRunId").to_string()
                || exec.attempt as u64 != integer(&e["githubExecution"], "runAttempt")
            {
                return Err(Fault::denied());
            }
        }
        // Every hash consumed by the engine must refer to the same retained byte object.
        let mut observations = BTreeSet::new();
        for (name, a) in &manifest.artifacts {
            let b = bytes
                .get(name.as_str())
                .ok_or_else(|| Fault::precondition("referenced artifact missing"))?;
            if hash(b) != a.sha256.as_str() {
                return Err(Fault::precondition("artifact mapping mismatch"));
            }
            let hosted = m["artifacts"]
                .as_array()
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|entry| string(entry, "name") == name.as_str())
                })
                .ok_or_else(|| Fault::precondition("artifact descriptor missing"))?;
            let (summary_kind, raw_kind) = match a.kind {
                engine::ArtifactKind::Measurement => {
                    ("ARTIFACT_KIND_MEASUREMENTS", "ARTIFACT_KIND_RAW_TEST_LOG")
                }
                engine::ArtifactKind::Profile => {
                    ("ARTIFACT_KIND_PROFILE_SUMMARY", "ARTIFACT_KIND_RAW_PROFILE")
                }
                engine::ArtifactKind::Correctness => (
                    "ARTIFACT_KIND_CORRECTNESS_RESULTS",
                    "ARTIFACT_KIND_RAW_TEST_LOG",
                ),
            };
            if string(hosted, "kind") == raw_kind {
                // Original native evidence remains hash-bound, consent-gated and inspected.
                // It supplements normalized observations; it cannot replace them.
                continue;
            }
            if string(hosted, "kind") != summary_kind {
                return Err(Fault::precondition("artifact kind mapping mismatch"));
            }
            let parsed: Value = serde_json::from_slice(b)?;
            let expected = match a.kind {
                engine::ArtifactKind::Measurement => serde_json::to_value(&manifest.measurements)?,
                engine::ArtifactKind::Profile => serde_json::to_value(&manifest.profile_paths)?,
                engine::ArtifactKind::Correctness => serde_json::to_value(&manifest.correctness)?,
            };
            if parsed != expected {
                return Err(Fault::precondition(
                    "structured artifact observations disagree with engine manifest",
                ));
            }
            inspected.insert(name.as_str().to_owned());
            observations.insert(summary_kind);
        }
        if observations.len() != 3 {
            return Err(Fault::precondition(
                "normalized measurement, profile and correctness artifacts required",
            ));
        }
        engine::verify(
            &frozen,
            identity,
            manifest.clone(),
            &|name: &types::Name| {
                bytes
                    .get(name.as_str())
                    .cloned()
                    .ok_or(types::Error::Incomplete("artifact"))
            },
        )
        .map_err(engine_error)
    };
    let b0 = verify(
        &bundle.incremental_baseline[0],
        cfg.incremental_base.as_str(),
        false,
    )?;
    let b1 = verify(
        &bundle.incremental_baseline[1],
        cfg.incremental_base.as_str(),
        false,
    )?;
    let baseline = engine::validate_baseline(&frozen, &b0, &b1).map_err(engine_error)?;
    let cumulative = if let Some(c) = &bundle.cumulative_baseline {
        let a = verify(&c[0], cfg.initial_base.as_str(), false)?;
        let b = verify(&c[1], cfg.initial_base.as_str(), false)?;
        Some(engine::validate_baseline(&frozen, &a, &b).map_err(engine_error)?)
    } else {
        None
    };
    let mut verify_round = |r: &Round| -> Result<VerifiedRound> {
        Ok(VerifiedRound {
            base: verify(&r.base, cfg.incremental_base.as_str(), false)?,
            head: verify(&r.head, string(m, "sourceHeadSha"), true)?,
            cumulative: r
                .cumulative_base
                .as_ref()
                .map(|c| verify(c, cfg.initial_base.as_str(), false))
                .transpose()?,
        })
    };
    let Some(first) = &bundle.first else {
        if !string(m, "candidateId").is_empty() || bundle.repeat.is_some() {
            return Err(Fault::precondition("candidate round required"));
        }
        require_inspected_artifacts(m, &inspected)?;
        return Ok(None);
    };
    let first = verify_round(first)?;
    let repeat = bundle.repeat.as_ref().map(&mut verify_round).transpose()?;
    require_inspected_artifacts(m, &inspected)?;
    fn round<'a>(
        r: &'a VerifiedRound,
        baseline: &'a engine::BaselineValidation,
        cumulative: Option<&'a engine::BaselineValidation>,
    ) -> Result<policy::Round<'a>> {
        Ok(policy::Round {
            incremental: policy::Comparison {
                base: &r.base,
                head: &r.head,
                baseline,
            },
            cumulative: match &r.cumulative {
                Some(base) => Some(policy::Comparison {
                    base,
                    head: &r.head,
                    baseline: cumulative
                        .ok_or_else(|| Fault::precondition("cumulative baseline missing"))?,
                }),
                None => None,
            },
        })
    }
    let first_round = round(&first, &baseline, cumulative.as_ref())?;
    let repeat_round = repeat
        .as_ref()
        .map(|r| round(r, &baseline, cumulative.as_ref()))
        .transpose()?;
    let result = policy::evaluate(&frozen, &first_round, repeat_round.as_ref(), &bundle.review)
        .map_err(engine_error)?;
    let verdict = match result.verdict {
        types::Verdict::Pass => "PERFORMANCE_VERDICT_PASS",
        types::Verdict::Regression => "PERFORMANCE_VERDICT_REGRESSION",
        types::Verdict::Inconclusive => "PERFORMANCE_VERDICT_INCONCLUSIVE",
        types::Verdict::NotEvaluated => "PERFORMANCE_VERDICT_NOT_EVALUATED",
    };
    let exceptions: Vec<i32> = result
        .exception
        .map(|v| match v {
            policy::Exception::EquallySimple => 1,
            policy::Exception::SingleUnprotectedCase => 2,
        })
        .into_iter()
        .collect();
    let comparison = |s: &policy::Statistics, kind: i32, base: &str| json!({"kind":kind,"baseSha":base,"headSha":m["sourceHeadSha"],"metric":policy["metric"],"cases":s.cases.iter().map(|(n,c)|json!({"caseId":n.as_str(),"baseValue":c.base,"headValue":c.head,"relativeChange":c.regression,"weightedAbsoluteChange":c.weighted_absolute_change})).collect::<Vec<_>>(),"portfolioScore":s.score,"improvement":s.improvement,"geometricMeanRatio":s.geometric_mean_ratio,"verdict":verdict,"exceptions":exceptions,"humanJudgmentRequired":result.human_judgment_required,"engineArtifactName":m["engineArtifactName"]});
    Ok(Some(
        json!({"_publicationAllowed":result.verdict==types::Verdict::Pass || result.reason==policy::Reason::HumanJudgment, "_measuredPaths":first.head.manifest().profile_paths.values().flatten().map(|p|p.as_str()).collect::<BTreeSet<_>>(), "id":m["candidateId"],"experimentId":e["id"],"baseSha":m["sourceBaseSha"],"headSha":m["sourceHeadSha"],"executionState":"EXECUTION_STATE_COMPLETED","verdict":verdict,"comparisons":[comparison(&result.incremental,2,cfg.incremental_base.as_str()),comparison(&result.cumulative,3,cfg.initial_base.as_str())],"correctness":first.head.manifest().correctness.iter().map(|(n,v)|json!({"checkName":n.as_str(),"state":match v {engine::CheckResult::Passed=>1,engine::CheckResult::Failed=>2,engine::CheckResult::NotRun=>3}})).collect::<Vec<_>>(),"exceptions":exceptions,"humanJudgmentRequired":result.human_judgment_required,"evidenceManifestSha256":m["manifestSha256"],"parentCandidateIds":spec.get("parentCandidateIds").cloned().unwrap_or(json!([]))}),
    ))
}

fn require_inspected_artifacts(m: &Value, inspected: &BTreeSet<String>) -> Result<()> {
    // Raw originals have their own consent and privacy checks. Every other file must
    // have passed exact typed inspection above, including baseline-only uploads.
    for artifact in m["artifacts"].as_array().ok_or_else(Fault::unavailable)? {
        if !matches!(
            string(artifact, "kind"),
            "ARTIFACT_KIND_RAW_PROFILE" | "ARTIFACT_KIND_RAW_TEST_LOG"
        ) && !inspected.contains(string(artifact, "name"))
        {
            return Err(crate::error::domain(
                Fault::precondition("unreferenced structured artifact is not eligible for hosting"),
                ErrorReason::ArtifactIneligible,
            ));
        }
    }
    Ok(())
}
