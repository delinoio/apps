// SPDX-License-Identifier: Apache-2.0
#[path = "../../../crates/perfine-core/tests/common/mod.rs"]
mod common;
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::allow_attributes,
    clippy::allow_attributes_without_reason,
    reason = "Canonical generated protocol code is owned by the generator"
)]
mod protocol {
    connectrpc::include_generated!();
}
use buffa::Message;
use common::*;
use perfine_core::{
    adapters::ArtifactStore, evidence::*, lifecycle::*, policy::*, storage::FileStore, types::*,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
};

fn run(root: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_perfine"))
        .arg("--repository")
        .arg(root)
        .arg("--json")
        .env("DELCORE_API_KEY", "fixture-key")
        .args(args)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)))
}
fn seeded(candidate: bool) -> (tempfile::TempDir, ResourceId, ResourceId) {
    seeded_values(candidate, [9950., 9950.])
}
fn seeded_values(candidate: bool, values: [f64; 2]) -> (tempfile::TempDir, ResourceId, ResourceId) {
    let root = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(root.path())
            .status()
            .unwrap()
            .success()
    );
    let cfg = config().freeze().unwrap();
    let id = ResourceId::generate();
    let cid = ResourceId::generate();
    let store = ArtifactStore::new(root.path().join(".perfine/artifacts")).unwrap();
    let engine = Engine::new(FileStore::open(root.path().join(".perfine/engine")).unwrap());
    let now = chrono::Utc::now().timestamp().unsigned_abs();
    engine
        .create(&request(), id.clone(), cfg.clone(), now)
        .unwrap();
    let make = |head: bool| {
        let mut m = manifest(
            &cfg,
            &id,
            head.then(|| cid.clone()),
            if head { sha('d') } else { sha('a') },
            if head { values } else { [10000., 10000.] },
        );
        m.artifacts.clear();
        for (kind, value) in [
            (ArtifactKind::Measurement, json!(m.measurements)),
            (ArtifactKind::Profile, json!(m.profile_paths)),
            (ArtifactKind::Correctness, json!(m.correctness)),
        ] {
            let bytes = serde_json::to_vec(&value).unwrap();
            let hash = store.put(&bytes).unwrap();
            m.artifacts.insert(
                name(hash.as_str()),
                Artifact {
                    kind,
                    sha256: hash,
                    size: bytes.len() as u64,
                },
            );
        }
        let verified = verify(&cfg, &m.identity, m.clone(), &store).unwrap();
        (m, verified)
    };
    let (a, av) = make(false);
    let (b, bv) = make(false);
    let (h, hv) = make(true);
    let baseline = validate_baseline(&cfg, &av, &bv).unwrap();
    let mut runs = serde_json::Map::new();
    let mut add = |label: String, m: &Manifest| {
        let hash = store.put(&serde_json::to_vec(m).unwrap()).unwrap();
        runs.insert(label, json!({"identity":m.identity,"artifact":hash}));
    };
    add("baseline-a".into(), &a);
    add("baseline-b".into(), &b);
    if candidate {
        engine
            .start_candidate(
                &request(),
                &id,
                cid.clone(),
                sha('d'),
                workspace(0),
                &av,
                &baseline,
                &BTreeSet::from([name("src/hot.rs")]),
                now,
            )
            .unwrap();
        let round = Round {
            incremental: Comparison {
                base: &av,
                head: &hv,
                baseline: &baseline,
            },
            cumulative: None,
        };
        engine
            .finish_candidate(
                &request(),
                &id,
                &cid,
                &round,
                None,
                &ReviewEvidence::default(),
                now,
            )
            .unwrap();
        add(format!("candidate-{}", cid.as_str()), &h);
    }
    engine.stop(&request(), &id, false, now).unwrap();
    fs::create_dir_all(root.path().join(".perfine").join(id.as_str())).unwrap();
    fs::write(root.path().join(".perfine").join(id.as_str()).join("journal.json"),serde_json::to_vec(&json!({"schema_version":1,"settings":digest(b"settings"),"language":"rust","optimize":candidate,"created_request":request(),"runs":runs,"failure":null,"attempts":{}})).unwrap()).unwrap();
    let command = json!({"executable":"true","arguments":[]});
    let settings = json!({"schema_version":1,"experiment":cfg.config(),"adapter":{"schema_version":1,"language":"rust","discovery_files":["src/hot.rs"],"build":[],"correctness":{"project-tests":command},"workloads":{"large":{"command":command,"profiles":["profile"],"format":"callgrind","output":{"kind":"callgrind"},"repetitions":1,"required_paths":["src/hot.rs"]}},"tool_probes":{"callgrind":command},"source_prefixes":{}},"docker":{"executable":"/usr/bin/docker","image":format!("sha256:{}","a".repeat(64)),"dependency_names":[],"user":"1:1","memory_bytes":1000000,"cpus":1,"process_limit":10,"command_timeout_seconds":10},"execution_root":"/tmp/perfine-acceptance","toolchain_probe":"callgrind","agent":null,"action_commit":null});
    fs::write(
        root.path().join("perfine.yaml"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    (root, id, cid)
}

#[test]
fn subprocess_review_confirmation_reuses_measurements_and_retains_attestation() {
    let (root, id, cid) = seeded(true);
    let report = run(
        root.path(),
        &[
            "compare",
            "--experiment",
            id.as_str(),
            "--candidate",
            cid.as_str(),
        ],
    );
    assert_ne!(
        report["result"]["candidate"]["evaluation"]["verdict"],
        "pass"
    );
    let dir = root.path().join(".perfine").join(id.as_str());
    let journal_before = fs::read(dir.join("journal.json")).unwrap();
    let journal: Value = serde_json::from_slice(&journal_before).unwrap();
    let hash = journal["runs"][format!("candidate-{}", cid.as_str())]["artifact"]
        .as_str()
        .unwrap();
    let m: Manifest = serde_json::from_slice(
        &fs::read(root.path().join(".perfine/artifacts").join(hash)).unwrap(),
    )
    .unwrap();
    let proof = m
        .artifacts
        .values()
        .find(|a| a.kind == ArtifactKind::Profile)
        .unwrap()
        .sha256
        .clone();
    let file = root.path().join("review.json");
    fs::write(&file,serde_json::to_vec(&json!({"schema_version":1,"statement":"Reviewer checked retained profile and local code: equally simple", "review":{"equally_simple_or_simpler":proof,"regression_explanation":null,"no_family_recurrence":null,"profile_aggregate_conflict":false}})).unwrap()).unwrap();
    let args = [
        "compare",
        "--experiment",
        id.as_str(),
        "--candidate",
        cid.as_str(),
        "--review",
        file.to_str().unwrap(),
    ];
    let preview = run(root.path(), &args);
    assert_eq!(preview["result"]["confirmation_required"], true);
    let token = preview["result"]["confirm_review"].as_str().unwrap();
    let mut confirmed = args.to_vec();
    confirmed.extend(["--confirm-review", token]);
    let result = run(root.path(), &confirmed);
    assert_eq!(
        result["result"]["candidate"]["evaluation"]["verdict"], "pass",
        "{result}"
    );
    assert_eq!(
        result["result"]["candidate"]["evaluation"]["exception"],
        "equally_simple"
    );
    assert_eq!(journal_before, fs::read(dir.join("journal.json")).unwrap());
    assert!(dir.join(format!("review-{}.json", cid.as_str())).exists());
    let mut bad: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    bad["review"]["equally_simple_or_simpler"] = json!(digest(b"absent"));
    fs::write(&file, serde_json::to_vec(&bad).unwrap()).unwrap();
    let preview = run(root.path(), &args);
    let token = preview["result"]["confirm_review"].as_str().unwrap();
    let mut confirmed = args.to_vec();
    confirmed.extend(["--confirm-review", token]);
    assert!(run(root.path(), &confirmed)["error"].is_object());
}

fn read_request(stream: &mut std::net::TcpStream) -> (String, String, Vec<u8>) {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut data = Vec::new();
    let mut byte = [0];
    while !data.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        data.push(byte[0]);
    }
    let headers = String::from_utf8(data).unwrap();
    let length = headers
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(|s| s.parse::<usize>().unwrap())
        })
        .unwrap();
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    (
        headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_owned(),
        headers,
        body,
    )
}
fn reply(stream: &mut std::net::TcpStream, status: u16, body: &Value) {
    let bytes = serde_json::to_vec(body).unwrap();
    write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();
    stream.write_all(&bytes).unwrap();
}
#[test]
fn subprocess_automatic_upload_checks_protobuf_hashes_retries_and_reconciles() {
    upload_roundtrip(false);
}
#[test]
fn subprocess_candidate_upload_preserves_real_baseline_reuse() {
    upload_roundtrip(true);
}
fn upload_roundtrip(candidate: bool) {
    let (root, id, cid) = seeded(candidate);
    // Upload must use retained provenance even after the local config is removed.
    fs::remove_file(root.path().join("perfine.yaml")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let base = endpoint.clone();
    let worker = std::thread::spawn(move || {
        let uid = ResourceId::generate();
        let mut manifest = Value::Null;
        let mut specification = Value::Null;
        let mut bytes = BTreeMap::new();
        let mut failed = false;
        let mut committed = false;
        let mut begin_ids = BTreeSet::new();
        loop {
            let (mut stream, _) = listener.accept().unwrap();
            let (path, headers, body) = read_request(&mut stream);
            if path.starts_with("/objects/") {
                assert!(!headers.to_ascii_lowercase().contains("authorization:"));
                if !failed {
                    failed = true;
                    reply(&mut stream, if candidate { 403 } else { 503 }, &json!({}));
                    continue;
                }
                bytes.insert(path.trim_start_matches("/objects/").to_owned(), body);
                reply(&mut stream, 200, &json!({}));
                continue;
            }
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-key")
            );
            let q: Value = serde_json::from_slice(&body).unwrap();
            ResourceId::new(q["requestId"].as_str().unwrap()).unwrap();
            let mut response = json!({"requestId":q["requestId"]});
            if path.ends_with("CheckUploadEligibility") {
                manifest = q["manifest"].clone();
                let mut typed: protocol::perfine::v1::EvidenceManifest =
                    serde_json::from_value(manifest.clone()).unwrap();
                let hash = typed.manifest_sha256.clone();
                typed.manifest_sha256.clear();
                assert_eq!(digest(&typed.encode_to_vec()).as_str(), hash);
                response["eligibleForCompleteEvaluation"] = json!(true);
                response["decisions"] = json!([]);
            } else if path.ends_with("BeginUpload") {
                assert!(
                    begin_ids.insert(q["requestId"].as_str().unwrap().to_owned()),
                    "resuming pending upload must refresh expired URLs with a new operation ID"
                );
                specification = q["specification"].clone();
                assert_eq!(specification["language"], "LANGUAGE_RUST");
                assert_eq!(q["manifest"], manifest);
                assert_eq!(
                    q["specification"]["fingerprintSha256"],
                    manifest["experimentFingerprintSha256"]
                );
                response["upload"] = json!({"id":uid,"state":"UPLOAD_STATE_PENDING"});
                response["targets"]=json!(manifest["artifacts"].as_array().unwrap().iter().map(|a|json!({"artifactName":a["name"],"signedPutUrl":format!("{base}/objects/{}",a["name"].as_str().unwrap()),"requiredHeaders":[{"name":"content-length","value":a["sizeBytes"]}]})).collect::<Vec<_>>());
            } else if path.ends_with("CompleteUpload") {
                for a in manifest["artifacts"].as_array().unwrap() {
                    let b = &bytes[a["name"].as_str().unwrap()];
                    assert_eq!(digest(b).as_str(), a["sha256"].as_str().unwrap());
                    assert_eq!(b.len().to_string(), a["sizeBytes"].as_str().unwrap());
                }
                let bundle: Value = serde_json::from_slice(&bytes["engine.json"]).unwrap();
                let cfg: Config = serde_json::from_value(bundle["config"].clone()).unwrap();
                let frozen = cfg.freeze().unwrap();
                for m in bundle["incremental_baseline"].as_array().unwrap() {
                    let m: Manifest = serde_json::from_value(m.clone()).unwrap();
                    for (kind, expected) in [
                        ("ARTIFACT_KIND_MEASUREMENTS", json!(m.measurements)),
                        ("ARTIFACT_KIND_PROFILE_SUMMARY", json!(m.profile_paths)),
                        ("ARTIFACT_KIND_CORRECTNESS_RESULTS", json!(m.correctness)),
                    ] {
                        assert!(
                            m.artifacts.keys().any(|name| manifest["artifacts"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|a| a["name"] == name.as_str()
                                    && a["kind"] == kind
                                    && serde_json::from_slice::<Value>(&bytes[name.as_str()])
                                        .unwrap()
                                        == expected)),
                            "normalized observations required"
                        );
                    }
                    verify(&frozen, &m.identity, m.clone(), &|n: &Name| {
                        Ok(bytes[n.as_str()].clone())
                    })
                    .unwrap();
                }
                assert_eq!(bundle["first"].is_null(), !candidate);
                if candidate {
                    assert_eq!(bundle["first"]["base"], bundle["incremental_baseline"][0]);
                }
                if let Ok(path) = std::env::var("PERFINE_ACCEPTANCE_EXPORT") {
                    let path = if candidate {
                        std::path::PathBuf::from(path).with_extension("candidate.json")
                    } else {
                        path.into()
                    };
                    fs::write(path, serde_json::to_vec(&json!({"specification":specification,"manifest":manifest,"bytes":bytes})).unwrap()).unwrap();
                }
                response["upload"] = json!({"id":uid,"state":"UPLOAD_STATE_COMPLETE"});
                response["evidenceCompletion"] = json!("COMPLETION_STATE_COMPLETE");
                committed = true;
            } else if path.ends_with("GetUpload") {
                response["upload"] = json!({"id":uid,"state":if committed {"UPLOAD_STATE_COMPLETE"} else {"UPLOAD_STATE_PENDING"}});
                if committed {
                    reply(&mut stream, 200, &response);
                    break;
                }
            } else {
                panic!("unexpected RPC {path}");
            }
            reply(&mut stream, 200, &response);
        }
    });
    let req = ResourceId::generate();
    let space = ResourceId::generate();
    let connection = ResourceId::generate();
    let mut args = vec![
        "--request-id",
        req.as_str(),
        "upload",
        "--experiment",
        id.as_str(),
        "--endpoint",
        &endpoint,
        "--space",
        space.as_str(),
        "--connection",
        connection.as_str(),
        "--consent-revision",
        "1",
    ];
    if candidate {
        args.extend(["--candidate", cid.as_str()]);
    }
    if candidate {
        let expired = run(root.path(), &args);
        assert!(
            expired["error"]["message"]
                .as_str()
                .unwrap()
                .contains("storage upload failed")
        );
    }
    let uploaded = run(root.path(), &args);
    assert_eq!(
        uploaded["result"]["completion"]["evidenceCompletion"], "COMPLETION_STATE_COMPLETE",
        "{uploaded}"
    );
    let reconciled = run(root.path(), &args);
    assert_eq!(reconciled["result"]["reconciled"], true, "{reconciled}");
    worker.join().unwrap();
}

