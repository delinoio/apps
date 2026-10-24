// SPDX-License-Identifier: FSL-1.1-ALv2
//! Versioned wire-to-engine conversion, shared by preparation and evidence acceptance.
use crate::evaluation::{evidence::Config, policy, types};
use crate::{Fault, Result, integer, string};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn invalid() -> Fault {
    Fault::invalid("unsupported or incomplete engine v1 configuration")
}
fn number(v: &Value, key: &str) -> Result<f64> {
    v[key]
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(invalid)
}
pub fn duration(v: &Value, key: &str) -> Result<u64> {
    string(v, key)
        .strip_suffix('s')
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0 && *n <= 31_536_000)
        .ok_or_else(invalid)
}
pub fn metric(p: &Value) -> Result<types::Metric> {
    Ok(match string(&p["metric"], "engineKind") {
        "ENGINE_METRIC_KIND_IR" => types::Metric::Ir,
        "ENGINE_METRIC_KIND_TIME" => types::Metric::Time,
        "ENGINE_METRIC_KIND_THROUGHPUT" => types::Metric::Throughput,
        "ENGINE_METRIC_KIND_MEMORY" => types::Metric::Memory,
        "ENGINE_METRIC_KIND_CUSTOM" => types::Metric::Custom,
        _ => return Err(invalid()),
    })
}
pub fn policy(p: &Value) -> Result<policy::Policy> {
    if integer(p, "schemaVersion") != 1 {
        return Err(invalid());
    }
    let u = &p["uncertainty"];
    if [
        "confidenceLevel",
        "maximumRelativeIntervalWidth",
        "nearThresholdMargin",
    ]
    .iter()
    .any(|k| u[*k].as_f64().unwrap_or(0.0) != 0.0)
    {
        return Err(invalid());
    }
    let exceptions = p
        .get("allowedExceptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let result = match string(p, "kind") {
        "POLICY_KIND_MEASUREMENTS_ONLY" => {
            if integer(p, "repetitions") != 0
                || !exceptions.is_empty()
                || p["minimumImprovement"].as_f64().unwrap_or(0.0) != 0.0
                || p["maximumCaseRegression"].as_f64().unwrap_or(0.0) != 0.0
                || [
                    "maximumRelativeRange",
                    "decisionMargin",
                    "repeatTolerance",
                    "nearThreshold",
                ]
                .iter()
                .any(|k| u.get(*k).is_some_and(|v| !v.is_null()))
            {
                return Err(invalid());
            }
            policy::Policy::MeasurementsOnly
        }
        kind => {
            if integer(u, "engineSchemaVersion") != 1 {
                return Err(invalid());
            }
            let min = p["minimumImprovement"].as_f64().unwrap_or(0.0);
            let max = p["maximumCaseRegression"].as_f64().unwrap_or(0.0);
            let near = number(u, "nearThreshold")?;
            let repeat = number(u, "repeatTolerance")?;
            match kind {
                "POLICY_KIND_CALLGRIND_IR" => {
                    if integer(p, "repetitions") != 1
                        || ["maximumRelativeRange", "decisionMargin"]
                            .iter()
                            .any(|k| u.get(*k).is_some_and(|v| !v.is_null()))
                        || exceptions.iter().any(|e| {
                            e != "POLICY_EXCEPTION_BELOW_MINIMUM_IMPROVEMENT"
                                && e != "POLICY_EXCEPTION_SINGLE_UNPROTECTED_REGRESSION"
                        })
                        || string(&p["metric"], "objective") != "METRIC_OBJECTIVE_MINIMIZE"
                    {
                        return Err(invalid());
                    }
                    policy::Policy::Ir(policy::IrPolicy {
                        minimum_improvement: min,
                        maximum_case_regression: max,
                        near_threshold: near,
                        repeat_tolerance: repeat,
                        allow_simple_exception: exceptions
                            .iter()
                            .any(|e| e == "POLICY_EXCEPTION_BELOW_MINIMUM_IMPROVEMENT"),
                        allow_single_case_exception: exceptions
                            .iter()
                            .any(|e| e == "POLICY_EXCEPTION_SINGLE_UNPROTECTED_REGRESSION"),
                    })
                }
                "POLICY_KIND_DECLARED" => {
                    if !exceptions.is_empty() {
                        return Err(invalid());
                    }
                    policy::Policy::Declared(policy::DeclaredPolicy {
                        objective: match string(&p["metric"], "objective") {
                            "METRIC_OBJECTIVE_MINIMIZE" => policy::Objective::Minimize,
                            "METRIC_OBJECTIVE_MAXIMIZE" => policy::Objective::Maximize,
                            _ => return Err(invalid()),
                        },
                        minimum_improvement: min,
                        maximum_case_regression: max,
                        repetitions: integer(p, "repetitions")
                            .try_into()
                            .map_err(|_| invalid())?,
                        near_threshold: near,
                        uncertainty: policy::Uncertainty {
                            maximum_relative_range: number(u, "maximumRelativeRange")?,
                            decision_margin: number(u, "decisionMargin")?,
                            repeat_tolerance: repeat,
                        },
                    })
                }
                _ => return Err(invalid()),
            }
        }
    };
    result.validate(metric(p)?).map_err(|_| invalid())?;
    Ok(result)
}
fn array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    v[key].as_array().ok_or_else(invalid)
}
fn unique_map(v: &Value, key: &str, value_key: &str) -> Result<Value> {
    let mut map = serde_json::Map::new();
    for item in array(v, key)? {
        let name = string(item, "name");
        if name.is_empty() || map.insert(name.into(), item[value_key].clone()).is_some() {
            return Err(invalid());
        }
    }
    Ok(Value::Object(map))
}
pub fn prepare(c: &Value, repository: &Value, now: chrono::DateTime<chrono::Utc>) -> Result<Value> {
    let env = &c["environment"];
    if integer(c, "schemaVersion") != 1
        || !string(env, "environmentSha256").is_empty()
        || !string(&c["policy"], "policySha256").is_empty()
        || !string(env, "runtimeVersion").is_empty()
        || !matches!(
            string(c, "language"),
            "LANGUAGE_RUST"
                | "LANGUAGE_C"
                | "LANGUAGE_CPP"
                | "LANGUAGE_ZIG"
                | "LANGUAGE_GO"
                | "LANGUAGE_JAVASCRIPT"
        )
        || array(env, "tools")?
            .iter()
            .any(|t| !string(t, "configurationSha256").is_empty())
    {
        return Err(invalid());
    }
    if string(&c["policy"]["metric"], "name").is_empty()
        || !matches!(
            string(&c["policy"]["metric"], "objective"),
            "METRIC_OBJECTIVE_MINIMIZE" | "METRIC_OBJECTIVE_MAXIMIZE"
        )
    {
        return Err(invalid());
    }
    let policy = policy(&c["policy"])?;
    let inputs = unique_map(c, "inputs", "sha256")?;
    let mut cases = BTreeMap::new();
    for case in array(c, "cases")? {
        // Core v1 has global input identities. Case inputs may only reference that same map.
        if let Some(a) = case.get("inputs").and_then(Value::as_array) {
            for i in a {
                if inputs.get(string(i, "name")) != Some(&i["sha256"]) {
                    return Err(invalid());
                }
            }
        }
        let frequency = match string(case, "frequencySource") {
            "FREQUENCY_SOURCE_DECLARED" => json!(number(case, "frequency")?),
            "FREQUENCY_SOURCE_ASSUMED_ONE" if number(case, "frequency")? == 1.0 => Value::Null,
            _ => return Err(invalid()),
        };
        if cases.insert(string(case,"id"), json!({"family":case["family"],"frequency":frequency,"protected":case["protectedCase"].as_bool().unwrap_or(false)})).is_some() { return Err(invalid()); }
    }
    let limits = &c["limits"];
    let start = duration(limits, "startDuration")?;
    let timeout = duration(limits, "candidateTimeout")?;
    if integer(limits, "simultaneousCandidates") > integer(limits, "candidateCount") {
        return Err(invalid());
    }
    let cfg: Config = serde_json::from_value(json!({"schema_version":1,
        "repository":format!("{}/{}",string(repository,"owner"),string(repository,"name")),
        "initial_base":c["initialBaseSha"],"incremental_base":c["baseSha"],
        "conditions":{"harness":c["harnessSha"],"workflow":c["workflowSha"],"inputs":inputs,
            "tools":unique_map(env,"tools","version")?,"toolchain":env["runtime"],
            "build_options":env.get("buildOptions").cloned().unwrap_or(json!([])),
            "runtime_flags":env.get("runtimeFlags").cloned().unwrap_or(json!([])),
            "environment":{"os":env["operatingSystem"],"architecture":env["architecture"],"runner":env["runner"],"hardware":env["hardware"],"variables":{}},
            "metric":{"metric":metric(&c["policy"])?,"unit":c["policy"]["metric"]["unit"]},
            "cases":cases,"correctness_checks":c["correctnessChecks"]},
        "policy":policy,"limits":{"candidate_count":integer(limits,"candidateCount"),"simultaneous_candidates":integer(limits,"simultaneousCandidates"),"start_duration_seconds":start,"candidate_duration_seconds":timeout},
        "workload_confirmation":c["workloadConfirmation"]
    })).map_err(|_| invalid())?;
    let frozen = cfg.clone().freeze().map_err(|_| invalid())?;
    let engine_json = serde_json::to_string(&cfg)?;
    crate::evidence::content_allowed(engine_json.as_bytes(), "ARTIFACT_KIND_ENGINE_EVIDENCE")?;
    for parent in c
        .get("parentCandidateIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        crate::id(parent.as_str().ok_or_else(invalid)?)?;
    }
    let mut spec = c.clone();
    spec["fingerprintSha256"] = json!(frozen.fingerprint());
    spec["environment"]["environmentSha256"] =
        json!(types::fingerprint(&cfg.conditions.environment).map_err(|_| invalid())?);
    spec["policy"]["policySha256"] = json!(types::fingerprint(&cfg.policy).map_err(|_| invalid())?);
    spec["workloadRepresentativenessConfirmed"] = json!(true);
    spec["startDuration"] = json!(format!("{start}s"));
    spec["limits"]
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("startDuration");
    spec["limits"]["startDeadline"] =
        json!((now + chrono::Duration::seconds(start as i64)).to_rfc3339());
    let spec: delino_protocol::perfine::v1::ExperimentSpecification = serde_json::from_value(spec)?;
    Ok(json!({"specification":spec,"engineConfigJson":engine_json,"preparedAt":now.to_rfc3339()}))
}

/// Verify every frozen field against core, including uploads prepared by a local CLI.
pub fn verify_spec(s: &Value, repository: &Value) -> Result<Config> {
    let mut c = s.clone();
    c["environment"]["environmentSha256"] = json!("");
    c["policy"]["policySha256"] = json!("");
    c["limits"]["startDuration"] = s["startDuration"].clone();
    let result = prepare(&c, repository, chrono::Utc::now())?;
    let canonical = &result["specification"];
    if canonical["fingerprintSha256"] != s["fingerprintSha256"]
        || canonical["policy"]["policySha256"] != s["policy"]["policySha256"]
        || canonical["environment"]["environmentSha256"] != s["environment"]["environmentSha256"]
    {
        return Err(Fault::precondition("frozen engine fingerprint mismatch"));
    }
    Ok(serde_json::from_str(string(&result, "engineConfigJson"))?)
}
