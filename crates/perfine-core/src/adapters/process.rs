// SPDX-License-Identifier: Apache-2.0
use crate::{
    interfaces::{Cancellation, ExecutionContext},
    types::*,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const OUTPUT_LIMIT: u64 = 64 * 1024 * 1024;

pub(crate) struct Output {
    pub code: i32,
    pub interrupted: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
/// Only trusted host utilities and the Docker client use this runner. Candidate
/// commands never execute on the host. Files avoid pipe backpressure deadlocks.
pub(crate) fn run(
    command: &mut Command,
    input: &[u8],
    context: Option<&ExecutionContext<'_>>,
    timeout: Duration,
    cleanup: impl FnOnce(),
) -> Result<Output> {
    let mut stdin = tempfile::tempfile()?;
    stdin.write_all(input)?;
    stdin.seek(SeekFrom::Start(0))?;
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command
        .stdin(Stdio::from(stdin))
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    }
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::Incomplete(
                "host tool not found; install Git/Docker, check PATH and the configured executable",
            )
        } else {
            Error::Io(error)
        }
    })?;
    #[cfg(windows)]
    let job = match super::host::windows::Job::attach(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup();
            return Err(error);
        }
    };
    let start = std::time::Instant::now();
    let mut interrupted = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let output_limit =
            stdout.metadata()?.len() > OUTPUT_LIMIT || stderr.metadata()?.len() > OUTPUT_LIMIT;
        if output_limit
            || start.elapsed() >= timeout
            || context.is_some_and(|c| {
                now() >= c.deadline_unix_seconds
                    || c.cancellation.state() == Cancellation::Immediate
            })
        {
            if output_limit {
                tracing::warn!(limit_bytes = OUTPUT_LIMIT, "command output limit exceeded");
            }
            cleanup();
            #[cfg(unix)]
            {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            #[cfg(windows)]
            job.terminate();
            let _ = child.kill();
            interrupted = true;
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(20));
    };
    fn read(file: &mut File) -> Result<Vec<u8>> {
        // Bound host memory even when an untrusted command floods stdout.
        if file.metadata()?.len() > OUTPUT_LIMIT {
            return Err(Error::Limit);
        }
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    Ok(Output {
        code: status.code().unwrap_or(-1),
        interrupted,
        stdout: read(&mut stdout)?,
        stderr: read(&mut stderr)?,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn captures_failure_without_pipe_deadlock() {
        let output = run(
            Command::new("/bin/sh").args(["-c", "printf failure >&2; exit 17"]),
            &[],
            None,
            Duration::from_secs(2),
            || {},
        )
        .unwrap();
        assert_eq!(output.code, 17);
        assert_eq!(output.stderr, b"failure");
    }
    #[test]
    fn output_flood_stops_before_timeout_and_calls_cleanup() {
        for redirect in ["2>/dev/null", "1>&2 2>/dev/null"] {
            let cleaned = std::cell::Cell::new(false);
            let start = std::time::Instant::now();
            let result = run(
                Command::new("/bin/sh").args([
                    "-c",
                    &format!("dd if=/dev/zero bs=1048576 count=65 {redirect}; sleep 30"),
                ]),
                &[],
                None,
                Duration::from_secs(30),
                || cleaned.set(true),
            );
            assert!(matches!(result, Err(Error::Limit)));
            assert!(cleaned.get());
            assert!(start.elapsed() < Duration::from_secs(5));
        }
    }
    #[test]
    fn deadline_kills_process_group_and_calls_cleanup() {
        let cleaned = std::cell::Cell::new(false);
        let start = std::time::Instant::now();
        let output = run(
            Command::new("/bin/sh").args(["-c", "printf partial; sleep 30 & wait"]),
            &[],
            None,
            Duration::from_millis(80),
            || cleaned.set(true),
        )
        .unwrap();
        assert!(output.interrupted);
        assert_eq!(output.stdout, b"partial");
        assert!(cleaned.get());
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}

#[cfg(test)]
mod portable_tests {
    use super::*;
    use crate::{interfaces::CancellationSignal, lifecycle::Workspace};
    const WORKER: &str = "adapters::process::portable_tests::worker";
    fn worker_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", WORKER, "--nocapture"])
            .env("PERFINE_PROCESS_TEST_MODE", mode);
        command
    }
    #[test]
    fn worker() {
        match std::env::var("PERFINE_PROCESS_TEST_MODE").as_deref() {
            Ok("sleep") => thread::sleep(Duration::from_secs(30)),
            Ok("tree") => {
                let mut child = worker_command("sleep").spawn().unwrap();
                println!("partial evidence");
                std::io::stdout().flush().unwrap();
                child.wait().unwrap();
            }
            Ok("exit") => {
                eprint!("expected failure");
                std::process::exit(17);
            }
            _ => (),
        }
    }
    struct Signal(Cancellation);
    impl CancellationSignal for Signal {
        fn state(&self) -> Cancellation {
            self.0
        }
    }
    #[test]
    fn timeout_and_immediate_cancel_retain_partial_output() {
        for cancel in [Cancellation::Continue, Cancellation::Immediate] {
            let cleaned = std::cell::Cell::new(false);
            let workspace = Workspace {
                directory: Name::new("fixture").unwrap(),
                branch: Name::new("fixture").unwrap(),
                build_directory: Name::new("fixture-build").unwrap(),
            };
            let signal = Signal(cancel);
            let context = ExecutionContext {
                workspace: &workspace,
                deadline_unix_seconds: now() + 30,
                cancellation: &signal,
            };
            let start = std::time::Instant::now();
            let output = run(
                &mut worker_command("tree"),
                &[],
                Some(&context),
                Duration::from_millis(500),
                || cleaned.set(true),
            )
            .unwrap();
            assert!(output.interrupted);
            assert!(cleaned.get());
            assert!(start.elapsed() < Duration::from_secs(5));
            if cancel == Cancellation::Continue {
                assert!(String::from_utf8_lossy(&output.stdout).contains("partial evidence"));
            }
        }
    }
    #[test]
    fn graceful_cancel_preserves_child_exit_status() {
        let workspace = Workspace {
            directory: Name::new("fixture").unwrap(),
            branch: Name::new("fixture").unwrap(),
            build_directory: Name::new("fixture-build").unwrap(),
        };
        let signal = Signal(Cancellation::GracefulStop);
        let context = ExecutionContext {
            workspace: &workspace,
            deadline_unix_seconds: now() + 30,
            cancellation: &signal,
        };
        let output = run(
            &mut worker_command("exit"),
            &[],
            Some(&context),
            Duration::from_secs(10),
            || panic!("graceful cancellation must not kill admitted work"),
        )
        .unwrap();
        assert!(!output.interrupted);
        assert_eq!(output.code, 17);
        assert!(String::from_utf8_lossy(&output.stderr).contains("expected failure"));
    }
}
