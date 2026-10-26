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
            ("LANE_CAPTURE_MIGRATION", "00006_captures.sql"),
            ("LANE_PLANNING_MIGRATION", "00007_planning.sql"),
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
            q["formatVersion"] = json!(3);
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
        f.sync("PullChanges", json!({"formatVersion":99}), 1)
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

impl Fixture {
    async fn capture(&mut self, method: &str, q: Value) -> Result<Value> {
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &self.p.account_id).await?;
        let result = lane_api::capture::dispatch(&mut tx, &self.p, method, &q, usize::MAX).await;
        if result.is_ok() {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        result
    }
}
fn image() -> Value {
    use base64::Engine;
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[0, 0, 0, 255])
            .unwrap();
    }
    json!({"capture":{"attachmentId":id(),"width":1,"height":1,"byteSize":png.len().to_string(),"sha256":lane_api::hash(&png),"pageUrl":"https://example.com/private?data","cssSelector":"<data>"},"png":base64::engine::general_purpose::STANDARD.encode(png)})
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn realqa_atomic_capture_history_move_ownership_and_deletion() {
    let mut f = Fixture::new().await;
    let source = id();
    let destination = id();
    let task = id();
    f.repo(&source, "https://example.com/old").await;
    f.repo(&destination, "https://example.com/new").await;
    let image = image();
    let q = json!({"id":task,"meta":{"requestId":id()},"repositoryId":source,"title":"Capture","description":"Problem","images":[image]});
    let mut invalid = q.clone();
    invalid["images"][0]["capture"]["sha256"] = json!("0".repeat(64));
    assert_eq!(
        f.capture("CreateCaptureTask", invalid)
            .await
            .unwrap_err()
            .reason,
        1
    );
    assert_eq!(f.count("lane_attachments").await, 0);
    assert_eq!(f.count("lane_entities").await, 2);
    let created = f.capture("CreateCaptureTask", q.clone()).await.unwrap();
    assert_eq!(created["result"]["version"]["formatVersion"], 3);
    assert_eq!(created["result"]["version"]["task"]["prompt"], "Problem");
    assert_eq!(
        f.capture("CreateCaptureTask", q.clone()).await.unwrap()["result"]["result"],
        "RESULT_KIND_REPLAYED"
    );
    assert_eq!(f.count("lane_attachments").await, 1);
    let attachment = image["capture"]["attachmentId"].clone();
    let get = json!({"taskId":task,"attachmentId":attachment});
    assert_eq!(
        f.capture("GetAttachment", get.clone()).await.unwrap()["state"],
        "ATTACHMENT_STATE_WAITING_FOR_SYNC"
    );
    let other = id();
    f.task(&other, &source).await;
    assert_eq!(
        f.capture(
            "GetAttachment",
            json!({"taskId":other,"attachmentId":attachment})
        )
        .await
        .unwrap_err()
        .reason,
        2
    );
    let mut body = created["result"]["version"]["task"].clone();
    body["captures"] = json!([]);
    body["repositoryId"] = json!(destination);
    let saved=f.call("SaveTask",json!({"id":task,"meta":{"requestId":id()},"expectedVersionId":created["result"]["version"]["versionId"],"task":body})).await.unwrap();
    assert_eq!(f.count("lane_attachments").await, 1);
    let history = f
        .call("ListHistory", json!({"id":task,"promptsOnly":true}))
        .await
        .unwrap();
    assert_eq!(history["versions"].as_array().unwrap().len(), 2);
    let restored=f.call("RestorePrompt",json!({"id":task,"meta":{"requestId":id()},"expectedVersionId":saved["result"]["version"]["versionId"],"sourceVersionId":created["result"]["version"]["versionId"]})).await.unwrap();
    assert_eq!(
        restored["result"]["version"]["task"]["captures"][0],
        image["capture"]
    );
    assert_eq!(
        restored["result"]["version"]["task"]["repositoryId"],
        destination
    );
    f.call(
        "DeleteRepository",
        json!({"id":source,"meta":{"requestId":id()}}),
    )
    .await
    .unwrap();
    assert_eq!(f.count("lane_attachments").await, 1);
    f.call(
        "DeleteRepository",
        json!({"id":destination,"meta":{"requestId":id()}}),
    )
    .await
    .unwrap();
    assert_eq!(f.count("lane_attachments").await, 0);
    assert_eq!(
        f.capture("GetAttachment", get).await.unwrap()["state"],
        "ATTACHMENT_STATE_DELETED"
    );
    assert_eq!(
        f.capture("CreateCaptureTask", q).await.unwrap()["result"]["result"],
        "RESULT_KIND_DELETED"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM lane_attachment_garbage")
        .fetch_one(&mut f.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn realqa_rule_conflicts_duplicates_cascade_and_sync_negotiation() {
    let mut f = Fixture::new().await;
    let repo = id();
    f.repo(&repo, "https://example.com/repo").await;
    let rule = id();
    let connection =
        json!({"prefix":"HTTPS://EXAMPLE.COM:443/app/?private#anchor","repositoryId":repo});
    let created = f
        .capture(
            "CreateURLConnection",
            json!({"id":rule,"meta":{"requestId":id()},"connection":connection}),
        )
        .await
        .unwrap();
    assert_eq!(
        created["result"]["version"]["urlConnection"]["prefix"],
        "https://example.com/app"
    );
    assert_eq!(
        f.capture(
            "CreateURLConnection",
            json!({"id":id(),"meta":{"requestId":id()},"connection":connection})
        )
        .await
        .unwrap_err()
        .reason,
        6
    );
    for prefix in ["https://example.com/a", "https://example.com/b"] {
        f.capture("SaveURLConnection",json!({"id":rule,"meta":{"requestId":id()},"expectedVersionId":created["result"]["version"]["versionId"],"connection":{"prefix":prefix,"repositoryId":repo}})).await.unwrap();
    }
    let listed = f.capture("ListURLConnections", json!({})).await.unwrap();
    assert_eq!(listed["items"][0]["result"], "RESULT_KIND_CONFLICT");
    let hs = f.heads(&rule).await;
    f.call("ResolveConflict",json!({"id":rule,"meta":{"requestId":id()},"expectedHeadIds":hs,"resolved":{"urlConnection":{"prefix":"https://example.com/c","repositoryId":repo}}})).await.unwrap();
    assert_eq!(
        f.sync("GetSyncInfo", json!({}), 1).await.unwrap()["formatVersion"],
        3
    );
    assert_eq!(
        f.sync("PullChanges", json!({"formatVersion":1}), 1)
            .await
            .unwrap_err()
            .reason,
        8
    );
    f.call(
        "DeleteRepository",
        json!({"id":repo,"meta":{"requestId":id()}}),
    )
    .await
    .unwrap();
    assert_eq!(
        f.capture("ListURLConnections", json!({})).await.unwrap()["items"],
        json!([])
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn format1_save_receipt_and_outbox_replay_survive_current_proto_defaults() {
    use delino_protocol::lane::v1::{SaveTaskRequest, SyncMutation};
    let mut f = Fixture::new().await;
    let repo = id();
    let task = id();
    f.repo(&repo, "https://example.com/legacy").await;
    let mut initial = f.task(&task, &repo).await;
    initial["formatVersion"] = json!(1);
    // These are the exact schema-1 envelope and RFC 8785 fingerprint, without a captures field.
    sqlx::query("UPDATE lane_versions SET envelope=$3 WHERE account_id=$1 AND version_id=$2")
        .bind(&f.p.account_id)
        .bind(string(&initial, "versionId"))
        .bind(&initial)
        .execute(&mut f.db)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE lane_version_identities SET fingerprint=$3 WHERE account_id=$1 AND version_id=$2",
    )
    .bind(&f.p.account_id)
    .bind(string(&initial, "versionId"))
    .bind(lane_api::hash(
        serde_jcs::to_string(&initial).unwrap().as_bytes(),
    ))
    .execute(&mut f.db)
    .await
    .unwrap();
    let request = id();
    let mut old_q = json!({"id":task,"meta":{"requestId":request,"origin":f.p.origin()},"expectedVersionId":initial["versionId"],"task":initial["task"]});
    old_q["task"]["title"] = json!("Legacy saved title");
    let mut saved = initial.clone();
    saved["versionId"] = json!(id());
    saved["parentVersionIds"] = json!([initial["versionId"]]);
    saved["task"] = old_q["task"].clone();
    let old_response = json!({"result":{"result":"RESULT_KIND_APPLIED","version":saved,"alternatives":[],"correlationId":""}});
    let fingerprint = lane_api::hash(serde_jcs::to_string(&old_q).unwrap().as_bytes());
    sqlx::query("INSERT INTO lane_entity_receipts(account_id,method,request_id,fingerprint,entity_id,response) VALUES($1,'SaveTask',$2,$3,$4,$5)").bind(&f.p.account_id).bind(&request).bind(&fingerprint).bind(&task).bind(&old_response).execute(&mut f.db).await.unwrap();
    let current: SaveTaskRequest = serde_json::from_value(old_q).unwrap();
    let mut current = serde_json::to_value(current).unwrap();
    // Prost's defaults=true adapter emits repeated captures even when absent in the old bytes.
    current["task"]["captures"] = json!([]);
    let replay = f.call("SaveTask", current).await.unwrap();
    assert_eq!(replay["result"]["result"], "RESULT_KIND_REPLAYED");
    assert_eq!(replay["result"]["version"], saved);
    let retained: String = sqlx::query_scalar(
        "SELECT fingerprint FROM lane_entity_receipts WHERE account_id=$1 AND request_id=$2",
    )
    .bind(&f.p.account_id)
    .bind(&request)
    .fetch_one(&mut f.db)
    .await
    .unwrap();
    assert_eq!(retained, fingerprint);
    let mutation = mutation(initial.clone());
    let typed: SyncMutation = serde_json::from_value(mutation).unwrap();
    let mut normalized = serde_json::to_value(typed).unwrap();
    normalized["version"]["task"]["captures"] = json!([]);
    let imported = f.push(normalized).await.unwrap();
    assert_eq!(
        imported["results"][0]["result"]["result"],
        "RESULT_KIND_REPLAYED"
    );
    let retained: Value = sqlx::query_scalar(
        "SELECT envelope FROM lane_versions WHERE account_id=$1 AND version_id=$2",
    )
    .bind(&f.p.account_id)
    .bind(string(&initial, "versionId"))
    .fetch_one(&mut f.db)
    .await
    .unwrap();
    assert_eq!(retained, initial);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn fresh_request_historical_replay_cannot_delete_moved_capture_or_url_rule() {
    let mut f = Fixture::new().await;
    let source = id();
    let destination = id();
    let task = id();
    let rule = id();
    f.repo(&source, "https://example.com/source").await;
    f.repo(&destination, "https://example.com/destination")
        .await;
    let image = image();
    let created=f.capture("CreateCaptureTask",json!({"id":task,"meta":{"requestId":id()},"repositoryId":source,"title":"Capture","description":"Original","images":[image]})).await.unwrap();
    let task_old = created["result"]["version"].clone();
    let rule_old=f.capture("CreateURLConnection",json!({"id":rule,"meta":{"requestId":id()},"connection":{"prefix":"https://example.com/app","repositoryId":source}})).await.unwrap()["result"]["version"].clone();
    let mut moved = task_old["task"].clone();
    moved["repositoryId"] = json!(destination);
    f.call("SaveTask",json!({"id":task,"meta":{"requestId":id()},"expectedVersionId":task_old["versionId"],"task":moved})).await.unwrap();
    f.capture("SaveURLConnection",json!({"id":rule,"meta":{"requestId":id()},"expectedVersionId":rule_old["versionId"],"connection":{"prefix":"https://example.com/app","repositoryId":destination}})).await.unwrap();
    f.call(
        "DeleteRepository",
        json!({"id":source,"meta":{"requestId":id()}}),
    )
    .await
    .unwrap();
    let before: Vec<u8> = sqlx::query_scalar(
        "SELECT pending_png FROM lane_attachments WHERE account_id=$1 AND id=$2",
    )
    .bind(&f.p.account_id)
    .bind(string(&image["capture"], "attachmentId"))
    .fetch_one(&mut f.db)
    .await
    .unwrap();
    // Use new request IDs, so the immutable-version replay branch must handle both.
    for old in [task_old.clone(), rule_old.clone()] {
        let replay = f.push(mutation(old)).await.unwrap();
        assert_eq!(
            replay["results"][0]["result"]["result"],
            "RESULT_KIND_REPLAYED"
        );
    }
    let task_now = f.call("GetTask", json!({"id":task})).await.unwrap();
    assert_eq!(task_now["result"]["result"], "RESULT_KIND_APPLIED");
    assert_eq!(
        task_now["result"]["version"]["task"]["repositoryId"],
        destination
    );
    assert_eq!(
        task_now["result"]["version"]["task"]["captures"][0],
        image["capture"]
    );
    let rules = f.capture("ListURLConnections", json!({})).await.unwrap();
    assert_eq!(rules["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        rules["items"][0]["version"]["urlConnection"]["repositoryId"],
        destination
    );
    for (entity, old) in [(&task, &task_old), (&rule, &rule_old)] {
        let history = f.call("ListHistory", json!({"id":entity})).await.unwrap();
        assert_eq!(history["versions"].as_array().unwrap().len(), 2);
        assert!(history["versions"].as_array().unwrap().contains(old));
    }
    let after: Vec<u8> = sqlx::query_scalar(
        "SELECT pending_png FROM lane_attachments WHERE account_id=$1 AND id=$2",
    )
    .bind(&f.p.account_id)
    .bind(string(&image["capture"], "attachmentId"))
    .fetch_one(&mut f.db)
    .await
    .unwrap();
    assert_eq!(after, before);
    assert_eq!(f.count("lane_attachment_identities").await, 1);
    let garbage: i64 = sqlx::query_scalar("SELECT count(*) FROM lane_attachment_garbage")
        .fetch_one(&mut f.db)
        .await
        .unwrap();
    assert_eq!(garbage, 0);
    f.close().await;
}

// ---------------------------------------------------------------------------------------------
// Planning: shared vectors executed against PostgreSQL, then targeted domain tests.
// ---------------------------------------------------------------------------------------------
use lane_api::planning::violation_of;
use std::collections::BTreeMap;

impl Fixture {
    /// Accepted bodies of every live task.
    async fn task_bodies(&mut self) -> BTreeMap<String, Value> {
        let rows: Vec<(String, Value)> = sqlx::query_as("SELECT e.id,v.envelope->'task' FROM lane_entities e JOIN lane_versions v ON v.account_id=e.account_id AND v.entity_id=e.id AND v.version_id=e.accepted_version_id WHERE e.account_id=$1 AND e.kind=2 AND NOT e.deleted")
            .bind(&self.p.account_id).fetch_all(&mut self.db).await.unwrap();
        rows.into_iter().collect()
    }
    async fn accepted(&mut self, entity: &str) -> String {
        sqlx::query_scalar(
            "SELECT accepted_version_id FROM lane_entities WHERE account_id=$1 AND id=$2",
        )
        .bind(&self.p.account_id)
        .bind(entity)
        .fetch_one(&mut self.db)
        .await
        .unwrap()
    }
    /// Run a mutation with the fixture's request, substituting the real expected version.
    async fn run(&mut self, method: &str, mut request: Value) -> Result<Value> {
        if request.get("expectedVersionId").is_some() {
            let current = self.accepted(string(&request, "id")).await;
            request["expectedVersionId"] = json!(current);
        }
        self.call(method, request).await
    }
    async fn planning(&mut self, entity: &str, item: Value) -> Result<Value> {
        self.call(
            "CreatePlanningItem",
            json!({"meta":{"requestId":id()},"id":entity,"item":item}),
        )
        .await
    }
    async fn label(&mut self, entity: &str, repo: &str, name: &str) -> Value {
        self.planning(
            entity,
            json!({"label":{"repositoryId":repo,"name":name,"color":"#aabbcc"}}),
        )
        .await
        .unwrap()
    }
    /// Counts that must not move when an operation is rejected.
    async fn footprint(&mut self) -> Vec<i64> {
        let mut out = Vec::new();
        for table in [
            "lane_versions",
            "lane_heads",
            "lane_entities",
            "lane_changes",
            "lane_entity_receipts",
            "lane_version_identities",
        ] {
            out.push(self.count(table).await);
        }
        out
    }
}

/// Recreate a fixture state through the public operations so every row is real.
async fn seed(f: &mut Fixture, state: &Value) {
    let entities = state["entities"].as_array().unwrap();
    let mut first_repo = String::new();
    for e in entities {
        if e["kind"] == "ENTITY_KIND_REPOSITORY" {
            let url = format!(
                "https://{}",
                e["repository"]["normalizedUrl"].as_str().unwrap()
            );
            if first_repo.is_empty() {
                first_repo = string(e, "entityId").to_owned();
            }
            f.repo(string(e, "entityId"), &url).await;
        }
    }
    for e in entities {
        let entity = string(e, "entityId");
        match string(e, "kind") {
            "ENTITY_KIND_LABEL" | "ENTITY_KIND_MILESTONE" | "ENTITY_KIND_CYCLE" => {
                let key = match string(e, "kind") {
                    "ENTITY_KIND_LABEL" => "label",
                    "ENTITY_KIND_MILESTONE" => "milestone",
                    _ => "cycle",
                };
                if e["deleted"] == true {
                    f.label(entity, &first_repo, &format!("deleted {entity}"))
                        .await;
                    f.call(
                        "DeletePlanningItem",
                        json!({"meta":{"requestId":id()},"id":entity}),
                    )
                    .await
                    .unwrap();
                } else {
                    f.planning(entity, json!({ key: e[key] })).await.unwrap();
                }
            }
            "ENTITY_KIND_TASK" if e["deleted"] == true => {
                f.call("CreateTask", json!({"meta":{"requestId":id()},"id":entity,"repository":{"id":first_repo},"title":"deleted"})).await.unwrap();
            }
            "ENTITY_KIND_TASK" => {
                let body = &e["task"];
                f.call("CreateTask", json!({"meta":{"requestId":id()},"id":entity,"repository":{"id":body["repositoryId"]},"title":body["title"],"description":body["description"],"progress":body["progress"],"proposal":body["proposal"]})).await.unwrap();
            }
            _ => {}
        }
    }
    // Second pass: planning fields and relationships once every task exists.
    for e in entities {
        if e["kind"] == "ENTITY_KIND_TASK" && e["deleted"] != true {
            let body = &e["task"];
            let plain = [
                "repositoryId",
                "title",
                "description",
                "progress",
                "proposal",
            ];
            if body
                .as_object()
                .unwrap()
                .keys()
                .any(|k| !plain.contains(&k.as_str()))
            {
                let entity = string(e, "entityId");
                f.run("SaveTask", json!({"meta":{"requestId":id()},"id":entity,"expectedVersionId":"x","task":body})).await.unwrap();
            }
        }
    }
    for e in entities {
        if e["kind"] == "ENTITY_KIND_TASK" && e["deleted"] == true {
            f.call(
                "DeleteTask",
                json!({"meta":{"requestId":id()},"id":string(e,"entityId")}),
            )
            .await
            .unwrap();
        }
    }
}
fn violation_name(f: &lane_api::Fault) -> Option<&'static str> {
    violation_of(f).map(|v| v.name())
}
fn expect_rejection(expected: &Value, got: &lane_api::Fault, case: &str) {
    let reason = match string(expected, "reason") {
        "ERROR_REASON_INVALID_INPUT" => 1,
        "ERROR_REASON_CONSTRAINT_VIOLATION" => 19,
        other => panic!("unexpected reason {other}"),
    };
    assert_eq!(got.reason & 0xff, reason, "{case}: {got:?}");
    assert_eq!(
        violation_name(got),
        expected.get("violation").and_then(Value::as_str),
        "{case}"
    );
}
fn affected_bodies(response: &Value) -> BTreeMap<String, Value> {
    response["affected"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            (
                string(&r["version"], "entityId").to_owned(),
                r["version"]["task"].clone(),
            )
        })
        .collect()
}
fn expected_bodies(list: &Value) -> BTreeMap<String, Value> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|v| (string(v, "entityId").to_owned(), v["task"].clone()))
        .collect()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn shared_planning_validation_and_graph_vectors_execute_database_domain() {
    for suite in ["planning-validation", "planning-graph"] {
        let doc = fixtures(suite);
        for case in doc["cases"].as_array().unwrap() {
            let name = format!("{suite}/{}", case["id"]);
            let mut f = Fixture::new().await;
            seed(&mut f, &doc["state"]).await;
            let before = f.footprint().await;
            let method = string(&case["input"], "method");
            let response = f.run(method, case["input"]["request"].clone()).await;
            let expected = &case["expected"];
            if expected["outcome"] == "error" {
                expect_rejection(expected, &response.unwrap_err(), &name);
                // A rejected operation leaves no version, change, receipt or cleanup behind.
                assert_eq!(f.footprint().await, before, "{name}: partial writes");
            } else {
                let response = response.unwrap_or_else(|e| panic!("{name}: {e:?}"));
                let version = &response["result"]["version"];
                for key in ["task", "label", "milestone", "cycle"] {
                    if expected["normalized"].get(key).is_some() {
                        assert_eq!(version[key], expected["normalized"][key], "{name}");
                    }
                }
                assert_eq!(version["formatVersion"], 3);
                if let Some(list) = expected.get("affected") {
                    assert_eq!(affected_bodies(&response), expected_bodies(list), "{name}");
                    // Cascaded versions exist in the store with the caller's origin.
                    for (task, body) in affected_bodies(&response) {
                        assert_eq!(f.task_bodies().await[&task], body, "{name}");
                    }
                }
            }
            f.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn shared_planning_cleanup_vectors_execute_database_domain() {
    let doc = fixtures("planning-cleanup");
    for case in doc["cases"].as_array().unwrap() {
        let name = format!("cleanup/{}", case["id"]);
        let mut f = Fixture::new().await;
        seed(&mut f, &doc["state"]).await;
        let before = f.task_bodies().await;
        let method = string(&case["input"], "method");
        let request = case["input"]["request"].clone();
        let mut request = request;
        request["meta"] = json!({"requestId":id()});
        f.run(method, request.clone())
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let after = f.task_bodies().await;
        let changed: BTreeMap<String, Value> = after
            .iter()
            .filter(|(k, v)| before.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(
            changed,
            expected_bodies(&case["expected"]["changed"]),
            "{name}"
        );
        for gone in case["expected"]["deleted"].as_array().unwrap() {
            let deleted: bool = sqlx::query_scalar(
                "SELECT deleted FROM lane_entities WHERE account_id=$1 AND id=$2",
            )
            .bind(&f.p.account_id)
            .bind(gone.as_str().unwrap())
            .fetch_one(&mut f.db)
            .await
            .unwrap();
            assert!(deleted, "{name}");
        }
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn shared_planning_list_vectors_execute_database_domain() {
    let doc = fixtures("planning-list");
    let mut f = Fixture::new().await;
    seed(&mut f, &doc["dataset"]).await;
    for case in doc["cases"].as_array().unwrap() {
        let name = format!("list/{}", case["id"]);
        let mut request = case["input"]["request"].clone();
        let mut pages: Vec<Vec<String>> = Vec::new();
        loop {
            let page = f
                .call("ListTasks", request.clone())
                .await
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            pages.push(
                page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| string(&r["version"], "entityId").to_owned())
                    .collect(),
            );
            let next = string(&page, "nextCursor").to_owned();
            if next.is_empty() {
                break;
            }
            request["page"]["cursor"] = json!(next);
        }
        let want_pages: Vec<Vec<String>> = case["expected"]["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                p.as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_owned())
                    .collect()
            })
            .collect();
        assert_eq!(pages, want_pages, "{name}");
    }
    // A cursor is bound to its account, filter and sort.
    let first = f
        .call(
            "ListTasks",
            json!({"page":{"size":2},"sortKey":"TASK_SORT_KEY_PRIORITY"}),
        )
        .await
        .unwrap();
    let cursor = string(&first, "nextCursor").to_owned();
    assert!(!cursor.is_empty());
    for other in [
        json!({"page":{"size":2,"cursor":cursor},"sortKey":"TASK_SORT_KEY_TYPE"}),
        json!({"page":{"size":2,"cursor":cursor},"sortKey":"TASK_SORT_KEY_PRIORITY","sortDirection":"SORT_DIRECTION_DESCENDING"}),
        json!({"page":{"size":2,"cursor":cursor},"sortKey":"TASK_SORT_KEY_PRIORITY","filter":{"includeUnlabeled":true}}),
    ] {
        assert_eq!(f.call("ListTasks", other).await.unwrap_err().reason, 1);
    }
    f.close().await;
}

const LABEL: &str = "019a0000-0000-7000-8000-0000000000a1";
const MILESTONE: &str = "019a0000-0000-7000-8000-0000000000a2";
const CYCLE: &str = "019a0000-0000-7000-8000-0000000000a3";
const TASK_B: &str = "019a0000-0000-7000-8000-000000000004";
const TASK_C: &str = "019a0000-0000-7000-8000-000000000005";

fn task_fields(title: &str, repo: &str, extra: Value) -> Value {
    let mut task = json!({"repositoryId":repo,"title":title,"description":"","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR"});
    for (k, v) in extra.as_object().into_iter().flatten() {
        task[k] = v.clone();
    }
    task
}
impl Fixture {
    async fn save_task(
        &mut self,
        entity: &str,
        task: Value,
        edits: Option<Value>,
    ) -> Result<Value> {
        let mut q =
            json!({"meta":{"requestId":id()},"id":entity,"expectedVersionId":"x","task":task});
        if let Some(edits) = edits {
            q["relationshipEdits"] = edits;
        }
        self.run("SaveTask", q).await
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn planning_item_lifecycle_conflict_history_and_deletion_redaction() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let request = json!({"meta":{"requestId":id()},"id":LABEL,"item":{"label":{"repositoryId":REPO,"name":" SecretLabelName ","color":"#aabbcc","description":"SecretDescription"}}});
    let created = f.call("CreatePlanningItem", request.clone()).await.unwrap();
    let label = &created["result"]["version"]["label"];
    assert_eq!(created["result"]["version"]["kind"], "ENTITY_KIND_LABEL");
    assert_eq!(created["result"]["version"]["formatVersion"], 3);
    assert_eq!(
        (label["name"].as_str(), label["color"].as_str()),
        (Some("SecretLabelName"), Some("#AABBCC"))
    );
    // Response-loss retry: same receipt, no new version; changed content is rejected.
    let replay = f.call("CreatePlanningItem", request.clone()).await.unwrap();
    assert_eq!(replay["result"]["result"], "RESULT_KIND_REPLAYED");
    assert_eq!(f.heads(LABEL).await.len(), 1);
    let mut changed = request.clone();
    changed["item"]["label"]["color"] = json!("#000000");
    assert_eq!(
        f.call("CreatePlanningItem", changed)
            .await
            .unwrap_err()
            .reason,
        6
    );
    // Wrong kind / unknown planning item.
    assert_eq!(
        f.call("GetTask", json!({"id":LABEL}))
            .await
            .unwrap_err()
            .reason,
        2
    );
    assert_eq!(
        f.call("GetPlanningItem", json!({"id":REPO}))
            .await
            .unwrap_err()
            .reason,
        2
    );
    let got = f
        .call("GetPlanningItem", json!({"id":LABEL}))
        .await
        .unwrap();
    assert_eq!(got["result"]["version"]["label"]["name"], "SecretLabelName");

    // Kind and repository are immutable; unchanged items save new versions with history.
    let v1 = created["result"]["version"]["versionId"].clone();
    let other_repo = "019a0000-0000-7000-8000-0000000000b1";
    f.repo(other_repo, "https://example.com/other").await;
    for item in [
        json!({"label":{"repositoryId":other_repo,"name":"SecretLabelName","color":"#AABBCC"}}),
        json!({"milestone":{"repositoryId":REPO,"name":"SecretLabelName"}}),
    ] {
        let r = f
            .call(
                "SavePlanningItem",
                json!({"meta":{"requestId":id()},"id":LABEL,"expectedVersionId":v1,"item":item}),
            )
            .await;
        assert_eq!(r.unwrap_err().reason, 1);
    }
    let saved = f.call("SavePlanningItem", json!({"meta":{"requestId":id()},"id":LABEL,"expectedVersionId":v1,"item":{"label":{"repositoryId":REPO,"name":"SecretLabelName","color":"#112233","description":"SecretDescription"}}})).await.unwrap();
    assert_eq!(saved["result"]["version"]["label"]["color"], "#112233");
    let history = f.call("ListHistory", json!({"id":LABEL})).await.unwrap();
    assert_eq!(history["versions"].as_array().unwrap().len(), 2);

    // A stale save is a whole-item conflict resolved through ResolveConflict.
    let stale = f.call("SavePlanningItem", json!({"meta":{"requestId":id()},"id":LABEL,"expectedVersionId":v1,"item":{"label":{"repositoryId":REPO,"name":"SecretLabelName","color":"#445566"}}})).await.unwrap();
    assert_eq!(stale["result"]["result"], "RESULT_KIND_CONFLICT");
    let blocked = f.run("SavePlanningItem", json!({"meta":{"requestId":id()},"id":LABEL,"expectedVersionId":"x","item":{"label":{"repositoryId":REPO,"name":"SecretLabelName","color":"#778899"}}})).await;
    assert_eq!(blocked.unwrap_err().reason, 4);
    let heads = f.heads(LABEL).await;
    assert_eq!(heads.len(), 2);
    let resolved = f.call("ResolveConflict", json!({"meta":{"requestId":id()},"id":LABEL,"expectedHeadIds":heads,"resolved":{"label":{"repositoryId":REPO,"name":"SecretLabelName","color":"#abcdef"}}})).await.unwrap();
    assert_eq!(resolved["result"]["version"]["label"]["color"], "#ABCDEF");
    assert_eq!(f.heads(LABEL).await.len(), 1);

    // Assign it, then delete it: the task loses only the assignment and keeps no name.
    f.task(TASK, REPO).await;
    let tagged = f
        .save_task(
            TASK,
            task_fields("Title", REPO, json!({"labelIds":[LABEL]})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        tagged["result"]["version"]["task"]["labelIds"],
        json!([LABEL])
    );
    let deleter = f.p.clone();
    f.p.origin_kind = "ORIGIN_KIND_MCP".into();
    f.p.device_name = "Deleting agent".into();
    let deleted = f
        .call(
            "DeletePlanningItem",
            json!({"meta":{"requestId":id()},"id":LABEL}),
        )
        .await
        .unwrap();
    assert_eq!(deleted["result"]["result"], "RESULT_KIND_DELETED");
    assert!(deleted["result"]["version"].get("label").is_none());
    let t = f.call("GetTask", json!({"id":TASK})).await.unwrap();
    assert!(t["result"]["version"]["task"].get("labelIds").is_none());
    assert_eq!(t["result"]["version"]["origin"]["kind"], "ORIGIN_KIND_MCP");
    f.p = deleter;
    // The item's content is gone from versions, changes, receipts and every task history.
    for table in ["lane_versions", "lane_changes", "lane_entity_receipts"] {
        let column = if table == "lane_versions" {
            "envelope"
        } else if table == "lane_changes" {
            "data"
        } else {
            "response"
        };
        let leaked: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE account_id=$1 AND ({column})::text LIKE '%SecretLabelName%' OR ({column})::text LIKE '%SecretDescription%'"))
            .bind(&f.p.account_id).fetch_one(&mut f.db).await.unwrap();
        assert_eq!(leaked, 0, "{table}");
    }
    let task_history = f.call("ListHistory", json!({"id":TASK})).await.unwrap();
    assert!(!task_history.to_string().contains("SecretLabelName"));
    assert_eq!(
        f.call("ListHistory", json!({"id":LABEL})).await.unwrap()["versions"],
        json!([])
    );
    // Replays after deletion report DELETED without content; the name is reusable.
    let late = f.call("CreatePlanningItem", request).await.unwrap();
    assert_eq!(late["result"]["result"], "RESULT_KIND_DELETED");
    assert!(late["result"]["version"].get("label").is_none());
    f.planning(
        MILESTONE,
        json!({"milestone":{"repositoryId":REPO,"name":"SecretLabelName"}}),
    )
    .await
    .unwrap();
    f.label(
        "019a0000-0000-7000-8000-0000000000a9",
        REPO,
        "secretlabelname",
    )
    .await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn conflicted_tasks_are_read_only_for_cascades_and_rollback_is_complete() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    f.task(TASK, REPO).await;
    f.task(TASK_B, REPO).await;
    f.task(TASK_C, REPO).await;
    let b0 = f.accepted(TASK_B).await;
    for title in ["one", "two"] {
        let r = f.call("SaveTask", json!({"meta":{"requestId":id()},"id":TASK_B,"expectedVersionId":b0,"task":task_fields(title, REPO, json!({}))})).await;
        let _ = r; // the second stale save preserves a conflict alternative
    }
    assert!(f.heads(TASK_B).await.len() > 1);
    let before = f.footprint().await;
    // Edits that would rewrite a conflicted task fail the whole operation, including the
    // primary write and the other affected task.
    for edits in [
        json!({"addSubtaskIds":[TASK_C, TASK_B]}),
        json!({"addBlockedTaskIds":[TASK_C, TASK_B]}),
    ] {
        let r = f
            .save_task(TASK, task_fields("Title", REPO, json!({})), Some(edits))
            .await;
        assert_eq!(r.unwrap_err().reason, 4);
        assert_eq!(f.footprint().await, before);
    }
    let r = f
        .save_task(
            TASK,
            task_fields("Title", REPO, json!({"relatedTaskIds":[TASK_B]})),
            None,
        )
        .await;
    assert_eq!(r.unwrap_err().reason, 4);
    assert_eq!(f.footprint().await, before);
    // The same edit without the conflicted task succeeds and reports the affected task.
    let ok = f
        .save_task(
            TASK,
            task_fields("Title", REPO, json!({})),
            Some(json!({"addSubtaskIds":[TASK_C]})),
        )
        .await
        .unwrap();
    assert_eq!(ok["affected"].as_array().unwrap().len(), 1);
    assert_eq!(ok["affected"][0]["version"]["task"]["parentTaskId"], TASK);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn repository_deletion_cascades_planning_items_and_account_clearing_covers_them() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let keep = "019a0000-0000-7000-8000-0000000000b1";
    f.repo(keep, "https://example.com/keep").await;
    f.label(LABEL, REPO, "L").await;
    f.planning(
        MILESTONE,
        json!({"milestone":{"repositoryId":REPO,"name":"M"}}),
    )
    .await
    .unwrap();
    f.planning(CYCLE, json!({"cycle":{"repositoryId":REPO,"name":"C","startDate":"2026-01-01","endDate":"2026-01-02"}})).await.unwrap();
    let survivor = "019a0000-0000-7000-8000-0000000000c1";
    f.label(survivor, keep, "L").await;
    f.task(TASK, REPO).await;
    let listed = f.call("ListPlanningItems", json!({})).await.unwrap();
    assert_eq!(listed["items"].as_array().unwrap().len(), 4);
    f.call(
        "DeleteRepository",
        json!({"meta":{"requestId":id()},"id":REPO}),
    )
    .await
    .unwrap();
    for item in [LABEL, MILESTONE, CYCLE] {
        let live: bool = sqlx::query_scalar(
            "SELECT NOT deleted FROM lane_entities WHERE account_id=$1 AND id=$2",
        )
        .bind(&f.p.account_id)
        .bind(item)
        .fetch_one(&mut f.db)
        .await
        .unwrap();
        assert!(!live, "{item}");
        assert_eq!(
            f.call("GetPlanningItem", json!({"id":item})).await.unwrap()["result"]["result"],
            "RESULT_KIND_DELETED"
        );
    }
    let listed = f.call("ListPlanningItems", json!({})).await.unwrap();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["version"]["entityId"], survivor);
    // Account clearing removes planning items and their versions.
    // The auth and limit migrations are outside this fixture; clearing also touches their tables.
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS lane_sync_rejections(account_id text,request_id text,reason int); CREATE TABLE IF NOT EXISTS lane_control_receipts(account_id text)").execute(&mut f.db).await.unwrap();
    let mut tx = f.db.begin().await.unwrap();
    lane_api::clear_account_content(&mut tx, &f.p.account_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(f.count("lane_entities").await, 0);
    assert_eq!(f.count("lane_versions").await, 0);
    assert_eq!(f.count("lane_heads").await, 0);
    f.close().await;
}

/// Run two operations on separate connections so the second queues on the account lock while
/// the first is uncommitted. Returns both outcomes after the first commits.
async fn raced(
    f: &mut Fixture,
    first: (&str, Value),
    second: (&str, Value),
) -> (Result<Value>, Result<Value>) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let mut other = PgConnection::connect(&url).await.unwrap();
    sqlx::query(&format!("SET search_path TO {}", f.schema))
        .execute(&mut other)
        .await
        .unwrap();
    let principal = f.p.clone();
    let (method, request) = (second.0.to_owned(), second.1);
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &f.p.account_id).await.unwrap();
    let a = entities::dispatch(&mut tx, &f.p, first.0, &first.1, usize::MAX).await;
    let handle = tokio::spawn(async move {
        let mut tx = other.begin().await.unwrap();
        account_lock(&mut tx, &principal.account_id).await.unwrap();
        let b = entities::dispatch(&mut tx, &principal, &method, &request, usize::MAX).await;
        if b.is_ok() {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        b
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !handle.is_finished(),
        "the second writer must wait for the account lock"
    );
    if a.is_ok() {
        tx.commit().await.unwrap();
    } else {
        tx.rollback().await.unwrap();
    }
    (a, handle.await.unwrap())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn concurrent_writers_cannot_jointly_violate_name_cycle_or_reference_rules() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    f.task(TASK, REPO).await;
    f.task(TASK_B, REPO).await;
    // Two writers racing on one case-insensitive name.
    let create = |entity: &str, name: &str| {
        (
            "CreatePlanningItem",
            json!({"meta":{"requestId":id()},"id":entity,"item":{"label":{"repositoryId":REPO,"name":name,"color":"#000000"}}}),
        )
    };
    let (a, b) = raced(
        &mut f,
        create(LABEL, "Dup"),
        create("019a0000-0000-7000-8000-0000000000a8", "dup"),
    )
    .await;
    assert!(a.is_ok());
    assert_eq!(
        violation_name(&b.unwrap_err()),
        Some("CONSTRAINT_VIOLATION_DUPLICATE_NAME")
    );
    // Two writers that each add one half of a parent cycle.
    let ta = f.accepted(TASK).await;
    let tb = f.accepted(TASK_B).await;
    let parent = |entity: &str, version: &str, parent: &str, repo: &str| {
        (
            "SaveTask",
            json!({"meta":{"requestId":id()},"id":entity,"expectedVersionId":version,"task":task_fields("Title", repo, json!({"parentTaskId":parent}))}),
        )
    };
    let (a, b) = raced(
        &mut f,
        parent(TASK, &ta, TASK_B, REPO),
        parent(TASK_B, &tb, TASK, REPO),
    )
    .await;
    assert!(a.is_ok());
    assert_eq!(
        violation_name(&b.unwrap_err()),
        Some("CONSTRAINT_VIOLATION_PARENT_CYCLE")
    );
    // Delete versus assign: the loser sees the winner's committed state.
    let ta = f.accepted(TASK).await;
    let assign = (
        "SaveTask",
        json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":ta,"task":task_fields("Title", REPO, json!({"labelIds":[LABEL],"parentTaskId":TASK_B}))}),
    );
    let delete = (
        "DeletePlanningItem",
        json!({"meta":{"requestId":id()},"id":LABEL}),
    );
    let (a, b) = raced(&mut f, delete.clone(), assign.clone()).await;
    assert!(a.is_ok());
    assert_eq!(
        violation_name(&b.unwrap_err()),
        Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND")
    );
    // Assign first, then delete: the committed assignment is cleaned up.
    f.label(CYCLE, REPO, "Other").await;
    let ta = f.accepted(TASK).await;
    let assign = (
        "SaveTask",
        json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":ta,"task":task_fields("Title", REPO, json!({"labelIds":[CYCLE],"parentTaskId":TASK_B}))}),
    );
    let delete = (
        "DeletePlanningItem",
        json!({"meta":{"requestId":id()},"id":CYCLE}),
    );
    let (a, b) = raced(&mut f, assign, delete).await;
    assert!(a.is_ok() && b.is_ok());
    let body = &f.task_bodies().await[TASK];
    assert!(body.get("labelIds").is_none());
    assert_eq!(body["parentTaskId"], TASK_B);
    f.close().await;
}

fn pushed_task(entity: &str, version: &str, parents: &[&str], task: Value, format: u64) -> Value {
    json!({"formatVersion":format,"entityId":entity,"kind":"ENTITY_KIND_TASK","versionId":version,"parentVersionIds":parents,"createdAt":"2026-01-01T00:00:00.000000Z","origin":{"kind":"ORIGIN_KIND_CLI","deviceId":"019a0000-0000-7000-8000-000000000001","deviceName":"Fixture CLI","accountId":""},"deleted":false,"task":task})
}
fn pushed_item(
    entity: &str,
    version: &str,
    kind: &str,
    key: &str,
    body: Value,
    format: u64,
) -> Value {
    json!({"formatVersion":format,"entityId":entity,"kind":kind,"versionId":version,"parentVersionIds":[],"createdAt":"2026-01-01T00:00:00.000000Z","origin":{"kind":"ORIGIN_KIND_CLI","deviceId":"019a0000-0000-7000-8000-000000000001","deviceName":"Fixture CLI","accountId":""},"deleted":false,key:body})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn pushed_versions_are_validated_against_accepted_state_and_rejections_change_nothing() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let other_repo = "019a0000-0000-7000-8000-0000000000b1";
    f.repo(other_repo, "https://example.com/other").await;
    let label = f
        .push(mutation(pushed_item(
            LABEL,
            &id(),
            "ENTITY_KIND_LABEL",
            "label",
            json!({"repositoryId":REPO,"name":"Docs","color":"#AABBCC"}),
            3,
        )))
        .await
        .unwrap();
    assert_eq!(
        label["results"][0]["result"]["version"]["kind"],
        "ENTITY_KIND_LABEL"
    );
    let foreign = "019a0000-0000-7000-8000-0000000000b2";
    f.push(mutation(pushed_item(
        foreign,
        &id(),
        "ENTITY_KIND_LABEL",
        "label",
        json!({"repositoryId":other_repo,"name":"Docs","color":"#AABBCC"}),
        3,
    )))
    .await
    .unwrap();
    let (v1, v2) = (id(), id());
    f.push(mutation(pushed_task(
        TASK,
        &v1,
        &[],
        task_fields("A", REPO, json!({"labelIds":[LABEL]})),
        3,
    )))
    .await
    .unwrap();
    f.push(mutation(pushed_task(
        TASK_B,
        &v2,
        &[],
        task_fields("B", REPO, json!({"parentTaskId":TASK})),
        3,
    )))
    .await
    .unwrap();
    let before = f.footprint().await;

    // Each constraint is a named, non-retryable violation that writes nothing.
    let cases = [
        (
            pushed_task(
                TASK,
                &id(),
                &[&v1],
                task_fields("A", REPO, json!({"parentTaskId":TASK_B})),
                3,
            ),
            "CONSTRAINT_VIOLATION_PARENT_CYCLE",
        ),
        (
            pushed_task(
                TASK,
                &id(),
                &[&v1],
                task_fields("A", REPO, json!({"dependsOnTaskIds":[TASK]})),
                3,
            ),
            "CONSTRAINT_VIOLATION_SELF_LINK",
        ),
        (
            pushed_task(
                TASK,
                &id(),
                &[&v1],
                task_fields("A", REPO, json!({"labelIds":[foreign]})),
                3,
            ),
            "CONSTRAINT_VIOLATION_CROSS_REPOSITORY",
        ),
        (
            pushed_task(
                TASK,
                &id(),
                &[&v1],
                task_fields("A", REPO, json!({"milestoneId":MILESTONE})),
                3,
            ),
            "CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND",
        ),
        (
            pushed_task(
                TASK,
                &id(),
                &[&v1],
                task_fields(
                    "A",
                    REPO,
                    json!({"startDate":"2026-02-02","dueDate":"2026-02-01"}),
                ),
                3,
            ),
            "CONSTRAINT_VIOLATION_DATE_ORDER",
        ),
        (
            pushed_task(
                TASK,
                &id(),
                &[&v1],
                task_fields("A", REPO, json!({"storyPoints":2147483648_u64})),
                3,
            ),
            "CONSTRAINT_VIOLATION_FIELD_RANGE",
        ),
        (
            pushed_item(
                "019a0000-0000-7000-8000-0000000000b3",
                &id(),
                "ENTITY_KIND_LABEL",
                "label",
                json!({"repositoryId":REPO,"name":"docs","color":"#000000"}),
                3,
            ),
            "CONSTRAINT_VIOLATION_DUPLICATE_NAME",
        ),
        (
            pushed_item(
                "019a0000-0000-7000-8000-0000000000b4",
                &id(),
                "ENTITY_KIND_CYCLE",
                "cycle",
                json!({"repositoryId":REPO,"name":"Sprint","startDate":"2026-02-02","endDate":"2026-02-01"}),
                3,
            ),
            "CONSTRAINT_VIOLATION_DATE_ORDER",
        ),
    ];
    for (version, expected) in cases {
        let rejected = f.push(mutation(version)).await.unwrap_err();
        assert_eq!(violation_name(&rejected), Some(expected));
        assert_eq!(rejected.code, connectrpc::ErrorCode::FailedPrecondition);
        assert_eq!(f.footprint().await, before, "{expected}");
    }
    // Planning data requires format 3.
    let old = pushed_task(
        TASK,
        &id(),
        &[&v1],
        task_fields("A", REPO, json!({"priority":"TASK_PRIORITY_HIGH"})),
        2,
    );
    assert_eq!(f.push(mutation(old)).await.unwrap_err().reason, 8);
    let old_item = pushed_item(
        "019a0000-0000-7000-8000-0000000000b5",
        &id(),
        "ENTITY_KIND_LABEL",
        "label",
        json!({"repositoryId":REPO,"name":"Old","color":"#000000"}),
        2,
    );
    assert_eq!(f.push(mutation(old_item)).await.unwrap_err().reason, 8);
    assert_eq!(f.footprint().await, before);
    // A format 2 task without planning fields is still accepted.
    f.push(mutation(pushed_task(
        TASK_C,
        &id(),
        &[],
        task_fields("C", REPO, json!({})),
        2,
    )))
    .await
    .unwrap();

    // The corrected version arrives as a new immutable version with a new request ID.
    let corrected = pushed_task(
        TASK,
        &id(),
        &[&v1],
        task_fields(
            "A",
            REPO,
            json!({"labelIds":[LABEL],"dependsOnTaskIds":[TASK_B]}),
        ),
        3,
    );
    let accepted = f.push(mutation(corrected)).await.unwrap();
    assert_eq!(
        accepted["results"][0]["result"]["result"],
        "RESULT_KIND_APPLIED"
    );
    // A concurrent version of a task is validated with the union of references: B -> A would
    // close a dependency cycle through A's accepted head.
    let concurrent = pushed_task(
        TASK_B,
        &id(),
        &[&v2],
        task_fields(
            "B",
            REPO,
            json!({"parentTaskId":TASK,"dependsOnTaskIds":[TASK]}),
        ),
        3,
    );
    assert_eq!(
        violation_name(&f.push(mutation(concurrent)).await.unwrap_err()),
        Some("CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE")
    );
    // Pull exposes the planning entities.
    let pulled = f
        .sync("PullChanges", json!({"pageSize":100}), usize::MAX)
        .await
        .unwrap();
    let kinds: Vec<&str> = pulled["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["entity"]["version"]["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"ENTITY_KIND_LABEL") && kinds.contains(&"ENTITY_KIND_TASK"));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn task_planning_requests_are_idempotent_and_account_isolated() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    f.task(TASK, REPO).await;
    f.label(LABEL, REPO, "L").await;
    let second = "019a0000-0000-7000-8000-0000000000d1";
    f.label(second, REPO, "M").await;
    let request = json!({"meta":{"requestId":id()},"id":TASK_B,"repository":{"id":REPO},"title":"New","priority":"TASK_PRIORITY_URGENT","type":"TASK_TYPE_BUG","storyPoints":0,"labelIds":[second,LABEL],"parentTaskId":TASK,"relationshipEdits":{"addBlockedTaskIds":[TASK]}});
    let created = f.call("CreateTask", request.clone()).await.unwrap();
    let body = &created["result"]["version"]["task"];
    assert_eq!(body["storyPoints"], 0);
    assert_eq!(
        body["labelIds"],
        json!(
            [LABEL, second]
                .iter()
                .cloned()
                .min()
                .map(|_| {
                    let mut v = vec![LABEL, second];
                    v.sort();
                    v
                })
                .unwrap()
        )
    );
    assert_eq!(created["affected"][0]["version"]["entityId"], TASK);
    let versions = f.count("lane_versions").await;
    // Same request, set fields spelled in another order: replay, no duplicate entity or history.
    let mut reordered = request.clone();
    reordered["labelIds"] = json!([LABEL, second]);
    reordered["relationshipEdits"]["addBlockedTaskIds"] = json!([TASK, TASK]);
    let replay = f.call("CreateTask", reordered).await.unwrap();
    assert_eq!(replay["result"]["result"], "RESULT_KIND_REPLAYED");
    assert_eq!(replay["affected"], created["affected"]);
    assert_eq!(f.count("lane_versions").await, versions);
    // Changed planning content under the same request ID is a reuse error.
    let mut changed = request.clone();
    changed["priority"] = json!("TASK_PRIORITY_LOW");
    assert_eq!(f.call("CreateTask", changed).await.unwrap_err().reason, 6);
    // Story points 0 differ from absent in the fingerprint.
    let mut absent = request.clone();
    absent.as_object_mut().unwrap().remove("storyPoints");
    assert_eq!(f.call("CreateTask", absent).await.unwrap_err().reason, 6);
    // Deleting an affected task redacts it inside the receipt that reported it.
    f.call("DeleteTask", json!({"meta":{"requestId":id()},"id":TASK}))
        .await
        .unwrap();
    let after = f.call("CreateTask", request.clone()).await.unwrap();
    assert_eq!(after["affected"][0]["result"], "RESULT_KIND_DELETED");
    assert!(after["affected"][0]["version"].get("task").is_none());
    // The created task lost its parent link to the deleted task.
    assert!(f.task_bodies().await[TASK_B].get("parentTaskId").is_none());
    // Relationship edits never touch progress or proposal.
    let child = f.call("GetTask", json!({"id":TASK_B})).await.unwrap();
    assert_eq!(
        child["result"]["version"]["task"]["progress"],
        "PROGRESS_TODO"
    );

    // Another account cannot see or reference these entities.
    let mut stranger = f.p.clone();
    stranger.account_id = id();
    stranger.tenant_id = id();
    sqlx::query("INSERT INTO lane_accounts VALUES($1,$2,$3)")
        .bind(&stranger.account_id)
        .bind(&stranger.tenant_id)
        .bind(id())
        .execute(&mut f.db)
        .await
        .unwrap();
    let theirs = "019a0000-0000-7000-8000-0000000000e1";
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &stranger.account_id).await.unwrap();
    entities::dispatch(
        &mut tx,
        &stranger,
        "CreateRepository",
        &json!({"meta":{"requestId":id()},"id":theirs,"url":"https://example.com/theirs"}),
        usize::MAX,
    )
    .await
    .unwrap();
    let probe = entities::dispatch(&mut tx, &stranger, "CreateTask", &json!({"meta":{"requestId":id()},"id":"019a0000-0000-7000-8000-0000000000e2","repository":{"id":theirs},"title":"x","labelIds":[LABEL],"parentTaskId":TASK}), usize::MAX).await;
    assert_eq!(
        violation_name(&probe.unwrap_err()),
        Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND")
    );
    tx.rollback().await.unwrap();
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &stranger.account_id).await.unwrap();
    assert_eq!(
        entities::dispatch(
            &mut tx,
            &stranger,
            "GetPlanningItem",
            &json!({"id":LABEL}),
            usize::MAX
        )
        .await
        .unwrap_err()
        .reason,
        2
    );
    assert!(
        entities::dispatch(
            &mut tx,
            &stranger,
            "ListPlanningItems",
            &json!({}),
            usize::MAX
        )
        .await
        .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    tx.rollback().await.unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn planning_item_listing_filters_pages_and_rejects_invalid_kinds() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let other = "019a0000-0000-7000-8000-0000000000b1";
    f.repo(other, "https://example.com/other").await;
    f.label(LABEL, REPO, "L").await;
    f.planning(
        MILESTONE,
        json!({"milestone":{"repositoryId":REPO,"name":"M","targetDate":"2026-05-01"}}),
    )
    .await
    .unwrap();
    f.planning(CYCLE, json!({"cycle":{"repositoryId":other,"name":"C","startDate":"2026-01-01","endDate":"2026-01-02"}})).await.unwrap();
    assert_eq!(
        f.call("ListPlanningItems", json!({"kind":"ENTITY_KIND_MILESTONE"}))
            .await
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.call("ListPlanningItems", json!({"repositoryId":REPO}))
            .await
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        f.call(
            "ListPlanningItems",
            json!({"repositoryId":other,"kind":"ENTITY_KIND_LABEL"})
        )
        .await
        .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        f.call("ListPlanningItems", json!({"kind":"ENTITY_KIND_TASK"}))
            .await
            .unwrap_err()
            .reason,
        1
    );
    let mut seen = Vec::new();
    let mut page = json!({"page":{"size":1}});
    loop {
        let r = f.call("ListPlanningItems", page.clone()).await.unwrap();
        seen.extend(
            r["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| string(&i["version"], "entityId").to_owned()),
        );
        if string(&r, "nextCursor").is_empty() {
            break;
        }
        page["page"]["cursor"] = r["nextCursor"].clone();
    }
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(seen, sorted);
    assert_eq!(seen.len(), 3);
    f.close().await;
}
