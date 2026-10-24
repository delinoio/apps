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
                &json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"perfine","version":"1.0.0"}}}),
            )?,
            Some("ping") => send(&output, &json!({"jsonrpc":"2.0","id":id,"result":{}}))?,
            Some("tools/list") => send(
                &output,
                &json!({"jsonrpc":"2.0","id":id,"result":{"tools":TOOLS.iter().map(|name|json!({"name":name,"description":format!("Perfine v1 {name}; same policy and authorization as CLI. Mutations deduplicate for 30 days."),"inputSchema":{"type":"object","required":["schema_version","request_id"],"properties":{"schema_version":{"type":"integer","const":1},"request_id":{"type":"string"},"experiment":{"type":"string"},"candidate":{"type":"string"},"immediate":{"type":"boolean"},"github":{"type":"boolean"},"action_commit":{"type":["string","null"]},"template":{"type":["string","null"]},"overwrite_sha256":{"type":["string","null"]},"envelope":{"type":"string"},"endpoint":{"type":"string"},"space":{"type":"string"},"connection":{"type":"string"},"draft":{"type":"string"},"approve":{"type":["string","null"]},"human_judgment":{"type":["string","null"]}}}})).collect::<Vec<_>>()}}),
            )?,
            Some("tools/call") => {
                let app = Arc::clone(&app);
                let output = Arc::clone(&output);
                workers.push(std::thread::spawn(move||{
                    let result=(||->Result<Value>{
                        let name=message["params"]["name"].as_str().ok_or_else(||anyhow::anyhow!("tool name required"))?;
                        anyhow::ensure!(TOOLS.contains(&name),"unknown tool");
                        let mut args=message["params"]["arguments"].clone();
                        let obj=args.as_object_mut().ok_or_else(||anyhow::anyhow!("arguments object required"))?;
                        obj.insert("command".into(),json!(name));
                        if name=="cancel" {obj.entry("immediate").or_insert(json!(false));}
                        if name=="init" {obj.entry("github").or_insert(json!(false));}
                        app.dispatch(serde_json::from_value::<Invocation>(args)?)
                    })();
                    let (error,value)=match result{Ok(v)=>(false,v),Err(e)=>(true,json!({"schema_version":1,"request_id":message["params"]["arguments"]["request_id"],"error":{"message":e.to_string()}}))};
                    let _=send(&output,&json!({"jsonrpc":"2.0","id":id,"result":{"isError":error,"content":[{"type":"text","text":value.to_string()}]}}));
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
