// SPDX-License-Identifier: Apache-2.0
use crate::{
    error::Result,
    store::{lock, mkdir, private},
};
use std::{
    fs::OpenOptions,
    io::Write,
    path::Path,
    time::{Duration, SystemTime},
};
/// Append one structured operation record. Records carry only the operation name, correlation
/// ID, result category and duration; never task content or label, milestone and cycle names
/// and descriptions.
pub fn record(
    root: &Path,
    operation: &str,
    reason: &str,
    correlation: &str,
    duration: Duration,
) -> Result<()> {
    let dir = root.join("logs");
    mkdir(&dir)?;
    let _lock = lock(&dir.join(".rotation.lock"), true)?;
    let path = dir.join(format!(
        "cli-{}-{}.log",
        chrono::Utc::now().format("%Y%m%d"),
        std::process::id()
    ));
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    private(&path, false)?;
    writeln!(
        file,
        "{}",
        serde_json::json!({"time":chrono::Utc::now().to_rfc3339(),"level":"INFO","msg":"operation","operation":operation,"reason":reason,"correlationId":correlation,"durationMs":u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)})
    )?;
    drop(file);
    let mut files = vec![];
    let now = SystemTime::now();
    let mut total = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_name() == ".rotation.lock" {
            continue;
        }
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified()?;
        if now.duration_since(modified).unwrap_or_default() >= Duration::from_hours(720) {
            std::fs::remove_file(entry.path())?;
            continue;
        }
        total += metadata.len();
        files.push((modified, entry.path(), metadata.len()));
    }
    files.sort();
    for (_, path, len) in files {
        if total <= 100 << 20 {
            break;
        }
        std::fs::remove_file(path)?;
        total -= len;
    }
    Ok(())
}
