// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{Json, Router, extract::Request, http::StatusCode, response::IntoResponse};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use perfine_api::auth::{self, Actions, Permission};
use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
const KEY: &[u8] = include_bytes!("fixtures/github-test-key.pem");
#[tokio::test]
async fn independent_capabilities_keys_and_tenant_states() {
    let space = uuid::Uuid::now_v7().to_string();
    let user = uuid::Uuid::now_v7().to_string();
    let key = uuid::Uuid::now_v7().to_string();
    let grant = Arc::new(Mutex::new("manage".to_owned()));
    let state = Arc::new(Mutex::new("TENANT_STATE_ACTIVE".to_owned()));
    let revoked = Arc::new(Mutex::new(false));
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let (g, s, r, log, sp) = (
        grant.clone(),
        state.clone(),
        revoked.clone(),
        seen.clone(),
        space.clone(),
    );
    let app=Router::new().fallback(move|req:Request|{let(g,s,r,log,sp,user,key)=(g.clone(),s.clone(),r.clone(),log.clone(),sp.clone(),user.clone(),key.clone());async move{
  let path=req.uri().path().to_owned();let body=axum::body::to_bytes(req.into_body(),64<<10).await.unwrap();let q:Value=serde_json::from_slice(&body).unwrap_or(json!({}));
  let result=match path.as_str(){
   "/token"=>json!({"access_token":"service","expires_in":3600}),
   "/delcore.v1.DelcoreService/GetCurrentUser"=>json!({"userId":user}),
   "/delcore.v1.DelcoreService/GetTenantStates"=>json!({"states":[{"tenantId":q["tenantIds"][0],"state":*s.lock().unwrap()}]}),
   "/delcore.v1.DelcoreService/Authorize"|"/delcore.v1.DelcoreService/AuthorizeApiKey"=>{assert_eq!(q["paid"],false);log.lock().unwrap().push(q["capability"].as_str().unwrap().into());json!({"allowed":!*r.lock().unwrap()&&q["capability"]==*g.lock().unwrap(),"tenantId":sp,"keyId":key,"userId":user})},
   _=>return StatusCode::NOT_FOUND.into_response(),
  };Json(result).into_response()
 }});
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let authority = Authority::new(
        reqwest::Client::new(),
        base.clone(),
        format!("{base}/token"),
        "resource".into(),
        "client".into(),
        "fixture".into(),
    );
    for header in ["Bearer human", "Bearer dlk_fixture"] {
        for granted in [
            Permission::Read,
            Permission::Experiment,
            Permission::Publish,
            Permission::Manage,
        ] {
            *grant.lock().unwrap() = granted.name().into();
            for requested in [
                Permission::Read,
                Permission::Experiment,
                Permission::Publish,
                Permission::Manage,
            ] {
                assert_eq!(
                    auth::authorize(&authority, header, &space, requested)
                        .await
                        .is_ok(),
                    granted == requested
                );
            }
        }
    }
    assert!(
        auth::authorize(
            &authority,
            "Bearer dlk_fixture",
            &uuid::Uuid::now_v7().to_string(),
            Permission::Manage
        )
        .await
        .is_err()
    );
    *revoked.lock().unwrap() = true;
    assert!(
        auth::authorize(&authority, "Bearer dlk_fixture", &space, Permission::Manage)
            .await
            .is_err()
    );
    *revoked.lock().unwrap() = false;
    for value in [
        "TENANT_STATE_BLOCKED",
        "TENANT_STATE_DELETING",
        "TENANT_STATE_DELETED",
        "TENANT_STATE_UNKNOWN",
    ] {
        *state.lock().unwrap() = value.into();
        assert!(
            auth::authorize(&authority, "Bearer human", &space, Permission::Manage)
                .await
                .is_err()
        );
    }
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|s| ["read", "experiment", "publish", "manage"].contains(&s.as_str()))
    );
    task.abort();
}
#[tokio::test]
async fn actions_jwt_requires_signature_issuer_audience_and_lifetime() {
    let private = RsaPrivateKey::from_pkcs8_pem(std::str::from_utf8(KEY).unwrap()).unwrap();
    let keys=serde_json::from_value(json!({"keys":[{"kty":"RSA","kid":"fixture","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())}]})).unwrap();
    let actions = Actions::new(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap(),
        "http://127.0.0.1:1/jwks".into(),
        keys,
    );
    let now = chrono::Utc::now().timestamp();
    let valid = json!({"iss":"https://token.actions.githubusercontent.com","aud":"https://perfine.api.delino.io","sub":"repo:owner/repo:ref:refs/heads/main","iat":now,"nbf":now-1,"exp":now+300,"repository_id":"42","run_id":"7","run_attempt":"1"});
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("fixture".into());
    let sign = |v: &Value| {
        jsonwebtoken::encode(
            &header,
            v,
            &jsonwebtoken::EncodingKey::from_rsa_pem(KEY).unwrap(),
        )
        .unwrap()
    };
    assert!(actions.verify(&sign(&valid)).await.is_ok());
    for (field, value) in [
        ("iss", json!("https://attacker.invalid")),
        ("aud", json!("https://git-tracker.api.delino.io")),
        ("exp", json!(now - 300)),
        ("nbf", json!(now + 300)),
    ] {
        let mut claims = valid.clone();
        claims[field] = value;
        assert!(actions.verify(&sign(&claims)).await.is_err());
    }
    let mut claims = valid;
    claims.as_object_mut().unwrap().remove("nbf");
    assert!(actions.verify(&sign(&claims)).await.is_err());
}

#[test]
fn organization_actions_bind_the_job_not_the_connection_creator() {
    let repository = json!({"githubRepositoryId":"42","owner":"org","name":"repo"});
    let execution = json!({"githubRunId":"7","runAttempt":2,"sourceSha":"a".repeat(40),"workflowSha":"b".repeat(40),"workflowPath":".github/workflows/perfine.yml"});
    let claims = json!({"repository_id":"42","repository":"org/repo","run_id":"7","run_attempt":"2","sha":"a".repeat(40),"workflow_sha":"b".repeat(40),"workflow_ref":"org/repo/.github/workflows/perfine.yml@refs/heads/main","ref":"refs/heads/main","event_name":"workflow_dispatch","actor_id":"99"});
    let remote = json!({"id":7,"run_attempt":2,"repository":{"id":42},"head_repository":{"id":42},"actor":{"id":99},"head_sha":"a".repeat(40),"path":".github/workflows/perfine.yml","event":"workflow_dispatch"});
    assert!(auth::execution_scope(&claims, &execution, &repository, &remote).is_ok());
    for key in [
        "repository_id",
        "repository",
        "run_id",
        "run_attempt",
        "sha",
        "workflow_sha",
        "workflow_ref",
        "ref",
        "event_name",
        "actor_id",
    ] {
        let mut changed = claims.clone();
        changed[key] = json!("other");
        assert!(
            auth::execution_scope(&changed, &execution, &repository, &remote).is_err(),
            "{key}"
        );
    }
    for key in [
        "id",
        "run_attempt",
        "repository",
        "head_repository",
        "actor",
        "head_sha",
        "path",
        "event",
    ] {
        let mut changed = remote.clone();
        changed[key] = json!(null);
        assert!(
            auth::execution_scope(&claims, &execution, &repository, &changed).is_err(),
            "{key}"
        );
    }
}
