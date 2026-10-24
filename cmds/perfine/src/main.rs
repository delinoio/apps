// SPDX-License-Identifier: Apache-2.0
mod config;
mod local;
mod mcp;
mod remote;
mod wire;
use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use perfine_core::types::{RequestId, ResourceId, Version};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    version,
    about = "Profile-backed local optimization. Request deduplication lasts 30 days."
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    repository: PathBuf,
    #[arg(long, global = true, default_value = "perfine.yaml")]
    config: PathBuf,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    no_color: bool,
    #[arg(long, global = true)]
    request_id: Option<String>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(crate) enum Command {
    Init {
        #[arg(long)]
        github: bool,
        #[arg(long)]
        action_commit: Option<String>,
        #[arg(long)]
        template: Option<PathBuf>,
        #[arg(long)]
        overwrite_sha256: Option<String>,
    },
    Doctor,
    Measure {
        #[arg(long)]
        experiment: String,
    },
    Optimize {
        #[arg(long)]
        experiment: String,
    },
    Compare {
        #[arg(long)]
        experiment: String,
        #[arg(long)]
        candidate: String,
    },
    Status {
        #[arg(long)]
        experiment: String,
    },
    Resume {
        #[arg(long)]
        experiment: String,
    },
    Cancel {
        #[arg(long)]
        experiment: String,
        #[arg(long)]
        immediate: bool,
    },
    Report {
        #[arg(long)]
        experiment: String,
    },
    Upload {
        #[arg(long)]
        experiment: String,
        #[arg(long)]
        envelope: PathBuf,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        space: String,
        #[arg(long)]
        connection: String,
    },
    Publish {
        #[arg(long)]
        experiment: String,
        #[arg(long)]
        candidate: String,
        #[arg(long)]
        draft: PathBuf,
        #[arg(long)]
        approve: Option<String>,
        #[arg(long)]
        human_judgment: Option<String>,
    },
    Mcp {
        #[command(subcommand)]
        action: Mcp,
    },
}
#[derive(Clone, Subcommand, Serialize, Deserialize)]
pub(crate) enum Mcp {
    Serve,
}
#[derive(Deserialize)]
pub(crate) struct Invocation {
    pub schema_version: Version,
    pub request_id: RequestId,
    #[serde(flatten)]
    pub command: Command,
}
pub(crate) struct App {
    pub repository: PathBuf,
    pub config: PathBuf,
}
impl App {
    pub(crate) fn dispatch(&self, invocation: Invocation) -> Result<Value> {
        let Version = invocation.schema_version;
        let request = &invocation.request_id;
        if matches!(
            invocation.command,
            Command::Init { .. }
                | Command::Measure { .. }
                | Command::Optimize { .. }
                | Command::Resume { .. }
                | Command::Cancel { .. }
                | Command::Upload { .. }
                | Command::Publish { .. }
        ) {
            self.claim_request(request, &invocation.command)?;
        }
        tracing::info!(request_id = request.as_str(), "capability invoked");
        let result = match invocation.command {
            Command::Init {
                github,
                action_commit,
                template,
                overwrite_sha256,
            } => self.init(github, action_commit, template, overwrite_sha256.as_deref()),
            Command::Doctor => self.doctor(),
            Command::Measure { experiment } => self.run(&experiment, request, false, false),
            Command::Optimize { experiment } => self.run(&experiment, request, true, false),
            Command::Resume { experiment } => self.run(&experiment, request, true, true),
            Command::Status { experiment } | Command::Report { experiment } => {
                self.report(&experiment)
            }
            Command::Compare {
                experiment,
                candidate,
            } => self.compare(&experiment, &candidate),
            Command::Cancel {
                experiment,
                immediate,
            } => self.cancel(&experiment, request, immediate),
            Command::Upload {
                experiment,
                envelope,
                endpoint,
                space,
                connection,
            } => self.upload(
                &experiment,
                request,
                &envelope,
                &endpoint,
                &space,
                &connection,
            ),
            Command::Publish {
                experiment,
                candidate,
                draft,
                approve,
                human_judgment,
            } => self.publish(
                &experiment,
                &candidate,
                request,
                &draft,
                approve.as_deref(),
                human_judgment,
            ),
            Command::Mcp { .. } => bail!("MCP serve is a transport, not a tool capability"),
        }?;
        Ok(json!({"schema_version":1,"request_id":request,"result":result}))
    }
}
fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_writer(io::stderr)
        .with_ansi(false)
        .init();
    let cli = Cli::parse();
    let request = cli
        .request_id
        .unwrap_or_else(|| ResourceId::generate().as_str().to_owned());
    let result = (|| -> Result<()> {
        let repository = cli.repository.canonicalize()?;
        let config = if cli.config.is_absolute() {
            cli.config
        } else {
            repository.join(cli.config)
        };
        let app = App { repository, config };
        if matches!(cli.command, Command::Mcp { .. }) {
            return mcp::serve(app);
        }
        let value = app.dispatch(Invocation {
            schema_version: Version,
            request_id: RequestId::new(&request)?,
            command: cli.command,
        })?;
        let mut stdout = io::stdout().lock();
        if !cli.json && !cli.no_color && std::env::var_os("NO_COLOR").is_none() {
            writeln!(stdout, "\x1b[36mPerfine\x1b[0m")?;
        }
        if !cli.json && value["result"]["experiment"].is_object() {
            let experiment = &value["result"]["experiment"];
            writeln!(stdout, "Request: {request}")?;
            writeln!(
                stdout,
                "Experiment: {}",
                experiment["id"].as_str().unwrap_or("unknown")
            )?;
            writeln!(
                stdout,
                "Execution: {}",
                experiment["state"].as_str().unwrap_or("unknown")
            )?;
            writeln!(
                stdout,
                "Base: {}",
                experiment["config"]["config"]["incremental_base"]
                    .as_str()
                    .unwrap_or("unknown")
            )?;
            if let Some(candidates) = experiment["candidates"].as_object() {
                if candidates.is_empty() {
                    writeln!(stdout, "Performance: not_evaluated (no candidate)")?;
                }
                for (id, candidate) in candidates {
                    writeln!(
                        stdout,
                        "Candidate {id}: execution={}, verdict={}",
                        candidate["state"].as_str().unwrap_or("unknown"),
                        candidate["evaluation"]["verdict"]
                            .as_str()
                            .unwrap_or("not_evaluated")
                    )?;
                    if !candidate["evaluation"].is_null() {
                        writeln!(
                            stdout,
                            "{}",
                            serde_json::to_string_pretty(&candidate["evaluation"])?
                        )?;
                    }
                }
            }
            if let Some(failure) = value["result"]["evidence"]["failure"].as_str() {
                writeln!(stdout, "Retained failure: {failure}")?;
            }
            writeln!(
                stdout,
                "Local evidence: .perfine/; request deduplication: 30 days"
            )?;
        } else {
            writeln!(stdout, "{}", serde_json::to_string_pretty(&value)?)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let output = json!({"schema_version":1,"request_id":request,"error":{"message":format!("{error:#}")}});
        let _ = writeln!(io::stdout(), "{output}");
        tracing::error!(request_id = request, "capability failed; inspect response");
        std::process::exit(1);
    }
}