#[test]
fn subprocess_upload_preserves_ineligible_incomplete_and_secret_evidence() {
    for status in [200, 401] {
        let (root, id, _) = seeded(false);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (_, _, body) = read_request(&mut stream);
            let q: Value = serde_json::from_slice(&body).unwrap();
            reply(
                &mut stream,
                status,
                &json!({"requestId":q["requestId"],"eligibleForCompleteEvaluation":false,"decisions":[{"artifactName":"raw","eligibility":"ARTIFACT_ELIGIBILITY_CONSENT_REQUIRED","reason":"manager consent required"}]}),
            );
        });
        let result = run(
            root.path(),
            &[
                "upload",
                "--experiment",
                id.as_str(),
                "--endpoint",
                &endpoint,
                "--space",
                ResourceId::generate().as_str(),
                "--connection",
                ResourceId::generate().as_str(),
            ],
        );
        worker.join().unwrap();
        if status == 200 {
            assert_eq!(result["result"]["uploaded"], false, "{result}");
            assert_eq!(result["result"]["evidence_completion"], "incomplete");
        } else {
            assert!(result["error"]["message"].as_str().unwrap().contains("401"));
        }
        assert!(root.path().join(".perfine/artifacts").is_dir());
    }
    let (root, id, _) = seeded(false);
    let path = root
        .path()
        .join(".perfine")
        .join(id.as_str())
        .join("journal.json");
    let mut journal: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let original = journal.clone();
    journal["runs"]
        .as_object_mut()
        .unwrap()
        .remove("baseline-b");
    fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    let args = [
        "upload",
        "--experiment",
        id.as_str(),
        "--endpoint",
        "https://invalid.example",
        "--space",
        "space",
        "--connection",
        "connection",
    ];
    assert!(
        run(root.path(), &args)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("incomplete evidence")
    );
    let mut journal = original;
    let store = ArtifactStore::new(root.path().join(".perfine/artifacts")).unwrap();
    let hash =
        Fingerprint::new(journal["runs"]["baseline-a"]["artifact"].as_str().unwrap()).unwrap();
    let mut m: Manifest = serde_json::from_slice(&store.get(&hash).unwrap()).unwrap();
    let bytes = b"authorization: bearer fixture-sensitive";
    let secret = store.put(bytes).unwrap();
    m.artifacts.insert(
        name(secret.as_str()),
        Artifact {
            kind: ArtifactKind::Correctness,
            sha256: secret.clone(),
            size: bytes.len() as u64,
        },
    );
    let hash = store.put(&serde_json::to_vec(&m).unwrap()).unwrap();
    journal["runs"]["baseline-a"]["artifact"] = json!(hash);
    fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    let rejected = run(root.path(), &args);
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("prohibited"),
        "{rejected}"
    );
    assert_eq!(store.get(&secret).unwrap(), bytes);
}

