// SPDX-License-Identifier: Apache-2.0
use crate::{App, Invocation};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
};

const TOOLS: &[&str] = &[
    "init", "doctor", "measure", "optimize", "compare", "status", "resume", "cancel", "report",
    "upload", "publish",
];
pub fn serve(app: App) -> Result<()> {
    let app = Arc::new(app);
    let output = Arc::new(Mutex::new(io::stdout()));
    let mut workers = vec![];
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.len() > 4 * 1024 * 1024 {
            continue;
        }
        let message: Value = if let Ok(v) = serde_json::from_str(&line) {
            v
        } else {
            send(
                &output,
                &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Invalid JSON"}}),
            )?;
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        match message["method"].as_str() {
            Some("initialize") => send(
                &output,
                &json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"perfine","version":env!("CARGO_PKG_VERSION")}}}),
            )?,
            Some("ping") => send(&output, &json!({"jsonrpc":"2.0","id":id,"result":{}}))?,
            Some("tools/list") => send(
                &output,
                &json!({"jsonrpc":"2.0","id":id,"result":{"tools":TOOLS.iter().map(|name|tool(name)).collect::<Vec<_>>()}}),
            )?,
            Some("tools/call") => {
                let app = Arc::clone(&app);
                let output = Arc::clone(&output);
                workers.retain(|worker: &std::thread::JoinHandle<()>| !worker.is_finished());
                workers.push(std::thread::spawn(move || {
                    let result = invoke(&app, &message);
                    let (error, value) = match result {
                        Ok(value) => (false, value),
                        Err(error) => (true, json!({"schema_version":1,"request_id":message["params"]["arguments"]["request_id"],"error":{"message":format!("{error:#}")}})),
                    };
                    let _ = send(&output, &json!({"jsonrpc":"2.0","id":id,"result":{"isError":error,"content":[{"type":"text","text":value.to_string()}]}}));
                }));
            }
            _ => send(
                &output,
                &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
            )?,
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}
fn send(output: &Mutex<io::Stdout>, value: &Value) -> Result<()> {
    let mut out = output.lock().map_err(|_| anyhow::anyhow!("stdout lock"))?;
    writeln!(out, "{value}")?;
    out.flush()?;
    drop(out);
    Ok(())
}

fn invoke(app: &App, message: &Value) -> Result<Value> {
    let name = message["params"]["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("tool name required"))?;
    anyhow::ensure!(TOOLS.contains(&name), "unknown tool");
    let mut args = message["params"]["arguments"].clone();
    let object = args
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("arguments object required"))?;
    object.insert("command".into(), json!(name));
    if name == "cancel" {
        object.entry("immediate").or_insert(json!(false));
    }
    if name == "init" {
        object.entry("github").or_insert(json!(false));
    }
    app.dispatch(serde_json::from_value::<Invocation>(args)?)
}
fn tool(name: &str) -> Value {
    let mut required = vec!["schema_version", "request_id"];
    if !matches!(name, "init" | "doctor") {
        required.push("experiment");
    }
    match name {
        "compare" => required.push("candidate"),
        "publish" => required.extend(["candidate", "draft"]),
        "upload" => required.extend(["envelope", "endpoint", "space", "connection"]),
        _ => (),
    }
    json!({"name":name,"description":format!("Perfine v1 {name}; same policy and authorization as CLI. Mutations deduplicate for 30 days."),"inputSchema":{"type":"object","required":required,"properties":{"schema_version":{"type":"integer","const":1},"request_id":{"type":"string"},"experiment":{"type":"string"},"candidate":{"type":"string"},"immediate":{"type":"boolean","default":false},"github":{"type":"boolean","default":false},"action_commit":{"type":["string","null"]},"template":{"type":["string","null"]},"overwrite_sha256":{"type":["string","null"]},"envelope":{"type":"string"},"endpoint":{"type":"string"},"space":{"type":"string"},"connection":{"type":"string"},"draft":{"type":"string"},"approve":{"type":["string","null"]},"human_judgment":{"type":["string","null"]}}}})
}
