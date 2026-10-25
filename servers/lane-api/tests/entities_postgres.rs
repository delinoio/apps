// SPDX-License-Identifier: FSL-1.1-ALv2
//! Real domain tests; set TEST_DATABASE_URL to a disposable database and run --ignored.
use lane_api::{Principal, Result, account_lock, entities, id, string, sync};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};

struct Fixture {
    db: PgConnection,
    schema: String,
    p: Principal,
    epoch: String,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must name a disposable database");
        let mut db = PgConnection::connect(&url).await.unwrap();
        let schema = format!("entities_{}", id().replace('-', ""));
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::query(&format!("SET search_path TO {schema}"))
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE lane_accounts(account_id text PRIMARY KEY,tenant_id text,epoch text); CREATE TABLE lane_devices(account_id text,id text,name text,last_sync_at timestamptz,disconnected_at timestamptz,PRIMARY KEY(account_id,id))").execute(&mut db).await.unwrap();
        sqlx::raw_sql(include_str!("../migrations/00002_entities.sql"))
            .execute(&mut db)
            .await
            .unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        for (env, name) in [
            ("LANE_WEBHOOK_MIGRATION", "00003_webhooks.sql"),
            ("LANE_PUSH_MIGRATION", "00004_push.sql"),
        ] {
            let path = std::env::var(env)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| root.join(name));
            sqlx::raw_sql(&std::fs::read_to_string(path).unwrap())
                .execute(&mut db)
                .await
                .unwrap();
        }
        let p = Principal {
            account_id: id(),
            tenant_id: id(),
            session_id: id(),
            device_id: "019a0000-0000-7000-8000-000000000001".into(),
            device_name: "Fixture CLI".into(),
            origin_kind: "ORIGIN_KIND_CLI".into(),
        };
        let epoch = id();
        sqlx::query("INSERT INTO lane_accounts VALUES($1,$2,$3)")
            .bind(&p.account_id)
            .bind(&p.tenant_id)
            .bind(&epoch)
            .execute(&mut db)
            .await
            .unwrap();
        Self {
            db,
            schema,
            p,
            epoch,
        }
    }
    async fn close(mut self) {
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&mut self.db)
            .await
            .unwrap();
    }
    async fn call(&mut self, method: &str, q: Value) -> Result<Value> {
        self.call_limit(method, q, usize::MAX).await
    }
    async fn call_limit(&mut self, method: &str, q: Value, max: usize) -> Result<Value> {
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &self.p.account_id).await?;
        let r = entities::dispatch(&mut tx, &self.p, method, &q, max).await;
        if r.is_ok() {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        r
    }
    async fn sync(&mut self, method: &str, mut q: Value, max: usize) -> Result<Value> {
        if q.get("formatVersion").is_none() {
            q["formatVersion"] = json!(1);
        }
        if q.get("epoch").is_none() {
            q["epoch"] = json!(self.epoch);
        }
        if q.get("deviceId").is_none() {
            q["deviceId"] = json!(self.p.device_id);
        }
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &self.p.account_id).await?;
        let r = sync::dispatch(&mut tx, &self.p, method, &q, max).await;
        if r.is_ok() {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        r
    }
    async fn push(&mut self, m: Value) -> Result<Value> {
        self.sync("PushChanges", json!({"mutations":[m]}), usize::MAX)
            .await
    }
    async fn repo(&mut self, entity: &str, url: &str) -> Value {
        self.call(
            "CreateRepository",
            json!({"meta":{"requestId":id()},"id":entity,"url":url}),
        )
        .await
        .unwrap()["result"]["version"]
            .clone()
    }
    async fn task(&mut self, entity: &str, repo: &str) -> Value {
        self.call(
            "CreateTask",
            json!({"meta":{"requestId":id()},"id":entity,"repository":{"id":repo},"title":"Title"}),
        )
        .await
        .unwrap()["result"]["version"]
            .clone()
    }
    async fn heads(&mut self, entity: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT version_id FROM lane_heads WHERE account_id=$1 AND entity_id=$2 ORDER BY version_id").bind(&self.p.account_id).bind(entity).fetch_all(&mut self.db).await.unwrap()
    }
    async fn count(&mut self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE account_id=$1"))
            .bind(&self.p.account_id)
            .fetch_one(&mut self.db)
            .await
            .unwrap()
    }
}
fn fixtures(name: &str) -> Value {
    let root = std::env::var("LANE_CONFORMANCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protos/lane/v1/fixtures")
        });
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(format!("{name}.json"))).unwrap())
            .unwrap();
    assert_eq!(v["formatVersion"], 1);
    v
}
fn mutation(v: Value) -> Value {
    let kind = if v["deleted"] == true {
        "DELETE"
    } else if v["parentVersionIds"]
        .as_array()
        .is_none_or(|x| x.is_empty())
    {
        "CREATE"
    } else {
        "SAVE"
    };
    json!({"meta":{"requestId":id()},"kind":format!("MUTATION_KIND_{kind}"),"version":v,"expectedHeadIds":[]})
}
const REPO: &str = "019a0000-0000-7000-8000-000000000002";
const TASK: &str = "019a0000-0000-7000-8000-000000000003";

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn shared_progress_defaults_invalid_and_search_vectors() {
    for case in fixtures("tasks")["cases"].as_array().unwrap() {
        let input = &case["input"];
        let expected = &case["expected"];
        if input.get("method").is_none()
            && input["operation"] != "search"
            && input["operation"] != "validateStoredTask"
        {
            continue;
        }
        let mut f = Fixture::new().await;
        f.repo(REPO, "https://github.com/Acme/Repo").await;
        if input["operation"] == "search" {
            f.call("CreateTask",json!({"meta":{"requestId":id()},"id":TASK,"repository":{"id":REPO},"title":if string(input,"title").trim().is_empty(){"Fixture"}else{string(input,"title")},"description":input["description"]})).await.unwrap();
            let r = f
                .call("ListTasks", json!({"search":input["query"]}))
                .await
                .unwrap();
            assert_eq!(
                !r["items"].as_array().unwrap().is_empty(),
                expected["matches"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
            f.close().await;
            continue;
        }
        if input["operation"] == "validateStoredTask" {
            let mut v = f.task(TASK, REPO).await;
            let parent = v["versionId"].clone();
            v["versionId"] = json!(id());
            v["parentVersionIds"] = json!([parent]);
            v["task"] = input["task"].clone();
            assert!(f.push(mutation(v)).await.is_err());
            f.close().await;
            continue;
        }
        let mut q = input["request"].clone();
        if let Some(before) = input.get("before") {
            let mut creation = before.clone();
            creation["meta"] = json!({"requestId":id()});
            creation["id"] = json!(TASK);
            creation["repository"] = json!({"id":REPO});
            let created = f.call("CreateTask", creation).await.unwrap();
            q["expectedVersionId"] = created["result"]["version"]["versionId"].clone();
        }
        let before = f.count("lane_versions").await;
        let r = f.call(string(input, "method"), q).await;
        if expected.get("reason").is_some() {
            assert_eq!(r.unwrap_err().reason, 1, "{}", case["id"]);
            assert_eq!(f.count("lane_versions").await, before);
        } else {
            let r = r.unwrap();
            let task = &r["result"]["version"]["task"];
            if let Some(task_expected) = expected.get("task") {
                assert_eq!(task, task_expected, "{}", case["id"]);
                assert_eq!(f.count("lane_versions").await, before + 1);
            } else {
                assert_eq!(task["progress"], expected["progress"]);
                assert_eq!(task["proposal"], expected["proposal"]);
                assert_eq!(
                    task.get("prompt").is_some(),
                    expected["promptPresent"].as_bool().unwrap()
                );
            }
        }
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn shared_sync_version_conflict_and_deletion_vectors() {
    for case in fixtures("sync")["cases"].as_array().unwrap() {
        let input = &case["input"];
        let expected = &case["expected"];
        if input.get("initial").is_none() {
            continue;
        }
        let mut f = Fixture::new().await;
        f.repo(REPO, "https://example.com/repo").await;
        for v in input["initial"].as_array().unwrap() {
            f.push(mutation(v.clone())).await.unwrap();
        }
        let before = f.count("lane_versions").await;
        let mut responses = Vec::new();
        if let Some(ms) = input["mutations"].as_array() {
            for m in ms {
                responses.push(f.push(m.clone()).await);
            }
        }
        if let Some(i) = input.get("interactiveRequest") {
            responses.push(f.call(string(i, "method"), i["request"].clone()).await);
        }
        let last = responses.last().unwrap();
        if let Some(reason) = expected["reason"].as_str() {
            let n = match reason {
                "ERROR_REASON_CONFLICT_READ_ONLY" => 4,
                "ERROR_REASON_REQUEST_ID_REUSED" => 6,
                _ => panic!("unhandled reason {reason}"),
            };
            assert_eq!(last.as_ref().unwrap_err().reason, n, "{}", case["id"]);
        }
        if expected["retryable"] == true {
            assert_eq!(last.as_ref().unwrap_err().reason, 9);
        }
        if expected.get("results").is_some() {
            assert!(responses[0].is_ok());
            assert_eq!(responses[1].as_ref().unwrap_err().reason, 5);
        }
        if let Some(heads) = expected.get("headIds") {
            assert_eq!(json!(f.heads(TASK).await), *heads, "{}", case["id"]);
        }
        if expected["newVersions"] == 0 || expected["writes"] == 0 {
            assert_eq!(f.count("lane_versions").await, before, "{}", case["id"]);
        }
        if expected["historyBodies"] == 0 {
            let bodies:i64=sqlx::query_scalar("SELECT count(*) FROM lane_versions WHERE account_id=$1 AND entity_id=$2 AND envelope ? 'task'").bind(&f.p.account_id).bind(TASK).fetch_one(&mut f.db).await.unwrap();
            assert_eq!(bodies, 0);
            let changes:Vec<Value>=sqlx::query_scalar("SELECT data FROM lane_changes WHERE account_id=$1 AND data->'entity'->'version'->>'entityId'=$2").bind(&f.p.account_id).bind(TASK).fetch_all(&mut f.db).await.unwrap();
            assert!(
                changes
                    .iter()
                    .all(|c| c["entity"]["version"].get("task").is_none())
            );
        }
        if let Some(alternatives) = expected.get("alternatives") {
            let mut alternatives = alternatives.clone();
            for v in alternatives.as_array_mut().unwrap() {
                if v.get("promptRestoration").is_none() {
                    v["promptRestoration"] = json!(false);
                }
            }
            assert_eq!(
                last.as_ref().unwrap()["results"][0]["result"]["alternatives"],
                alternatives,
                "{}",
                case["id"]
            );
        }
        if let Some(n) = expected["newVersions"].as_i64() {
            assert_eq!(f.count("lane_versions").await, before + n, "{}", case["id"]);
        }
        if let Some(ids) = expected.get("newPromptHistoryVersionIds") {
            let initial_ids: Vec<String> = input["initial"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| string(v, "versionId").to_owned())
                .collect();
            let got: Vec<String> = sqlx::query_scalar("SELECT version_id FROM lane_versions WHERE account_id=$1 AND entity_id=$2 AND prompt_version AND NOT(version_id=ANY($3)) ORDER BY version_id")
                .bind(&f.p.account_id).bind(TASK).bind(initial_ids).fetch_all(&mut f.db).await.unwrap();
            assert_eq!(json!(got), *ids, "{}", case["id"]);
            let marked: Vec<String> = sqlx::query_scalar("SELECT version_id FROM lane_versions WHERE account_id=$1 AND envelope->>'promptRestoration'='true' ORDER BY version_id")
                .bind(&f.p.account_id).fetch_all(&mut f.db).await.unwrap();
            assert_eq!(json!(marked), expected["restorationVersionIds"]);
            for index in input["replayMutationIndices"].as_array().unwrap() {
                let before_replay = f.count("lane_versions").await;
                f.push(input["mutations"][index.as_u64().unwrap() as usize].clone())
                    .await
                    .unwrap();
                assert_eq!(f.count("lane_versions").await, before_replay);
            }
        }
        if let Some(expected_result) = expected.get("result") {
            assert_eq!(
                last.as_ref().unwrap()["results"][0]["result"]["result"],
                *expected_result,
                "{}",
                case["id"]
            );
        }
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn stale_interactive_save_preserves_alternative_resolution_and_receipts() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let base = f.task(TASK, REPO).await;
    let save = json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":base["versionId"],"progress":"PROGRESS_DONE"});
    f.call("SetProgress", save.clone()).await.unwrap();
    let stale = json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":base["versionId"],"progress":"PROGRESS_BLOCKED"});
    let conflict = f.call("SetProgress", stale.clone()).await.unwrap();
    assert_eq!(conflict["result"]["result"], "RESULT_KIND_CONFLICT");
    assert_eq!(f.heads(TASK).await.len(), 2);
    assert_eq!(f.call("SetProgress", stale).await.unwrap(), conflict);
    let resolve = json!({"meta":{"requestId":id()},"id":TASK,"expectedHeadIds":f.heads(TASK).await,"resolved":conflict["result"]["version"]});
    f.call("ResolveConflict", resolve.clone()).await.unwrap();
    let mut loser = resolve;
    loser["meta"]["requestId"] = json!(id());
    assert_eq!(
        f.call("ResolveConflict", loser).await.unwrap_err().reason,
        5
    );
    assert_eq!(f.heads(TASK).await.len(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn replay_redaction_mapping_move_and_stable_pull() {
    let mut f = Fixture::new().await;
    let remote = id();
    let local = id();
    let other = id();
    let rv = f.repo(&remote, "https://example.com/original").await;
    f.repo(&other, "https://example.com/other").await;
    let mut local_v = rv.clone();
    local_v["entityId"] = json!(local);
    local_v["versionId"] = json!(id());
    let merged = f.push(mutation(local_v.clone())).await.unwrap();
    assert_eq!(
        merged["repositoryMappings"][0],
        json!({"localId":local,"canonicalId":remote})
    );
    let created=f.call("CreateTask",json!({"meta":{"requestId":id()},"id":TASK,"repository":{"id":local},"title":"Straße","prompt":"private Markdown"})).await.unwrap();
    let query = json!({"meta":{"requestId":id()},"id":id(),"repository":{"id":remote},"title":"deleted secret"});
    let doomed = f.call("CreateTask", query.clone()).await.unwrap();
    let page = f
        .sync("PullChanges", json!({"pageSize":1}), usize::MAX)
        .await
        .unwrap();
    let high: i64 =
        sqlx::query_scalar("SELECT max(sequence) FROM lane_changes WHERE account_id=$1")
            .bind(&f.p.account_id)
            .fetch_one(&mut f.db)
            .await
            .unwrap();
    let mut task = created["result"]["version"]["task"].clone();
    task["repositoryId"] = json!(other);
    f.call("SaveTask",json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":created["result"]["version"]["versionId"],"task":task})).await.unwrap();
    f.call_limit(
        "DeleteRepository",
        json!({"meta":{"requestId":id()},"id":remote}),
        1,
    )
    .await
    .unwrap();
    let replay = f.call_limit("CreateTask", query, 1).await.unwrap();
    assert_eq!(replay["result"]["result"], "RESULT_KIND_DELETED");
    assert!(replay["result"]["version"].get("task").is_none());
    let history = f.call("ListHistory", json!({"id":TASK})).await.unwrap();
    assert_eq!(history["versions"].as_array().unwrap().len(), 2);
    let retained = f
        .call_limit("GetTask", json!({"id":TASK}), 1)
        .await
        .unwrap();
    assert_eq!(
        retained["result"]["version"]["task"]["prompt"],
        "private Markdown"
    );
    let mut cursor = string(&page, "nextCursor").to_owned();
    loop {
        let page = f
            .sync("PullChanges", json!({"cursor":cursor,"pageSize":1}), 1)
            .await
            .unwrap();
        for change in page["changes"].as_array().unwrap() {
            assert!(string(change, "sequence").parse::<i64>().unwrap() <= high);
            if change["entity"]["version"]["entityId"] == doomed["result"]["version"]["entityId"] {
                assert!(change["entity"]["version"].get("task").is_none());
            }
        }
        cursor = string(&page, "nextCursor").to_owned();
        if page["hasMore"] == false {
            break;
        }
    }
    let next = f
        .sync("PullChanges", json!({"cursor":cursor}), 1)
        .await
        .unwrap();
    assert!(!next["changes"].as_array().unwrap().is_empty());
    let original_p = f.p.clone();
    f.p.account_id = id();
    f.p.tenant_id = id();
    assert!(f.call("GetTask", json!({"id":TASK})).await.is_err());
    f.p = original_p;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn prompt_restore_and_proposal_transition_preserve_history() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let mut r=f.call("CreateTask",json!({"meta":{"requestId":id()},"id":TASK,"repository":{"id":REPO},"title":"Title","proposal":"PROPOSAL_PENDING","progress":"PROGRESS_BLOCKED","prompt":"same"})).await.unwrap();
    let initial = r["result"]["version"]["versionId"].clone();
    for proposal in ["PROPOSAL_REJECTED", "PROPOSAL_ACCEPTED"] {
        r=f.call("SetProposal",json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":r["result"]["version"]["versionId"],"proposal":proposal})).await.unwrap();
        assert_eq!(
            r["result"]["version"]["task"]["progress"],
            "PROGRESS_BLOCKED"
        );
    }
    r=f.call("RestorePrompt",json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":r["result"]["version"]["versionId"],"sourceVersionId":initial})).await.unwrap();
    assert_ne!(r["result"]["version"]["versionId"], initial);
    let history = f
        .call("ListHistory", json!({"id":TASK,"promptsOnly":true}))
        .await
        .unwrap();
    assert_eq!(history["versions"].as_array().unwrap().len(), 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn limits_epochs_cursors_identity_reservation_and_atomic_rollback() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo.git.git").await;
    let v = f.task(TASK, REPO).await;
    let accepted = f.count("lane_versions").await;
    let bad = json!({"meta":{"requestId":id()},"id":id(),"repository":{"url":"https://example.com/rollback"},"title":" "});
    assert_eq!(f.call("CreateTask", bad).await.unwrap_err().reason, 1);
    assert_eq!(f.count("lane_entities").await, 2);
    let mut update = v.clone();
    update["parentVersionIds"] = json!([v["versionId"]]);
    update["versionId"] = json!(id());
    update["task"]["title"] = json!("oversized");
    let m = mutation(update.clone());
    assert_eq!(
        f.sync("PushChanges", json!({"mutations":[m.clone()]}), 1)
            .await
            .unwrap_err()
            .reason,
        15
    );
    assert_eq!(f.count("lane_versions").await, accepted);
    f.push(m).await.unwrap();
    let page = f.sync("PullChanges", json!({}), 1).await.unwrap();
    let cursor = page["nextCursor"].clone();
    assert_eq!(
        f.sync("PullChanges", json!({"epoch":id(),"cursor":cursor}), 1)
            .await
            .unwrap_err()
            .reason,
        17
    );
    assert_eq!(
        f.sync("PullChanges", json!({"formatVersion":2}), 1)
            .await
            .unwrap_err()
            .reason,
        8
    );
    let first_watch = f
        .sync("WatchChanges", json!({"cursor":cursor}), 1)
        .await
        .unwrap();
    assert_eq!(first_watch["cursor"], cursor);
    f.call_limit(
        "DeleteTask",
        json!({"meta":{"requestId":id()},"id":TASK}),
        1,
    )
    .await
    .unwrap();
    let next_watch = f
        .sync("WatchChanges", json!({"cursor":cursor}), 1)
        .await
        .unwrap();
    assert_ne!(next_watch["cursor"], cursor);
    let mut reused = v;
    reused["entityId"] = json!(id());
    reused["task"]["title"] = json!("changed");
    assert_eq!(f.push(mutation(reused)).await.unwrap_err().reason, 6);
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &f.p.account_id).await.unwrap();
    let created = id();
    entities::dispatch(&mut tx,&f.p,"CreateTask",&json!({"meta":{"requestId":id()},"id":created,"repository":{"id":REPO},"title":"rolled back"}),usize::MAX).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        f.call("GetTask", json!({"id":created}))
            .await
            .unwrap_err()
            .reason,
        2
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn repository_conflicts_reserve_urls_and_mapping_preserves_immutable_versions() {
    let mut f = Fixture::new().await;
    let a = id();
    let b = id();
    let c = id();
    let original = f.repo(&a, "https://example.com/a").await;
    let saved=f.call("SaveRepository",json!({"meta":{"requestId":id()},"id":a,"expectedVersionId":original["versionId"],"url":"https://example.com/b"})).await.unwrap();
    f.repo(&b, "https://example.com/a").await;
    let conflict=f.call("SaveRepository",json!({"meta":{"requestId":id()},"id":a,"expectedVersionId":original["versionId"],"url":"https://example.com/c"})).await.unwrap();
    assert_eq!(conflict["result"]["result"], "RESULT_KIND_CONFLICT");
    let err = f
        .call(
            "CreateRepository",
            json!({"meta":{"requestId":id()},"id":c,"url":"https://example.com/c"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.reason, 1);
    let mut resolved = saved["result"]["version"].clone();
    resolved["repository"]["normalizedUrl"] = json!("example.com/a");
    let observed = f.heads(&a).await;
    let err=f.call("ResolveConflict",json!({"meta":{"requestId":id()},"id":a,"expectedHeadIds":observed,"resolved":resolved})).await.unwrap_err();
    assert_eq!(err.reason, 1);
    let mut local = original;
    local["entityId"] = json!(c);
    local["versionId"] = json!(id());
    local["repository"]["normalizedUrl"] = json!("example.com/a");
    local["repository"]["displayName"] = json!("Different name");
    let merged = f.push(mutation(local.clone())).await.unwrap();
    assert_eq!(
        merged["results"][0]["result"]["result"],
        "RESULT_KIND_CONFLICT"
    );
    let stored: Value = sqlx::query_scalar(
        "SELECT envelope FROM lane_versions WHERE account_id=$1 AND version_id=$2",
    )
    .bind(&f.p.account_id)
    .bind(string(&local, "versionId"))
    .fetch_one(&mut f.db)
    .await
    .unwrap();
    assert_eq!(stored, local);
    f.close().await;
}
