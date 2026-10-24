// SPDX-License-Identifier: Apache-2.0
use anyhow::{Result, ensure};
use perfine_core::{
    adapters::{
        agent::{LocalAgent, LocalAgentConfig},
        environment::DockerConfig,
        measurement::AdapterConfig,
    },
    evidence::Config,
    types::{CommitSha, Name, Version},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub provider: LocalAgent,
    pub docker: DockerConfig,
    pub executable: String,
    pub auth_file: PathBuf,
    pub network: String,
    pub model: Option<String>,
    pub maximum_source_bytes: usize,
    pub maximum_edits: usize,
}
impl AgentConfig {
    pub(crate) fn local(&self) -> LocalAgentConfig {
        LocalAgentConfig {
            provider: self.provider,
            docker: self.docker.clone(),
            executable: self.executable.clone(),
            auth_file: self.auth_file.clone(),
            network: self.network.clone(),
            model: self.model.clone(),
            maximum_source_bytes: self.maximum_source_bytes,
            maximum_edits: self.maximum_edits,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub schema_version: Version,
    pub experiment: Config,
    pub adapter: AdapterConfig,
    pub docker: DockerConfig,
    /// Operator-owned execution root outside the repository and evidence store.
    pub execution_root: PathBuf,
    pub toolchain_probe: Name,
    pub agent: Option<AgentConfig>,
    pub action_commit: Option<CommitSha>,
}
impl Settings {
    pub(crate) fn read(path: &Path) -> Result<Self> {
        Self::parse(&fs::read(path)?)
    }
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Self = serde_yaml::from_slice(bytes)?;
        value.experiment.validate()?;
        value.adapter.validate()?;
        value.docker.validate()?;
        ensure!(
            value.execution_root.is_absolute(),
            "execution_root must be absolute"
        );
        Ok(value)
    }
}
