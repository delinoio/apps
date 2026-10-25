// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{Json, Router, extract::Request, http::StatusCode, response::IntoResponse};
use buffa::Message;
use perfine_api::evaluation::{Limits, evidence as engine, policy, types::*};
use perfine_api::{Server, auth::Actions, github::GitHub, storage::R2};
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};
fn uid() -> String {
    uuid::Uuid::now_v7().to_string()
}
fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}
fn fixture(eid: &str) -> (Value, Value, BTreeMap<String, Vec<u8>>) {
    let conditions = Conditions {
        harness: CommitSha::new("b".repeat(40)).unwrap(),
        workflow: CommitSha::new("c".repeat(40)).unwrap(),
        inputs: BTreeMap::from([(name("input"), digest(b"input"))]),
        tools: BTreeMap::from([(name("callgrind"), name("3.25"))]),
        toolchain: name("rustc"),
        build_options: vec![],
        runtime_flags: vec![],
        environment: Environment {
            os: name("linux"),
            architecture: name("x86_64"),
            runner: name("fixture"),
            hardware: BTreeMap::from([(name("cpu"), name("fixture"))]),
            variables: BTreeMap::new(),
        },
        metric: MetricSpec {
            metric: Metric::Ir,
            unit: name("instructions"),
        },
        cases: BTreeMap::from([(
            name("case"),
            Case {
                family: name("family"),
                frequency: Some(1.0),
                protected: false,
            },
        )]),
        correctness_checks: vec![name("tests")],
    };
    let config = engine::Config {
        schema_version: Version,
        repository: name("owner/repo"),
        initial_base: CommitSha::new("a".repeat(40)).unwrap(),
        incremental_base: CommitSha::new("a".repeat(40)).unwrap(),
        conditions,
        policy: policy::Policy::Ir(policy::IrPolicy::default()),
        limits: Limits {
            candidate_count: 1,
            simultaneous_candidates: 1,
            start_duration_seconds: 60,
            candidate_duration_seconds: 30,
        },
        workload_confirmation: name("confirmed"),
    };
    let frozen = config.clone().freeze().unwrap();
    let candidate = ResourceId::generate();
    let mut bytes = BTreeMap::new();
    let mut manifest = |n: &str, head: bool| {
        let measurements = BTreeMap::from([(
            name("case"),
            engine::Samples {
                values: vec![if head { 800.0 } else { 1000.0 }],
            },
        )]);
        let correctness = BTreeMap::from([(name("tests"), engine::CheckResult::Passed)]);
        let paths = BTreeMap::from([(name("case"), BTreeSet::from([name("src/hot.rs")]))]);
        let artifacts = [
            (
                "measurement",
                engine::ArtifactKind::Measurement,
                serde_json::to_vec(&measurements).unwrap(),
            ),
            (
                "profile",
                engine::ArtifactKind::Profile,
                serde_json::to_vec(&paths).unwrap(),
            ),
            (
                "tests",
                engine::ArtifactKind::Correctness,
                serde_json::to_vec(&correctness).unwrap(),
            ),
        ]
        .into_iter()
        .map(|(label, kind, content)| {
            let key = format!("{n}-{label}");
            let a = engine::Artifact {
                kind,
                sha256: digest(&content),
                size: content.len() as u64,
            };
            bytes.insert(key.clone(), content);
            (name(&key), a)
        })
        .collect();
        engine::Manifest {
            schema_version: Version,
            identity: engine::RunIdentity {
                experiment: ResourceId::new(eid).unwrap(),
                candidate: head.then_some(candidate.clone()),
                run: ResourceId::generate(),
                external_execution: None,
                source: CommitSha::new(if head { "d".repeat(40) } else { "a".repeat(40) }).unwrap(),
                configuration: frozen.fingerprint().clone(),
                conditions: fingerprint(&config.conditions).unwrap(),
            },
            conditions: config.conditions.clone(),
            policy: config.policy.clone(),
            completion: engine::Completion::Complete,
            measurements,
            correctness,
            artifacts,
            profile_paths: paths,
        }
    };
    let bundle = perfine_api::evidence::Bundle {
        schema_version: Version,
        config: config.clone(),
        incremental_baseline: [manifest("baseline1", false), manifest("baseline2", false)],
        cumulative_baseline: None,
        first: Some(perfine_api::evidence::Round {
            base: manifest("base", false),
            head: manifest("head", true),
            cumulative_base: None,
        }),
        repeat: None,
        review: policy::ReviewEvidence::default(),
    };
    bytes.insert("engine".into(), serde_json::to_vec(&bundle).unwrap());
    let environment = json!({"operatingSystem":"linux","architecture":"x86_64","runner":"fixture","runtime":"rustc","hardware":{"cpu":"fixture"},"tools":[{"name":"callgrind","version":"3.25"}],"environmentSha256":fingerprint(&config.conditions.environment).unwrap()});
    let policy = json!({"schemaVersion":1,"kind":"POLICY_KIND_CALLGRIND_IR","metric":{"name":"Ir","unit":"instructions","objective":"METRIC_OBJECTIVE_MINIMIZE","engineKind":"ENGINE_METRIC_KIND_IR"},"minimumImprovement":0.01,"maximumCaseRegression":0.01,"repetitions":1,"allowedExceptions":["POLICY_EXCEPTION_BELOW_MINIMUM_IMPROVEMENT","POLICY_EXCEPTION_SINGLE_UNPROTECTED_REGRESSION"],"uncertainty":{"engineSchemaVersion":1,"nearThreshold":0.001,"repeatTolerance":0.001},"policySha256":fingerprint(&config.policy).unwrap()});
    let spec = json!({"schemaVersion":1,"baseSha":"a".repeat(40),"initialBaseSha":"a".repeat(40),"harnessSha":"b".repeat(40),"language":"LANGUAGE_RUST","environment":environment,"policy":policy,"cases":[{"id":"case","family":"family","frequency":1.0,"frequencySource":"FREQUENCY_SOURCE_DECLARED"}],"limits":{"candidateCount":1,"simultaneousCandidates":1,"startDeadline":(chrono::Utc::now()+chrono::Duration::seconds(60)).to_rfc3339(),"candidateTimeout":"30s"},"fingerprintSha256":frozen.fingerprint(),"workloadRepresentativenessConfirmed":true,"workflowSha":config.conditions.workflow,"inputs":[{"name":"input","sha256":digest(b"input")}],"correctnessChecks":["tests"],"workloadConfirmation":"confirmed","startDuration":"60s"});
    let manifest = json!({"schemaVersion":1,"experimentId":eid,"candidateId":candidate,"sourceBaseSha":"a".repeat(40),"sourceHeadSha":"d".repeat(40),"harnessSha":"b".repeat(40),"environment":environment,"inputs":[{"name":"input","sha256":digest(b"input")}],"policy":policy,"metrics":[policy["metric"].clone()],"artifacts":bytes.iter().map(|(n,b)|json!({"name":n,"kind":if n=="engine"{"ARTIFACT_KIND_ENGINE_EVIDENCE"}else if n.ends_with("measurement"){"ARTIFACT_KIND_MEASUREMENTS"}else if n.ends_with("profile"){"ARTIFACT_KIND_PROFILE_SUMMARY"}else{"ARTIFACT_KIND_CORRECTNESS_RESULTS"},"mediaType":"application/json","sizeBytes":b.len().to_string(),"sha256":perfine_api::hash(b),"completion":"COMPLETION_STATE_COMPLETE","requiredForEvaluation":true})).collect::<Vec<_>>(),"completion":"COMPLETION_STATE_COMPLETE","experimentFingerprintSha256":frozen.fingerprint(),"engineArtifactName":"engine","runStartedAt":chrono::Utc::now().to_rfc3339()});
    let mut m: delino_protocol::perfine::v1::EvidenceManifest =
        serde_json::from_value(manifest).unwrap();
    m.manifest_sha256 = perfine_api::hash(&m.encode_to_vec());
    let spec: delino_protocol::perfine::v1::ExperimentSpecification =
        serde_json::from_value(spec).unwrap();
    (
        serde_json::to_value(spec).unwrap(),
        serde_json::to_value(m).unwrap(),
        bytes,
    )
}
fn configuration(spec: &Value) -> Value {
    let mut c = spec.clone();
    c["environment"]["environmentSha256"] = json!("");
    c["policy"]["policySha256"] = json!("");
    c["limits"]["startDuration"] = spec["startDuration"].clone();
    // Round-trip the public input type, not an ad hoc hidden engine JSON request.
    serde_json::to_value(
        serde_json::from_value::<delino_protocol::perfine::v1::ExperimentConfiguration>(c).unwrap(),
    )
    .unwrap()
}
#[test]
fn preparation_freezes_exact_policy_and_rejects_statistical_aliases() {
    let (spec, _, _) = fixture(&uid());
    let mut c = configuration(&spec);
    let repo = json!({"owner":"owner","name":"repo"});
    let now = chrono::Utc::now();
    let prepared = perfine_api::preparation::prepare(&c, &repo, now).unwrap();
    assert_eq!(
        prepared["specification"]["fingerprintSha256"],
        spec["fingerprintSha256"]
    );
    let cfg: engine::Config =
        serde_json::from_str(prepared["engineConfigJson"].as_str().unwrap()).unwrap();
    assert_eq!(
        cfg.freeze().unwrap().fingerprint().as_str(),
        spec["fingerprintSha256"].as_str().unwrap()
    );
    c["policy"]["uncertainty"]["confidenceLevel"] = json!(0.95);
    assert!(perfine_api::preparation::prepare(&c, &repo, now).is_err());
    c = configuration(&spec);
    c["policy"]["kind"] = json!("POLICY_KIND_DECLARED");
    c["policy"]["metric"] = json!({"name":"latency","unit":"ns","engineKind":"ENGINE_METRIC_KIND_TIME","objective":"METRIC_OBJECTIVE_MINIMIZE"});
    c["policy"]["repetitions"] = json!(3);
    c["policy"]["allowedExceptions"] = json!([]);
    c["policy"]["uncertainty"]["maximumRelativeRange"] = json!(0.05);
    c["policy"]["uncertainty"]["decisionMargin"] = json!(0.02);
    let prepared = perfine_api::preparation::prepare(&c, &repo, now).unwrap();
    let cfg: engine::Config =
        serde_json::from_str(prepared["engineConfigJson"].as_str().unwrap()).unwrap();
    let policy::Policy::Declared(p) = cfg.policy else {
        panic!("declared policy")
    };
    assert_eq!(p.uncertainty.maximum_relative_range, 0.05);
    assert_eq!(p.uncertainty.decision_margin, 0.02);
    assert_eq!(p.uncertainty.repeat_tolerance, 0.001);
    c["policy"]["uncertainty"]
        .as_object_mut()
        .unwrap()
        .remove("decisionMargin");
    assert!(perfine_api::preparation::prepare(&c, &repo, now).is_err());
    let mut altered = spec.clone();
    altered["limits"]["candidateCount"] = json!(2);
    assert!(perfine_api::preparation::verify_spec(&altered, &repo).is_err());
}
#[test]
fn policy_ratios_reject_non_numbers_without_changing_valid_fingerprints() {
    let (spec, _, _) = fixture(&uid());
    let repo = json!({"owner":"owner","name":"repo"});
    let now = chrono::Utc::now();
    let baseline = configuration(&spec);
    let prepared = perfine_api::preparation::prepare(&baseline, &repo, now).unwrap();
    assert_eq!(
        prepared["specification"]["fingerprintSha256"],
        spec["fingerprintSha256"]
    );
    let mut protobuf_json = baseline.clone();
    protobuf_json["policy"]["minimumImprovement"] = json!("NaN");
    let typed: delino_protocol::perfine::v1::ExperimentConfiguration =
        serde_json::from_value(protobuf_json).unwrap();
    let converted = serde_json::to_value(typed).unwrap();
    assert_eq!(converted["policy"]["minimumImprovement"], "NaN");
    assert_eq!(
        perfine_api::preparation::prepare(&converted, &repo, now)
            .unwrap_err()
            .code,
        connectrpc::ErrorCode::InvalidArgument
    );

    for key in ["minimumImprovement", "maximumCaseRegression"] {
        for value in [
            json!("NaN"),
            json!("Infinity"),
            json!("-Infinity"),
            json!(true),
            json!([]),
        ] {
            let mut config = baseline.clone();
            config["policy"][key] = value.clone();
            assert_eq!(
                perfine_api::preparation::prepare(&config, &repo, now)
                    .unwrap_err()
                    .code,
                connectrpc::ErrorCode::InvalidArgument,
                "prepare {key}={value}"
            );
            let mut altered = spec.clone();
            altered["policy"][key] = value.clone();
            assert_eq!(
                perfine_api::preparation::verify_spec(&altered, &repo)
                    .unwrap_err()
                    .code,
                connectrpc::ErrorCode::InvalidArgument,
                "verify {key}={value}"
            );
        }
        let mut omitted = baseline.clone();
        omitted["policy"].as_object_mut().unwrap().remove(key);
        assert!(perfine_api::preparation::prepare(&omitted, &repo, now).is_ok());
        omitted["policy"][key] = Value::Null;
        assert!(perfine_api::preparation::prepare(&omitted, &repo, now).is_ok());
    }

    for key in [
        "confidenceLevel",
        "maximumRelativeIntervalWidth",
        "nearThresholdMargin",
    ] {
        for value in [
            json!("NaN"),
            json!("Infinity"),
            json!("-Infinity"),
            json!(true),
            json!(0.5),
        ] {
            let mut config = baseline.clone();
            config["policy"]["uncertainty"][key] = value.clone();
            assert_eq!(
                perfine_api::preparation::prepare(&config, &repo, now)
                    .unwrap_err()
                    .code,
                connectrpc::ErrorCode::InvalidArgument,
                "prepare {key}={value}"
            );
            let mut altered = spec.clone();
            altered["policy"]["uncertainty"][key] = value.clone();
            assert_eq!(
                perfine_api::preparation::verify_spec(&altered, &repo)
                    .unwrap_err()
                    .code,
                connectrpc::ErrorCode::InvalidArgument,
                "verify {key}={value}"
            );
        }
        let mut accepted = baseline.clone();
        accepted["policy"]["uncertainty"][key] = json!(0);
        assert!(perfine_api::preparation::prepare(&accepted, &repo, now).is_ok());
        accepted["policy"]["uncertainty"][key] = Value::Null;
        assert!(perfine_api::preparation::prepare(&accepted, &repo, now).is_ok());
        accepted["policy"]["uncertainty"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(perfine_api::preparation::prepare(&accepted, &repo, now).is_ok());
    }
}
#[test]
fn malformed_manifest_policy_fails_before_evaluation() {
    let eid = uid();
    let (spec, manifest, bytes) = fixture(&eid);
    let experiment = json!({"id":eid,"origin":"EXECUTION_ORIGIN_LOCAL","specification":spec});
    for key in ["minimumImprovement", "maximumCaseRegression"] {
        for value in [
            json!("NaN"),
            json!("Infinity"),
            json!("-Infinity"),
            json!(true),
        ] {
            let mut altered = manifest.clone();
            altered["policy"][key] = value.clone();
            assert_eq!(
                perfine_api::evidence::evaluate(&experiment, &altered, &bytes)
                    .unwrap_err()
                    .code,
                connectrpc::ErrorCode::FailedPrecondition,
                "evaluate {key}={value}"
            );
        }
    }
}
#[test]
fn shared_evaluator_rejects_corrupt_or_incomplete_evidence() {
    let eid = uid();
    let (spec, m, mut bytes) = fixture(&eid);
    let e = json!({"id":eid,"origin":"EXECUTION_ORIGIN_LOCAL","specification":spec});
    let result = perfine_api::evidence::evaluate(&e, &m, &bytes)
        .unwrap()
        .unwrap();
    assert_eq!(result["verdict"], "PERFORMANCE_VERDICT_PASS");
    bytes.insert("base-measurement".into(), b"{}".to_vec());
    assert!(perfine_api::evidence::evaluate(&e, &m, &bytes).is_err());
    assert!(
        perfine_api::evidence::content_allowed(b"password=secret", "ARTIFACT_KIND_RAW_TEST_LOG")
            .is_err()
    );
    assert!(
        perfine_api::evidence::content_allowed(
            br#"{"diff":"content"}"#,
            "ARTIFACT_KIND_PROFILE_SUMMARY"
        )
        .is_err()
    );
}
#[test]
fn unreferenced_structured_artifacts_are_rejected_with_or_without_candidates() {
    for candidate in [true, false] {
        let eid = uid();
        let (spec, mut manifest, mut bytes) = fixture(&eid);
        let e = json!({"id":eid,"origin":"EXECUTION_ORIGIN_LOCAL","specification":spec});
        if !candidate {
            let mut bundle: perfine_api::evidence::Bundle =
                serde_json::from_slice(&bytes["engine"]).unwrap();
            bundle.first = None;
            manifest["candidateId"] = json!("");
            let retained: BTreeSet<_> = bundle
                .incremental_baseline
                .iter()
                .flat_map(|r| r.artifacts.keys().map(|n| n.as_str().to_owned()))
                .chain(std::iter::once("engine".into()))
                .collect();
            bytes.retain(|name, _| retained.contains(name));
            manifest["artifacts"]
                .as_array_mut()
                .unwrap()
                .retain(|a| retained.contains(a["name"].as_str().unwrap()));
            bytes.insert("engine".into(), serde_json::to_vec(&bundle).unwrap());
        }
        assert!(perfine_api::evidence::evaluate(&e, &manifest, &bytes).is_ok());
        let extra = br#"{"payload":"fn private_implementation() {}"}"#.to_vec();
        bytes.insert("extra".into(), extra.clone());
        for kind in [
            "ARTIFACT_KIND_PROFILE_SUMMARY",
            "ARTIFACT_KIND_MEASUREMENTS",
            "ARTIFACT_KIND_CORRECTNESS_RESULTS",
        ] {
            let mut m = manifest.clone();
            m["artifacts"].as_array_mut().unwrap().push(json!({"name":"extra","kind":kind,"mediaType":"application/json","sizeBytes":extra.len().to_string(),"sha256":perfine_api::hash(&extra),"completion":"COMPLETION_STATE_COMPLETE","containsSourceSnippets":false}));
            assert!(
                perfine_api::evidence::evaluate(&e, &m, &bytes).is_err(),
                "{candidate}: {kind}"
            );
        }
    }
}
#[test]
fn raw_profiles_cannot_replace_normalized_observations() {
    let eid = uid();
    let (spec, mut m, mut bytes) = fixture(&eid);
    add_raw(
        &mut m,
        &mut bytes,
        b"events: Ir\nfl=src/hot.rs\n1 10\n".to_vec(),
    );
    let e = json!({"id":eid,"origin":"EXECUTION_ORIGIN_LOCAL","specification":spec});
    assert!(perfine_api::evidence::evaluate(&e, &m, &bytes).is_ok());
    let mut bundle: perfine_api::evidence::Bundle =
        serde_json::from_slice(&bytes["engine"]).unwrap();
    bundle
        .first
        .as_mut()
        .unwrap()
        .head
        .artifacts
        .remove(&name("head-profile"));
    bytes.insert("engine".into(), serde_json::to_vec(&bundle).unwrap());
    assert!(perfine_api::evidence::evaluate(&e, &m, &bytes).is_err());
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL; all providers are mocks"]
async fn postgres_private_lifecycle_revocation_tombstone_and_retry() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("perfine_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(3))
        .after_connect(move |c, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {search}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for _ in 0..2 {
        sqlx::raw_sql(include_str!("../migrations/00001_perfine.sql"))
            .execute(&db)
            .await
            .unwrap();
    }
    let actor = uid();
    let space = uid();
    let grants = Arc::new(Mutex::new(vec!["manage".to_owned()]));
    let state = Arc::new(Mutex::new("TENANT_STATE_ACTIVE".to_owned()));
    let denied_delegate = Arc::new(Mutex::new(None::<String>));
    let denied = denied_delegate.clone();
    let objects = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
    let fail_delete = Arc::new(Mutex::new(false));
    let (g, st, o, fd, act) = (
        grants.clone(),
        state.clone(),
        objects.clone(),
        fail_delete.clone(),
        actor.clone(),
    );
    let pulls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let creates = Arc::new(Mutex::new(0u32));
    let creation_override = Arc::new(Mutex::new(None::<Value>));
    let response = creation_override.clone();
    let (p, posts) = (pulls.clone(), creates.clone());
    let app=Router::new().fallback(move|req:Request|{let (g,st,o,fd,act,p,posts,denied,response)=(g.clone(),st.clone(),o.clone(),fd.clone(),act.clone(),p.clone(),posts.clone(),denied.clone(),response.clone());async move{
  let path=req.uri().path().to_owned();let method=req.method().clone();let body=axum::body::to_bytes(req.into_body(),8<<20).await.unwrap();
  if path.starts_with("/bucket/") {
   let mut objects=o.lock().unwrap();return match method.as_str(){"PUT"=>{objects.insert(path,body.to_vec());StatusCode::OK.into_response()},"GET"=>objects.get(&path).cloned().map(IntoResponse::into_response).unwrap_or(StatusCode::NOT_FOUND.into_response()),"DELETE"=>if *fd.lock().unwrap(){StatusCode::SERVICE_UNAVAILABLE.into_response()}else{objects.remove(&path);StatusCode::NO_CONTENT.into_response()},_=>StatusCode::METHOD_NOT_ALLOWED.into_response()};
  }
  let q:Value=serde_json::from_slice(&body).unwrap_or(json!({}));let value=match path.as_str(){
   "/token"=>json!({"access_token":"fixture-service","expires_in":3600}),
   "/delcore.v1.DelcoreService/GetCurrentUser"=>json!({"userId":act}),
   "/delcore.v1.DelcoreService/GetTenantStates"=>json!({"states":q["tenantIds"].as_array().unwrap().iter().map(|id|json!({"tenantId":id,"state":*st.lock().unwrap()})).collect::<Vec<_>>()}),
   "/delcore.v1.DelcoreService/Authorize"=>{assert_eq!(q["paid"],false);json!({"allowed":g.lock().unwrap().iter().any(|v|q["capability"]==*v) && denied.lock().unwrap().as_ref().is_none_or(|subject| q["userId"] != *subject)})},
   "/repos/owner/repo/pulls" if method=="POST"=>{
    assert_eq!(q["draft"],true);*posts.lock().unwrap()+=1;
    if let Some(pr)=response.lock().unwrap().clone(){p.lock().unwrap().push(pr.clone());return Json(pr).into_response();}
    p.lock().unwrap().push(json!({"number":1,"html_url":"https://github.com/owner/repo/pull/1","title":q["title"],"body":q["body"],"draft":true,"head":{"sha":"d".repeat(40)},"base":{"sha":"a".repeat(40)}}));
    return StatusCode::SERVICE_UNAVAILABLE.into_response();
   },
   "/repos/owner/repo/pulls"=>json!(*p.lock().unwrap()),
   "/repos/owner/repo/actions/runs/7/attempts/2"=>json!({"id":7,"run_attempt":2,"repository":{"id":42},"head_repository":{"id":42},"actor":{"id":99},"head_sha":"d".repeat(40),"path":".github/workflows/perfine.yml","event":"workflow_dispatch"}),
   "/repos/owner/repo/actions/runs/1/cancel"=>json!({}),
   "/repos/owner/repo/actions/runs/1"=>json!({"id":1,"run_attempt":2,"status":"completed","conclusion":"success"}),
   "/repos/owner/repo/actions/runs/1/attempts/1"=>json!({"id":1,"run_attempt":1,"status":"completed","conclusion":"cancelled"}),
   "/repos/owner/repo/actions/runs/2/attempts/1"=>json!({"id":2,"run_attempt":2,"status":"completed","conclusion":"success"}),
   "/repos/owner/repo"=>json!({"id":42}),
   "/repos/owner/repo/commits/main"=>json!({"sha":"a".repeat(40)}),
   "/repos/owner/repo/commits/candidate"=>json!({"sha":"d".repeat(40)}),
   path if path.starts_with("/repos/owner/repo/compare/")=>json!({"total_commits":2,"status":"ahead","base_commit":{"sha":"a".repeat(40)},"merge_base_commit":{"sha":"a".repeat(40)},"commits":[{"sha":"e".repeat(40)},{"sha":"d".repeat(40)}],"files":[{"filename":"src/hot.rs","status":"modified"}]}),
   path if path.starts_with("/repos/owner/repo/commits/")=>{let sha=path.rsplit('/').next().unwrap();json!({"sha":sha,"parents":[{"sha":if sha=="e".repeat(40){"a".repeat(40)}else{"e".repeat(40)}}],"files":[{"filename":"src/hot.rs","status":"modified"}]})},
   "/user"=>json!({"id":7}),
   "/repositories/42"=>json!({"id":42,"full_name":"owner/repo","permissions":{"admin":true}}),
   "/app/installations/1/access_tokens"=>json!({"token":"fixture-installation"}),
   _=>return StatusCode::NOT_FOUND.into_response(),
  };Json(value).into_response()
 }});
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut server = Server {
        metrics: Default::default(),
        db: db.clone(),
        authority: Authority::new(
            http.clone(),
            base.clone(),
            format!("{base}/token"),
            "resource".into(),
            "client".into(),
            "fixture".into(),
        ),
        actions: Actions::new(http.clone(), format!("{base}/jwks"), actions_keys()),
        github: GitHub {
            http: http.clone(),
            base: base.clone(),
            app_id: "1".into(),
            key: jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!(
                "fixtures/github-test-key.pem"
            ))
            .unwrap(),
        },
        storage: R2 {
            endpoint: base,
            bucket: "bucket".into(),
            access: "fixture".into(),
            secret: "fixture".into(),
            http: http.clone(),
        },
        quota: 10_000_000,
        upload_limit: 2_000_000,
        rate: 100,
    };
    async fn call(s: &Server, space: &str, op: &str, mut q: Value) -> perfine_api::Result<Value> {
        let rid = uid();
        q["spaceId"] = json!(space);
        q["requestId"] = json!(rid);
        s.execute("Bearer fixture", "github-fixture", op, q, &rid)
            .await
    }
    let c=call(&server,&space,"ConnectRepository",json!({"repository":{"githubRepositoryId":"42","owner":"owner","name":"repo"},"githubInstallationId":"1"})).await.unwrap()["connection"].clone();
    let cid = c["id"].as_str().unwrap();
    // Block reconciliation after its candidate query, then commit a reconnect.
    // It must inspect the replacement state after acquiring the space lock.
    let race_run = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO perfine_experiments(id,space,connection,data) VALUES($1,$2,$3,$4)")
        .bind(race_run).bind(uuid::Uuid::parse_str(&space).unwrap()).bind(uuid::Uuid::parse_str(cid).unwrap())
        .bind(json!({"id":race_run,"spaceId":space,"connectionId":cid,"origin":"EXECUTION_ORIGIN_GITHUB_ACTIONS","executionState":"EXECUTION_STATE_RUNNING","githubExecution":{"githubRunId":"1","runAttempt":1}})).execute(&db).await.unwrap();
    sqlx::query("UPDATE perfine_connections SET active=false,data=jsonb_set(data,'{state}','\"CONNECTION_STATE_DISCONNECTED\"') WHERE id=$1")
        .bind(uuid::Uuid::parse_str(cid).unwrap()).execute(&db).await.unwrap();
    let worker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&db)
        .await
        .unwrap();
    let mut reconnect = admin.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&space)
        .execute(&mut *reconnect)
        .await
        .unwrap();
    *grants.lock().unwrap() = vec!["manage".into(), "experiment".into()];
    let replacement = async {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event='advisory')")
                    .bind(worker_pid).fetch_one(&admin).await.unwrap();
                if waiting { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("reconciliation must reach the held space lock");
        sqlx::query(&format!(
            "UPDATE {schema}.perfine_connections SET active=true,data=$2 WHERE id=$1"
        ))
        .bind(uuid::Uuid::parse_str(cid).unwrap())
        .bind(&c)
        .execute(&mut *reconnect)
        .await
        .unwrap();
        reconnect.commit().await.unwrap();
    };
    let (result, ()) = tokio::join!(server.maintain(), replacement);
    result.unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT active FROM perfine_connections WHERE id=$1")
            .bind(uuid::Uuid::parse_str(cid).unwrap())
            .fetch_one(&db)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT data->>'executionState' FROM perfine_experiments WHERE id=$1"
        )
        .bind(race_run)
        .fetch_one(&db)
        .await
        .unwrap(),
        "EXECUTION_STATE_CANCELLED"
    );
    // Even an attempt-scoped endpoint must not be trusted if its identity disagrees.
    sqlx::query("UPDATE perfine_experiments SET data=jsonb_set(jsonb_set(data,'{executionState}','\"EXECUTION_STATE_RUNNING\"'),'{githubExecution,githubRunId}','\"2\"') WHERE id=$1").bind(race_run).execute(&db).await.unwrap();
    server.maintain().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT data->>'executionState' FROM perfine_experiments WHERE id=$1"
        )
        .bind(race_run)
        .fetch_one(&db)
        .await
        .unwrap(),
        "EXECUTION_STATE_RUNNING"
    );
    sqlx::query("DELETE FROM perfine_experiments WHERE id=$1")
        .bind(race_run)
        .execute(&db)
        .await
        .unwrap();
    *grants.lock().unwrap() = vec!["manage".into()];

    assert!(
        call(
            &server,
            &space,
            "ListExperiments",
            json!({"connectionId":cid})
        )
        .await
        .is_err()
    );
    assert!(
        call(
            &server,
            &space,
            "StartExperiment",
            json!({"connectionId":cid,"experimentId":uid()})
        )
        .await
        .is_err()
    );
    assert!(call(&server,&uid(),"ConnectRepository",json!({"repository":{"githubRepositoryId":"42","owner":"owner","name":"repo"},"githubInstallationId":"1"})).await.is_err());
    *grants.lock().unwrap() = vec!["read".into(), "experiment".into(), "manage".into()];
    let eid = uid();
    let (spec, m, bytes) = fixture(&eid);
    // The connection creator has GitHub actor 7; actor 99 can run this authorized org workflow.
    let claims = json!({"iss":"https://token.actions.githubusercontent.com","aud":"https://perfine.api.delino.io","sub":"repo:owner/repo:ref:refs/heads/main","iat":chrono::Utc::now().timestamp(),"nbf":chrono::Utc::now().timestamp()-1,"exp":chrono::Utc::now().timestamp()+300,"repository":"owner/repo","repository_id":"42","run_id":"7","run_attempt":"2","sha":"d".repeat(40),"workflow_sha":"c".repeat(40),"ref":"refs/heads/main","workflow_ref":"owner/repo/.github/workflows/perfine.yml@refs/heads/main","event_name":"workflow_dispatch","actor_id":"99"});
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("fixture".into());
    let token = format!(
        "Actions {}",
        jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!(
                "fixtures/github-test-key.pem"
            ))
            .unwrap()
        )
        .unwrap()
    );
    let request = uid();
    let eligibility = json!({"spaceId":space,"requestId":request,"connectionId":cid,"manifest":m});
    assert!(
        server
            .execute(
                &token,
                "",
                "CheckUploadEligibility",
                eligibility.clone(),
                &request
            )
            .await
            .is_ok()
    );
    // Replace the delegate after the initial Actions check but before admission.
    let replacement_subject = uid();
    *denied_delegate.lock().unwrap() = Some(replacement_subject.clone());
    let mut reconnect = admin.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&space)
        .execute(&mut *reconnect)
        .await
        .unwrap();
    let replacement = async {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event='advisory')").bind(worker_pid).fetch_one(&admin).await.unwrap();
                if waiting { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("Actions admission must wait for the space lock");
        sqlx::query(&format!(
            "UPDATE {schema}.perfine_connections SET owner_subject=$2 WHERE id=$1"
        ))
        .bind(uuid::Uuid::parse_str(cid).unwrap())
        .bind(&replacement_subject)
        .execute(&mut *reconnect)
        .await
        .unwrap();
        reconnect.commit().await.unwrap();
    };
    let (result, ()) = tokio::join!(
        server.execute(
            &token,
            "",
            "CheckUploadEligibility",
            eligibility.clone(),
            &request
        ),
        replacement
    );
    assert_eq!(
        result.unwrap_err().code,
        connectrpc::ErrorCode::PermissionDenied
    );
    sqlx::query("UPDATE perfine_connections SET owner_subject=$2 WHERE id=$1")
        .bind(uuid::Uuid::parse_str(cid).unwrap())
        .bind(&actor)
        .execute(&db)
        .await
        .unwrap();
    *denied_delegate.lock().unwrap() = None;
    // A full batch of inaccessible repositories must not starve the next connection.
    let blocked_space = uuid::Uuid::now_v7();
    for repository in 1000..1100 {
        let blocked = uuid::Uuid::now_v7();
        let blocked_run = uuid::Uuid::now_v7();
        let mut data = c.clone();
        data["id"] = json!(blocked);
        data["spaceId"] = json!(blocked_space);
        data["repository"]["githubRepositoryId"] = json!(repository.to_string());
        data["_lastExecutionCheckAt"] = json!("2000-01-01T00:00:00Z");
        sqlx::query("INSERT INTO perfine_connections(id,space,repository,owner_subject,github_actor,active,data) VALUES($1,$2,$3,$4,'99',true,$5)")
            .bind(blocked).bind(blocked_space).bind(i64::from(repository)).bind(&actor).bind(data).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO perfine_experiments(id,space,connection,data) VALUES($1,$2,$3,$4)")
            .bind(blocked_run).bind(blocked_space).bind(blocked)
            .bind(json!({"id":blocked_run,"spaceId":blocked_space,"connectionId":blocked,"origin":"EXECUTION_ORIGIN_GITHUB_ACTIONS","executionState":"EXECUTION_STATE_RUNNING","githubExecution":{"githubRunId":"999","runAttempt":1}})).execute(&db).await.unwrap();
    }
    let eligible_run = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO perfine_experiments(id,space,connection,data) VALUES($1,$2,$3,$4)")
        .bind(eligible_run).bind(uuid::Uuid::parse_str(&space).unwrap()).bind(uuid::Uuid::parse_str(cid).unwrap())
        .bind(json!({"id":eligible_run,"spaceId":space,"connectionId":cid,"origin":"EXECUTION_ORIGIN_GITHUB_ACTIONS","executionState":"EXECUTION_STATE_RUNNING","githubExecution":{"githubRunId":"1","runAttempt":1}})).execute(&db).await.unwrap();
    sqlx::query("UPDATE perfine_connections SET data=jsonb_set(data,'{_lastExecutionCheckAt}','\"2001-01-01T00:00:00Z\"') WHERE id=$1")
        .bind(uuid::Uuid::parse_str(cid).unwrap()).execute(&db).await.unwrap();
    server.maintain().await.unwrap();
    let execution_state = || {
        sqlx::query_scalar::<_, String>(
            "SELECT data->>'executionState' FROM perfine_experiments WHERE id=$1",
        )
        .bind(eligible_run)
        .fetch_one(&db)
    };
    assert_eq!(execution_state().await.unwrap(), "EXECUTION_STATE_RUNNING");
    server.maintain().await.unwrap();
    assert_eq!(
        execution_state().await.unwrap(),
        "EXECUTION_STATE_CANCELLED"
    );
    sqlx::query("DELETE FROM perfine_experiments WHERE space=$1 OR id=$2")
        .bind(blocked_space)
        .bind(eligible_run)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM perfine_connections WHERE space=$1")
        .bind(blocked_space)
        .execute(&db)
        .await
        .unwrap();
    *grants.lock().unwrap() = vec!["manage".into()];
    assert!(
        server
            .execute(
                &token,
                "",
                "CheckUploadEligibility",
                eligibility.clone(),
                &request
            )
            .await
            .is_err()
    );
    *grants.lock().unwrap() = vec!["read".into(), "experiment".into(), "manage".into()];
    let mut wrong = eligibility.clone();
    wrong["spaceId"] = json!(uid());
    assert!(
        server
            .execute(&token, "", "CheckUploadEligibility", wrong, &request)
            .await
            .is_err()
    );
    let prepared = call(
        &server,
        &space,
        "PrepareExperiment",
        json!({"connectionId":cid,"configuration":configuration(&spec)}),
    )
    .await
    .unwrap();
    assert_eq!(
        prepared["specification"]["fingerprintSha256"],
        spec["fingerprintSha256"]
    );
    let mut invalid_configuration = configuration(&spec);
    invalid_configuration["policy"]["minimumImprovement"] = json!("NaN");
    assert_eq!(
        call(
            &server,
            &space,
            "PrepareExperiment",
            json!({"connectionId":cid,"configuration":invalid_configuration})
        )
        .await
        .unwrap_err()
        .code,
        connectrpc::ErrorCode::InvalidArgument
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perfine_experiments WHERE space=$1")
            .bind(uuid::Uuid::parse_str(&space).unwrap())
            .fetch_one(&server.db)
            .await
            .unwrap(),
        0
    );
    let mut fabricated = spec.clone();
    fabricated["fingerprintSha256"] = json!("f".repeat(64));
    assert!(call(&server,&space,"BeginUpload",json!({"connectionId":cid,"experimentId":eid,"specification":fabricated,"manifest":m,"origin":"EXECUTION_ORIGIN_LOCAL"})).await.is_err());

    let mut invalid_spec = spec.clone();
    invalid_spec["policy"]["maximumCaseRegression"] = json!("Infinity");
    assert_eq!(
        call(&server, &space, "BeginUpload", json!({"connectionId":cid,"experimentId":eid,"specification":invalid_spec,"manifest":m,"origin":"EXECUTION_ORIGIN_LOCAL"}))
            .await
            .unwrap_err()
            .code,
        connectrpc::ErrorCode::InvalidArgument
    );

    let q = json!({"connectionId":cid,"experimentId":eid,"specification":spec,"manifest":m,"origin":"EXECUTION_ORIGIN_LOCAL"});
    let original_quota = server.quota;
    server.quota = 1;
    assert!(
        call(&server, &space, "BeginUpload", q.clone())
            .await
            .is_err()
    );
    server.quota = original_quota;
    let mut raw = m.clone();
    raw["artifacts"].as_array_mut().unwrap().push(json!({"name":"raw","kind":"ARTIFACT_KIND_RAW_PROFILE","mediaType":"text/plain","sizeBytes":"2","sha256":perfine_api::hash(b"{}"),"completion":"COMPLETION_STATE_COMPLETE","containsSourceSnippets":true}));
    fn checksum(m: &mut Value) {
        let mut typed: delino_protocol::perfine::v1::EvidenceManifest =
            serde_json::from_value(m.clone()).unwrap();
        typed.manifest_sha256.clear();
        typed.manifest_sha256 = perfine_api::hash(&typed.encode_to_vec());
        *m = serde_json::to_value(typed).unwrap();
    }
    checksum(&mut raw);
    assert_eq!(
        call(
            &server,
            &space,
            "CheckUploadEligibility",
            json!({"connectionId":cid,"manifest":raw})
        )
        .await
        .unwrap()["eligibleForCompleteEvaluation"],
        false
    );
    call(&server,&space,"SetRawArtifactConsent",json!({"connectionId":cid,"expectedRevision":"0","allowedKinds":["ARTIFACT_KIND_RAW_PROFILE"]})).await.unwrap();
    assert!(
        call(
            &server,
            &space,
            "SetRawArtifactConsent",
            json!({"connectionId":cid,"expectedRevision":"0"})
        )
        .await
        .is_err()
    );
    raw["consentRevision"] = json!("1");
    raw["runStartedAt"] = json!(chrono::Utc::now().to_rfc3339());
    checksum(&mut raw);
    assert_eq!(
        call(
            &server,
            &space,
            "CheckUploadEligibility",
            json!({"connectionId":cid,"manifest":raw})
        )
        .await
        .unwrap()["eligibleForCompleteEvaluation"],
        true
    );
    call(
        &server,
        &space,
        "SetRawArtifactConsent",
        json!({"connectionId":cid,"expectedRevision":"1","allowedKinds":[]}),
    )
    .await
    .unwrap();
    assert_eq!(
        call(
            &server,
            &space,
            "CheckUploadEligibility",
            json!({"connectionId":cid,"manifest":raw})
        )
        .await
        .unwrap()["eligibleForCompleteEvaluation"],
        false
    );
    call(&server,&space,"SetRawArtifactConsent",json!({"connectionId":cid,"expectedRevision":"2","allowedKinds":["ARTIFACT_KIND_RAW_PROFILE"]})).await.unwrap();
    // Exercise the complete storage/evaluation path for all official profile families.
    for (language, source) in [
        ("LANGUAGE_RUST", "hot.rs"),
        ("LANGUAGE_C", "hot.c"),
        ("LANGUAGE_CPP", "hot.cpp"),
        ("LANGUAGE_ZIG", "hot.zig"),
        ("LANGUAGE_GO", "hot.go"),
        ("LANGUAGE_JAVASCRIPT", "hot.js"),
        ("LANGUAGE_GO", "secret.go"),
        ("LANGUAGE_RUST", "structured.rs"),
    ] {
        let raw_id = uid();
        let (mut rs, mut rm, mut rb) = fixture(&raw_id);
        rs["language"] = json!(language);
        let raw_bytes = if language == "LANGUAGE_GO" {
            use std::io::Write;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder
                .write_all(&[50, 0, 50, 6, b'h', b'o', b't', b'.', b'g', b'o'])
                .unwrap();
            if source == "secret.go" {
                encoder
                    .write_all(&[50, 5, b'g', b'h', b'p', b'_', b'x'])
                    .unwrap();
            }
            let encoded = encoder.finish().unwrap();
            if source == "secret.go" {
                encoded
            } else {
                include_bytes!("fixtures/go-darwin-arm64.pprof").to_vec()
            }
        } else if language == "LANGUAGE_JAVASCRIPT" {
            serde_json::to_vec(
                &json!({"nodes":[{"callFrame":{"url":source,"functionName":"hot"}}],"samples":[1]}),
            )
            .unwrap()
        } else {
            format!("version: 1\nevents: Ir\nfl={source}\nfn=hot\n1 10\n").into_bytes()
        };
        let raw_hash = perfine_api::hash(&raw_bytes);
        add_raw(&mut rm, &mut rb, raw_bytes);
        if source == "structured.rs" {
            let extra = br#"{"payload":"fn private_implementation() {}"}"#.to_vec();
            rm["artifacts"].as_array_mut().unwrap().push(json!({"name":"extra","kind":"ARTIFACT_KIND_PROFILE_SUMMARY","mediaType":"application/json","sizeBytes":extra.len().to_string(),"sha256":perfine_api::hash(&extra),"completion":"COMPLETION_STATE_COMPLETE","containsSourceSnippets":false}));
            rb.insert("extra".into(), extra);
        }

        rm["consentRevision"] = json!("3");
        rm["runStartedAt"] = json!(chrono::Utc::now().to_rfc3339());
        checksum(&mut rm);
        let begin = call(&server,&space,"BeginUpload",json!({"connectionId":cid,"experimentId":raw_id,"specification":rs,"manifest":rm,"origin":"EXECUTION_ORIGIN_LOCAL"})).await.unwrap();
        for t in begin["targets"].as_array().unwrap() {
            http.put(t["signedPutUrl"].as_str().unwrap())
                .body(rb[t["artifactName"].as_str().unwrap()].clone())
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
        }
        // Revocation after registration blocks completion, even with containsSourceSnippets=false.
        if language == "LANGUAGE_GO" {
            call(
                &server,
                &space,
                "SetRawArtifactConsent",
                json!({"connectionId":cid,"expectedRevision":"3","allowedKinds":[]}),
            )
            .await
            .unwrap();
            assert!(
                call(
                    &server,
                    &space,
                    "CompleteUpload",
                    json!({"uploadId":begin["upload"]["id"]})
                )
                .await
                .is_err()
            );
            // Restore the fixture's pre-run consent record only in the disposable test DB.
            sqlx::query("UPDATE perfine_connections SET data=jsonb_set(data,'{consent}', $1) WHERE id=$2").bind(json!({"revision":"3","allowedKinds":["ARTIFACT_KIND_RAW_PROFILE"],"effectiveAt":"2020-01-01T00:00:00Z"})).bind(uuid::Uuid::parse_str(cid).unwrap()).execute(&db).await.unwrap();
        }
        if matches!(source, "secret.go" | "structured.rs") {
            *fail_delete.lock().unwrap() = true;
            assert!(
                call(
                    &server,
                    &space,
                    "CompleteUpload",
                    json!({"uploadId":begin["upload"]["id"]})
                )
                .await
                .is_err()
            );
            let rejected: i64 = sqlx::query_scalar("SELECT count(*) FROM perfine_rejected_objects")
                .fetch_one(&db)
                .await
                .unwrap();
            assert!(
                rejected > 0,
                "rejection must durably enqueue cleanup with one pool connection"
            );
            assert_eq!(
                sqlx::query_scalar::<_, String>(
                    "SELECT data->>'state' FROM perfine_uploads WHERE id=$1"
                )
                .bind(uuid::Uuid::parse_str(begin["upload"]["id"].as_str().unwrap()).unwrap())
                .fetch_one(&db)
                .await
                .unwrap(),
                "UPLOAD_STATE_FAILED"
            );
            let upload_id = uuid::Uuid::parse_str(begin["upload"]["id"].as_str().unwrap()).unwrap();
            let expiry: chrono::DateTime<chrono::Utc> =
                sqlx::query_scalar("SELECT staging_expires_at FROM perfine_uploads WHERE id=$1")
                    .bind(upload_id)
                    .fetch_one(&db)
                    .await
                    .unwrap();
            let retry = call(&server,&space,"BeginUpload",json!({"connectionId":cid,"experimentId":raw_id,"specification":rs,"manifest":rm,"origin":"EXECUTION_ORIGIN_LOCAL"})).await.unwrap();
            assert_eq!(retry["upload"]["state"], "UPLOAD_STATE_FAILED");
            assert!(retry["targets"].as_array().unwrap().is_empty());
            assert_eq!(
                sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
                    "SELECT staging_expires_at FROM perfine_uploads WHERE id=$1"
                )
                .bind(upload_id)
                .fetch_one(&db)
                .await
                .unwrap(),
                expiry
            );
            let error = call(
                &server,
                &space,
                "CompleteUpload",
                json!({"uploadId":begin["upload"]["id"]}),
            )
            .await
            .unwrap_err();
            assert_eq!(error.message, "rejected upload cannot be completed");
            // No reservation is released while a signed PUT can still recreate bytes.
            *fail_delete.lock().unwrap() = false;
            server.maintain().await.unwrap();
            let reserved: i64 =
                sqlx::query_scalar("SELECT reserved_bytes FROM perfine_uploads WHERE id=$1")
                    .bind(upload_id)
                    .fetch_one(&db)
                    .await
                    .unwrap();
            assert!(reserved > 0);
            sqlx::query("UPDATE perfine_uploads SET staging_expires_at=now()-interval '1 minute' WHERE id=$1")
                .bind(upload_id).execute(&db).await.unwrap();
            *fail_delete.lock().unwrap() = true;
            server.maintain().await.unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT reserved_bytes FROM perfine_uploads WHERE id=$1"
                )
                .bind(upload_id)
                .fetch_one(&db)
                .await
                .unwrap(),
                reserved
            );
            *fail_delete.lock().unwrap() = false;
            sqlx::query("UPDATE perfine_rejected_objects SET expires_at=now()-interval '1 minute',next_attempt=now()")
                .execute(&db).await.unwrap();
            server.maintain().await.unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perfine_rejected_objects")
                    .fetch_one(&db)
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT reserved_bytes FROM perfine_uploads WHERE id=$1"
                )
                .bind(upload_id)
                .fetch_one(&db)
                .await
                .unwrap(),
                0
            );
            server.maintain().await.unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT reserved_bytes FROM perfine_uploads WHERE id=$1"
                )
                .bind(upload_id)
                .fetch_one(&db)
                .await
                .unwrap(),
                0
            );
            // Removing the cleanup record must not permit a later URL renewal.
            let retry = call(&server,&space,"BeginUpload",json!({"connectionId":cid,"experimentId":raw_id,"specification":rs,"manifest":rm,"origin":"EXECUTION_ORIGIN_LOCAL"})).await.unwrap();
            assert_eq!(retry["upload"]["state"], "UPLOAD_STATE_FAILED");
            assert!(retry["targets"].as_array().unwrap().is_empty());
            let prefix = format!("/bucket/{raw_id}/");
            assert!(
                !objects
                    .lock()
                    .unwrap()
                    .keys()
                    .any(|key| key.starts_with(&prefix))
            );
            assert!(call(&server,&space,"GetEvidenceDownload",json!({"experimentId":raw_id,"manifestSha256":rm["manifestSha256"],"artifactName":"native-profile"})).await.is_err());
            call(
                &server,
                &space,
                "DeleteExperiment",
                json!({"experimentId":raw_id}),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE perfine_uploads SET staging_expires_at=now()-interval '1 minute'")
                .execute(&db)
                .await
                .unwrap();
            server.maintain().await.unwrap();
            continue;
        }
        call(
            &server,
            &space,
            "CompleteUpload",
            json!({"uploadId":begin["upload"]["id"]}),
        )
        .await
        .unwrap();
        let link=call(&server,&space,"GetEvidenceDownload",json!({"experimentId":raw_id,"manifestSha256":rm["manifestSha256"],"artifactName":"native-profile"})).await.unwrap();
        let retained = http
            .get(link["signedGetUrl"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(perfine_api::hash(&retained), raw_hash);
        call(
            &server,
            &space,
            "DeleteExperiment",
            json!({"experimentId":raw_id}),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE perfine_uploads SET staging_expires_at=now()-interval '1 minute'")
            .execute(&db)
            .await
            .unwrap();
        server.maintain().await.unwrap();
    }
    let page = call(
        &server,
        &space,
        "ListCloudTools",
        json!({"connectionId":cid,"page":{"pageSize":1}}),
    )
    .await
    .unwrap();
    assert_eq!(page["tools"].as_array().unwrap().len(), 1);
    let second = call(
        &server,
        &space,
        "ListCloudTools",
        json!({"connectionId":cid,"page":{"pageSize":1,"cursor":page["page"]["nextCursor"]}}),
    )
    .await
    .unwrap();
    assert_ne!(page["tools"][0]["toolId"], second["tools"][0]["toolId"]);
    let begin = call(&server, &space, "BeginUpload", q.clone())
        .await
        .unwrap();
    let upload = begin["upload"]["id"].as_str().unwrap();
    for t in begin["targets"].as_array().unwrap() {
        let name = t["artifactName"].as_str().unwrap();
        http.put(t["signedPutUrl"].as_str().unwrap())
            .body(bytes[name].clone())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    let completed = call(
        &server,
        &space,
        "CompleteUpload",
        json!({"uploadId":upload}),
    )
    .await
    .unwrap();
    assert_eq!(completed["evidenceCompletion"], "COMPLETION_STATE_COMPLETE");
    let e = call(
        &server,
        &space,
        "GetExperiment",
        json!({"experimentId":eid}),
    )
    .await
    .unwrap();
    assert_eq!(e["experiment"]["verdict"], "PERFORMANCE_VERDICT_PASS");
    let mut invalid_upload_manifest = m.clone();
    invalid_upload_manifest["runStartedAt"] =
        json!((chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339());
    checksum(&mut invalid_upload_manifest);
    let second_begin = call(
        &server,
        &space,
        "BeginUpload",
        json!({"connectionId":cid,"experimentId":eid,"specification":spec,"manifest":invalid_upload_manifest,"origin":"EXECUTION_ORIGIN_LOCAL"}),
    )
    .await
    .unwrap();
    let second_upload = second_begin["upload"]["id"].as_str().unwrap();
    for target in second_begin["targets"].as_array().unwrap() {
        let name = target["artifactName"].as_str().unwrap();
        http.put(target["signedPutUrl"].as_str().unwrap())
            .body(bytes[name].clone())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    invalid_upload_manifest["policy"]["minimumImprovement"] = json!("NaN");
    checksum(&mut invalid_upload_manifest);
    sqlx::query(
        "UPDATE perfine_uploads SET data=jsonb_set(data,'{manifest}',$2::jsonb) WHERE id=$1",
    )
    .bind(uuid::Uuid::parse_str(second_upload).unwrap())
    .bind(&invalid_upload_manifest)
    .execute(&db)
    .await
    .unwrap();
    assert_eq!(
        call(
            &server,
            &space,
            "CompleteUpload",
            json!({"uploadId":second_upload})
        )
        .await
        .unwrap_err()
        .code,
        connectrpc::ErrorCode::FailedPrecondition
    );
    let download = call(
        &server,
        &space,
        "GetEvidenceDownload",
        json!({"experimentId":eid,"manifestSha256":m["manifestSha256"],"artifactName":"engine"}),
    )
    .await
    .unwrap();
    assert!(
        download["signedGetUrl"]
            .as_str()
            .unwrap()
            .contains("X-Amz-Expires=900")
    );
    *grants.lock().unwrap() = vec!["publish".into()];
    let draft = json!({"targetRepository":c["repository"],"headRepository":c["repository"],"baseRef":"main","headRef":"candidate","baseSha":"a".repeat(40),"headSha":"d".repeat(40),"title":"Optimize measured path","body":"Verified fixture evidence","candidateId":m["candidateId"],"evidenceManifestSha256":m["manifestSha256"]});
    // A signed job cannot turn upload authority into a human approval.
    let denied_request = uid();
    assert!(server.execute(&token,"","ApproveDraft",json!({"requestId":denied_request,"spaceId":space,"experimentId":eid,"draft":draft}),&denied_request).await.is_err());
    let approval = call(
        &server,
        &space,
        "ApproveDraft",
        json!({"experimentId":eid,"draft":draft}),
    )
    .await
    .unwrap();
    let aid = approval["approval"]["id"].as_str().unwrap();
    let request = uid();
    let mut approved =
        json!({"requestId":request,"spaceId":space,"experimentId":eid,"draft":draft});
    let first = server
        .execute(
            "Bearer fixture",
            "",
            "ApproveDraft",
            approved.clone(),
            &request,
        )
        .await
        .unwrap();
    let replay = server
        .execute(
            "Bearer fixture",
            "",
            "ApproveDraft",
            approved.clone(),
            &request,
        )
        .await
        .unwrap();
    assert_eq!(first, replay);
    approved["draft"]["title"] = json!("Different");
    assert!(
        server
            .execute("Bearer fixture", "", "ApproveDraft", approved, &request)
            .await
            .is_err()
    );

    let mut changed = draft.clone();
    changed["title"] = json!("Changed");
    assert!(
        call(
            &server,
            &space,
            "PublishDraft",
            json!({"approvalId":aid,"expectedDraft":changed})
        )
        .await
        .is_err()
    );
    call(
        &server,
        &space,
        "PublishDraft",
        json!({"approvalId":aid,"expectedDraft":draft}),
    )
    .await
    .unwrap();
    // An entire first batch has invalid approvers. Their failures must not
    // prevent the later valid approval from running in the next batch.
    for _ in 0..20 {
        let blocked = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO perfine_approvals(id,experiment,data,publication,created_at) SELECT $1,experiment,jsonb_set(jsonb_set(data,'{id}',to_jsonb($1::text)),'{approverSubject}','\"invalid-subject\"'::jsonb),jsonb_set(publication,'{approvalId}',to_jsonb($1::text)),created_at-interval '1 day' FROM perfine_approvals WHERE id=$2")
            .bind(blocked).bind(uuid::Uuid::parse_str(aid).unwrap()).execute(&db).await.unwrap();
    }
    *grants.lock().unwrap() = vec!["read".into(), "experiment".into(), "manage".into()];
    server.maintain().await.unwrap();
    assert_eq!(*creates.lock().unwrap(), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM perfine_approvals WHERE publication ? '_nextAttemptAt'"
        )
        .fetch_one(&db)
        .await
        .unwrap(),
        20
    );
    grants.lock().unwrap().push("publish".into());
    server.maintain().await.unwrap();
    let status = call(&server, &space, "GetPublication", json!({"approvalId":aid}))
        .await
        .unwrap();
    assert_eq!(
        status["publication"]["state"],
        "PUBLICATION_STATE_RECONCILING"
    );
    let delayed = std::mem::take(&mut *pulls.lock().unwrap());
    sqlx::query(
        "UPDATE perfine_approvals SET publication=publication-'_nextAttemptAt' WHERE id=$1",
    )
    .bind(uuid::Uuid::parse_str(aid).unwrap())
    .execute(&db)
    .await
    .unwrap();
    server.maintain().await.unwrap();
    assert_eq!(*creates.lock().unwrap(), 1);
    *pulls.lock().unwrap() = delayed;
    sqlx::query(
        "UPDATE perfine_approvals SET publication=publication-'_nextAttemptAt' WHERE id=$1",
    )
    .bind(uuid::Uuid::parse_str(aid).unwrap())
    .execute(&db)
    .await
    .unwrap();
    server.maintain().await.unwrap();
    let status = call(&server, &space, "GetPublication", json!({"approvalId":aid}))
        .await
        .unwrap();
    assert_eq!(
        status["publication"]["state"],
        "PUBLICATION_STATE_PUBLISHED"
    );
    assert_eq!(*creates.lock().unwrap(), 1);
    let again = call(
        &server,
        &space,
        "ApproveDraft",
        json!({"experimentId":eid,"draft":draft}),
    )
    .await
    .unwrap();
    call(
        &server,
        &space,
        "PublishDraft",
        json!({"approvalId":again["approval"]["id"],"expectedDraft":draft}),
    )
    .await
    .unwrap();
    server.maintain().await.unwrap();
    assert_eq!(*creates.lock().unwrap(), 1);
    // Branches can move after preflight and before GitHub processes the POST.
    for changed in ["base", "head", "title", "body", "draft", "none"] {
        // Each scenario uses a distinct fixture candidate with no previous dispatch.
        let candidate_id = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO perfine_candidates(id,experiment,data) SELECT $1,experiment,jsonb_set(data,'{id}',to_jsonb($1::text)) FROM perfine_candidates WHERE id=$2")
            .bind(candidate_id).bind(uuid::Uuid::parse_str(draft["candidateId"].as_str().unwrap()).unwrap()).execute(&db).await.unwrap();
        let mut draft = draft.clone();
        draft["candidateId"] = json!(candidate_id.to_string());
        pulls.lock().unwrap().clear();
        let mut created = json!({"number":2,"html_url":"https://github.com/owner/repo/pull/2","title":draft["title"],"body":draft["body"],"draft":true,"head":{"sha":draft["headSha"]},"base":{"sha":draft["baseSha"]}});
        match changed {
            "base" | "head" => created[changed]["sha"] = json!("f".repeat(40)),
            "title" | "body" => created[changed] = json!("Changed"),
            "draft" => created["draft"] = json!(false),
            _ => {}
        }
        *creation_override.lock().unwrap() = Some(created);
        let a = call(
            &server,
            &space,
            "ApproveDraft",
            json!({"experimentId":eid,"draft":draft}),
        )
        .await
        .unwrap();
        let aid = &a["approval"]["id"];
        call(
            &server,
            &space,
            "PublishDraft",
            json!({"approvalId":aid,"expectedDraft":draft}),
        )
        .await
        .unwrap();
        server.maintain().await.unwrap();
        let result = call(&server, &space, "GetPublication", json!({"approvalId":aid}))
            .await
            .unwrap();
        assert_eq!(
            result["publication"]["state"],
            if changed == "none" {
                "PUBLICATION_STATE_PUBLISHED"
            } else {
                "PUBLICATION_STATE_FAILED"
            },
            "{changed}"
        );
        let before = *creates.lock().unwrap();
        server.maintain().await.unwrap();
        assert_eq!(
            *creates.lock().unwrap(),
            before,
            "must not repeat a delivered POST"
        );
    }
    // A still-valid staging PUT must not change accepted bytes.
    http.put(begin["targets"][0]["signedPutUrl"].as_str().unwrap())
        .body("corrupt")
        .send()
        .await
        .unwrap();
    let downloaded = http
        .get(download["signedGetUrl"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(downloaded.as_ref(), bytes["engine"]);
    *grants.lock().unwrap() = vec!["manage".into()];
    assert!(
        call(
            &server,
            &space,
            "GetExperiment",
            json!({"experimentId":eid})
        )
        .await
        .is_err()
    );
    call(
        &server,
        &space,
        "DeleteExperiment",
        json!({"experimentId":eid}),
    )
    .await
    .unwrap();
    *grants.lock().unwrap() = vec!["read".into(), "experiment".into(), "manage".into()];
    assert!(
        call(
            &server,
            &space,
            "GetExperiment",
            json!({"experimentId":eid})
        )
        .await
        .is_err()
    );
    assert!(call(&server, &space, "BeginUpload", q).await.is_err());
    sqlx::query("UPDATE perfine_uploads SET staging_expires_at=now()-interval '1 minute'")
        .execute(&db)
        .await
        .unwrap();
    *fail_delete.lock().unwrap() = true;
    server.maintain().await.unwrap();
    let d = call(&server, &space, "GetDeletion", json!({"experimentId":eid}))
        .await
        .unwrap();
    assert_eq!(d["deletion"]["state"], "DELETION_STATE_RETRYING");
    *fail_delete.lock().unwrap() = false;
    sqlx::query("UPDATE perfine_cleanup SET next_attempt=now()")
        .execute(&db)
        .await
        .unwrap();
    server.maintain().await.unwrap();
    assert!(objects.lock().unwrap().is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perfine_tombstones")
            .fetch_one(&db)
            .await
            .unwrap(),
        9
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perfine_experiments")
            .fetch_one(&db)
            .await
            .unwrap(),
        0
    );
    let running = uid();
    sqlx::query("INSERT INTO perfine_experiments(id,space,connection,data) VALUES($1,$2,$3,$4)")
        .bind(uuid::Uuid::parse_str(&running).unwrap()).bind(uuid::Uuid::parse_str(&space).unwrap()).bind(uuid::Uuid::parse_str(cid).unwrap())
        .bind(json!({"id":running,"spaceId":space,"connectionId":cid,"origin":"EXECUTION_ORIGIN_GITHUB_ACTIONS","executionState":"EXECUTION_STATE_RUNNING","githubExecution":{"githubRunId":"1","runAttempt":1}})).execute(&db).await.unwrap();
    call(
        &server,
        &space,
        "DeleteExperiment",
        json!({"experimentId":running}),
    )
    .await
    .unwrap();
    server.maintain().await.unwrap();
    let retained: Value = sqlx::query_scalar("SELECT data FROM perfine_experiments WHERE id=$1")
        .bind(uuid::Uuid::parse_str(&running).unwrap())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(retained["executionState"], "EXECUTION_STATE_CANCELLED");
    assert_eq!(
        retained["cancellation"]["state"],
        "CANCELLATION_STATE_CONFIRMED"
    );
    sqlx::query("UPDATE perfine_cleanup SET next_attempt=now()")
        .execute(&db)
        .await
        .unwrap();
    server.maintain().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perfine_experiments")
            .fetch_one(&db)
            .await
            .unwrap(),
        0
    );
    let blocked_run = uuid::Uuid::now_v7();
    sqlx::query("UPDATE perfine_connections SET active=true,data=jsonb_set(data,'{state}','\"CONNECTION_STATE_CONNECTED\"') WHERE id=$1")
        .bind(uuid::Uuid::parse_str(cid).unwrap()).execute(&db).await.unwrap();
    sqlx::query("INSERT INTO perfine_experiments(id,space,connection,data) VALUES($1,$2,$3,$4)")
        .bind(blocked_run).bind(uuid::Uuid::parse_str(&space).unwrap()).bind(uuid::Uuid::parse_str(cid).unwrap())
        .bind(json!({"id":blocked_run,"spaceId":space,"connectionId":cid,"origin":"EXECUTION_ORIGIN_GITHUB_ACTIONS","executionState":"EXECUTION_STATE_RUNNING","githubExecution":{"githubRunId":"1","runAttempt":1}})).execute(&db).await.unwrap();
    *state.lock().unwrap() = "TENANT_STATE_BLOCKED".into();
    assert!(
        call(&server, &space, "ListConnections", json!({}))
            .await
            .is_err()
    );
    server.maintain().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perfine_preparations")
            .fetch_one(&db)
            .await
            .unwrap(),
        1
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT active FROM perfine_connections WHERE id=$1")
            .bind(uuid::Uuid::parse_str(cid).unwrap())
            .fetch_one(&db)
            .await
            .unwrap()
    );
    let stopped: Value = sqlx::query_scalar("SELECT data FROM perfine_experiments WHERE id=$1")
        .bind(blocked_run)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(
        stopped["cancellation"]["state"],
        "CANCELLATION_STATE_CONFIRMED"
    );
    *state.lock().unwrap() = "TENANT_STATE_ACTIVE".into();
    assert!(call(&server,&uid(),"ConnectRepository",json!({"repository":{"githubRepositoryId":"42","owner":"owner","name":"repo"},"githubInstallationId":"1"})).await.is_err());
    *state.lock().unwrap() = "TENANT_STATE_BLOCKED".into();
    sqlx::query("DELETE FROM perfine_experiments WHERE id=$1")
        .bind(blocked_run)
        .execute(&db)
        .await
        .unwrap();
    // A connection-only space must release its repository without losing cleanup identity.
    let connection_only_space = uuid::Uuid::now_v7();
    sqlx::query("UPDATE perfine_connections SET space=$2,active=true,data=jsonb_set(jsonb_set(data,'{state}','\"CONNECTION_STATE_CONNECTED\"'),'{spaceId}',to_jsonb($2::text)) WHERE id=$1")
        .bind(uuid::Uuid::parse_str(cid).unwrap()).bind(connection_only_space).execute(&db).await.unwrap();
    server.maintain().await.unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT active FROM perfine_connections WHERE id=$1")
            .bind(uuid::Uuid::parse_str(cid).unwrap())
            .fetch_one(&db)
            .await
            .unwrap()
    );
    *state.lock().unwrap() = "TENANT_STATE_DELETED".into();
    server.maintain().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM perfine_preparations")
            .fetch_one(&db)
            .await
            .unwrap(),
        0
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT active FROM perfine_connections WHERE id=$1")
            .bind(uuid::Uuid::parse_str(cid).unwrap())
            .fetch_one(&db)
            .await
            .unwrap()
    );
    *state.lock().unwrap() = "TENANT_STATE_ACTIVE".into();
    let replacement=call(&server,&uid(),"ConnectRepository",json!({"repository":{"githubRepositoryId":"42","owner":"owner","name":"repo"},"githubInstallationId":"1"})).await.unwrap();
    assert_ne!(replacement["connection"]["id"], c["id"]);
    // A full batch waiting for signed PUT expiry must yield to unvisited cleanup.
    let mut cleanup_ids = Vec::new();
    for index in 0..101 {
        let experiment = uuid::Uuid::now_v7();
        cleanup_ids.push(experiment);
        sqlx::query("INSERT INTO perfine_experiments(id,space,connection,data) VALUES($1,$2,$3,$4)")
            .bind(experiment).bind(connection_only_space).bind(uuid::Uuid::parse_str(cid).unwrap())
            .bind(json!({"id":experiment,"spaceId":connection_only_space,"connectionId":cid,"origin":"EXECUTION_ORIGIN_LOCAL","executionState":"EXECUTION_STATE_COMPLETED"})).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO perfine_cleanup(experiment,space,data,next_attempt) VALUES($1,$2,$3,now()-interval '1 hour')")
            .bind(experiment).bind(connection_only_space).bind(json!({"experimentId":experiment,"state":"DELETION_STATE_PENDING"})).execute(&db).await.unwrap();
        if index < 100 {
            sqlx::query("INSERT INTO perfine_uploads(id,experiment,data,reserved_bytes,staging_expires_at) VALUES($1,$2,$3,0,now()+interval '1 hour')")
                .bind(uuid::Uuid::now_v7()).bind(experiment).bind(json!({"state":"UPLOAD_STATE_PENDING"})).execute(&db).await.unwrap();
        }
    }
    server.maintain().await.unwrap();
    let healthy = cleanup_ids[100];
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT attempts FROM perfine_cleanup WHERE experiment=$1")
            .bind(healthy)
            .fetch_one(&db)
            .await
            .unwrap(),
        0
    );
    // Make retries due again while preserving relative due times, without sleeping.
    sqlx::query("UPDATE perfine_cleanup SET next_attempt=next_attempt-interval '2 minutes' WHERE experiment=ANY($1)").bind(&cleanup_ids).execute(&db).await.unwrap();
    server.maintain().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT data->>'state' FROM perfine_cleanup WHERE experiment=$1"
        )
        .bind(healthy)
        .fetch_one(&db)
        .await
        .unwrap(),
        "DELETION_STATE_COMPLETE"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM perfine_uploads WHERE experiment=ANY($1)"
        )
        .bind(&cleanup_ids)
        .fetch_one(&db)
        .await
        .unwrap(),
        100
    );
    db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    task.abort();
}