#[test]
fn subprocess_single_case_exception_requires_both_retained_attestations() {
    let (root, id, cid) = seeded_values(true, [8700., 10200.]);
    let journal: Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join(".perfine")
                .join(id.as_str())
                .join("journal.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let store = ArtifactStore::new(root.path().join(".perfine/artifacts")).unwrap();
    let hash = Fingerprint::new(
        journal["runs"][format!("candidate-{}", cid.as_str())]["artifact"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let m: Manifest = serde_json::from_slice(&store.get(&hash).unwrap()).unwrap();
    let proof = &m
        .artifacts
        .values()
        .find(|a| a.kind == ArtifactKind::Profile)
        .unwrap()
        .sha256;
    let path = root.path().join("review.json");
    fs::write(&path,serde_json::to_vec(&json!({"schema_version":1,"statement":"Reviewer confirmed profile explanation and no recurrence across families", "review":{"equally_simple_or_simpler":null,"regression_explanation":proof,"no_family_recurrence":proof,"profile_aggregate_conflict":false}})).unwrap()).unwrap();
    let args = [
        "compare",
        "--experiment",
        id.as_str(),
        "--candidate",
        cid.as_str(),
        "--review",
        path.to_str().unwrap(),
    ];
    let preview = run(root.path(), &args);
    let token = preview["result"]["confirm_review"].as_str().unwrap();
    let mut confirmed = args.to_vec();
    confirmed.extend(["--confirm-review", token]);
    let reviewed = run(root.path(), &confirmed);
    assert_eq!(
        reviewed["result"]["candidate"]["evaluation"]["verdict"], "pass",
        "{reviewed}"
    );
    assert_eq!(
        reviewed["result"]["candidate"]["evaluation"]["exception"],
        "single_unprotected_case"
    );
}
