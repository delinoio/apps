// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{Json, Router, extract::Request, http::StatusCode, routing::post};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use git_tracker_api::{Server, auth::Actions, github::GitHub};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header, jwk::JwkSet};
use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use server_common::{
    authority::Authority,
    telemetry::{Metrics, Service},
};
use sha2::Sha256;
use std::{sync::Arc, time::Duration};
const KEY: &[u8] = include_bytes!("fixtures/github-test-key.pem");
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn postgres_permissions_atomic_upload_oidc_visibility_and_cleanup() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("tracker_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
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
    sqlx::raw_sql(include_str!("../migrations/00001_git_tracker.sql"))
        .execute(&db)
        .await
        .unwrap();
    let actor = uuid::Uuid::now_v7().to_string();
    let space = uuid::Uuid::now_v7().to_string();
    let commit = "a".repeat(40);
    let commit_mock = commit.clone();
    let actor_mock = actor.clone();
    let finalized = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let finalized_mock = finalized.clone();
    // Per-page delay (ms) for the optional PR comment listing; zero keeps the immediate 401.
    let page_delay = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let page_delay_mock = page_delay.clone();
    let pages_served = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let pages_served_mock = pages_served.clone();
    let mock=Router::new().route("/delcore.v1.DelcoreService/CheckTenantLifecycle",post(move||{let finalized=finalized_mock.clone();async move{Json(json!({"finalizedTenantIds":*finalized.lock().unwrap()}))}})).fallback(move|r:Request|{let commit=commit_mock.clone();let actor=actor_mock.clone();let page_delay=page_delay_mock.clone();let pages_served=pages_served_mock.clone();async move{
        let p=r.uri().path();
        let delay=page_delay.load(std::sync::atomic::Ordering::SeqCst);
        if delay>0&&p=="/repos/owner/repo/issues/1/comments"{
            tokio::time::sleep(Duration::from_millis(delay)).await;
            pages_served.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
            // A full page without a Git Tracker comment forces pagination to continue.
            return (StatusCode::OK,Json(json!((0..100).map(|i|json!({"id":i+1,"body":"unrelated"})).collect::<Vec<_>>())));
        }let auth=r.headers().get("authorization").and_then(|v|v.to_str().ok()).unwrap_or("");
        let out=match p{
            "/token"=>json!({"access_token":"service","expires_in":3600}),
            "/delcore.v1.DelcoreService/GetCurrentUser" if auth=="Bearer manager"=>json!({"userId":actor}),
            "/delcore.v1.DelcoreService/ListMembers" if auth=="Bearer manager"=>json!({}),
            "/delcore.v1.DelcoreService/Authorize"=>json!({"allowed":true}),
            "/user"=>json!({"id":7}),
            "/repos/owner/repo/pulls/1"=>json!({"base":{"ref":"main","sha":commit},"head":{"sha":commit}}),
            "/repos/owner/repo"|"/repositories/42"=>json!({"id":42,"full_name":"owner/repo","private":false,"permissions":{"admin":true}}),
            "/repos/owner/repo/installation"=>json!({"id":1}),
            "/app/installations/1/access_tokens"=>json!({"token":"installation"}),
            // Exact branch-head lookups. The generic commit route below also resolves SHAs and tags, so
            // only these routes may make a caller-supplied value a branch. Unlisted names (a tag such as
            // `v1`, or a commit SHA) answer 401 like a missing ref.
            "/repos/owner/repo/git/ref/heads/main"=>json!({"ref":"refs/heads/main","object":{"type":"commit","sha":commit}}),
            "/repos/owner/repo/git/ref/heads/release/1%2E0"=>json!({"ref":"refs/heads/release/1.0","object":{"type":"commit","sha":commit}}),
            p if p==format!("/repos/owner/repo/git/ref/heads/{}","b".repeat(40))=>json!({"ref":format!("refs/heads/{}","b".repeat(40)),"object":{"type":"commit","sha":commit}}),
            "/repos/owner/repo/git/ref/heads/mismatch"=>json!({"ref":"refs/heads/other","object":{"type":"commit","sha":commit}}),
            "/repos/owner/repo/git/ref/heads/annotated"=>json!({"ref":"refs/heads/annotated","object":{"type":"tag","sha":commit}}),
            "/repos/owner/repo/git/ref/heads/badsha"=>json!({"ref":"refs/heads/badsha","object":{"type":"commit","sha":"not-a-sha"}}),
            p if p.starts_with("/repos/owner/repo/commits/")=>json!({"sha":commit,"parents":[]}),
            _=>return (StatusCode::UNAUTHORIZED,Json(json!({"code":"unauthenticated"}))),
        };(StatusCode::OK,Json(out))
    }});
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mock_task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let private = RsaPrivateKey::from_pkcs8_pem(std::str::from_utf8(KEY).unwrap()).unwrap();
    let jwks:JwkSet=serde_json::from_value(json!({"keys":[{"kty":"RSA","kid":"fixture","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())}]})).unwrap();
    let server = Arc::new(Server {
        db: db.clone(),
        authority: Authority::new(
            http.clone(),
            base.clone(),
            format!("{base}/token"),
            "resource".into(),
            "client".into(),
            "fixture".into(),
        ),
        github: GitHub::new(
            http.clone(),
            base.clone(),
            "1".into(),
            EncodingKey::from_rsa_pem(KEY).unwrap(),
        ),
        actions: Actions::new(http.clone(), format!("{base}/jwks"), jwks),
        audit_retention: chrono::Duration::days(90),
        webhook_secret: "fixture-only".into(),
        metrics: Arc::new(Metrics::new(Service::GitTracker)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "http://{}/gittracker.v1.GitTrackerService",
        listener.local_addr().unwrap()
    );
    let router = Router::new()
        .fallback_service(
            connectrpc::Router::new()
                .add_service(server.clone())
                .into_axum_service(),
        )
        // Same ingress stamp as the production router; without it publication is skipped.
        .layer(axum::middleware::from_fn(git_tracker_api::capture_ingress));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    async fn call(
        http: &reqwest::Client,
        base: &str,
        method: &str,
        authorization: &str,
        q: Value,
    ) -> (StatusCode, Value) {
        let r = http
            .post(format!("{base}/{method}"))
            .header("Authorization", authorization)
            .header("Connect-Protocol-Version", "1")
            .json(&q)
            .send()
            .await
            .unwrap();
        let status = r.status();
        assert!(r.headers().contains_key("X-Correlation-Id"));
        (status, r.json().await.unwrap())
    }
    let (status, connected) = call(
        &http,
        &base,
        "ConnectRepository",
        "Bearer manager",
        json!({"spaceId":space,"fullName":"owner/repo","githubAccessToken":"fixture"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{connected}");
    let rid = connected["repository"]["id"].as_str().unwrap().to_owned();
    let (_,issued)=call(&http,&base,"IssueToken","Bearer manager",json!({"repositoryId":rid,"expiresAt":(chrono::Utc::now()+chrono::Duration::days(1)).to_rfc3339()})).await;
    let auth = format!("Bearer {}", issued["token"].as_str().unwrap());
    let context = json!({"repositoryId":rid,"executionId":"run","commit":commit,"branch":"main"});
    let dataset = json!({"id":"size","name":"Size","columns":[{"id":"bytes","unit":"B","direction":"DIRECTION_LOWER"}]});
    let upload = |id: &str| json!({"context":context,"dataset":dataset,"rows":[{"id":id,"values":{"bytes":"9007199254740993.00000000000001"}}]});
    let (a, b) = tokio::join!(
        call(&http, &base, "Upload", &auth, upload("one")),
        call(&http, &base, "Upload", &auth, upload("two"))
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    let (_, execution) = call(
        &http,
        &base,
        "GetExecution",
        "",
        json!({"repositoryId":rid,"executionId":"run"}),
    )
    .await;
    assert_eq!(
        execution["execution"]["measurements"][0]["rows"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (status, complete) =
        call(&http, &base, "Complete", &auth, json!({"context":context})).await;
    assert_eq!(status, StatusCode::OK, "{complete}");
    let (_, again) = call(&http, &base, "Complete", &auth, json!({"context":context})).await;
    assert_eq!(complete, again);
    let (_, closed) = call(&http, &base, "Upload", &auth, upload("one")).await;
    assert_eq!(closed["code"], "failed_precondition");
    let (_, history) = call(
        &http,
        &base,
        "GetHistory",
        "",
        json!({"repositoryId":rid,"branch":"main"}),
    )
    .await;
    assert_eq!(history["executions"][0]["representative"], true);
    let (_, compare) = call(
        &http,
        &base,
        "Compare",
        "",
        json!({"repositoryId":rid,"baselineCommit":commit,"targetCommit":commit}),
    )
    .await;
    assert_eq!(
        compare["comparisons"][0]["status"],
        "COMPARISON_STATUS_UNCHANGED"
    );
    let now = chrono::Utc::now().timestamp();
    let claims = json!({"iss":"https://token.actions.githubusercontent.com","aud":"https://git-tracker.api.delino.io","exp":now+300,"iat":now,"sub":"repo:owner/repo:ref:refs/heads/main","repository_id":"42","run_id":"1","run_attempt":"1","sha":commit,"ref":"refs/heads/main","event_name":"push"});
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture".into());
    let token =
        jsonwebtoken::encode(&header, &claims, &EncodingKey::from_rsa_pem(KEY).unwrap()).unwrap();
    let mut c = context.clone();
    c["executionId"] = "42/1/1".into();
    let (status, v) = call(
        &http,
        &base,
        "Complete",
        &format!("Actions {token}"),
        json!({"context":c}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    c["executionId"] = "42/other/1".into();
    let (_, denied) = call(
        &http,
        &base,
        "Complete",
        &format!("Actions {token}"),
        json!({"context":c}),
    )
    .await;
    assert_eq!(denied["code"], "permission_denied");
    let github_failures = server
        .metrics
        .events
        .with_label_values(&["github", "failure"]);
    let before = github_failures.get();
    let mut bad_commit = context.clone();
    bad_commit["executionId"] = json!("bad-commit");
    bad_commit["commit"] = json!("b".repeat(40));
    let (_, failed) = call(
        &http,
        &base,
        "Upload",
        &auth,
        json!({"context":bad_commit,"dataset":dataset,"rows":[]}),
    )
    .await;
    assert_eq!(failed["code"], "unavailable");
    assert_eq!(github_failures.get(), before + 1);
    let mut pr_context = context.clone();
    pr_context["executionId"] = json!("pr-warning");
    pr_context["pullRequest"] = json!("1");
    pr_context["baselineCommit"] = json!(commit);
    let (status, warning) = call(
        &http,
        &base,
        "Upload",
        &auth,
        json!({"context":pr_context,"dataset":dataset,"rows":[{"id":"one","values":{"bytes":"1"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{warning}");
    assert_eq!(warning["warnings"][0], "PR comment could not be updated");
    assert_eq!(github_failures.get(), before + 2);
    // Slow optional publication must not turn a saved upload into a timeout: every listing page
    // succeeds within its own limit, but the pages together exceed the client deadline.
    page_delay.store(700, std::sync::atomic::Ordering::SeqCst);
    let mut slow_context = pr_context.clone();
    slow_context["executionId"] = json!("pr-slow");
    let started = std::time::Instant::now();
    let slow = http
        .post(format!("{base}/Upload"))
        .header("Authorization", &auth)
        .header("Connect-Protocol-Version", "1")
        .header("Connect-Timeout-Ms", "4000")
        .json(&json!({"context":slow_context,"dataset":dataset,"rows":[{"id":"one","values":{"bytes":"1"}}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(slow.status(), StatusCode::OK);
    let slow: Value = slow.json().await.unwrap();
    assert_eq!(slow["warnings"][0], "PR comment could not be updated");
    // Cutoff is the client deadline minus the reserve, so the response arrives before 4 s.
    assert!(
        started.elapsed() < Duration::from_millis(3900),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(github_failures.get(), before + 3);
    assert!(pages_served.load(std::sync::atomic::Ordering::SeqCst) < 4);
    // The execution is committed and the repository lock was released by the timed-out publisher.
    let (status, saved) = call(
        &http,
        &base,
        "GetExecution",
        "",
        json!({"repositoryId":rid,"executionId":"pr-slow"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["execution"]["measurements"][0]["rows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    page_delay.store(0, std::sync::atomic::Ordering::SeqCst);
    let lock_probe = std::time::Instant::now();
    let (status, after) = call(
        &http,
        &base,
        "Complete",
        &auth,
        json!({"context":slow_context}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert!(lock_probe.elapsed() < Duration::from_secs(2));
    // A client deadline shorter than the reserve leaves no publication budget at all.
    let mut spent_context = pr_context.clone();
    spent_context["executionId"] = json!("pr-spent");
    let spent = http
        .post(format!("{base}/Upload"))
        .header("Authorization", &auth)
        .header("Connect-Protocol-Version", "1")
        .header("Connect-Timeout-Ms", "1500")
        .json(&json!({"context":spent_context,"dataset":dataset,"rows":[{"id":"one","values":{"bytes":"1"}}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(spent.status(), StatusCode::OK);
    let spent: Value = spent.json().await.unwrap();
    assert_eq!(spent["warnings"][0], "PR comment could not be updated");
    assert_eq!(github_failures.get(), before + 5);
    // Only an exact `refs/heads/<branch>` head may name a branch. Commit SHAs, tags and mismatching or
    // non-commit refs fail with the GitHub unavailable fault and persist nothing.
    let recorded = |id: String| {
        let db = db.clone();
        let rid = rid.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT jsonb_exists(state->'records',$2) FROM git_tracker_repositories WHERE id::text=$1",
            )
            .bind(rid)
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };
    let rejected = [
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "v1",
        "mismatch",
        "annotated",
        "badsha",
        "main/",
        "a//b",
        "../main",
    ];
    for (i, branch) in rejected.iter().enumerate() {
        let id = format!("rejected-upload-{i}");
        let mut c = context.clone();
        c["executionId"] = json!(&id);
        c["branch"] = json!(branch);
        let (_, failed) = call(
            &http,
            &base,
            "Upload",
            &auth,
            json!({"context":c,"dataset":dataset,"rows":[{"id":"one","values":{"bytes":"1"}}]}),
        )
        .await;
        assert_eq!(failed["code"], "unavailable", "upload branch {branch}");
        assert!(!recorded(id.clone()).await, "upload branch {branch}");
        let id = format!("rejected-complete-{i}");
        c["executionId"] = json!(&id);
        let (_, failed) = call(&http, &base, "Complete", &auth, json!({"context":c})).await;
        assert_eq!(failed["code"], "unavailable", "complete branch {branch}");
        assert!(!recorded(id.clone()).await, "complete branch {branch}");
        let (_, failed) = call(
            &http,
            &base,
            "GetHistory",
            "",
            json!({"repositoryId":rid,"branch":branch}),
        )
        .await;
        assert_eq!(failed["code"], "unavailable", "history branch {branch}");
    }
    for (i, branch) in ["release/1.0", &"b".repeat(40)].iter().enumerate() {
        let id = format!("exact-upload-{i}");
        let mut c = context.clone();
        c["executionId"] = json!(&id);
        c["branch"] = json!(branch);
        let (status, v) = call(
            &http,
            &base,
            "Upload",
            &auth,
            json!({"context":c,"dataset":dataset,"rows":[{"id":"one","values":{"bytes":"1"}}]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "upload branch {branch}: {v}");
        assert!(recorded(id.clone()).await);
        let id = format!("exact-complete-{i}");
        c["executionId"] = json!(&id);
        let (status, v) = call(&http, &base, "Complete", &auth, json!({"context":c})).await;
        assert_eq!(status, StatusCode::OK, "complete branch {branch}: {v}");
        let (status, v) = call(
            &http,
            &base,
            "GetHistory",
            "",
            json!({"repositoryId":rid,"branch":branch}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "history branch {branch}: {v}");
    }
    let (_, revoked) = call(
        &http,
        &base,
        "RevokeToken",
        "Bearer manager",
        json!({"repositoryId":rid,"tokenId":issued["tokenId"]}),
    )
    .await;
    assert!(revoked.get("code").is_none());
    let (_, denied) = call(&http, &base, "Complete", &auth, json!({"context":context})).await;
    assert_eq!(denied["code"], "unauthenticated");
    let event = serde_json::to_vec(&json!({"action":"deleted","installation":{"id":1}})).unwrap();
    assert!(
        server
            .webhook("installation", "sha256=00", &event)
            .await
            .is_err()
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(b"fixture-only").unwrap();
    mac.update(&event);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    server
        .webhook("installation", &signature, &event)
        .await
        .unwrap();
    let (_, private) = call(
        &http,
        &base,
        "GetExecution",
        "",
        json!({"repositoryId":rid,"executionId":"run"}),
    )
    .await;
    assert_eq!(private["code"], "unauthenticated");
    // Only committed transitions contribute to interruption alerts.
    let events = server
        .metrics
        .events
        .with_label_values(&["execution", "interrupted"]);
    let before = events.get();
    let state: Value =
        sqlx::query_scalar("SELECT state FROM git_tracker_repositories WHERE id::text=$1")
            .bind(&rid)
            .fetch_one(&db)
            .await
            .unwrap();
    let mut stale = state["records"]["run"].clone();
    stale["execution"]["state"] = json!(1);
    stale["execution"]["context"]["execution_id"] = json!("stale-rpc");
    stale["lastData"] = json!((chrono::Utc::now() - chrono::Duration::hours(7)).to_rfc3339());
    sqlx::query("UPDATE git_tracker_repositories SET state=jsonb_set(state,ARRAY['records','stale-rpc'],$2) WHERE id::text=$1")
        .bind(&rid).bind(&stale).execute(&db).await.unwrap();
    sqlx::query("ALTER TABLE git_tracker_repositories ADD CONSTRAINT reject_interruption CHECK ((state #>> '{records,stale-rpc,execution,state}')::int <> 3)")
        .execute(&db).await.unwrap();
    let query = json!({"repositoryId":rid,"executionId":"run"});
    let (_, failed) = call(
        &http,
        &base,
        "GetExecution",
        "Bearer manager",
        query.clone(),
    )
    .await;
    assert_eq!(failed["code"], "unavailable");
    assert_eq!(events.get(), before);
    sqlx::query("ALTER TABLE git_tracker_repositories DROP CONSTRAINT reject_interruption")
        .execute(&db)
        .await
        .unwrap();
    let (status, _) = call(
        &http,
        &base,
        "GetExecution",
        "Bearer manager",
        query.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events.get(), before + 1);
    call(&http, &base, "GetExecution", "Bearer manager", query).await;
    assert_eq!(events.get(), before + 1);
    stale["execution"]["context"]["execution_id"] = json!("stale-maintenance");
    sqlx::query("UPDATE git_tracker_repositories SET state=jsonb_set(state,ARRAY['records','stale-maintenance'],$2) WHERE id::text=$1")
        .bind(&rid).bind(&stale).execute(&db).await.unwrap();
    server.maintain().await.unwrap();
    assert_eq!(events.get(), before + 2);
    server.maintain().await.unwrap();
    assert_eq!(events.get(), before + 2);
    let (status, _) = call(
        &http,
        &base,
        "Delete",
        "Bearer manager",
        json!({"repositoryId":rid,"kind":"DELETE_KIND_EXECUTION","id":"run","confirmed":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    finalized.lock().unwrap().push(space);
    server.maintain().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM git_tracker_repositories")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let failures = server
        .metrics
        .events
        .with_label_values(&["persistence", "failure"]);
    let before = failures.get();
    sqlx::query("DROP TABLE git_tracker_repositories CASCADE")
        .execute(&db)
        .await
        .unwrap();
    let (_, failed) = call(&http, &base, "ListRepositories", "", json!({})).await;
    assert_eq!(failed["code"], "unavailable");
    assert_eq!(failures.get(), before + 1);
    task.abort();
    mock_task.abort();
    drop(server);
    db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
