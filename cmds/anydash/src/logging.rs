// SPDX-License-Identifier: Apache-2.0
//! Structured `tracing` logs written to `anydash.YYYY-MM-DD.log` in the OS log directory,
//! rotated daily and kept for 7 days or 50 MiB in total, whichever limit is reached first.
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _};
pub const KEEP_DAYS: i64 = 7;
pub const MAX_TOTAL_BYTES: u64 = 50 << 20;
const PRUNE_EVERY_BYTES: u64 = 1 << 20;
#[must_use]
pub fn file_name(date: chrono::NaiveDate) -> String {
    format!("anydash.{}.log", date.format("%Y-%m-%d"))
}
fn file_date(name: &str) -> Option<chrono::NaiveDate> {
    let date = name.strip_prefix("anydash.")?.strip_suffix(".log")?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}
/// Delete log files older than 7 days, then the oldest files until the total is at most
/// 50 MiB. The current file restarts empty when it alone exceeds the limit.
pub fn prune(dir: &Path, today: chrono::NaiveDate, current: Option<&Path>) -> std::io::Result<()> {
    let mut files: Vec<(chrono::NaiveDate, PathBuf, u64)> = vec![];
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(date) = file_date(&name) else {
            continue;
        };
        let len = entry.metadata().map_or(0, |m| m.len());
        if (today - date).num_days() >= KEEP_DAYS {
            std::fs::remove_file(entry.path())?;
            continue;
        }
        files.push((date, entry.path(), len));
    }
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.2).sum();
    for (_, path, len) in &files {
        if total <= MAX_TOTAL_BYTES {
            break;
        }
        if Some(path.as_path()) == current {
            OpenOptions::new().write(true).open(path)?.set_len(0)?;
        } else {
            std::fs::remove_file(path)?;
        }
        total -= len;
    }
    Ok(())
}
pub struct Rolling {
    dir: PathBuf,
    date: Option<chrono::NaiveDate>,
    file: Option<File>,
    since_prune: u64,
}
impl Rolling {
    #[must_use]
    pub const fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            date: None,
            file: None,
            since_prune: 0,
        }
    }
    fn open(&mut self, today: chrono::NaiveDate) -> std::io::Result<&mut File> {
        if self.date != Some(today) || self.file.is_none() {
            crate::store::private_dir(&self.dir)
                .map_err(|_| std::io::Error::other("log directory"))?;
            let path = self.dir.join(file_name(today));
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            self.file = Some(options.open(&path)?);
            self.date = Some(today);
            prune(&self.dir, today, Some(&path))?;
        }
        self.file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("log file"))
    }
}
impl Write for Rolling {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let today = chrono::Utc::now().date_naive();
        let n = self.open(today)?.write(buf)?;
        self.since_prune += n as u64;
        if self.since_prune >= PRUNE_EVERY_BYTES {
            self.since_prune = 0;
            let path = self.dir.join(file_name(today));
            prune(&self.dir, today, Some(&path))?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.as_mut().map_or(Ok(()), File::flush)
    }
}
#[derive(Clone)]
struct Shared(Arc<Mutex<Rolling>>);
impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("log lock"))?
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("log lock"))?
            .flush()
    }
}
/// Install the subscriber. Logs are ANSI-colored unless `color` is false; foreground runs also
/// print to the terminal (stderr). `RUST_LOG` is not consulted; the level is `info`.
pub fn init(dir: &Path, color: bool, terminal: bool) {
    let shared = Shared(Arc::new(Mutex::new(Rolling::new(dir.to_path_buf()))));
    let file = tracing_subscriber::fmt::layer()
        .with_ansi(color)
        .with_target(false)
        .with_writer(move || shared.clone())
        .with_filter(tracing_subscriber::filter::LevelFilter::INFO);
    let stderr = terminal.then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(color)
            .with_target(false)
            .with_writer(std::io::stderr)
            .with_filter(tracing_subscriber::filter::LevelFilter::INFO)
    });
    let _ = tracing_subscriber::registry()
        .with(file)
        .with(stderr)
        .try_init();
}
