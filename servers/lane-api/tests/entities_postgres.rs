// SPDX-License-Identifier: FSL-1.1-ALv2
//! Real domain tests; set TEST_DATABASE_URL to a disposable database and run --ignored.
use delino_protocol::lane::v1::ResolveConflictRequest;
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
fn typed_resolution(q: &Value) -> Value {
    serde_json::to_value(serde_json::from_value::<ResolveConflictRequest>(q.clone()).unwrap())
        .unwrap()
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
async fn resolved_task_planning_sets_replay_before_head_validation() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let labels = [id(), id()];
    for label in &labels {
        f.call(
            "CreatePlanningItem",
            json!({"meta":{"requestId":id()},"id":label,
            "item":{"label":{"repositoryId":REPO,"name":label,"color":"#abcdef"}}}),
        )
        .await
        .unwrap();
    }
    let endpoints = [id(), id()];
    for endpoint in &endpoints {
        f.task(endpoint, REPO).await;
    }
    let base = f.task(TASK, REPO).await;
    let first = f
        .call(
            "SetProgress",
            json!({"meta":{"requestId":id()},"id":TASK,
        "expectedVersionId":base["versionId"],"progress":"PROGRESS_DONE"}),
        )
        .await
        .unwrap();
    f.call(
        "SetProgress",
        json!({"meta":{"requestId":id()},"id":TASK,
        "expectedVersionId":base["versionId"],"progress":"PROGRESS_BLOCKED"}),
    )
    .await
    .unwrap();
    let heads = f.heads(TASK).await;
    assert_eq!(heads.len(), 2);
    let mut resolved = first["result"]["version"].clone();
    resolved["task"]["labelIds"] = json!([labels[1], labels[0]]);
    resolved["task"]["dependsOnTaskIds"] = json!([endpoints[1], endpoints[0]]);
    resolved["task"]["relatedTaskIds"] = json!([endpoints[1], endpoints[0]]);
    let q = typed_resolution(&json!({"meta":{"requestId":id()},"id":TASK,
        "expectedHeadIds":heads,"resolved":resolved}));
    let before_failed = (
        f.count("lane_versions").await,
        f.count("lane_entity_receipts").await,
    );
    let mut invalid = q.clone();
    invalid["meta"]["requestId"] = json!(id());
    invalid["resolved"]["task"]["labelIds"] = json!([id()]);
    assert_eq!(
        lane_api::planning::violation_of(&f.call("ResolveConflict", invalid).await.unwrap_err())
            .map(|v| v.name()),
        Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND")
    );
    assert_eq!(
        (
            f.count("lane_versions").await,
            f.count("lane_entity_receipts").await
        ),
        before_failed
    );

    let applied = f
        .call("ResolveConflict", typed_resolution(&q))
        .await
        .unwrap();
    let version = applied["result"]["version"]["versionId"].clone();
    let stable = (
        f.count("lane_versions").await,
        f.count("lane_entity_receipts").await,
        f.count("lane_changes").await,
    );
    let mut equivalent = q.clone();
    for (key, values) in [
        ("labelIds", &labels),
        ("dependsOnTaskIds", &endpoints),
        ("relatedTaskIds", &endpoints),
    ] {
        equivalent["resolved"]["task"][key] = json!([values[0], values[1], values[0]]);
        let replay = f
            .call("ResolveConflict", typed_resolution(&equivalent))
            .await
            .unwrap();
        assert_eq!(replay["result"]["result"], "RESULT_KIND_REPLAYED");
        assert_eq!(replay["result"]["version"]["versionId"], version);
        assert_eq!(
            (
                f.count("lane_versions").await,
                f.count("lane_entity_receipts").await,
                f.count("lane_changes").await
            ),
            stable
        );
    }
    for key in ["labelIds", "dependsOnTaskIds", "relatedTaskIds"] {
        let mut changed = q.clone();
        changed["resolved"]["task"][key] = json!([]);
        assert_eq!(
            f.call("ResolveConflict", changed).await.unwrap_err().reason,
            6
        );
    }
    for field in ["title", "prompt"] {
        let mut changed = q.clone();
        changed["resolved"]["task"][field] = json!("");
        assert_eq!(
            f.call("ResolveConflict", changed).await.unwrap_err().reason,
            6
        );
    }
    let mut changed = q.clone();
    changed["resolved"]["versionId"] = json!(id());
    assert_eq!(
        f.call("ResolveConflict", changed).await.unwrap_err().reason,
        6
    );
    assert_eq!(
        (
            f.count("lane_versions").await,
            f.count("lane_entity_receipts").await,
            f.count("lane_changes").await
        ),
        stable
    );
    let mut stale = q.clone();
    stale["meta"]["requestId"] = json!(id());
    assert_eq!(
        f.call("ResolveConflict", stale).await.unwrap_err().reason,
        5
    );
    let edited = f
        .call(
            "SetProgress",
            json!({"meta":{"requestId":id()},"id":TASK,
        "expectedVersionId":version,"progress":"PROGRESS_IN_PROGRESS"}),
        )
        .await
        .unwrap();
    assert_eq!(edited["result"]["result"], "RESULT_KIND_APPLIED");
    let replay = f.call("ResolveConflict", equivalent.clone()).await.unwrap();
    assert_eq!(
        replay["result"]["version"]["versionId"],
        applied["result"]["version"]["versionId"]
    );
    f.call("DeleteTask", json!({"meta":{"requestId":id()},"id":TASK}))
        .await
        .unwrap();
    let redacted = f.call("ResolveConflict", equivalent).await.unwrap();
    assert_eq!(redacted["result"]["result"], "RESULT_KIND_DELETED");
    assert_eq!(redacted["result"]["version"]["deleted"], true);
    assert!(redacted["result"]["version"].get("task").is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn resolution_pinned_to_reviewed_heads_is_refused_after_a_newer_conflict_forms() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let base = f.task(TASK, REPO).await;
    let first = f
        .call(
            "SetProgress",
            json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":base["versionId"],"progress":"PROGRESS_DONE"}),
        )
        .await
        .unwrap();
    let h1 = first["result"]["version"].clone();
    let h2 = f
        .call(
            "SetProgress",
            json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":base["versionId"],"progress":"PROGRESS_BLOCKED"}),
        )
        .await
        .unwrap()["result"]["version"]
        .clone();
    let reviewed = f.heads(TASK).await;
    assert_eq!(reviewed.len(), 2);
    // Another client resolves H1/H2 into R, then a draft based on H1 forms a new conflict R/S.
    let resolved = f
        .call(
            "ResolveConflict",
            json!({"meta":{"requestId":id()},"id":TASK,"expectedHeadIds":reviewed,"resolved":h2}),
        )
        .await
        .unwrap();
    let r = resolved["result"]["version"]["versionId"].clone();
    f.call(
        "SetProgress",
        json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":h1["versionId"],"progress":"PROGRESS_IN_PROGRESS"}),
    )
    .await
    .unwrap();
    let newer = f.heads(TASK).await;
    assert_eq!(newer.len(), 2);
    assert!(newer.contains(&r.as_str().unwrap().to_string()));
    let versions = f.count("lane_versions").await;
    // The client still pins H1/H2: the server refuses it and creates nothing.
    let stale = f
        .call(
            "ResolveConflict",
            json!({"meta":{"requestId":id()},"id":TASK,"expectedHeadIds":reviewed,"resolved":h2}),
        )
        .await;
    assert_eq!(stale.unwrap_err().reason, 5);
    assert_eq!(f.heads(TASK).await, newer);
    assert_eq!(f.count("lane_versions").await, versions);
    // After reviewing R/S, a fresh resolution binds to exactly those heads and keeps both parents.
    let fresh = f
        .call(
            "ResolveConflict",
            json!({"meta":{"requestId":id()},"id":TASK,"expectedHeadIds":newer,"resolved":h2}),
        )
        .await
        .unwrap();
    let mut parents: Vec<String> = fresh["result"]["version"]["parentVersionIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    parents.sort();
    assert_eq!(parents, newer);
    assert_eq!(f.heads(TASK).await.len(), 1);
    assert_eq!(f.count("lane_versions").await, versions + 1);
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

const PORT_URL: &str = "example.com:443/a/b";
const PORTLESS_URL: &str = "example.com/a/b";

/// A format-2 repository version as a synchronizing client would upload it.
fn repo_version(entity: &str, url: &str, parents: &[String], name: Option<&str>) -> Value {
    let mut v = json!({
        "formatVersion":2,
        "entityId":entity,
        "kind":"ENTITY_KIND_REPOSITORY",
        "versionId":id(),
        "parentVersionIds":parents,
        "createdAt":"2026-01-01T00:00:00.000000Z",
        "origin":{"kind":"ORIGIN_KIND_CLI","deviceId":"","deviceName":"Fixture CLI","accountId":""},
        "deleted":false,
        "promptRestoration":false,
        "repository":{"normalizedUrl":url},
    });
    if let Some(name) = name {
        v["repository"]["displayName"] = json!(name);
    }
    v
}
fn resolve_mutation(v: Value, expected: &[String]) -> Value {
    json!({"meta":{"requestId":id()},"kind":"MUTATION_KIND_RESOLVE","version":v,"expectedHeadIds":expected})
}
impl Fixture {
    async fn stored_envelope(&mut self, version: &str) -> Value {
        sqlx::query_scalar(
            "SELECT envelope FROM lane_versions WHERE account_id=$1 AND version_id=$2",
        )
        .bind(&self.p.account_id)
        .bind(version)
        .fetch_one(&mut self.db)
        .await
        .unwrap()
    }
    async fn projected_url(&mut self, entity: &str) -> Option<String> {
        sqlx::query_scalar("SELECT normalized_url FROM lane_entities WHERE account_id=$1 AND id=$2")
            .bind(&self.p.account_id)
            .bind(entity)
            .fetch_one(&mut self.db)
            .await
            .unwrap()
    }
    /// Create a repository interactively from its raw SSH URL, then save `alternatives + 1`
    /// concurrent versions from the original one so that every head retains the explicit port.
    async fn conflicted_port_repository(
        &mut self,
        entity: &str,
        raw: &str,
        normalized: &str,
        alternatives: usize,
    ) -> Vec<String> {
        let original = self.repo(entity, raw).await;
        assert_eq!(original["repository"]["normalizedUrl"], json!(normalized));
        for n in 0..=alternatives {
            let saved = self
                .call(
                    "SaveRepository",
                    json!({"meta":{"requestId":id()},"id":entity,"expectedVersionId":original["versionId"],"url":raw,"displayName":format!("Head {n}")}),
                )
                .await
                .unwrap();
            assert_eq!(
                saved["result"]["version"]["repository"]["normalizedUrl"],
                json!(normalized)
            );
        }
        let heads = self.heads(entity).await;
        assert_eq!(heads.len(), alternatives + 1);
        heads
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn sync_create_preserves_explicit_port_without_mapping_to_a_portless_repository() {
    for with_portless in [false, true] {
        let mut f = Fixture::new().await;
        let portless = id();
        if with_portless {
            f.repo(&portless, "https://example.com/a/b").await;
        }
        let entity = id();
        let version = repo_version(&entity, PORT_URL, &[], Some("Port"));
        let m = mutation(version.clone());
        let pushed = f.push(m.clone()).await.unwrap();
        assert_eq!(
            pushed["results"][0]["result"]["result"],
            "RESULT_KIND_APPLIED"
        );
        assert_eq!(pushed["repositoryMappings"], json!([]));
        assert_eq!(
            pushed["results"][0]["result"]["version"]["repository"],
            version["repository"]
        );
        // The persisted envelope, entity projection and head all keep the explicit port.
        let stored = f.stored_envelope(string(&version, "versionId")).await;
        assert_eq!(stored["repository"], version["repository"]);
        assert_eq!(f.projected_url(&entity).await.as_deref(), Some(PORT_URL));
        assert_eq!(
            f.heads(&entity).await,
            vec![string(&version, "versionId").to_owned()]
        );
        if with_portless {
            assert_eq!(
                f.projected_url(&portless).await.as_deref(),
                Some(PORTLESS_URL)
            );
            assert_eq!(f.heads(&portless).await.len(), 1);
        }
        assert_eq!(f.count("lane_repository_mappings").await, 0);
        // Replaying the same mutation acknowledges the original result without new rows.
        let (versions, entities) = (
            f.count("lane_versions").await,
            f.count("lane_entities").await,
        );
        let replay = f.push(m).await.unwrap();
        assert_eq!(
            replay["results"][0]["result"]["result"],
            "RESULT_KIND_REPLAYED"
        );
        assert_eq!(
            replay["results"][0]["result"]["version"],
            pushed["results"][0]["result"]["version"]
        );
        assert_eq!(replay["repositoryMappings"], json!([]));
        // A fresh transport request for the identical immutable version is also a replay.
        let again = f.push(mutation(version.clone())).await.unwrap();
        assert_eq!(
            again["results"][0]["result"]["result"],
            "RESULT_KIND_REPLAYED"
        );
        assert_eq!(
            again["results"][0]["result"]["version"]["repository"],
            version["repository"]
        );
        assert_eq!(f.count("lane_versions").await, versions);
        assert_eq!(f.count("lane_entities").await, entities);
        assert_eq!(f.count("lane_repository_mappings").await, 0);
        // A true URL collision still maps the later repository to the owner of the exact URL.
        let duplicate = id();
        let collided = f
            .push(mutation(repo_version(&duplicate, PORT_URL, &[], None)))
            .await
            .unwrap();
        assert_eq!(
            collided["repositoryMappings"],
            json!([{"localId":duplicate,"canonicalId":entity}])
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn interactive_port_repository_version_replays_through_sync_create() {
    let mut f = Fixture::new().await;
    let entity = id();
    let created = f
        .call(
            "CreateRepository",
            json!({"meta":{"requestId":id()},"id":entity,"url":"ssh://git@example.com:443/a/b.git","displayName":"Port"}),
        )
        .await
        .unwrap()["result"]["version"]
        .clone();
    assert_eq!(
        created["repository"],
        json!({"normalizedUrl":PORT_URL,"displayName":"Port"})
    );
    let (versions, entities) = (
        f.count("lane_versions").await,
        f.count("lane_entities").await,
    );
    let replay = f.push(mutation(created.clone())).await.unwrap();
    assert_eq!(
        replay["results"][0]["result"]["result"],
        "RESULT_KIND_REPLAYED"
    );
    assert_eq!(replay["results"][0]["result"]["version"], created);
    assert_eq!(f.count("lane_versions").await, versions);
    assert_eq!(f.count("lane_entities").await, entities);
    assert_eq!(f.count("lane_repository_mappings").await, 0);
    assert_eq!(
        f.stored_envelope(string(&created, "versionId")).await,
        created
    );
    // A genuinely changed body under the same version ID is still rejected without writes.
    for changed in [
        json!({"normalizedUrl":"example.com/a/b","displayName":"Port"}),
        json!({"normalizedUrl":PORT_URL,"displayName":"Changed"}),
        json!({"normalizedUrl":"example.com:8443/a/b","displayName":"Port"}),
    ] {
        let mut v = created.clone();
        v["repository"] = changed;
        let err = f.push(mutation(v)).await.unwrap_err();
        assert_eq!(err.reason, 6);
    }
    assert_eq!(
        f.stored_envelope(string(&created, "versionId")).await,
        created
    );
    assert_eq!(f.count("lane_versions").await, versions);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn interactive_and_sync_resolution_retain_the_explicit_port() {
    for with_portless in [false, true] {
        let mut f = Fixture::new().await;
        if with_portless {
            f.repo(&id(), "https://example.com/a/b").await;
            f.repo(&id(), "https://example.com/c/d").await;
        }
        // Interactive ResolveConflict.
        let a = id();
        let heads = f
            .conflicted_port_repository(&a, "ssh://git@example.com:443/a/b.git", PORT_URL, 1)
            .await;
        let history = f.count("lane_versions").await;
        let resolved = f
            .call(
                "ResolveConflict",
                json!({"meta":{"requestId":id()},"id":a,"expectedHeadIds":heads,"resolved":{"repository":{"normalizedUrl":PORT_URL,"displayName":"Merged"}}}),
            )
            .await
            .unwrap();
        assert_eq!(
            resolved["result"]["version"]["repository"],
            json!({"normalizedUrl":PORT_URL,"displayName":"Merged"})
        );
        let head = f.heads(&a).await;
        assert_eq!(head.len(), 1);
        assert_eq!(f.count("lane_versions").await, history + 1);
        let stored = f.stored_envelope(&head[0]).await;
        assert_eq!(stored["repository"]["normalizedUrl"], json!(PORT_URL));
        let mut parents: Vec<String> = stored["parentVersionIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        parents.sort();
        assert_eq!(parents, heads);
        assert_eq!(f.projected_url(&a).await.as_deref(), Some(PORT_URL));

        // Sync RESOLVE with the complete observed head set.
        let b = id();
        let heads = f
            .conflicted_port_repository(
                &b,
                "ssh://git@example.com:443/c/d.git",
                "example.com:443/c/d",
                1,
            )
            .await;
        let history = f.count("lane_versions").await;
        let version = repo_version(&b, "example.com:443/c/d", &heads, Some("Merged"));
        let result = f
            .push(resolve_mutation(version.clone(), &heads))
            .await
            .unwrap();
        assert_eq!(result["repositoryMappings"], json!([]));
        assert_eq!(
            result["results"][0]["result"]["version"]["repository"],
            version["repository"]
        );
        assert_eq!(
            f.heads(&b).await,
            vec![string(&version, "versionId").to_owned()]
        );
        assert_eq!(f.count("lane_versions").await, history + 1);
        assert_eq!(
            f.projected_url(&b).await.as_deref(),
            Some("example.com:443/c/d")
        );
        assert_eq!(
            f.stored_envelope(string(&version, "versionId")).await["repository"],
            version["repository"]
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn stale_and_colliding_port_resolutions_are_rejected_without_writes() {
    let mut f = Fixture::new().await;
    f.repo(&id(), "https://example.com/a/b").await;
    let a = id();
    let heads = f
        .conflicted_port_repository(&a, "ssh://git@example.com:443/a/b.git", PORT_URL, 1)
        .await;
    let snapshot = (
        f.count("lane_versions").await,
        f.count("lane_entities").await,
        f.count("lane_repository_mappings").await,
    );
    // True URL collision: the portless URL is owned by another repository.
    let err = f
        .call(
            "ResolveConflict",
            json!({"meta":{"requestId":id()},"id":a,"expectedHeadIds":heads,"resolved":{"repository":{"normalizedUrl":PORTLESS_URL}}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.reason, 1);
    let err = f
        .push(resolve_mutation(
            repo_version(&a, PORTLESS_URL, &heads, None),
            &heads,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.reason, 1);
    assert_eq!(f.heads(&a).await, heads);
    assert_eq!(
        (
            f.count("lane_versions").await,
            f.count("lane_entities").await,
            f.count("lane_repository_mappings").await,
        ),
        snapshot
    );
    // A resolution that keeps the port succeeds even though the portless repository exists.
    f.call(
        "ResolveConflict",
        json!({"meta":{"requestId":id()},"id":a,"expectedHeadIds":heads,"resolved":{"repository":{"normalizedUrl":PORT_URL}}}),
    )
    .await
    .unwrap();
    assert_eq!(f.projected_url(&a).await.as_deref(), Some(PORT_URL));
    // Stale: the observed heads were already replaced by the resolution above.
    let snapshot = (
        f.count("lane_versions").await,
        f.count("lane_entities").await,
        f.count("lane_repository_mappings").await,
    );
    let current = f.heads(&a).await;
    let err = f
        .call(
            "ResolveConflict",
            json!({"meta":{"requestId":id()},"id":a,"expectedHeadIds":heads,"resolved":{"repository":{"normalizedUrl":PORT_URL}}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.reason, 5);
    let err = f
        .push(resolve_mutation(
            repo_version(&a, PORT_URL, &heads, None),
            &heads,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.reason, 5);
    assert_eq!(f.heads(&a).await, current);
    assert_eq!(
        (
            f.count("lane_versions").await,
            f.count("lane_entities").await,
            f.count("lane_repository_mappings").await,
        ),
        snapshot
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn malformed_repository_identifiers_are_rejected_atomically() {
    let mut f = Fixture::new().await;
    for url in [
        "example.com:8443/Mixed/Case",
        "example.com/path.git",
        "example.com:22/a/b",
    ] {
        let entity = id();
        let version = repo_version(&entity, url, &[], None);
        f.push(mutation(version.clone())).await.unwrap();
        assert_eq!(
            f.stored_envelope(string(&version, "versionId")).await["repository"]["normalizedUrl"],
            json!(url)
        );
        assert_eq!(f.projected_url(&entity).await.as_deref(), Some(url));
    }
    let snapshot = (
        f.count("lane_versions").await,
        f.count("lane_entities").await,
        f.count("lane_repository_mappings").await,
    );
    for url in [
        "example.com:443/a/b?x=1",
        "example.com:443/a/b#frag",
        "example.com:443/a/../b",
        "example.com:443/a/./b",
        "example.com:443/a/%2Fb",
        "example.com:443/a/%5Cb",
        "example.com:443",
        "/a/b",
        "",
    ] {
        let err = f
            .push(mutation(repo_version(&id(), url, &[], None)))
            .await
            .unwrap_err();
        assert_eq!(err.reason, 1, "{url:?}");
    }
    assert_eq!(
        (
            f.count("lane_versions").await,
            f.count("lane_entities").await,
            f.count("lane_repository_mappings").await,
        ),
        snapshot
    );
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
        "ERROR_REASON_NOT_FOUND" => 2,
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
async fn explicit_removal_targets_are_account_scoped_and_atomic() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let other_repo = "019a0000-0000-7000-8000-0000000000b1";
    let deleted = "019a0000-0000-7000-8000-0000000000b2";
    let missing = "019a0000-0000-7000-8000-0000000000b3";
    let foreign_account_task = "019a0000-0000-7000-8000-0000000000b4";
    let new_task = "019a0000-0000-7000-8000-0000000000b5";
    f.repo(other_repo, "https://example.com/other").await;
    f.task(TASK, REPO).await;
    f.task(TASK_B, REPO).await;
    f.task(TASK_C, other_repo).await;
    f.task(deleted, REPO).await;
    f.call(
        "DeleteTask",
        json!({"meta":{"requestId":id()},"id":deleted}),
    )
    .await
    .unwrap();
    f.label(LABEL, REPO, "Label").await;

    let mut foreign = f.p.clone();
    foreign.account_id = id();
    foreign.tenant_id = id();
    sqlx::query("INSERT INTO lane_accounts(account_id,tenant_id,epoch) VALUES($1,$2,$3)")
        .bind(&foreign.account_id)
        .bind(&foreign.tenant_id)
        .bind(id())
        .execute(&mut f.db)
        .await
        .unwrap();
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &foreign.account_id).await.unwrap();
    entities::dispatch(
        &mut tx,
        &foreign,
        "CreateRepository",
        &json!({"meta":{"requestId":id()},"id":REPO,"url":"https://example.com/foreign"}),
        usize::MAX,
    )
    .await
    .unwrap();
    entities::dispatch(&mut tx, &foreign, "CreateTask", &json!({"meta":{"requestId":id()},"id":foreign_account_task,"repository":{"id":REPO},"title":"Foreign"}), usize::MAX).await.unwrap();
    tx.commit().await.unwrap();

    for method in ["CreateTask", "SaveTask"] {
        for field in ["removeSubtaskIds", "removeBlockedTaskIds"] {
            for (target, reason, violation) in [
                (
                    missing,
                    19,
                    Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND"),
                ),
                (deleted, 2, None),
                (LABEL, 19, Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND")),
                (TASK_C, 19, Some("CONSTRAINT_VIOLATION_CROSS_REPOSITORY")),
                (
                    foreign_account_task,
                    19,
                    Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND"),
                ),
            ] {
                let edits = json!({field:[target],"addSubtaskIds":[TASK_B]});
                let request = if method == "CreateTask" {
                    json!({"meta":{"requestId":id()},"id":new_task,"repository":{"id":REPO},"title":"New","relationshipEdits":edits})
                } else {
                    json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":f.accepted(TASK).await,"task":task_fields("Title", REPO, json!({})),"relationshipEdits":edits})
                };
                let before = f.footprint().await;
                let bodies = f.task_bodies().await;
                let heads = f.heads(TASK).await;
                let peer_heads = f.heads(TASK_B).await;
                let error = f.call(method, request).await.unwrap_err();
                assert_eq!(error.reason & 0xff, reason, "{method}/{field}/{target}");
                assert_eq!(
                    violation_name(&error),
                    violation,
                    "{method}/{field}/{target}"
                );
                assert_eq!(f.footprint().await, before, "{method}/{field}/{target}");
                assert_eq!(f.task_bodies().await, bodies);
                assert_eq!(f.heads(TASK).await, heads);
                assert_eq!(f.heads(TASK_B).await, peer_heads);
                assert!(!f.task_bodies().await.contains_key(new_task));
            }
        }
    }

    let request = json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":f.accepted(TASK).await,"task":task_fields("Title", REPO, json!({})),"relationshipEdits":{"removeSubtaskIds":[missing]}});
    let before = f.footprint().await;
    assert_eq!(
        violation_name(&f.call("SaveTask", request.clone()).await.unwrap_err()),
        Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND")
    );
    assert_eq!(f.footprint().await, before);
    f.task(missing, REPO).await;
    assert!(f.call("SaveTask", request).await.is_ok());

    for (add, remove, peer_key) in [
        ("addSubtaskIds", "removeSubtaskIds", "parentTaskId"),
        (
            "addBlockedTaskIds",
            "removeBlockedTaskIds",
            "dependsOnTaskIds",
        ),
    ] {
        f.save_task(
            TASK,
            task_fields("Title", REPO, json!({})),
            Some(json!({add:[TASK_B]})),
        )
        .await
        .unwrap();
        let before_peer = f.accepted(TASK_B).await;
        let removed = f
            .save_task(
                TASK,
                task_fields("Title", REPO, json!({})),
                Some(json!({remove:[TASK_B]})),
            )
            .await
            .unwrap();
        assert_eq!(removed["affected"].as_array().unwrap().len(), 1);
        assert_eq!(removed["affected"][0]["version"]["entityId"], TASK_B);
        assert_ne!(f.accepted(TASK_B).await, before_peer);
        assert!(f.task_bodies().await[TASK_B].get(peer_key).is_none());
        let absent_peer = f.accepted(TASK_B).await;
        let no_op = f
            .save_task(
                TASK,
                task_fields("Title", REPO, json!({})),
                Some(json!({remove:[TASK_B]})),
            )
            .await
            .unwrap();
        assert!(no_op["affected"].as_array().is_none_or(Vec::is_empty));
        assert_eq!(f.accepted(TASK_B).await, absent_peer);
    }
    f.save_task(
        TASK,
        task_fields("Title", REPO, json!({})),
        Some(json!({"addSubtaskIds":[TASK_B]})),
    )
    .await
    .unwrap();
    let peer_version = f.accepted(TASK_B).await;
    for title in ["First", "Alternative"] {
        f.call(
            "SaveTask",
            json!({"meta":{"requestId":id()},"id":TASK_B,"expectedVersionId":peer_version,"task":task_fields(title, REPO, json!({"parentTaskId":TASK}))}),
        )
        .await
        .unwrap();
    }
    assert_eq!(f.heads(TASK_B).await.len(), 2);
    let before = f.footprint().await;
    let error = f
        .save_task(
            TASK,
            task_fields("Title", REPO, json!({})),
            Some(json!({"removeSubtaskIds":[TASK_B]})),
        )
        .await
        .unwrap_err();
    assert_eq!(error.reason, 4);
    assert_eq!(f.footprint().await, before);
    f.close().await;
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
async fn stale_specialized_edits_are_validated_against_the_current_heads() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    f.task(TASK, REPO).await;
    f.task(TASK_B, REPO).await;
    // A depends on B, then drops that dependency, then B depends on A.
    let with_dependency = f
        .save_task(
            TASK,
            task_fields("Title", REPO, json!({"dependsOnTaskIds":[TASK_B]})),
            None,
        )
        .await
        .unwrap();
    let stale = string(&with_dependency["result"]["version"], "versionId").to_owned();
    f.save_task(TASK, task_fields("Title", REPO, json!({})), None)
        .await
        .unwrap();
    f.save_task(
        TASK_B,
        task_fields("Title", REPO, json!({"dependsOnTaskIds":[TASK]})),
        None,
    )
    .await
    .unwrap();
    let before = f.footprint().await;
    // The copied stale A -> B edge would close a cycle across the current heads.
    for (method, extra) in [
        ("SetProgress", json!({"progress":"PROGRESS_DONE"})),
        ("RestorePrompt", json!({"sourceVersionId":stale})),
    ] {
        let mut q = json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":stale});
        for (k, v) in extra.as_object().unwrap() {
            q[k] = v.clone();
        }
        let r = f.call(method, q).await.unwrap_err();
        assert_eq!(r.reason & 0xff, 19, "{method}: {r:?}");
        assert_eq!(
            violation_name(&r),
            Some("CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE"),
            "{method}"
        );
        assert_eq!(f.footprint().await, before, "{method}: partial writes");
    }
    // A stale edit that closes no cycle still becomes a conflict alternative.
    f.task(TASK_C, REPO).await;
    let c0 = f.accepted(TASK_C).await;
    f.call("SetProgress", json!({"meta":{"requestId":id()},"id":TASK_C,"expectedVersionId":c0,"progress":"PROGRESS_DONE"})).await.unwrap();
    let alternative = f
        .call("SetProgress", json!({"meta":{"requestId":id()},"id":TASK_C,"expectedVersionId":c0,"progress":"PROGRESS_IN_PROGRESS"}))
        .await
        .unwrap();
    assert_eq!(alternative["result"]["result"], "RESULT_KIND_CONFLICT");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn stale_save_is_validated_without_applying_discarded_relationship_edits() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    f.task(TASK, REPO).await;
    f.task(TASK_B, REPO).await;
    let b1 = f.accepted(TASK_B).await;
    // B advances past the version the stale save is based on, then A becomes a child of B.
    f.call("SaveTask", json!({"meta":{"requestId":id()},"id":TASK_B,"expectedVersionId":b1,"task":task_fields("Title two", REPO, json!({}))})).await.unwrap();
    let a1 = f.accepted(TASK).await;
    f.call("SaveTask", json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":a1,"task":task_fields("Title", REPO, json!({"parentTaskId":TASK_B}))})).await.unwrap();
    let before = f.footprint().await;
    // The edits would detach A, but a stale save never applies them, so A stays a child of B
    // and the alternative that names A as B's parent closes a parent cycle.
    let stale = json!({"meta":{"requestId":id()},"id":TASK_B,"expectedVersionId":b1,
        "task":task_fields("Title three", REPO, json!({"parentTaskId":TASK})),
        "relationshipEdits":{"removeSubtaskIds":[TASK]}});
    let r = f.call("SaveTask", stale).await.unwrap_err();
    assert_eq!(r.reason & 0xff, 19, "{r:?}");
    assert_eq!(
        violation_name(&r),
        Some("CONSTRAINT_VIOLATION_PARENT_CYCLE")
    );
    assert_eq!(f.footprint().await, before, "partial writes");
    // Without the cycle the stale save is preserved as an alternative and edits nothing else.
    let stale = json!({"meta":{"requestId":id()},"id":TASK_B,"expectedVersionId":b1,
        "task":task_fields("Title four", REPO, json!({})),
        "relationshipEdits":{"removeSubtaskIds":[TASK]}});
    let r = f.call("SaveTask", stale).await.unwrap();
    assert_eq!(r["result"]["result"], "RESULT_KIND_CONFLICT");
    assert!(
        r.get("affected")
            .is_none_or(|a| a.as_array().is_some_and(Vec::is_empty))
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn saving_either_endpoint_removes_a_one_directional_pushed_related_link() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    f.task(TASK, REPO).await;
    // A synchronized write stores the link on B only; A has no backlink.
    f.push(mutation(pushed_task(
        TASK_B,
        &id(),
        &[],
        task_fields("B", REPO, json!({"relatedTaskIds":[TASK]})),
        3,
    )))
    .await
    .unwrap();
    let bodies = f.task_bodies().await;
    assert!(bodies[TASK].get("relatedTaskIds").is_none());
    assert_eq!(bodies[TASK_B]["relatedTaskIds"], json!([TASK]));
    // Saving A without the relation clears it on B as well.
    let a1 = f.accepted(TASK).await;
    let saved = f
        .call("SaveTask", json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":a1,"task":task_fields("Title", REPO, json!({}))}))
        .await
        .unwrap();
    assert_eq!(saved["affected"].as_array().unwrap().len(), 1);
    assert_eq!(saved["affected"][0]["version"]["entityId"], TASK_B);
    let bodies = f.task_bodies().await;
    assert!(bodies[TASK_B].get("relatedTaskIds").is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn list_ignores_references_retained_by_conflicted_tasks_after_deletion() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    f.label(LABEL, REPO, "Doomed").await;
    f.planning(
        MILESTONE,
        json!({"milestone":{"repositoryId":REPO,"name":"Doomed"}}),
    )
    .await
    .unwrap();
    f.task(TASK, REPO).await;
    f.task(TASK_B, REPO).await;
    let fields = || {
        task_fields(
            "Title",
            REPO,
            json!({"labelIds":[LABEL],"milestoneId":MILESTONE}),
        )
    };
    let assigned = f.save_task(TASK, fields(), None).await.unwrap();
    let base = string(&assigned["result"]["version"], "versionId").to_owned();
    // Two saves from the same base: the second is a conflict alternative, so the task is
    // conflicted and deletion cleanup deliberately leaves its accepted body untouched.
    for title in ["one", "two"] {
        let mut task = fields();
        task["title"] = json!(title);
        f.call(
            "SaveTask",
            json!({"meta":{"requestId":id()},"id":TASK,"expectedVersionId":base,"task":task}),
        )
        .await
        .unwrap();
    }
    assert_eq!(f.heads(TASK).await.len(), 2);
    for item in [LABEL, MILESTONE] {
        f.call(
            "DeletePlanningItem",
            json!({"meta":{"requestId":id()},"id":item}),
        )
        .await
        .unwrap();
    }
    let accepted = f.task_bodies().await;
    assert_eq!(accepted[TASK]["labelIds"], json!([LABEL]));
    assert_eq!(accepted[TASK]["milestoneId"], MILESTONE);
    let list = |filter: Value| json!({"page":{"size":10},"filter":filter});
    let listed = |page: Value| -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| string(&r["version"], "entityId").to_owned())
            .collect()
    };
    for filter in [
        json!({"labelIds":[LABEL]}),
        json!({"milestoneIds":[MILESTONE]}),
    ] {
        let page = f.call("ListTasks", list(filter)).await.unwrap();
        assert!(listed(page).is_empty());
    }
    for filter in [
        json!({"includeUnlabeled":true}),
        json!({"includeNoMilestone":true}),
    ] {
        let page = f.call("ListTasks", list(filter)).await.unwrap();
        assert_eq!(listed(page), [TASK, TASK_B]);
    }
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
    // A child whose parent version was never accepted (a rejected chain) is a retryable
    // missing-parent error, not a constraint violation, even when it also breaks a rule.
    let orphan = pushed_task(
        TASK,
        &id(),
        &[&id()],
        task_fields("A", REPO, json!({"parentTaskId":TASK_B})),
        3,
    );
    let missing = f.push(mutation(orphan)).await.unwrap_err();
    assert_eq!((missing.reason, violation_name(&missing)), (9, None));
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

// Prompt-history membership is derived from the submitted parent snapshots, never from the
// accepted head, and both write paths share one rule.
impl Fixture {
    async fn envelope(&mut self, version: &str) -> Value {
        sqlx::query_scalar(
            "SELECT envelope FROM lane_versions WHERE account_id=$1 AND version_id=$2",
        )
        .bind(&self.p.account_id)
        .bind(version)
        .fetch_one(&mut self.db)
        .await
        .unwrap()
    }
    async fn flag(&mut self, version: &str) -> bool {
        sqlx::query_scalar(
            "SELECT prompt_version FROM lane_versions WHERE account_id=$1 AND version_id=$2",
        )
        .bind(&self.p.account_id)
        .bind(version)
        .fetch_one(&mut self.db)
        .await
        .unwrap()
    }
    async fn history(&mut self, entity: &str, prompts_only: bool) -> Vec<String> {
        let r = self
            .call(
                "ListHistory",
                json!({"id":entity,"promptsOnly":prompts_only,"page":{"size":100}}),
            )
            .await
            .unwrap();
        let mut ids: Vec<String> = r["versions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| string(v, "versionId").to_owned())
            .collect();
        ids.sort();
        ids
    }
    /// Create a task with an optional prompt and return its first version ID.
    async fn prompt_task(&mut self, entity: &str, prompt: Option<&str>) -> String {
        let mut q = json!({"meta":{"requestId":id()},"id":entity,"repository":{"id":REPO},"title":"initial"});
        if let Some(prompt) = prompt {
            q["prompt"] = json!(prompt);
        }
        let r = self.call("CreateTask", q).await.unwrap();
        string(&r["result"]["version"], "versionId").to_owned()
    }
    /// Submit SaveTask with the complete body of `base` changed by `edit`.
    async fn edit(&mut self, entity: &str, base: &str, edit: impl FnOnce(&mut Value)) -> Value {
        let mut task = self.envelope(base).await["task"].clone();
        edit(&mut task);
        self.call_new(
            "SaveTask",
            json!({"meta":{"requestId":id()},"id":entity,"expectedVersionId":base,"task":task}),
        )
        .await
        .unwrap()
    }
    /// Like `call`, but records the version this request added in `newVersionId`. A preserved
    /// stale save answers with the accepted head, so the added head is found by comparison.
    async fn call_new(&mut self, method: &str, q: Value) -> Result<Value> {
        let entity = string(&q, "id").to_owned();
        let before = self.heads(&entity).await;
        let mut r = self.call(method, q).await?;
        let added = self
            .heads(&entity)
            .await
            .into_iter()
            .find(|h| !before.contains(h));
        r["newVersionId"] =
            json!(added.unwrap_or_else(|| string(&r["result"]["version"], "versionId").to_owned()));
        Ok(r)
    }
    async fn resolve(&mut self, entity: &str, body: Value) -> Value {
        let heads = self.heads(entity).await;
        self.call(
            "ResolveConflict",
            json!({"meta":{"requestId":id()},"id":entity,"expectedHeadIds":heads,"resolved":{"task":body}}),
        )
        .await
        .unwrap()
    }
}
/// The version a request added (see `Fixture::call_new`), else the version it returned.
fn version_id(r: &Value) -> String {
    r.get("newVersionId")
        .map_or_else(
            || string(&r["result"]["version"], "versionId"),
            |v| v.as_str().unwrap(),
        )
        .to_owned()
}
fn set_prompt(task: &mut Value, prompt: Option<&str>) {
    match prompt {
        Some(prompt) => task["prompt"] = json!(prompt),
        None => {
            task.as_object_mut().unwrap().remove("prompt");
        }
    }
}
const PROMPT_CASES: [(Option<&str>, Option<&str>); 2] =
    [(Some("old"), Some("new")), (None, Some(""))];

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn stale_save_membership_follows_the_submitted_base_not_the_accepted_head() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    for (old, new) in PROMPT_CASES {
        // C changes A's prompt but equals the accepted head B: it is a member.
        let task = id();
        let a = f.prompt_task(&task, old).await;
        let b = version_id(&f.edit(&task, &a, |t| set_prompt(t, new)).await);
        let stale = f
            .edit(&task, &a, |t| {
                t["title"] = json!("changed");
                set_prompt(t, new);
            })
            .await;
        assert_eq!(stale["result"]["result"], "RESULT_KIND_CONFLICT");
        let c = version_id(&stale);
        assert!(f.flag(&b).await && f.flag(&c).await, "{old:?}->{new:?}");
        let mut members = vec![b.clone(), c.clone()];
        members.extend(old.map(|_| a.clone()));
        members.sort();
        assert_eq!(f.history(&task, true).await, members, "{old:?}->{new:?}");
        let mut all = vec![a.clone(), b, c];
        all.sort();
        assert_eq!(f.history(&task, false).await, all);
        assert_eq!(f.heads(&task).await.len(), 2);

        // C changes only the title relative to A although B differs: it is not a member.
        let task = id();
        let a = f.prompt_task(&task, old).await;
        let b = version_id(&f.edit(&task, &a, |t| set_prompt(t, new)).await);
        let stale = f
            .edit(&task, &a, |t| {
                t["title"] = json!("changed");
                set_prompt(t, old);
            })
            .await;
        assert_eq!(stale["result"]["result"], "RESULT_KIND_CONFLICT");
        let c = version_id(&stale);
        assert!(!f.flag(&c).await, "{old:?}->{new:?}");
        let mut members = vec![b.clone()];
        members.extend(old.map(|_| a.clone()));
        members.sort();
        assert_eq!(f.history(&task, true).await, members, "{old:?}->{new:?}");
        let mut all = vec![a, b, c];
        all.sort();
        assert_eq!(f.history(&task, false).await, all);
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn resolution_membership_compares_every_parent() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    for (old, new) in PROMPT_CASES {
        let task = id();
        let a = f.prompt_task(&task, old).await;
        let b = version_id(
            &f.edit(&task, &a, |t| t["title"] = json!("title only"))
                .await,
        );
        f.edit(&task, &a, |t| set_prompt(t, new)).await;
        let body = f.envelope(&b).await["task"].clone();
        // The body equals the accepted parent B but differs from C.
        let resolved = version_id(&f.resolve(&task, body).await);
        assert!(f.flag(&resolved).await, "{old:?}->{new:?}");
        assert!(f.history(&task, true).await.contains(&resolved));
        assert_eq!(f.heads(&task).await, vec![resolved]);
    }

    // Resolving to the shared prompt and captures of every parent is not a member.
    let task = id();
    let a = f.prompt_task(&task, Some("same")).await;
    let b = version_id(&f.edit(&task, &a, |t| t["title"] = json!("b")).await);
    f.edit(&task, &a, |t| t["description"] = json!("c")).await;
    let body = f.envelope(&b).await["task"].clone();
    let resolved = version_id(&f.resolve(&task, body).await);
    assert!(!f.flag(&resolved).await);
    assert!(!f.history(&task, true).await.contains(&resolved));

    // Capture metadata differences make a resolution a member, and a stale save a member.
    let task = id();
    let mut image = image();
    image["capture"]["pageTitle"] = json!("Before");
    let created = f
        .capture("CreateCaptureTask", json!({"id":task,"meta":{"requestId":id()},"repositoryId":REPO,"title":"Capture","description":"Problem","images":[image]}))
        .await
        .unwrap();
    let a = version_id(&created);
    let b = version_id(
        &f.edit(&task, &a, |t| t["title"] = json!("title only"))
            .await,
    );
    let stale = f
        .edit(&task, &a, |t| {
            t["captures"][0]["pageTitle"] = json!("After")
        })
        .await;
    assert_eq!(stale["result"]["result"], "RESULT_KIND_CONFLICT");
    assert!(f.flag(&version_id(&stale)).await);
    let body = f.envelope(&b).await["task"].clone();
    let resolved = version_id(&f.resolve(&task, body).await);
    assert!(f.flag(&resolved).await);
    assert!(f.history(&task, true).await.contains(&resolved));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn stale_specialized_edits_use_the_submitted_base_for_membership() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    for (method, extra) in [
        ("SetProgress", json!({"progress":"PROGRESS_DONE"})),
        ("SetProposal", json!({"proposal":"PROPOSAL_ACCEPTED"})),
    ] {
        let task = id();
        let r = f
            .call("CreateTask", json!({"meta":{"requestId":id()},"id":task,"repository":{"id":REPO},"title":"initial","proposal":"PROPOSAL_PENDING","prompt":"old"}))
            .await
            .unwrap();
        let a = version_id(&r);
        // The accepted head changes the prompt; a stale edit that keeps A's prompt is not a
        // member although it differs from the head.
        let b = version_id(&f.edit(&task, &a, |t| t["prompt"] = json!("new")).await);
        let mut q = json!({"meta":{"requestId":id()},"id":task,"expectedVersionId":a});
        for (k, v) in extra.as_object().unwrap() {
            q[k] = v.clone();
        }
        let stale = f.call_new(method, q).await.unwrap();
        assert_eq!(
            stale["result"]["result"], "RESULT_KIND_CONFLICT",
            "{method}"
        );
        let c = version_id(&stale);
        assert_ne!(c, b);
        assert!(!f.flag(&c).await, "{method}");
        assert!(!f.history(&task, true).await.contains(&c), "{method}");
        assert!(f.history(&task, false).await.contains(&c), "{method}");
    }
    // A fresh edit from the head keeps the head's prompt and stays outside the prompt history.
    let task = id();
    let a = f.prompt_task(&task, Some("old")).await;
    let b = version_id(&f.edit(&task, &a, |t| t["prompt"] = json!("new")).await);
    let fresh = f
        .call_new("SetProgress", json!({"meta":{"requestId":id()},"id":task,"expectedVersionId":b,"progress":"PROGRESS_DONE"}))
        .await
        .unwrap();
    assert!(!f.flag(&version_id(&fresh)).await);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn capture_changes_and_normalization_decide_membership() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    let (first, second) = (image(), image());
    let task = id();
    let created = f
        .capture("CreateCaptureTask", json!({"id":task,"meta":{"requestId":id()},"repositoryId":REPO,"title":"Capture","description":"Problem","images":[first.clone(),second.clone()]}))
        .await
        .unwrap();
    let v0 = version_id(&created);
    let (c1, c2) = (first["capture"].clone(), second["capture"].clone());

    // Reordering and removal are fresh capture changes.
    let reordered = version_id(
        &f.edit(&task, &v0, |t| t["captures"] = json!([c2, c1]))
            .await,
    );
    assert!(f.flag(&reordered).await);
    // A stale title-only edit from V0 is unchanged relative to its base although the head
    // reordered the captures.
    let stale = f
        .edit(&task, &v0, |t| t["title"] = json!("title only"))
        .await;
    assert_eq!(stale["result"]["result"], "RESULT_KIND_CONFLICT");
    assert!(!f.flag(&version_id(&stale)).await);
    // A stale removal differs from its base and is a member, on a task of its own.
    let task = id();
    let (third, fourth) = (image(), image());
    let created = f
        .capture("CreateCaptureTask", json!({"id":task,"meta":{"requestId":id()},"repositoryId":REPO,"title":"Capture","description":"Problem","images":[third.clone(),fourth.clone()]}))
        .await
        .unwrap();
    let v0 = version_id(&created);
    f.edit(&task, &v0, |t| {
        t["captures"] = json!([fourth["capture"].clone(), third["capture"].clone()])
    })
    .await;
    let removal = f
        .edit(&task, &v0, |t| {
            t["captures"] = json!([third["capture"].clone()])
        })
        .await;
    assert_eq!(removal["result"]["result"], "RESULT_KIND_CONFLICT");
    assert!(f.flag(&version_id(&removal)).await);
    // Attachment ownership and retained references are unchanged.
    assert_eq!(f.count("lane_attachments").await, 4);
    let other = id();
    f.task(&other, REPO).await;
    let other_base = f.accepted(&other).await;
    let foreign = f
        .call(
            "SaveTask",
            json!({"meta":{"requestId":id()},"id":other,"expectedVersionId":other_base,"task":task_fields("Title", REPO, json!({"captures":[c1]}))}),
        )
        .await
        .unwrap_err();
    assert_eq!(foreign.reason & 0xff, 13, "{foreign:?}");

    // A removal head can restore the retained references from V0.
    let task = id();
    let created = f
        .capture("CreateCaptureTask", json!({"id":task,"meta":{"requestId":id()},"repositoryId":REPO,"title":"Capture","description":"Problem","images":[image()]}))
        .await
        .unwrap();
    let v0 = version_id(&created);
    let removed = version_id(
        &f.edit(&task, &v0, |t| {
            t.as_object_mut().unwrap().remove("captures");
        })
        .await,
    );
    assert!(f.flag(&removed).await);
    let restored = f
        .call_new("RestorePrompt", json!({"meta":{"requestId":id()},"id":task,"expectedVersionId":removed,"sourceVersionId":v0}))
        .await
        .unwrap();
    assert!(f.flag(&version_id(&restored)).await);

    // Omitted and empty capture arrays are equal after normalization.
    let task = id();
    let v0 = f.prompt_task(&task, Some("p")).await;
    let v1 = version_id(
        &f.edit(&task, &v0, |t| {
            t["title"] = json!("one");
            t["captures"] = json!([]);
        })
        .await,
    );
    assert!(!f.flag(&v1).await);
    let v2 = version_id(
        &f.edit(&task, &v1, |t| {
            t["title"] = json!("two");
            t.as_object_mut().unwrap().remove("captures");
        })
        .await,
    );
    assert!(!f.flag(&v2).await);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn fresh_edits_restorations_and_replay_keep_their_membership() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    for prompt in [Some("same"), None] {
        let task = id();
        let a = f.prompt_task(&task, prompt).await;
        let title = version_id(&f.edit(&task, &a, |t| t["title"] = json!("changed")).await);
        assert!(!f.flag(&title).await);
        let changed = version_id(
            &f.edit(&task, &title, |t| set_prompt(t, Some("different")))
                .await,
        );
        assert!(f.flag(&changed).await);
        // Restoring equal text or an absent prompt is explicit and remains a member, once.
        let back = version_id(&f.edit(&task, &changed, |t| set_prompt(t, prompt)).await);
        assert!(f.flag(&back).await);
        let restore = json!({"meta":{"requestId":id()},"id":task,"expectedVersionId":back,"sourceVersionId":a});
        let restored = f.call("RestorePrompt", restore.clone()).await.unwrap();
        assert!(f.flag(&version_id(&restored)).await);
        let versions = f.count("lane_versions").await;
        let replay = f.call("RestorePrompt", restore).await.unwrap();
        assert_eq!(replay["result"]["result"], "RESULT_KIND_REPLAYED");
        assert_eq!(f.count("lane_versions").await, versions);
    }

    // A replayed stale save creates no extra version.
    let task = id();
    let a = f.prompt_task(&task, Some("old")).await;
    f.edit(&task, &a, |t| t["prompt"] = json!("new")).await;
    let mut stale = json!({"meta":{"requestId":id()},"id":task,"expectedVersionId":a});
    let mut body = f.envelope(&a).await["task"].clone();
    body["title"] = json!("changed");
    stale["task"] = body;
    let first = f.call_new("SaveTask", stale.clone()).await.unwrap();
    let versions = f.count("lane_versions").await;
    f.call("SaveTask", stale).await.unwrap();
    assert_eq!(f.count("lane_versions").await, versions);
    assert!(!f.flag(&version_id(&first)).await);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn interactive_and_sync_classify_equivalent_graphs_identically() {
    let mut f = Fixture::new().await;
    f.repo(REPO, "https://example.com/repo").await;
    // Graph one: A(old) -> B(title only); C(prompt new) stale from A; resolution with B's body.
    // Graph two: A(old) -> B(prompt new); C(title changed, prompt old) stale from A.
    let mut interactive: Vec<Vec<bool>> = Vec::new();
    let mut synced: Vec<Vec<bool>> = Vec::new();
    for shape in [1, 2] {
        let task = id();
        let a = f.prompt_task(&task, Some("old")).await;
        let (b, c) = if shape == 1 {
            (
                version_id(
                    &f.edit(&task, &a, |t| t["title"] = json!("title only"))
                        .await,
                ),
                version_id(&f.edit(&task, &a, |t| t["prompt"] = json!("new")).await),
            )
        } else {
            (
                version_id(&f.edit(&task, &a, |t| t["prompt"] = json!("new")).await),
                version_id(&f.edit(&task, &a, |t| t["title"] = json!("changed")).await),
            )
        };
        let mut ids = vec![a, b.clone(), c];
        if shape == 1 {
            let body = f.envelope(&b).await["task"].clone();
            ids.push(version_id(&f.resolve(&task, body).await));
        }
        let mut flags = Vec::new();
        for version in &ids {
            flags.push(f.flag(version).await);
        }
        interactive.push(flags);

        let task = id();
        let versions: Vec<String> = (0..4).map(|_| id()).collect();
        let body = |title: &str, prompt: &str| task_fields(title, REPO, json!({"prompt":prompt}));
        f.push(mutation(pushed_task(
            &task,
            &versions[0],
            &[],
            body("initial", "old"),
            3,
        )))
        .await
        .unwrap();
        let specs: [(&str, &str); 2] = if shape == 1 {
            [("title only", "old"), ("initial", "new")]
        } else {
            [("initial", "new"), ("changed", "old")]
        };
        for (slot, (title, prompt)) in specs.iter().enumerate() {
            f.push(mutation(pushed_task(
                &task,
                &versions[slot + 1],
                &[&versions[0]],
                body(title, prompt),
                3,
            )))
            .await
            .unwrap();
        }
        let mut count = 3;
        if shape == 1 {
            let heads = f.heads(&task).await;
            assert_eq!(heads.len(), 2);
            f.push(json!({"meta":{"requestId":id()},"kind":"MUTATION_KIND_RESOLVE","expectedHeadIds":heads,"version":pushed_task(&task, &versions[3], &[&versions[1], &versions[2]], body("title only", "old"), 3)}))
                .await
                .unwrap();
            count = 4;
        }
        let mut flags = Vec::new();
        for version in &versions[..count] {
            flags.push(f.flag(version).await);
        }
        synced.push(flags);
    }
    assert_eq!(interactive, synced);
    assert_eq!(interactive[0], vec![true, false, true, true]);
    assert_eq!(interactive[1], vec![true, true, false]);
    f.close().await;
}
