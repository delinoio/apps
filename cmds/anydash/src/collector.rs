// SPDX-License-Identifier: Apache-2.0
//! The single shared collector process: the per-user lock, the local IPC service and the
//! scheduler loop.
#[cfg(windows)]
use crate::core::Owner;
use crate::{
    core::{Core, Options},
    error::{Failure, Result},
    ipc,
    pb::ErrorReason,
    settings::Dirs,
    time,
};
use fs2::FileExt as _;
use std::{fs::File, sync::Arc, time::Duration};
const TICK: Duration = Duration::from_secs(15);
/// Take the exclusive advisory lock on `<data dir>/collector.lock`, or `None` when another
/// process holds it. The lock is released when the file is dropped.
pub fn try_lock(dirs: &Dirs) -> Result<Option<File>> {
    crate::store::private_dir(&dirs.data)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dirs.data.join("collector.lock"))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(_) => Ok(None),
    }
}
#[must_use]
pub fn already_running() -> Failure {
    Failure::local(
        ErrorReason::Internal,
        "Another AnyDash collector already runs for this OS user.",
    )
}
/// A started collector: the lock, the core and the IPC listener.
pub struct Running {
    pub core: Arc<Core>,
    _lock: File,
    listener: Option<ipc::Listener>,
}
/// Take the lock and bind the IPC endpoint without starting the loop.
pub fn start(options: Options) -> Result<Running> {
    let dirs = options.settings.dirs();
    let lock = try_lock(&dirs)?.ok_or_else(already_running)?;
    let core = Core::open(options)?;
    let listener = ipc::bind(&dirs)?;
    Ok(Running {
        core,
        _lock: lock,
        listener: Some(listener),
    })
}
impl Running {
    /// Serve IPC and run the scheduler until an explicit quit or a stop signal, then release
    /// held leases.
    pub async fn run(mut self) -> Result<()> {
        let core = Arc::clone(&self.core);
        tracing::info!(event = "collector_started", owner = ?core.owner, collector_version = crate::core::VERSION);
        let server = self
            .listener
            .take()
            .map(|l| tokio::spawn(ipc::serve(Arc::clone(&core), l)));
        let signals = {
            let core = Arc::clone(&core);
            tokio::spawn(async move {
                wait_for_signal().await;
                tracing::info!(event = "stop_signal");
                let _ = core.stopped.send(true);
                core.wake.notify_one();
            })
        };
        let mut stopped = core.stopped.subscribe();
        let mut tick = tokio::time::interval(TICK);
        loop {
            if *stopped.borrow() {
                break;
            }
            tokio::select! {
                _ = tick.tick() => {}
                () = core.wake.notified() => {}
                _ = stopped.changed() => break,
            }
            core.cycle(time::now()).await;
        }
        core.release_all().await;
        signals.abort();
        if let Some(server) = server {
            server.abort();
            // Wait until the listener is closed, so that a command issued right after the quit
            // runs directly instead of reaching a closing endpoint.
            let _ = server.await;
        }
        tracing::info!(event = "collector_stopped");
        Ok(())
    }
}
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
/// Run the collector in this process.
pub async fn run(options: Options) -> Result<()> {
    #[cfg(windows)]
    if options.owner == Owner::Service {
        crate::windows::detach_console();
    }
    start(options)?.run().await
}
