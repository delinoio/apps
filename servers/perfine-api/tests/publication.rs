// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{Json, Router, extract::Request};
use perfine_api::github::GitHub;
use serde_json::json;
use std::sync::{Arc, Mutex};
#[tokio::test]
async fn verifies_all_commits_and_rejects_hidden_infrastructure_or_unmeasured_sources() {
    let base = "a".repeat(40);
    let middle = "b".repeat(40);
    let head = "c".repeat(40);
    let comparison = json!({"status":"ahead","total_commits":2,"base_commit":{"sha":base},"merge_base_commit":{"sha":base},"commits":[{"sha":middle},{"sha":head}],"files":[{"filename":"src/hot.go","status":"modified"}]});
    let first = json!({"sha":middle,"parents":[{"sha":base}],"files":[{"filename":"src/hot.go","status":"modified"}]});
    let second = json!({"sha":head,"parents":[{"sha":middle}],"files":[{"filename":"src/hot.go","status":"modified"}]});
    let responses = Arc::new(Mutex::new([
        comparison.clone(),
        first.clone(),
        second.clone(),
    ]));
    let r = responses.clone();
    let mid = middle.clone();
    let router = Router::new().fallback(move |req: Request| {
        let r = r.clone();
        let mid = mid.clone();
        async move {
            let i = if req.uri().path().contains("/compare/") {
                0
            } else if req.uri().path().ends_with(&mid) {
                1
            } else {
                2
            };
            Json(r.lock().unwrap()[i].clone())
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let github = GitHub {
        http: reqwest::Client::new(),
        base: api,
        app_id: "1".into(),
        key: jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!(
            "fixtures/github-test-key.pem"
        ))
        .unwrap(),
    };
    let draft = json!({"targetRepository":{"owner":"upstream","name":"repo"},"headRepository":{"owner":"fork","name":"repo"},"baseSha":base,"headSha":head,"evidenceManifestSha256":"f".repeat(64)});
    let candidate = json!({"_publicationAllowed":true,"baseSha":base,"headSha":head,"evidenceManifestSha256":"f".repeat(64),"_measuredPaths":["src/hot.go"]});
    assert!(
        github
            .production_diff(&draft, &candidate, "fixture")
            .await
            .is_ok()
    );
    for (index, field, value) in [
        (0, "total_commits", json!(3)),
        (0, "merge_base_commit", json!({"sha":"d".repeat(40)})),
        (0, "commits", json!([])),
        (0, "status", json!("diverged")),
        (1, "parents", json!([{ "sha":base }, {"sha":head}])),
        (
            1,
            "files",
            json!([{"filename":".github/workflows/temp.yml","status":"added"}]),
        ),
        (
            1,
            "files",
            json!([{"filename":"src/unmeasured.go","status":"modified"}]),
        ),
        (
            2,
            "files",
            json!([{"filename":"src/hot.go","status":"renamed"}]),
        ),
        (2, "parents", json!([{"sha":base}])),
    ] {
        *responses.lock().unwrap() = [comparison.clone(), first.clone(), second.clone()];
        responses.lock().unwrap()[index][field] = value;
        assert!(
            github
                .production_diff(&draft, &candidate, "fixture")
                .await
                .is_err(),
            "{index} {field}"
        );
    }
    *responses.lock().unwrap() = [comparison, first, second];
    for field in ["baseSha", "headSha", "evidenceManifestSha256"] {
        let mut stale = draft.clone();
        stale[field] = json!("d".repeat(40));
        assert!(
            github
                .production_diff(&stale, &candidate, "fixture")
                .await
                .is_err()
        );
    }
    task.abort();
}
