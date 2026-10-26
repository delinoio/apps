// SPDX-License-Identifier: Apache-2.0
//! Command-line and local MCP adapters for task planning, planning items and the development
//! reset, exercised against a real local store.
use lane::{
    auth::Auth,
    cli::{self, Dispatcher, Options},
    protocol::{self, strv},
    runtime::Runtime,
};
use serde_json::{Value, json};
use std::io::Cursor;

struct Harness {
    dir: tempfile::TempDir,
    runtime: Runtime,
}
impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(
            Some(dir.path().to_path_buf()),
            Auth::default(),
            "ORIGIN_KIND_CLI",
        );
        Self { dir, runtime }
    }
    fn run(&mut self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json".to_owned()];
        all.extend(args.iter().map(|a| (*a).to_owned()));
        let mut out = vec![];
        let code = cli::run(
            &all,
            &mut self.runtime,
            &mut Cursor::new(""),
            &mut out,
            &mut vec![],
        );
        let value = serde_json::from_slice(&out).unwrap_or_else(|e| {
            panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out));
        });
        (code, value)
    }
    fn ok(&mut self, args: &[&str]) -> Value {
        let (code, value) = self.run(args);
        assert_eq!((code, &value["ok"]), (0, &json!(true)), "{args:?}: {value}");
        value["result"].clone()
    }
    fn fail(&mut self, args: &[&str], code: i32) -> Value {
        let (got, value) = self.run(args);
        assert_eq!(
            (got, &value["ok"]),
            (code, &json!(false)),
            "{args:?}: {value}"
        );
        value["error"].clone()
    }
    fn repository(&mut self, url: &str) -> String {
        let r = self.ok(&["repository", "create", "--url", url]);
        strv(&r["result"]["version"], "entityId").to_owned()
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "One scenario walks the complete command lifecycle."
)]
fn planning_item_commands_cover_the_full_lifecycle() {
    let mut h = Harness::new();
    let repo = h.repository("https://example.com/cli");
    // The label color shares the --color flag name with terminal color control.
    let label = h.ok(&[
        "label",
        "create",
        "--repository-id",
        &repo,
        "--name",
        "Backend",
        "--color",
        "#1d76db",
        "--color=never",
        "--description",
        "server work",
    ]);
    let label_version = label["result"]["version"].clone();
    let label_id = strv(&label_version, "entityId").to_owned();
    assert_eq!(label_version["label"]["color"], "#1D76DB");
    let milestone = h.ok(&[
        "milestone",
        "create",
        "--repository-id",
        &repo,
        "--name",
        "Beta",
        "--target-date",
        "2026-06-30",
        "--completed",
        "false",
    ]);
    let milestone_id = strv(&milestone["result"]["version"], "entityId").to_owned();
    let cycle = h.ok(&[
        "cycle",
        "create",
        "--repository-id",
        &repo,
        "--name",
        "Sprint 1",
        "--start-date",
        "2026-01-01",
        "--end-date",
        "2026-01-14",
    ]);
    let cycle_id = strv(&cycle["result"]["version"], "entityId").to_owned();

    let listed = h.ok(&["label", "list", "--repository-id", &repo]);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        h.ok(&["cycle", "list"])["items"][0]["version"]["entityId"],
        cycle_id
    );
    assert_eq!(
        h.ok(&["label", "get", &label_id])["result"]["version"]["label"]["name"],
        "Backend"
    );
    // A command group only addresses its own kind.
    assert_eq!(
        h.fail(&["milestone", "get", &label_id], 1)["reason"],
        "ERROR_REASON_NOT_FOUND"
    );
    assert_eq!(
        h.fail(&["milestone", "delete", &label_id], 1)["reason"],
        "ERROR_REASON_NOT_FOUND"
    );

    // Edit requires the expected version and keeps fields it does not mention.
    assert_eq!(
        h.fail(&["label", "edit", &label_id, "--name", "X"], 2)["reason"],
        "ERROR_REASON_INVALID_INPUT"
    );
    let edited = h.ok(&[
        "label",
        "edit",
        &label_id,
        "--expected-version",
        strv(&label_version, "versionId"),
        "--color",
        "#00ff00",
    ]);
    let body = &edited["result"]["version"]["label"];
    assert_eq!(
        (body["name"].as_str(), body["color"].as_str()),
        (Some("Backend"), Some("#00FF00"))
    );
    assert_eq!(body["description"], "server work");
    let error = h.fail(
        &[
            "label",
            "edit",
            &label_id,
            "--expected-version",
            strv(&edited["result"]["version"], "versionId"),
            "--repository-id",
            &protocol::id(),
        ],
        2,
    );
    assert_eq!(error["reason"], "ERROR_REASON_INVALID_INPUT");
    let m = h.ok(&[
        "milestone",
        "edit",
        &milestone_id,
        "--expected-version",
        strv(&milestone["result"]["version"], "versionId"),
        "--completed",
        "true",
        "--target-date",
        "none",
    ]);
    assert_eq!(m["result"]["version"]["milestone"]["completed"], true);
    assert!(m["result"]["version"]["milestone"]["targetDate"].is_null());
    let history = h.ok(&["label", "history", &label_id]);
    assert_eq!(history["versions"].as_array().unwrap().len(), 2);
    // Validation errors carry the named violation.
    let dup = h.fail(
        &[
            "label",
            "create",
            "--repository-id",
            &repo,
            "--name",
            "BACKEND",
            "--color",
            "#000000",
        ],
        1,
    );
    assert_eq!(dup["reason"], "ERROR_REASON_CONSTRAINT_VIOLATION");
    assert_eq!(dup["violation"], "CONSTRAINT_VIOLATION_DUPLICATE_NAME");
    h.fail(
        &[
            "cycle",
            "create",
            "--repository-id",
            &repo,
            "--name",
            "Bad",
            "--start-date",
            "2026-02-01",
            "--end-date",
            "2026-01-01",
        ],
        1,
    );
    h.fail(
        &[
            "label",
            "create",
            "--repository-id",
            &repo,
            "--name",
            "Bad",
            "--color",
            "red",
        ],
        1,
    );
    h.fail(&["task", "list", "--color", "#fff"], 2);

    // Deleting detaches the item from tasks and keeps the tasks.
    let task = h.ok(&[
        "task",
        "create",
        "--repository-id",
        &repo,
        "--title",
        "T",
        "--label-id",
        &label_id,
        "--milestone-id",
        &milestone_id,
        "--cycle-id",
        &cycle_id,
    ]);
    let task_id = strv(&task["result"]["version"], "entityId").to_owned();
    assert_eq!(
        h.ok(&["milestone", "delete", &milestone_id])["result"]["result"],
        "RESULT_KIND_DELETED"
    );
    let after = h.ok(&["task", "get", &task_id]);
    assert!(after["result"]["version"]["task"]["milestoneId"].is_null());
    assert_eq!(
        after["result"]["version"]["task"]["labelIds"],
        json!([label_id])
    );
    assert_eq!(
        h.ok(&["label", "list"])["items"].as_array().unwrap().len(),
        1
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "One scenario walks task options, filters, sorting and clearing."
)]
fn task_commands_set_filter_sort_and_clear_planning_fields() {
    let mut h = Harness::new();
    let repo = h.repository("https://example.com/tasks");
    let label = h.ok(&[
        "label",
        "create",
        "--repository-id",
        &repo,
        "--name",
        "L",
        "--color",
        "#111111",
    ]);
    let label_id = strv(&label["result"]["version"], "entityId").to_owned();
    let created = |h: &mut Harness, title: &str, extra: &[&str]| -> Value {
        let mut args = vec!["task", "create", "--repository-id", &repo, "--title", title];
        args.extend_from_slice(extra);
        h.ok(&args)
    };
    let urgent = created(
        &mut h,
        "Urgent bug",
        &[
            "--priority",
            "urgent",
            "--type",
            "bug",
            "--story-points",
            "0",
            "--due-date",
            "2026-03-01",
            "--label-id",
            &label_id,
        ],
    );
    let low = created(
        &mut h,
        "Low feature",
        &[
            "--priority",
            "low",
            "--type",
            "feature",
            "--story-points",
            "8",
            "--start-date",
            "2026-02-01",
        ],
    );
    let plain = created(&mut h, "Plain", &[]);
    let id_of = |v: &Value| strv(&v["result"]["version"], "entityId").to_owned();
    let (urgent_id, low_id, plain_id) = (id_of(&urgent), id_of(&low), id_of(&plain));
    assert_eq!(urgent["result"]["version"]["task"]["storyPoints"], 0);
    assert!(plain["result"]["version"]["task"]["storyPoints"].is_null());

    let ids = |h: &mut Harness, extra: &[&str]| -> Vec<String> {
        let mut args = vec!["task", "list"];
        args.extend_from_slice(extra);
        h.ok(&args)["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| strv(&i["version"], "entityId").to_owned())
            .collect()
    };
    assert_eq!(
        ids(&mut h, &["--priority", "urgent"]),
        vec![urgent_id.clone()]
    );
    assert_eq!(
        ids(&mut h, &["--priority", "urgent", "--priority-unassigned"]).len(),
        2
    );
    assert_eq!(
        ids(&mut h, &["--points-min", "0", "--points-max", "0"]),
        vec![urgent_id.clone()]
    );
    assert_eq!(
        ids(&mut h, &["--points-unassigned"]),
        vec![plain_id.clone()]
    );
    assert_eq!(ids(&mut h, &["--unlabeled"]).len(), 2);
    assert_eq!(
        ids(&mut h, &["--label-id", &label_id]),
        vec![urgent_id.clone()]
    );
    assert_eq!(
        ids(&mut h, &["--due-from", "2026-03-01"]),
        vec![urgent_id.clone()]
    );
    // Unassigned values sort last in both directions.
    assert_eq!(
        ids(&mut h, &["--sort", "story-points", "--direction", "desc"]),
        vec![low_id.clone(), urgent_id.clone(), plain_id.clone()]
    );
    assert_eq!(
        ids(&mut h, &["--sort", "priority"]),
        vec![urgent_id.clone(), low_id.clone(), plain_id.clone()]
    );
    // Pages continue consistently through the cursor.
    let first = h.ok(&["task", "list", "--sort", "priority", "--page-size", "2"]);
    let cursor = strv(&first, "nextCursor").to_owned();
    assert_ne!(cursor, "");
    let second = h.ok(&[
        "task",
        "list",
        "--sort",
        "priority",
        "--page-size",
        "2",
        "--cursor",
        &cursor,
    ]);
    assert_eq!(second["items"][0]["version"]["entityId"], plain_id);
    h.fail(
        &[
            "task",
            "list",
            "--sort",
            "due-date",
            "--page-size",
            "2",
            "--cursor",
            &cursor,
        ],
        1,
    );
    for bad in [
        vec!["task", "list", "--sort", "color"],
        vec!["task", "list", "--direction", "up"],
        vec!["task", "list", "--priority", "none"],
        vec!["task", "list", "--relationship", "friends"],
        vec!["task", "list", "--due-from", "2026-13-01"],
        vec!["repository", "list", "--priority", "high"],
    ] {
        assert_eq!(
            h.fail(&bad, 2)["reason"],
            "ERROR_REASON_INVALID_INPUT",
            "{bad:?}"
        );
    }

    // Edit: replace sets, clear values, `none`, and relationship edits listing affected tasks.
    let version = strv(&low["result"]["version"], "versionId").to_owned();
    let edited = h.ok(&[
        "task",
        "edit",
        &low_id,
        "--expected-version",
        &version,
        "--priority",
        "none",
        "--story-points",
        "none",
        "--start-date",
        "none",
        "--label-id",
        &label_id,
        "--related",
        &plain_id,
        "--add-subtask",
        &urgent_id,
    ]);
    let t = &edited["result"]["version"]["task"];
    assert_eq!(t["priority"], "TASK_PRIORITY_UNSPECIFIED");
    assert!(t["storyPoints"].is_null() && t["startDate"].is_null());
    assert_eq!(t["relatedTaskIds"], json!([plain_id]));
    assert_eq!(
        t["type"], "TASK_TYPE_FEATURE",
        "unmentioned fields are preserved"
    );
    let affected: Vec<&str> = edited["affected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| strv(&a["version"], "entityId"))
        .collect();
    assert_eq!(affected.len(), 2);
    assert!(affected.contains(&urgent_id.as_str()) && affected.contains(&plain_id.as_str()));
    assert_eq!(
        h.ok(&["task", "get", &urgent_id])["result"]["version"]["task"]["parentTaskId"],
        low_id
    );
    assert_eq!(
        ids(&mut h, &["--relationship", "subtasks"]),
        vec![low_id.clone()]
    );
    assert_eq!(
        ids(&mut h, &["--relationship", "parent"]),
        vec![urgent_id.clone()]
    );
    let mut related = ids(&mut h, &["--relationship", "related", "--no-relationships"]);
    related.sort();
    let mut expected = vec![low_id.clone(), plain_id.clone()];
    expected.sort();
    assert_eq!(related, expected);

    // Field-specific rejections before dispatch.
    let version = strv(&edited["result"]["version"], "versionId").to_owned();
    let range = h.fail(
        &[
            "task",
            "edit",
            &low_id,
            "--expected-version",
            &version,
            "--story-points",
            "-1",
        ],
        2,
    );
    assert_eq!(range["violation"], "CONSTRAINT_VIOLATION_FIELD_RANGE");
    let range = h.fail(
        &[
            "task",
            "create",
            "--repository-id",
            &repo,
            "--title",
            "x",
            "--story-points",
            "2147483648",
        ],
        2,
    );
    assert_eq!(range["violation"], "CONSTRAINT_VIOLATION_FIELD_RANGE");
    for bad in [
        vec![
            "task",
            "edit",
            &low_id,
            "--expected-version",
            &version,
            "--priority",
            "critical",
        ],
        vec![
            "task",
            "edit",
            &low_id,
            "--expected-version",
            &version,
            "--due-date",
            "2026-02-30",
        ],
        vec![
            "task",
            "edit",
            &low_id,
            "--expected-version",
            &version,
            "--label-id",
            "nope",
        ],
        vec![
            "task",
            "edit",
            &low_id,
            "--expected-version",
            &version,
            "--clear-labels",
            "--label-id",
            &label_id,
        ],
    ] {
        assert_eq!(
            h.fail(&bad, 2)["reason"],
            "ERROR_REASON_INVALID_INPUT",
            "{bad:?}"
        );
    }
    // Store constraint failures return the named violation.
    let cycle = h.fail(
        &[
            "task",
            "edit",
            &low_id,
            "--expected-version",
            &version,
            "--parent-id",
            &urgent_id,
        ],
        1,
    );
    assert_eq!(cycle["violation"], "CONSTRAINT_VIOLATION_PARENT_CYCLE");
    let cleared = h.ok(&[
        "task",
        "edit",
        &low_id,
        "--expected-version",
        &version,
        "--clear-labels",
        "--clear-related",
        "--duplicate-of",
        &plain_id,
        "--remove-subtask",
        &urgent_id,
    ]);
    let t = &cleared["result"]["version"]["task"];
    assert_eq!(t["labelIds"], json!([]));
    assert_eq!(t["duplicateOfTaskId"], plain_id);
    assert!(
        h.ok(&["task", "get", &urgent_id])["result"]["version"]["task"]["parentTaskId"].is_null()
    );
}

#[test]
fn development_reset_command_rejections_and_success() {
    let mut h = Harness::new();
    let repo = h.repository("https://example.com/reset-cli");
    h.ok(&[
        "task",
        "create",
        "--repository-id",
        &repo,
        "--title",
        "Gone",
    ]);
    let space = h.ok(&["sync", "status"])["spaceId"]
        .as_str()
        .unwrap()
        .to_owned();
    for args in [
        vec!["data", "reset"],
        vec!["data", "reset", "--confirm-development-reset"],
        vec!["--remote", "data", "reset", "--confirm-development-reset"],
        vec!["--space", &space, "data", "reset"],
        vec![
            "--space",
            &space,
            "data",
            "reset",
            "--confirm-development-reset",
            "--request-json",
            "{}",
        ],
        vec![
            "--space",
            &space,
            "data",
            "reset",
            "--confirm-development-reset",
            "extra",
        ],
        vec![
            "--space",
            &space,
            "task",
            "list",
            "--confirm-development-reset",
        ],
        vec![
            "--space",
            "invalid",
            "data",
            "reset",
            "--confirm-development-reset",
        ],
    ] {
        assert_eq!(
            h.fail(&args, 2)["reason"],
            "ERROR_REASON_INVALID_INPUT",
            "{args:?}"
        );
    }
    assert_eq!(
        h.ok(&["task", "list"])["items"].as_array().unwrap().len(),
        1,
        "rejections change nothing"
    );
    let result = h.ok(&[
        "--space",
        &space,
        "data",
        "reset",
        "--confirm-development-reset",
    ]);
    assert_eq!(result["completed"], true);
    assert_eq!(result["spaceId"], space.as_str());
    assert_eq!(
        h.ok(&["task", "list"])["items"].as_array().unwrap().len(),
        0
    );
    assert_eq!(h.ok(&["sync", "status"])["spaceId"], space.as_str());
    // The command is not exposed through MCP.
    assert!(
        lane::mcp::tools()
            .iter()
            .all(|(_, t)| !t["name"].as_str().unwrap().contains("reset"))
    );
}

#[test]
fn mcp_exposes_typed_planning_tools_and_extended_task_schemas() {
    let tools = lane::mcp::tools();
    let by_name = |name: &str| -> Value {
        tools
            .iter()
            .find(|(m, _)| m == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .1
            .clone()
    };
    for (method, tool) in [
        ("GetPlanningItem", "lane_get_planning_item"),
        ("ListPlanningItems", "lane_list_planning_items"),
        ("CreatePlanningItem", "lane_create_planning_item"),
        ("SavePlanningItem", "lane_save_planning_item"),
        ("DeletePlanningItem", "lane_delete_planning_item"),
    ] {
        let t = by_name(method);
        assert_eq!(t["name"], tool);
        assert!(
            !t["description"].as_str().unwrap().contains("Online-only"),
            "{tool}"
        );
        assert!(t["inputSchema"]["properties"].is_object());
    }
    let create = by_name("CreatePlanningItem")["inputSchema"].clone();
    assert!(create["properties"]["item"]["properties"]["label"]["properties"]["color"].is_object());
    assert!(
        by_name("DeletePlanningItem")["description"]
            .as_str()
            .unwrap()
            .contains("Permanently deletes")
    );
    let create_task = by_name("CreateTask")["inputSchema"]["properties"].clone();
    for field in [
        "priority",
        "type",
        "labelIds",
        "startDate",
        "dueDate",
        "storyPoints",
        "milestoneId",
        "cycleId",
        "parentTaskId",
        "dependsOnTaskIds",
        "relatedTaskIds",
        "duplicateOfTaskId",
        "relationshipEdits",
    ] {
        assert!(create_task[field].is_object(), "{field}");
    }
    let save = by_name("SaveTask")["inputSchema"]["properties"].clone();
    assert!(save["relationshipEdits"]["properties"]["addSubtaskIds"].is_object());
    let list = by_name("ListTasks")["inputSchema"]["properties"].clone();
    for field in ["filter", "sortKey", "sortDirection"] {
        assert!(list[field].is_object(), "{field}");
    }
    assert!(
        list["filter"]["properties"]["storyPoints"]["properties"]["includeUnassigned"].is_object()
    );
    assert!(
        list["sortKey"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("TASK_SORT_KEY_MILESTONE_NAME"))
    );
}

/// Blocking reader fed by a channel so a test can send MCP requests one at a time. Dropping
/// the sender ends the input.
struct ChannelReader {
    receiver: std::sync::mpsc::Receiver<Vec<u8>>,
    pending: Cursor<Vec<u8>>,
}
impl ChannelReader {
    const fn new(receiver: std::sync::mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            receiver,
            pending: Cursor::new(vec![]),
        }
    }
}
impl std::io::Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.pending.read(buf)?;
            if read > 0 {
                return Ok(read);
            }
            match self.receiver.recv() {
                Ok(bytes) => self.pending = Cursor::new(bytes),
                Err(_) => return Ok(0),
            }
        }
    }
}
/// MCP output shared between the session thread and the test thread.
#[derive(Clone, Default)]
struct SharedOutput(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for SharedOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl SharedOutput {
    /// Wait until a complete reply with the given JSON-RPC ID has been written.
    fn wait_for_reply(&self, id: &Value) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
            let found = text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|reply| reply["id"] == *id);
            if let Some(reply) = found {
                return reply;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no MCP reply for request {id}"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

#[test]
fn local_mcp_uses_the_mcp_origin_for_planning_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::new(
        Some(dir.path().to_path_buf()),
        Auth::default(),
        "ORIGIN_KIND_MCP",
    );
    let repo = runtime
        .invoke(
            &Options::default(),
            "CreateRepository",
            json!({"url":"https://example.com/mcp"}),
        )
        .unwrap()["result"]["version"]["entityId"]
        .clone();
    let label_id = protocol::id();
    let call = |id: u32, name: &str, arguments: Value| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}});
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test"}}});
    let messages = [
        init,
        call(
            2,
            "lane_create_planning_item",
            json!({"id":label_id,"item":{"label":{"repositoryId":repo,"name":"MCP","color":"#abcdef"}}}),
        ),
        call(
            3,
            "lane_list_planning_items",
            json!({"kind":"ENTITY_KIND_LABEL"}),
        ),
        call(
            4,
            "lane_create_planning_item",
            json!({"item":{"label":{"repositoryId":repo,"name":"Both"},"cycle":{"repositoryId":repo}}}),
        ),
        call(5, "lane_get_planning_item", json!({"id":label_id})),
    ];
    // The server handles each tools/call on its own worker, so it only orders requests whose
    // replies the client has already received. Drive the session like a real client: send one
    // request and wait for its reply before sending the next.
    let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
    let out = SharedOutput::default();
    let mut writer = out.clone();
    let mut input = std::io::BufReader::new(ChannelReader::new(receiver));
    let mut replies: Vec<Value> = vec![];
    std::thread::scope(|scope| {
        let session = scope.spawn(|| {
            runtime
                .mcp(&Options::default(), &mut input, &mut writer)
                .unwrap();
        });
        for message in &messages {
            sender
                .send(format!("{message}\n").into_bytes())
                .expect("the MCP session is running");
            replies.push(out.wait_for_reply(&message["id"]));
        }
        drop(sender);
        session.join().unwrap();
    });
    replies.sort_by_key(|r| r["id"].as_i64());
    let created = &replies[1]["result"]["structuredContent"];
    assert_eq!(
        created["result"]["version"]["origin"]["kind"],
        "ORIGIN_KIND_MCP"
    );
    assert_eq!(created["result"]["version"]["label"]["color"], "#ABCDEF");
    assert_eq!(
        replies[2]["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // The oneof schema rejects an item that sets two kinds.
    assert_eq!(replies[3]["error"]["code"], -32602);
    assert_eq!(
        replies[4]["result"]["structuredContent"]["result"]["version"]["label"]["name"],
        "MCP"
    );
}

#[test]
fn operation_logs_record_duration_and_no_planning_content() {
    let mut h = Harness::new();
    let repo = h.repository("https://example.com/logs");
    h.ok(&[
        "label",
        "create",
        "--repository-id",
        &repo,
        "--name",
        "Confidential label",
        "--color",
        "#010203",
        "--description",
        "confidential description",
    ]);
    h.ok(&[
        "task",
        "create",
        "--repository-id",
        &repo,
        "--title",
        "Confidential task",
    ]);
    let mut text = String::new();
    for entry in std::fs::read_dir(h.dir.path().join("logs")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "log") {
            text.push_str(&std::fs::read_to_string(path).unwrap());
        }
    }
    assert!(
        text.contains("CreatePlanningItem")
            && text.contains("durationMs")
            && text.contains("correlationId")
    );
    assert!(!text.contains("Confidential") && !text.contains("confidential"));
}

