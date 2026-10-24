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
    let mut child = command.spawn()?;
    let start = std::time::Instant::now();
    let mut interrupted = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout
            || context.is_some_and(|c| {
                now() >= c.deadline_unix_seconds
                    || c.cancellation.state() == Cancellation::Immediate
            })
        {
            cleanup();
            #[cfg(unix)]
            {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = child.kill();
            interrupted = true;
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(20));
    };
    fn read(file: &mut File) -> Result<Vec<u8>> {
        // Bound host memory even when an untrusted command floods stdout.
        if file.metadata()?.len() > 64 * 1024 * 1024 {
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

#[cfg(test)]
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
