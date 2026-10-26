// SPDX-License-Identifier: Apache-2.0
use crate::{
    error::{Failure, Result, invalid},
    planning_args::{self, TaskPlanning},
    protocol::{self, strv},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write};
#[expect(
    clippy::struct_excessive_bools,
    reason = "These are independent public command-line switches."
)]
#[derive(Default, Clone)]
pub struct Options {
    pub remote: bool,
    pub json: bool,
    pub once: bool,
    pub confirm_epoch_change: bool,
    pub retry_blocked: bool,
    pub confirm_development_reset: bool,
    pub space: String,
    pub expected_space: String,
    pub reauth_challenge: String,
    pub endpoint: String,
    pub redirect_uri: String,
    pub color: String,
}
pub trait Dispatcher {
    fn invoke(&mut self, opt: &Options, method: &str, q: Value) -> Result<Value>;
    fn mcp(
        &mut self,
        opt: &Options,
        input: &mut dyn std::io::BufRead,
        out: &mut (dyn Write + Send),
    ) -> Result<()>;
}
fn pop(flags: &mut BTreeMap<String, Vec<String>>, key: &str) -> String {
    flags
        .remove(key)
        .and_then(|v| v.last().cloned())
        .unwrap_or_default()
}
pub fn run(
    args: &[String],
    dispatcher: &mut impl Dispatcher,
    input: &mut dyn std::io::BufRead,
    out: &mut (dyn Write + Send),
    err: &mut (dyn Write + Send),
) -> i32 {
    let mut opt = Options {
        json: args.iter().any(|s| s == "--json"),
        ..Options::default()
    };
    let mut code = 2;
    match execute(args, dispatcher, input, out, &mut opt, &mut code) {
        Ok(()) => 0,
        Err(e) => {
            if opt.json {
                let _ = writeln!(out, "{}", json!({"ok":false,"error":e}));
            } else {
                let _ = writeln!(err, "{}", e.message);
            }
            code
        }
    }
}
fn execute(
    args: &[String],
    dispatcher: &mut impl Dispatcher,
    input: &mut dyn std::io::BufRead,
    out: &mut (dyn Write + Send),
    opt: &mut Options,
    code: &mut i32,
) -> Result<()> {
    let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut words = vec![];
    let mut i = 0;
    while i < args.len() {
        let x = if args[i] == "-h" { "--help" } else { &args[i] };
        if let Some(flag) = x.strip_prefix("--") {
            let (k, v) = if let Some((k, v)) = flag.split_once('=') {
                (k, v.to_owned())
            } else if [
                "json",
                "remote",
                "once",
                "help",
                "version",
                "clear-prompt",
                "confirm-epoch-change",
                "retry-blocked",
            ]
            .contains(&flag)
                || planning_args::SWITCHES.contains(&flag)
            {
                (flag, "true".into())
            } else {
                i += 1;
                (
                    flag,
                    args.get(i)
                        .ok_or_else(|| invalid(&format!("--{flag} requires a value")))?
                        .clone(),
                )
            };
            flags.entry(k.into()).or_default().push(v);
        } else {
            words.push(x.to_owned());
        }
        i += 1;
    }
    opt.json = pop(&mut flags, "json") == "true";
    opt.remote = pop(&mut flags, "remote") == "true";
    opt.once = pop(&mut flags, "once") == "true";
    opt.confirm_epoch_change = pop(&mut flags, "confirm-epoch-change") == "true";
    opt.retry_blocked = pop(&mut flags, "retry-blocked") == "true";
    opt.space = pop(&mut flags, "space");
    opt.expected_space = pop(&mut flags, "expected-space");
    opt.reauth_challenge = pop(&mut flags, "reauth-challenge");
    opt.endpoint = pop(&mut flags, "endpoint");
    opt.redirect_uri = pop(&mut flags, "redirect-uri");
    opt.confirm_development_reset = pop(&mut flags, "confirm-development-reset") == "true";
    // `--color never|auto` selects terminal color; any other value is a label color
    // (`label create/edit --color #RRGGBB`), which the planning parser reads below.
    for value in flags.remove("color").unwrap_or_default() {
        if matches!(value.as_str(), "auto" | "never") {
            opt.color = value;
        } else {
            flags.entry("label-color".into()).or_default().push(value);
        }
    }
    if pop(&mut flags, "version") == "true" {
        writeln!(out, "lane {}", env!("CARGO_PKG_VERSION"))?;
        return Ok(());
    }
    let commands: Value = serde_json::from_str(include_str!("commands.json"))?;
    if pop(&mut flags, "help") == "true" || words.is_empty() {
        writeln!(
            out,
            "Lane — local task and prompt management\nUsage: lane [--remote] [--json] [--space UUID] COMMAND\nLocal commands need no login or running app. --remote explicitly selects server data.\nMutations accept --request-id UUID; updates require --expected-version UUID.\nAll typed commands accept --request-json JSON or - for stdin.\n--expected-space UUID with --remote refuses a changed active account.\n--color=never or NO_COLOR disables terminal color."
        )?;
        for (k, v) in commands.as_object().unwrap() {
            writeln!(out, "  {k:24} {}", strv(v, "help"))?;
        }
        return Ok(());
    }
    if opt.remote && !opt.space.is_empty() {
        return Err(invalid("--space cannot be used with --remote"));
    }
    if !opt.remote && !opt.expected_space.is_empty() {
        return Err(invalid("--expected-space requires --remote"));
    }
    if !matches!(opt.color.as_str(), "" | "auto" | "never") {
        return Err(invalid("--color must be auto or never"));
    }
    let mut key = words.remove(0);
    if commands.get(&key).is_none() && !words.is_empty() {
        key.push(' ');
        key.push_str(&words.remove(0));
    }
    let command = commands
        .get(&key)
        .ok_or_else(|| invalid("unknown command; use --help"))?;
    let mut method = strv(command, "method").to_owned();
    let group = key.split(' ').next().unwrap_or("").to_owned();
    if flags.contains_key("label-color") && !matches!(key.as_str(), "label create" | "label edit") {
        return Err(invalid("--color must be auto or never"));
    }
    if opt.confirm_development_reset && method != "ResetDevelopmentData" {
        return Err(invalid("--confirm-development-reset requires data reset"));
    }
    if opt.confirm_epoch_change && method != "ConnectSync" {
        return Err(invalid(
            "--confirm-epoch-change requires sync connect or login",
        ));
    }
    if opt.retry_blocked && method != "SyncRun" {
        return Err(invalid("--retry-blocked requires sync run"));
    }
    if method == "MCP" {
        if !words.is_empty() || !flags.is_empty() {
            return Err(invalid("unexpected MCP argument"));
        }
        *code = 1;
        return dispatcher.mcp(opt, input, out);
    }
    let mut raw = pop(&mut flags, "request-json");
    let had_payload = !raw.is_empty();
    if raw == "-" {
        raw.clear();
        input.read_to_string(&mut raw)?;
    }
    let mut q = if raw.is_empty() {
        json!({})
    } else {
        serde_json::from_str::<Value>(&raw)?
    };
    if !q.is_object() {
        return Err(invalid("--request-json requires a JSON object"));
    }
    if method == "RPC" {
        if words.len() != 1 {
            return Err(invalid("rpc requires an exact typed method name"));
        }
        method = words.remove(0);
        if protocol::method(&method).is_none_or(|m| m.parent_service().name() == "SyncService") {
            return Err(invalid("unknown typed method"));
        }
    }
    if !opt.reauth_challenge.is_empty() && (!opt.remote || method != "DeleteAccountData") {
        return Err(invalid(
            "--reauth-challenge requires remote DeleteAccountData",
        ));
    }
    if words.len() > 1 || (method == "ResetDevelopmentData" && !words.is_empty()) {
        return Err(invalid("too many positional arguments"));
    }
    if let Some(word) = words.pop() {
        q[if method == "SyncCallback" {
            "callbackUri"
        } else {
            "id"
        }] = word.into();
    }
    let mapping: Value = serde_json::from_str(include_str!("flags.json"))?;
    for (k, f) in mapping.as_object().unwrap() {
        if let Some(v) = flags.remove(k) {
            q[f.as_str().unwrap()] = v.last().unwrap().clone().into();
        }
    }
    let mut task_planning = None;
    match key.as_str() {
        "task create" | "task edit" => task_planning = Some(TaskPlanning::take(&mut flags)?),
        "task list" => planning_args::take_list(&mut flags, &mut q)?,
        _ => {}
    }
    if let Some(planning) = &task_planning
        && method == "CreateTask"
    {
        planning.apply_to_request(&mut q);
        if !planning.edits().is_null() {
            q["relationshipEdits"] = planning.edits().clone();
        }
    }
    if let Some(kind) = planning_args::group_kind(&group) {
        match method.as_str() {
            "ListPlanningItems" => q["kind"] = kind.into(),
            "CreatePlanningItem" => {
                let body = planning_args::item_body(&group, &mut flags, &mut q, &json!({}))?;
                if !q["item"].is_object() {
                    q["item"] = json!({});
                }
                q["item"][group.as_str()] = body;
            }
            "SavePlanningItem" | "GetPlanningItem" | "DeletePlanningItem" | "ListHistory"
                if !strv(&q, "id").is_empty() =>
            {
                if method == "SavePlanningItem" && q.get("expectedVersionId").is_none() {
                    return Err(invalid("--expected-version is required"));
                }
                *code = 1;
                let read = dispatcher.invoke(opt, "GetPlanningItem", json!({"id":q["id"]}))?;
                // A conflicted item has no single version; every alternative shares its kind.
                let read_kind = if read["result"]["version"].is_object() {
                    &read["result"]["version"]["kind"]
                } else {
                    &read["result"]["alternatives"][0]["kind"]
                };
                if read_kind != kind {
                    return Err(Failure::new("NOT_FOUND", "Planning item does not exist."));
                }
                *code = 2;
                if method == "SavePlanningItem" && !read["result"]["version"].is_object() {
                    // Only resolution or permanent deletion can change a conflicted item.
                    return Err(Failure::new(
                        "CONFLICT_READ_ONLY",
                        "Resolve the conflict before editing.",
                    ));
                }
                if method == "SavePlanningItem" {
                    let base = &read["result"]["version"][group.as_str()];
                    if !base.is_object() {
                        return Err(Failure::new(
                            "NOT_FOUND",
                            "Planning item is unavailable or deleted.",
                        ));
                    }
                    let body = planning_args::item_body(&group, &mut flags, &mut q, base)?;
                    if !q["item"].is_object() {
                        q["item"] = json!({});
                    }
                    q["item"][group.as_str()] = body;
                }
            }
            _ => {}
        }
    }
    for (k, prefix) in [
        ("progress", "PROGRESS_"),
        ("proposal", "PROPOSAL_"),
        ("states", "EVENT_STATE_"),
    ] {
        if let Some(values) = flags.remove(k) {
            let values: Vec<_> = values
                .iter()
                .flat_map(|v| v.split(','))
                .map(|v| {
                    let v = v.to_uppercase().replace('-', "_");
                    if v.starts_with(prefix) {
                        v
                    } else {
                        format!("{prefix}{v}")
                    }
                })
                .collect();
            q[k] = if matches!(method.as_str(), "ListTasks" | "ListEvents") {
                json!(values)
            } else if values.len() == 1 {
                json!(values[0])
            } else {
                return Err(invalid("mutation requires one state"));
            };
        }
    }
    if let Some(heads) = flags.remove("expected-head") {
        q["expectedHeadIds"] = json!(heads);
    }
    for (k, f) in [
        ("tasks-json", "tasks"),
        ("resolved-json", "resolved"),
        ("images-json", "images"),
    ] {
        let raw = pop(&mut flags, k);
        if !raw.is_empty() {
            q[f] = serde_json::from_str(&raw)?;
        }
    }
    let mut page = json!({});
    let size = pop(&mut flags, "page-size");
    if !size.is_empty() {
        page["size"] = size
            .parse::<u32>()
            .map_err(|_| invalid("invalid page size"))?
            .into();
    }
    let cursor = pop(&mut flags, "cursor");
    if !cursor.is_empty() {
        page["cursor"] = cursor.into();
    }
    if !page.as_object().unwrap().is_empty() {
        q["page"] = page;
    }
    let request = pop(&mut flags, "request-id");
    if !request.is_empty() {
        q["meta"] = json!({"requestId":request});
    }
    let enabled = pop(&mut flags, "enabled");
    if !enabled.is_empty() {
        q["enabled"] = enabled
            .parse::<bool>()
            .map_err(|_| invalid("--enabled requires true or false"))?
            .into();
    }
    let content = pop(&mut flags, "content");
    if !content.is_empty() {
        q["content"] = format!("NOTIFICATION_CONTENT_{}", content.to_uppercase()).into();
    }
    let retention = pop(&mut flags, "retention");
    if !retention.is_empty() {
        if !matches!(retention.as_str(), "keep-local" | "remove-local") {
            return Err(invalid("--retention requires keep-local or remove-local"));
        }
        q["retention"] = format!(
            "LOCAL_RETENTION_{}",
            retention.to_uppercase().replace('-', "_")
        )
        .into();
    }
    let stdin = pop(&mut flags, "prompt-stdin") == "true";
    let clear = pop(&mut flags, "clear-prompt") == "true";
    if stdin {
        if q.get("prompt").is_some() || clear {
            return Err(invalid("choose one prompt source"));
        }
        let mut prompt = String::new();
        input.read_to_string(&mut prompt)?;
        q["prompt"] = prompt.into();
    }
    if clear {
        q["prompt"] = Value::Null;
    }
    if let Some(k) = flags.keys().next() {
        return Err(invalid(&format!("unknown flag --{k}")));
    }
    match key.as_str() {
        "task accept" => q["proposal"] = "PROPOSAL_ACCEPTED".into(),
        "task reject" => q["proposal"] = "PROPOSAL_REJECTED".into(),
        "task prompt-history" => q["promptsOnly"] = true.into(),
        _ => {}
    }
    if method == "ResetDevelopmentData" {
        let reason = if opt.remote {
            Some("data reset is local-only; remove --remote")
        } else if !protocol::valid_id(&opt.space) {
            Some("data reset requires an explicit --space UUID")
        } else if !opt.confirm_development_reset {
            Some("data reset requires --confirm-development-reset")
        } else if had_payload || !q.as_object().unwrap().is_empty() {
            Some("data reset accepts no request payload")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(invalid(reason));
        }
    }
    if matches!(method.as_str(), "CreateURLConnection" | "SaveURLConnection")
        && q.get("connection").is_none()
    {
        let mut connection = json!({});
        for field in ["prefix", "repositoryId"] {
            if let Some(value) = q.as_object_mut().unwrap().remove(field) {
                connection[field] = value;
            }
        }
        q["connection"] = connection;
    }
    if method == "CreateTask" {
        let mut target = json!({});
        for (k, f) in [("repositoryId", "id"), ("repositoryUrl", "url")] {
            if let Some(v) = q.as_object_mut().unwrap().remove(k) {
                target[f] = v;
            }
        }
        if !target.as_object().unwrap().is_empty() {
            q["repository"] = target;
        }
    }
    if method == "SaveTask"
        && q.get("task").is_some_and(Value::is_object)
        && let Some(planning) = &task_planning
    {
        planning.apply_to_task(&mut q["task"]);
        if !planning.edits().is_null() {
            q["relationshipEdits"] = planning.edits().clone();
        }
    }
    if method == "SaveTask" && q.get("task").is_none() {
        if q.get("expectedVersionId").is_none() {
            return Err(invalid("--expected-version is required"));
        }
        *code = 1;
        let read = dispatcher.invoke(opt, "GetTask", json!({"id":q["id"]}))?;
        let mut task = read["result"]["version"]["task"].clone();
        if !task.is_object() {
            return Err(invalid("task unavailable"));
        }
        for f in [
            "title",
            "description",
            "prompt",
            "repositoryId",
            "progress",
            "proposal",
        ] {
            if let Some(v) = q.as_object_mut().unwrap().remove(f) {
                if v.is_null() {
                    task.as_object_mut().unwrap().remove(f);
                } else {
                    task[f] = v;
                }
            }
        }
        if let Some(planning) = &task_planning {
            planning.apply_to_task(&mut task);
            if !planning.edits().is_null() {
                q["relationshipEdits"] = planning.edits().clone();
            }
        }
        q["task"] = task;
        *code = 2;
    }
    let required: Value = serde_json::from_str(include_str!("required.json"))?;
    if let Some(fields) = required[&method].as_array() {
        for f in fields {
            let f = f.as_str().unwrap();
            if q[f].is_null() || q[f] == "" {
                return Err(invalid(&format!("{key} requires {f}; use --help")));
            }
        }
    }
    if protocol::method(&method).is_some() {
        q = protocol::request(&method, &q)?;
    }
    *code = 1;
    let response = dispatcher.invoke(opt, &method, q)?;
    if opt.json {
        writeln!(out, "{}", json!({"ok":true,"result":response}))?;
    } else {
        use std::io::IsTerminal;
        let color = opt.color != "never"
            && std::env::var_os("NO_COLOR").is_none()
            && std::io::stdout().is_terminal();
        if color {
            write!(out, "\x1b[36m")?;
        }
        let result = human(out, &response, "");
        if color {
            write!(out, "\x1b[0m")?;
        }
        result?;
    }
    Ok(())
}
fn human(out: &mut (dyn Write + Send), value: &Value, indent: &str) -> Result<()> {
    match value {
        Value::Object(m) => {
            for (k, v) in m {
                if v.is_object() || v.is_array() {
                    writeln!(out, "{indent}{k}:")?;
                    human(out, v, &format!("{indent}  "))?;
                } else {
                    writeln!(
                        out,
                        "{indent}{k}: {}",
                        v.as_str().map_or_else(|| v.to_string(), str::to_owned)
                    )?;
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                writeln!(out, "{indent}-")?;
                human(out, v, &format!("{indent}  "))?;
            }
        }
        _ => writeln!(out, "{indent}{value}")?,
    }
    Ok(())
}