#[test]
fn planning_item_conflicts_list_get_and_resolve_by_kind() {
    let mut h = Harness::new();
    let repo = h.repository("https://example.com/conflicts");
    let label = h.ok(&[
        "label",
        "create",
        "--repository-id",
        &repo,
        "--name",
        "A",
        "--color",
        "#111111",
    ]);
    let version = label["result"]["version"].clone();
    let id = strv(&version, "entityId").to_owned();
    h.ok(&[
        "label",
        "edit",
        &id,
        "--expected-version",
        strv(&version, "versionId"),
        "--name",
        "B",
    ]);
    // A second edit from the same base is preserved as an alternative.
    assert_eq!(
        h.fail(
            &[
                "label",
                "edit",
                &id,
                "--expected-version",
                strv(&version, "versionId"),
                "--name",
                "C"
            ],
            1
        )["reason"],
        "ERROR_REASON_STALE_VERSION"
    );
    let listed = h.ok(&["conflict", "list", "--kind", "label"]);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        h.ok(&["conflict", "list", "--kind", "milestone"])["items"],
        json!([])
    );
    assert_eq!(
        h.ok(&["conflict", "list"])["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let got = h.ok(&["conflict", "get", &id, "--kind", "label"]);
    let alternatives = got["result"]["alternatives"].as_array().unwrap();
    assert_eq!(alternatives.len(), 2);
    let mut heads: Vec<&str> = alternatives.iter().map(|a| strv(a, "versionId")).collect();
    heads.sort_unstable();
    let resolved = json!({"formatVersion":3,"entityId":id,"kind":"ENTITY_KIND_LABEL",
        "label":{"repositoryId":repo,"name":"Final","color":"#222222"}})
    .to_string();
    let mut args = vec!["conflict", "resolve", id.as_str()];
    for head in &heads {
        args.extend(["--expected-head", head]);
    }
    args.extend(["--resolved-json", resolved.as_str()]);
    let result = h.ok(&args);
    assert_eq!(result["result"]["version"]["label"]["name"], "Final");
    assert_eq!(
        h.ok(&["conflict", "list", "--kind", "label"])["items"],
        json!([])
    );
}

#[test]
fn typed_request_json_items_are_preserved_and_overridden_by_flags() {
    let mut h = Harness::new();
    let repo = h.repository("https://example.com/request-json");
    // Create with a typed item and no flat fields.
    let request = json!({"item":{"label":{"repositoryId":repo,"name":"Typed","color":"#abcdef",
        "description":"from json"}}})
    .to_string();
    let created = h.ok(&["label", "create", "--request-json", &request]);
    let version = created["result"]["version"].clone();
    let id = strv(&version, "entityId").to_owned();
    assert_eq!(version["label"]["name"], "Typed");
    assert_eq!(version["label"]["color"], "#ABCDEF");
    // A flag overrides the supplied item while its other fields stay.
    let request = json!({"item":{"label":{"repositoryId":repo,"name":"Typed","color":"#abcdef",
        "description":"from json"}}})
    .to_string();
    let flagged = h.ok(&[
        "label",
        "create",
        "--request-json",
        &request,
        "--name",
        "Flagged",
    ]);
    assert_eq!(flagged["result"]["version"]["label"]["name"], "Flagged");
    assert_eq!(
        flagged["result"]["version"]["label"]["description"],
        "from json"
    );
    // A JSON-only edit applies the supplied item instead of the stored body.
    let request = json!({"id":id,"expectedVersionId":version["versionId"],
        "item":{"label":{"repositoryId":repo,"name":"Typed","color":"#000000",
        "description":"edited"}}})
    .to_string();
    let edited = h.ok(&["label", "edit", "--request-json", &request]);
    let body = &edited["result"]["version"]["label"];
    assert_eq!(
        (&body["color"], &body["description"]),
        (&json!("#000000"), &json!("edited"))
    );
    // Flags still override the supplied item on edit.
    let request = json!({"id":id,"expectedVersionId":edited["result"]["version"]["versionId"],
        "item":{"label":{"repositoryId":repo,"name":"Typed","color":"#111111"}}})
    .to_string();
    let mixed = h.ok(&[
        "label",
        "edit",
        "--request-json",
        &request,
        "--name",
        "Renamed",
    ]);
    let body = &mixed["result"]["version"]["label"];
    assert_eq!(
        (&body["name"], &body["color"]),
        (&json!("Renamed"), &json!("#111111"))
    );
    // Without a supplied item the stored body is the base.
    let request =
        json!({"id":id,"expectedVersionId":mixed["result"]["version"]["versionId"]}).to_string();
    let kept = h.ok(&[
        "label",
        "edit",
        "--request-json",
        &request,
        "--description",
        "kept",
    ]);
    let body = &kept["result"]["version"]["label"];
    assert_eq!(
        (&body["name"], &body["color"]),
        (&json!("Renamed"), &json!("#111111"))
    );
}
