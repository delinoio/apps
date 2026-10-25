// SPDX-License-Identifier: Apache-2.0
use super::{ArtifactStore, host, process};
use crate::{interfaces::*, lifecycle::Workspace, types::*};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command as Process,
    sync::Mutex,
    time::Duration,
};

/// Operator-owned immutable execution image. Provision compilers, profilers and
/// dependencies in advance; candidate execution has no package-download network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    pub executable: PathBuf,
    pub image: String,
    pub dependency_names: Vec<Name>,
    pub user: String,
    pub memory_bytes: u64,
    pub cpus: u32,
    pub process_limit: u32,
    pub command_timeout_seconds: u64,
}
impl DockerConfig {
    pub fn validate(&self) -> Result<()> {
        let digest = self
            .image
            .rsplit_once("@sha256:")
            .map(|(_, d)| d)
            .or_else(|| self.image.strip_prefix("sha256:"));
        if !(self.executable.is_absolute()
            || self.executable.to_str().is_some_and(|p| {
                !p.is_empty() && !p.contains(['/', '\\', ':']) && p != "." && p != ".."
            }))
            || digest.is_none_or(|d| d.len() != 64 || !d.bytes().all(|b| b.is_ascii_hexdigit()))
            || !self
                .user
                .split(':')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            || self.user.split(':').count() != 2
            || self.memory_bytes == 0
            || self.cpus == 0
            || self.process_limit == 0
            || self.command_timeout_seconds == 0
        {
            return Err(Error::Invalid(
                "Docker image digest, numeric uid:gid and resource limits",
            ));
        }
        Ok(())
    }
    fn command(&self) -> Result<Process> {
        let mut command = host::command(&self.executable)?;
        // A nonexistent, unique config path prevents Docker from loading host
        // registry credentials, contexts, proxy settings or credential helpers.
        command.env(
            "DOCKER_CONFIG",
            std::env::temp_dir().join(format!(
                "perfine-docker-{}",
                ResourceId::generate().as_str()
            )),
        );
        #[cfg(target_os = "macos")]
        if let Some(home) = std::env::var_os("HOME") {
            let socket = PathBuf::from(home).join(".docker/run/docker.sock");
            if socket.exists() {
                command.env("DOCKER_HOST", format!("unix://{}", socket.display()));
            }
        }
        #[cfg(windows)]
        command.env("DOCKER_HOST", "npipe:////./pipe/dockerDesktopLinuxEngine");
        Ok(command)
    }
    pub(crate) fn base(&self, name: &str, network: &str) -> Result<Process> {
        self.validate()?;
        let mut cmd = self.command()?;
        cmd.args([
            "run",
            "--platform=linux/amd64",
            "--rm",
            "--pull=never",
            "--init",
            "--name",
            name,
            "--network",
            network,
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--pids-limit",
            &self.process_limit.to_string(),
            "--memory",
            &self.memory_bytes.to_string(),
            "--cpus",
            &self.cpus.to_string(),
            "--user",
            &self.user,
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,size=268435456",
            "--env",
            "HOME=/tmp/home",
        ]);
        Ok(cmd)
    }
    pub(crate) fn remove(&self, name: &str) -> Result<()> {
        let mut command = self.command()?;
        command.args(["rm", "--force", name]);
        let output = process::run(&mut command, &[], None, Duration::from_secs(15), || {})?;
        if output.code != 0
            && !String::from_utf8_lossy(&output.stderr).contains("No such container")
        {
            return Err(Error::Incomplete(
                "container termination; retain name for cleanup",
            ));
        }
        Ok(())
    }
}
/// A separate Git object database avoids exposing the user's Git config, hooks,
/// remotes and shared worktree metadata through a container mount.
pub struct DockerEnvironment {
    pub config: DockerConfig,
    pub artifacts: ArtifactStore,
    repository: PathBuf,
    root: PathBuf,
    prepared: Mutex<BTreeMap<String, (Workspace, CommitSha)>>,
    active: Mutex<BTreeMap<String, Vec<String>>>,
}
impl DockerEnvironment {
    pub fn new(
        repository: impl AsRef<Path>,
        root: impl AsRef<Path>,
        config: DockerConfig,
        artifacts: ArtifactStore,
    ) -> Result<Self> {
        config.validate()?;
        fs::create_dir_all(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        let repository = repository.as_ref().canonicalize()?;
        if root.starts_with(&repository)
            || repository.starts_with(&root)
            || root.starts_with(artifacts.root())
            || artifacts.root().starts_with(&root)
        {
            return Err(Error::Invalid(
                "separate repository, execution and evidence roots",
            ));
        }
        host::private_directory(&root)?;
        Ok(Self {
            config,
            artifacts,
            repository,
            root,
            prepared: Mutex::new(BTreeMap::new()),
            active: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn validate_request(&self, request: &EnvironmentRequest) -> Result<()> {
        if request.role != ExecutionRole::Candidate
            || !request.network_access.is_empty()
            || request.dependency_access != self.config.dependency_names
        {
            return Err(Error::Invalid(
                "candidate role, offline network and declared image dependencies",
            ));
        }
        // No arbitrary environment keys: loader, startup, auth and proxy variables
        // are not accepted, even if supplied by a repository config.
        for (key, value) in &request.environment {
            if !matches!(
                key.as_str(),
                "LANG" | "LC_ALL" | "TZ" | "RUST_BACKTRACE" | "GOMAXPROCS"
            ) || value.contains('\0')
            {
                return Err(Error::Invalid("candidate environment allowlist"));
            }
        }
        for p in [
            &request.workspace.directory,
            &request.workspace.build_directory,
        ] {
            let path = Path::new(p.as_str());
            if path.parent().and_then(|p| p.canonicalize().ok()).as_deref()
                != Some(self.root.as_path())
                || path.file_name().is_none()
                || path.file_name().is_some_and(|n| n == "." || n == "..")
            {
                return Err(Error::Invalid("task-owned direct child path"));
            }
        }
        if request.workspace.directory == request.workspace.build_directory {
            return Err(Error::Invalid("distinct build directory"));
        }
        Ok(())
    }
    /// Agent writes and candidate execution must reference a checkout prepared by
    /// this instance, never an arbitrary clean user repository.
    pub fn validate_workspace(&self, workspace: &Workspace) -> Result<()> {
        let prepared = self.prepared.lock().map_err(|_| Error::State)?;
        let (owned, _) = prepared
            .get(workspace.directory.as_str())
            .ok_or(Error::Invalid("unprepared candidate workspace"))?;
        if serde_json::to_vec(owned)? != serde_json::to_vec(workspace)? {
            return Err(Error::Mismatch("task-owned workspace"));
        }
        Ok(())
    }
    /// Reattach a durable task-owned checkout after process interruption. Never adopt user work.
    pub fn reattach(&self, request: &EnvironmentRequest) -> Result<()> {
        self.validate_request(request)?;
        let marker = PathBuf::from(format!(
            "{}.ownership.json",
            request.workspace.directory.as_str()
        ));
        if fs::symlink_metadata(&marker)?.file_type().is_symlink() {
            return Err(Error::Invalid("ownership marker symlink"));
        }
        let expected = serde_json::to_vec(&(request, &self.config))?;
        if fs::read(marker)? != expected {
            return Err(Error::Mismatch("retained workspace ownership"));
        }
        for path in [
            &request.workspace.directory,
            &request.workspace.build_directory,
        ] {
            let p = Path::new(path.as_str());
            if p.canonicalize()? != p || !p.is_dir() {
                return Err(Error::Invalid("retained workspace path"));
            }
        }
        let directory = Path::new(request.workspace.directory.as_str());
        if String::from_utf8_lossy(&git(directory, &["branch", "--show-current"])?).trim()
            != request.workspace.branch.as_str()
            || !git(directory, &["remote"])?.is_empty()
            || directory.join(".git/objects/info/alternates").exists()
        {
            return Err(Error::Mismatch("retained workspace branch or remotes"));
        }
        self.require_clean(&request.workspace)?;
        git(
            directory,
            &["merge-base", "--is-ancestor", request.base.as_str(), "HEAD"],
        )?;
        // A killed CLI can leave a container alive. Refuse overlapping execution;
        // the operator can wait for it to exit before resuming the same attempt.
        let docker = |args: &[&str]| -> Result<Vec<u8>> {
            let mut command = self.config.command()?;
            command.args(args);
            let out = process::run(&mut command, &[], None, Duration::from_secs(15), || {})?;
            if out.code != 0 {
                return Err(Error::Incomplete(
                    "container reconciliation unavailable; start Docker Desktop in Linux mode or the local Docker daemon before resume",
                ));
            }
            Ok(out.stdout)
        };
        let ids = String::from_utf8(docker(&["ps", "--quiet"])?)
            .map_err(|_| Error::Invalid("container IDs"))?;
        for id in ids.split_whitespace() {
            let containers: serde_json::Value = serde_json::from_slice(&docker(&["inspect", id])?)?;
            for container in containers
                .as_array()
                .ok_or(Error::Invalid("container inspection"))?
            {
                if container["Config"]["Labels"]["perfine.workspace"].as_str()
                    == Some(fingerprint(&request.workspace)?.as_str())
                {
                    return Err(Error::Incomplete(
                        "retained workspace still has an active container; wait before resume",
                    ));
                }
                for mount in container["Mounts"]
                    .as_array()
                    .ok_or(Error::Invalid("container mounts"))?
                {
                    if mount["Source"].as_str().is_some_and(|p| {
                        [
                            request.workspace.directory.as_str(),
                            request.workspace.build_directory.as_str(),
                        ]
                        .iter()
                        .any(|expected| mount_matches(p, expected))
                    }) {
                        return Err(Error::Incomplete(
                            "retained workspace still has an active container; wait before resume",
                        ));
                    }
                }
            }
        }
        self.prepared.lock().map_err(|_| Error::State)?.insert(
            request.workspace.directory.as_str().into(),
            (request.workspace.clone(), request.base.clone()),
        );
        Ok(())
    }
    pub fn source(&self, workspace: &Workspace) -> Result<CommitSha> {
        let out = git(
            Path::new(workspace.directory.as_str()),
            &["rev-parse", "HEAD"],
        )?;
        CommitSha::new(String::from_utf8_lossy(&out).trim())
    }
    pub fn require_clean(&self, workspace: &Workspace) -> Result<()> {
        if !git(
            Path::new(workspace.directory.as_str()),
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty()
        {
            return Err(Error::Invalid("dirty candidate tree"));
        }
        Ok(())
    }
    /// Exposed for diagnostics/tests; only execute() is an execution boundary.
    pub fn candidate_arguments(
        &self,
        command: &Command,
        request: &EnvironmentRequest,
        name: &str,
    ) -> Result<Vec<String>> {
        self.validate_request(request)?;
        let mut cmd = self.config.base(name, "none")?;
        cmd.args([
            "--label",
            &format!(
                "perfine.workspace={}",
                fingerprint(&request.workspace)?.as_str()
            ),
        ]);
        for (path, target, readonly) in [
            (&request.workspace.directory, "/work", true),
            (&request.workspace.build_directory, "/build", false),
        ] {
            let source = host::mount_path(Path::new(path.as_str()))?;
            let suffix = if readonly { ",readonly" } else { "" };
            cmd.args([
                "--mount",
                &format!("type=bind,src={source},dst={target}{suffix}"),
            ]);
        }
        cmd.args([
            "--workdir",
            "/work",
            "--env",
            "CARGO_TARGET_DIR=/build/target",
            "--env",
            "GOCACHE=/build/go-cache",
            "--env",
            "GOTMPDIR=/tmp",
            "--env",
            "GOTOOLCHAIN=local",
            "--env",
            "GOPROXY=off",
            "--env",
            "GOSUMDB=off",
        ]);
        for (k, v) in &request.environment {
            cmd.args(["--env", &format!("{}={v}", k.as_str())]);
        }
        // Explicit entrypoint prevents an image entrypoint from interpreting argv.
        cmd.args([
            "--entrypoint",
            command.executable.as_str(),
            &self.config.image,
        ]);
        cmd.args(&command.arguments);
        Ok(cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect())
    }
}
impl EnvironmentAdapter for DockerEnvironment {
    fn prepare(&self, request: &EnvironmentRequest) -> Result<()> {
        self.validate_request(request)?;
        let directory = Path::new(request.workspace.directory.as_str());
        let build = Path::new(request.workspace.build_directory.as_str());
        if directory.exists() || build.exists() {
            return Err(Error::Invalid("workspace already exists"));
        }
        git(
            &self.repository,
            &[
                "cat-file",
                "-e",
                &format!("{}^{{commit}}", request.base.as_str()),
            ],
        )?;
        git(
            &self.repository,
            &[
                "check-ref-format",
                "--branch",
                request.workspace.branch.as_str(),
            ],
        )?;
        fs::create_dir(directory)?;
        fs::create_dir(build)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // This task-owned, credential-free mount must work for any configured
            // numeric container UID without host root or a privileged chown container.
            fs::set_permissions(build, fs::Permissions::from_mode(0o1777))?;
        }
        // clone --no-local avoids alternates/hardlinks and copies committed objects only.
        git(
            &self.repository,
            &[
                "clone",
                "--no-local",
                "--no-checkout",
                "--",
                self.repository
                    .to_str()
                    .ok_or(Error::Invalid("repository path"))?,
                request.workspace.directory.as_str(),
            ],
        )?;
        git(directory, &["remote", "remove", "origin"])?;
        git(
            directory,
            &[
                "checkout",
                "--no-recurse-submodules",
                "-b",
                request.workspace.branch.as_str(),
                request.base.as_str(),
            ],
        )?;
        self.require_clean(&request.workspace)?;
        if self.source(&request.workspace)? != request.base {
            return Err(Error::Mismatch("prepared source"));
        }
        // The host-owned marker is outside all source/build mounts and contains no credentials.
        let marker = PathBuf::from(format!(
            "{}.ownership.json",
            request.workspace.directory.as_str()
        ));
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(marker)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec(&(request, &self.config))?)?;
        file.sync_all()?;
        self.prepared.lock().map_err(|_| Error::State)?.insert(
            request.workspace.directory.as_str().into(),
            (request.workspace.clone(), request.base.clone()),
        );
        tracing::info!(
            base = request.base.as_str(),
            "prepared isolated candidate checkout"
        );
        Ok(())
    }
    fn execute(
        &self,
        command: &Command,
        request: &EnvironmentRequest,
        context: &ExecutionContext<'_>,
    ) -> Result<CommandOutput> {
        self.validate_request(request)?;
        let key = request.workspace.directory.as_str();
        let prepared = self.prepared.lock().map_err(|_| Error::State)?;
        let (workspace, base) = prepared.get(key).ok_or(Error::State)?;
        if serde_json::to_vec(workspace)? != serde_json::to_vec(&request.workspace)?
            || serde_json::to_vec(workspace)? != serde_json::to_vec(context.workspace)?
            || base != &request.base
        {
            return Err(Error::Mismatch("prepared workspace"));
        }
        drop(prepared);
        if process::now() >= context.deadline_unix_seconds
            || context.cancellation.state() == Cancellation::Immediate
        {
            return Err(Error::Limit);
        }
        let name = format!("perfine-{}", ResourceId::generate().as_str());
        let args = self.candidate_arguments(command, request, &name)?;
        self.active
            .lock()
            .map_err(|_| Error::State)?
            .entry(key.into())
            .or_default()
            .push(name.clone());
        let mut cmd = self.config.command()?;
        cmd.args(args);
        let out = process::run(
            &mut cmd,
            &[],
            Some(context),
            Duration::from_secs(self.config.command_timeout_seconds),
            || {
                let _ = self.config.remove(&name);
            },
        );
        // Retry cleanup even when the Docker client failed before reporting status.
        if out.is_err() || out.as_ref().is_ok_and(|o| o.interrupted) {
            self.config.remove(&name)?;
        }
        if let Some(names) = self.active.lock().map_err(|_| Error::State)?.get_mut(key) {
            names.retain(|n| n != &name);
        }
        let out = out?;
        let output = CommandOutput {
            exit_code: out.code,
            stdout_artifact: self.artifacts.put(&out.stdout)?,
            stderr_artifact: self.artifacts.put(&out.stderr)?,
        };
        if out.interrupted {
            tracing::info!(
                stdout = output.stdout_artifact.as_str(),
                stderr = output.stderr_artifact.as_str(),
                "retained interrupted command evidence"
            );
            return Err(Error::Limit);
        }
        Ok(output)
    }
    fn cancel(&self, workspace: &Workspace) -> Result<()> {
        let names = self
            .active
            .lock()
            .map_err(|_| Error::State)?
            .get(workspace.directory.as_str())
            .cloned()
            .unwrap_or_default();
        for name in names {
            self.config.remove(&name)?;
        }
        Ok(())
    }
}
/// Git is always run without user/system config, hooks, credential helpers or a
/// pager. Repository Git operations never execute in the credentialed agent role.
pub fn git(directory: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = host::command(Path::new("git"))?;
    // Empty task-owned paths also work on Windows, unlike /dev/null.
    let empty = tempfile::tempdir()?;
    let config = empty.path().join("config");
    fs::write(&config, b"")?;
    command
        .env("HOME", empty.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            &format!("core.hooksPath={}", empty.path().display()),
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.longpaths=true",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "protocol.file.allow=always",
            "-c",
            "user.name=Perfine",
            "-c",
            "user.email=perfine@localhost",
            "-c",
            "commit.gpgsign=false",
            "-C",
        ])
        .arg(directory)
        .args(args);
    let output = process::run(&mut command, &[], None, Duration::from_secs(120), || {})?;
    if output.interrupted {
        return Err(Error::Limit);
    }
    if output.code != 0 {
        return Err(Error::Incomplete("Git command failed"));
    }
    Ok(output.stdout)
}

// Desktop reports daemon paths rather than host drive/verbatim paths. Compare
// known mappings for legacy containers; new runs also carry a workspace label.
fn mount_matches(actual: &str, expected: &str) -> bool {
    let Ok(expected) = host::mount_path(Path::new(expected)) else {
        return false;
    };
    let actual = actual
        .strip_prefix("/host_mnt")
        .or_else(|| actual.strip_prefix("/run/desktop/mnt/host"))
        .unwrap_or(actual);
    if expected.as_bytes().get(1) == Some(&b':') {
        let drive = format!("/{}{}", &expected[..1], &expected[2..]);
        actual.eq_ignore_ascii_case(&expected) || actual.eq_ignore_ascii_case(&drive)
    } else {
        actual == expected
    }
}

#[cfg(test)]
mod portability_tests {
    use super::*;
    fn config() -> DockerConfig {
        DockerConfig {
            executable: std::env::current_exe().unwrap(),
            image: format!("sha256:{}", "a".repeat(64)),
            dependency_names: vec![],
            user: "1000:1000".into(),
            memory_bytes: 1024,
            cpus: 1,
            process_limit: 16,
            command_timeout_seconds: 5,
        }
    }
    #[test]
    fn docker_platform_and_environment_are_explicit() {
        let command = config().base("fixture", "none").unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert!(args.contains(&std::ffi::OsStr::new("--platform=linux/amd64")));
        assert!(args.contains(&std::ffi::OsStr::new("--pull=never")));
        for (key, _) in command.get_envs() {
            assert!(matches!(
                key.to_str().unwrap(),
                "PATH" | "DOCKER_CONFIG" | "DOCKER_HOST" | "SystemRoot" | "WINDIR" | "TEMP" | "TMP"
            ));
        }
    }
    #[test]
    fn desktop_mount_reconciliation_handles_host_mapping() {
        assert!(mount_matches(
            "/host_mnt/Users/한글/jobs",
            "/Users/한글/jobs"
        ));
        assert!(mount_matches(
            "/run/desktop/mnt/host/c/User Files/jobs",
            r"\\?\C:\User Files\jobs"
        ));
        assert!(!mount_matches(
            "/host_mnt/Users/other/jobs",
            "/Users/한글/jobs"
        ));
    }
    #[test]
    fn git_preserves_unicode_paths_and_ignores_repository_hooks() {
        let temp = tempfile::tempdir().unwrap();
        let repository = temp.path().join("user files 한글");
        fs::create_dir(&repository).unwrap();
        git(&repository, &["init", "-q"]).unwrap();
        fs::write(repository.join("source.txt"), b"line\n").unwrap();
        git(&repository, &["add", "."]).unwrap();
        git(&repository, &["commit", "-qm", "fixture"]).unwrap();
        assert_eq!(
            git(&repository, &["show", "HEAD:source.txt"]).unwrap(),
            b"line\n"
        );
        assert!(
            git(&repository, &["status", "--porcelain"])
                .unwrap()
                .is_empty()
        );
    }
}