fn actions_keys() -> jsonwebtoken::jwk::JwkSet {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts};
    let private = RsaPrivateKey::from_pkcs8_pem(
        std::str::from_utf8(include_bytes!("fixtures/github-test-key.pem")).unwrap(),
    )
    .unwrap();
    serde_json::from_value(json!({"keys":[{"kty":"RSA","kid":"fixture","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())}]})).unwrap()
}
fn add_raw(m: &mut Value, bytes: &mut BTreeMap<String, Vec<u8>>, raw: Vec<u8>) {
    let mut bundle: perfine_api::evidence::Bundle =
        serde_json::from_slice(&bytes["engine"]).unwrap();
    bundle.first.as_mut().unwrap().head.artifacts.insert(
        name("native-profile"),
        engine::Artifact {
            kind: engine::ArtifactKind::Profile,
            sha256: digest(&raw),
            size: raw.len() as u64,
        },
    );
    m["artifacts"].as_array_mut().unwrap().push(json!({"name":"native-profile","kind":"ARTIFACT_KIND_RAW_PROFILE","mediaType":"application/octet-stream","sizeBytes":raw.len().to_string(),"sha256":perfine_api::hash(&raw),"completion":"COMPLETION_STATE_COMPLETE","requiredForEvaluation":true,"containsSourceSnippets":false}));
    bytes.insert("native-profile".into(), raw);
    bytes.insert("engine".into(), serde_json::to_vec(&bundle).unwrap());
    let engine = m["artifacts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|a| a["name"] == "engine")
        .unwrap();
    engine["sha256"] = json!(perfine_api::hash(&bytes["engine"]));
    engine["sizeBytes"] = json!(bytes["engine"].len().to_string());
}

#[test]
fn identical_baseline_reuse_is_valid_but_changed_run_identity_is_rejected() {
    let eid = uid();
    let (spec, mut m, mut bytes) = fixture(&eid);
    let e = json!({"id":eid,"origin":"EXECUTION_ORIGIN_LOCAL","specification":spec});
    let mut bundle: perfine_api::evidence::Bundle =
        serde_json::from_slice(&bytes["engine"]).unwrap();
    // Reusing the baseline replaces the comparison run and its artifact references.
    // Keep the fixture eligible under the hosted ban on uninspected extra JSON.
    for name in bundle.first.as_ref().unwrap().base.artifacts.keys() {
        bytes.remove(name.as_str());
        m["artifacts"]
            .as_array_mut()
            .unwrap()
            .retain(|a| a["name"] != name.as_str());
    }
    bundle.first.as_mut().unwrap().base = bundle.incremental_baseline[0].clone();
    bytes.insert("engine".into(), serde_json::to_vec(&bundle).unwrap());
    assert_eq!(
        perfine_api::evidence::evaluate(&e, &m, &bytes)
            .unwrap()
            .unwrap()["verdict"],
        "PERFORMANCE_VERDICT_PASS"
    );
    bundle
        .first
        .as_mut()
        .unwrap()
        .base
        .measurements
        .values_mut()
        .next()
        .unwrap()
        .values[0] += 1.0;
    bytes.insert("engine".into(), serde_json::to_vec(&bundle).unwrap());
    assert!(perfine_api::evidence::evaluate(&e, &m, &bytes).is_err());
}
