// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{Json, Router, extract::Request};
use perfine_api::github::{GitHub, PublicationTokens};
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
            assert_eq!(
                req.headers()["authorization"],
                if req.uri().path().contains("/compare/") {
                    "Bearer target-fixture"
                } else {
                    "Bearer head-fixture"
                }
            );
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
            .production_diff(
                &draft,
                &candidate,
                &PublicationTokens {
                    head: "head-fixture".into(),
                    target: "target-fixture".into()
                }
            )
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
                .production_diff(
                    &draft,
                    &candidate,
                    &PublicationTokens {
                        head: "head-fixture".into(),
                        target: "target-fixture".into()
                    }
                )
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
                .production_diff(
                    &stale,
                    &candidate,
                    &PublicationTokens {
                        head: "head-fixture".into(),
                        target: "target-fixture".into()
                    }
                )
                .await
                .is_err()
        );
    }
    task.abort();
}

#[tokio::test]
async fn private_fork_publication_uses_separate_exact_repository_installations() {
    use axum::body::to_bytes;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let router = Router::new().fallback(move |req: Request| {
        let seen = seen.clone();
        async move {
            let path = req.uri().path().to_string();
            let auth = req.headers()["authorization"].to_str().unwrap().to_string();
            let body = to_bytes(req.into_body(), 4096).await.unwrap();
            seen.lock().unwrap().push(path.clone());
            let response = match path.as_str() {
                "/app/installations/1/access_tokens" | "/app/installations/2/access_tokens" => {
                    let head = path.contains("/1/");
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(body["repository_ids"], json!([if head { 42 } else { 43 }]));
                    assert!(auth.starts_with("Bearer ey"));
                    json!({"token": if head { "head-token" } else { "target-token" }})
                }
                "/repos/upstream/repo/installation" => {
                    assert!(auth.starts_with("Bearer ey"));
                    json!({"id":2})
                }
                "/repositories/42" | "/repos/fork/repo" => {
                    assert_eq!(auth, "Bearer head-token");
                    json!({"id":42,"full_name":"fork/repo"})
                }
                "/repositories/43" | "/repos/upstream/repo" => {
                    assert_eq!(auth, "Bearer target-token");
                    json!({"id":43,"full_name":"upstream/repo"})
                }
                "/repos/upstream/repo/commits/main" => {
                    assert_eq!(auth, "Bearer target-token");
                    json!({"sha":"a".repeat(40)})
                }
                "/repos/fork/repo/commits/candidate" => {
                    assert_eq!(auth, "Bearer head-token");
                    json!({"sha":"b".repeat(40)})
                }
                _ => json!({}),
            };
            Json(response)
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let github = GitHub {
        http: reqwest::Client::new(),
        base: format!("http://{}", listener.local_addr().unwrap()),
        app_id: "1".into(),
        key: jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!(
            "fixtures/github-test-key.pem"
        ))
        .unwrap(),
    };
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let c = json!({"repository":{"githubRepositoryId":42,"owner":"fork","name":"repo"},"githubInstallationId":1});
    let d = json!({"headRepository":c["repository"],"targetRepository":{"githubRepositoryId":43,"owner":"upstream","name":"repo"},"baseRef":"main","headRef":"candidate","baseSha":"a".repeat(40),"headSha":"b".repeat(40)});
    let tokens = github.publication_tokens(&c, &d).await.unwrap();
    github.binding(&d, &tokens).await.unwrap();
    assert!(
        requests
            .lock()
            .unwrap()
            .contains(&"/repos/upstream/repo/installation".into())
    );
    let mut changed = d.clone();
    changed["targetRepository"]["owner"] = json!("renamed");
    assert!(github.publication_tokens(&c, &changed).await.is_err());
    changed = d.clone();
    changed["targetRepository"]["githubRepositoryId"] = json!(44);
    assert!(github.publication_tokens(&c, &changed).await.is_err());
    changed = d;
    changed["headRepository"]["githubRepositoryId"] = json!(44);
    assert!(github.publication_tokens(&c, &changed).await.is_err());
    task.abort();
}
