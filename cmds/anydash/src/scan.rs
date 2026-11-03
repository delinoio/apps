// SPDX-License-Identifier: Apache-2.0
//! Session path discovery and incremental file scanning into the local store.
use crate::{
    error::{Failure, Result},
    parse::{self, CodexState, Event},
    pb::{LocalTool, ParserState, ParserStatus, SourcePath, SourcePathOrigin},
    settings::Settings,
    store::{FileCursor, Store},
    time,
};
use serde_json::Value;
use std::{
    io::{BufRead as _, BufReader, Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
};
pub const PARSER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_PATH_BYTES: usize = 4096;
#[must_use]
pub const fn tool_name(tool: LocalTool) -> &'static str {
    match tool {
        LocalTool::Codex => "codex",
        LocalTool::ClaudeCode => "claude-code",
        LocalTool::Unspecified => "unknown",
    }
}
#[must_use]
pub fn parse_tool(name: &str) -> Option<LocalTool> {
    match name {
        "codex" => Some(LocalTool::Codex),
        "claude-code" => Some(LocalTool::ClaudeCode),
        _ => None,
    }
}
/// Auto-detected session directories with their origin.
#[must_use]
pub fn detected(settings: &Settings) -> Vec<(LocalTool, PathBuf, SourcePathOrigin)> {
    let origin = |env: bool| {
        if env {
            SourcePathOrigin::Environment
        } else {
            SourcePathOrigin::Default
        }
    };
    let (codex, env) = settings.codex_home();
    let mut out = vec![
        (LocalTool::Codex, codex.join("sessions"), origin(env)),
        (
            LocalTool::Codex,
            codex.join("archived_sessions"),
            origin(env),
        ),
    ];
    for (dir, env) in settings.claude_dirs() {
        out.push((LocalTool::ClaudeCode, dir.join("projects"), origin(env)));
    }
    out.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    out
}
/// Detected and user-added source paths with origin, enabled state and existence.
pub fn list(settings: &Settings, store: &Store) -> Result<Vec<SourcePath>> {
    let rows = store.path_rows()?;
    let mut out: Vec<SourcePath> = detected(settings)
        .into_iter()
        .map(|(tool, path, origin)| {
            let text = path.to_string_lossy().into_owned();
            let enabled = !rows
                .iter()
                .any(|(t, p, user, enabled)| *t == tool as i32 && *p == text && !user && !enabled);
            SourcePath {
                tool: tool.into(),
                exists: path.is_dir(),
                path: text,
                origin: origin.into(),
                enabled,
            }
        })
        .collect();
    for (tool, path, user, enabled) in rows {
        if user && !out.iter().any(|p| p.tool == tool && p.path == path) {
            out.push(SourcePath {
                tool,
                exists: Path::new(&path).is_dir(),
                path,
                origin: SourcePathOrigin::UserAdded.into(),
                enabled,
            });
        }
    }
    Ok(out)
}
fn validate(path: &str) -> Result<String> {
    if path.len() > MAX_PATH_BYTES || !Path::new(path).is_absolute() {
        return Err(Failure::invalid(
            "path",
            "Session paths must be absolute and at most 4,096 bytes.",
        ));
    }
    Ok(path.trim_end_matches(['/', '\\']).to_owned())
}
fn find(settings: &Settings, store: &Store, tool: LocalTool, path: &str) -> Result<SourcePath> {
    list(settings, store)?
        .into_iter()
        .find(|p| p.tool == tool as i32 && p.path == path)
        .ok_or_else(|| {
            Failure::local(
                crate::pb::ErrorReason::NotFound,
                "The session path is not listed.",
            )
        })
}
pub fn add(settings: &Settings, store: &Store, tool: LocalTool, path: &str) -> Result<SourcePath> {
    let path = validate(path)?;
    let is_detected = detected(settings)
        .iter()
        .any(|(t, p, _)| *t == tool && p.to_string_lossy() == path);
    if is_detected {
        store.delete_path(tool as i32, &path)?;
    } else {
        store.put_path(tool as i32, &path, true, true)?;
    }
    find(settings, store, tool, &path)
}
pub fn set_enabled(
    settings: &Settings,
    store: &Store,
    tool: LocalTool,
    path: &str,
    enabled: bool,
) -> Result<SourcePath> {
    let path = validate(path)?;
    let current = find(settings, store, tool, &path)?;
    if current.origin == SourcePathOrigin::UserAdded as i32 {
        store.put_path(tool as i32, &path, true, enabled)?;
    } else if enabled {
        store.delete_path(tool as i32, &path)?;
    } else {
        store.put_path(tool as i32, &path, false, false)?;
    }
    find(settings, store, tool, &path)
}
#[derive(Clone, Debug, Default)]
pub struct SyncReport {
    pub files_read: usize,
    pub changed_buckets: usize,
    /// The newest Codex `rate_limits` observation read in this sync.
    pub codex_rate_limits: Option<(i64, Value)>,
    pub parsers: Vec<ParserStatus>,
}
/// `*.jsonl` for both tools and `*.jsonl.zst` for Codex.
fn session_file(path: &Path, tool: LocalTool) -> bool {
    let extension = |p: &Path, e: &str| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(e));
    extension(path, "jsonl")
        || (tool == LocalTool::Codex
            && extension(path, "zst")
            && extension(&path.with_extension(""), "jsonl"))
}
fn walk(dir: &Path, tool: LocalTool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, tool, out);
        } else if kind.is_file() && session_file(&path, tool) {
            out.push(path);
        }
    }
}
#[cfg(unix)]
fn identity(m: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt as _;
    format!("{}:{}", m.dev(), m.ino())
}
#[cfg(windows)]
fn identity(m: &std::fs::Metadata) -> String {
    // The stable Windows file index is not exposed by std; the creation time identifies a
    // replaced file at the same path.
    use std::os::windows::fs::MetadataExt as _;
    format!("created:{}", m.creation_time())
}
fn mtime(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
}
/// Scan every enabled source path, update file contributions and bucket versions, and
/// return parser statuses. Original session files are opened read-only.
pub fn sync(settings: &Settings, store: &Store, now: i64) -> Result<SyncReport> {
    let cutoff = now - time::RETENTION_SECONDS;
    let paths = list(settings, store)?;
    let mut report = SyncReport::default();
    for tool in [LocalTool::Codex, LocalTool::ClaudeCode] {
        let sources: Vec<&SourcePath> = paths.iter().filter(|p| p.tool == tool as i32).collect();
        let active: Vec<&&SourcePath> = sources.iter().filter(|p| p.enabled && p.exists).collect();
        let mut files = vec![];
        for source in &active {
            walk(Path::new(&source.path), tool, &mut files);
        }
        files.sort();
        files.dedup();
        for file in files {
            match sync_file(store, tool, &file, cutoff, &mut report) {
                Ok(true) => report.files_read += 1,
                Ok(false) => {}
                Err(e) => {
                    store.rollback();
                    tracing::warn!(event = "session_file_failed", tool = tool_name(tool), correlation_id = %e.correlation_id);
                }
            }
        }
        let skipped = store.skipped(tool as i32)?;
        let state = if sources.iter().all(|p| !p.enabled) && !sources.is_empty() {
            ParserState::Disabled
        } else if active.is_empty() {
            ParserState::NoSources
        } else if skipped > 0 {
            ParserState::Partial
        } else {
            ParserState::Ok
        };
        report.parsers.push(ParserStatus {
            tool: tool.into(),
            state: state.into(),
            skipped_record_count: skipped,
            observed_at: Some(time::timestamp(now)),
            parser_version: PARSER_VERSION.to_owned(),
            source_path_count: u32::try_from(active.len()).unwrap_or(u32::MAX),
        });
    }
    store.prune(cutoff)?;
    report.changed_buckets =
        store.refresh_buckets(time::now_micros(), time::bucket_start(cutoff))?;
    Ok(report)
}
#[expect(
    clippy::too_many_lines,
    reason = "Cursor checks, reading and contribution writes share one transaction."
)]
fn sync_file(
    store: &Store,
    tool: LocalTool,
    path: &Path,
    cutoff: i64,
    report: &mut SyncReport,
) -> Result<bool> {
    let meta = std::fs::metadata(path)?;
    let key = path.to_string_lossy().into_owned();
    let compressed = path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("zst"));
    let id = identity(&meta);
    let size = meta.len();
    let modified = mtime(&meta);
    let previous = store.file(&key)?;
    let reread = previous.as_ref().is_none_or(|p| {
        p.identity != id
            || size < p.offset
            || (compressed && (p.size != size || p.mtime != modified))
    });
    if !reread && previous.as_ref().is_some_and(|p| p.offset == size) {
        if let Some(p) = previous
            .as_ref()
            .filter(|p| p.mtime != modified || p.size != size)
        {
            store.put_file(&FileCursor {
                size,
                mtime: modified,
                ..p.clone()
            })?;
        }
        return Ok(false);
    }
    let mut cursor = match previous {
        Some(p) if !reread => p,
        _ => FileCursor {
            path: key.clone(),
            tool: tool as i32,
            identity: id,
            ..FileCursor::default()
        },
    };
    let mut state: CodexState = serde_json::from_str(&cursor.state).unwrap_or_default();
    let mut bytes = vec![];
    let mut file = std::fs::File::open(path)?;
    if compressed {
        zstd::stream::read::Decoder::new(file)?.read_to_end(&mut bytes)?;
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
    } else {
        file.seek(SeekFrom::Start(cursor.offset))?;
        let mut reader = BufReader::new(file);
        let mut line = vec![];
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
                break;
            }
            bytes.extend_from_slice(&line);
        }
    }
    let consumed = bytes.len();
    store.begin()?;
    if reread {
        store.reset_file(&key)?;
    }
    let mut events = vec![];
    for line in bytes.split(|b| *b == b'\n') {
        let Ok(text) = std::str::from_utf8(line) else {
            cursor.skipped += 1;
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let result = match tool {
            LocalTool::Codex => parse::codex_line(text, &mut state, &key, cutoff, &mut events),
            _ => parse::claude_line(text, cutoff, &mut events),
        };
        if result.is_err() {
            cursor.skipped += 1;
        }
    }
    for event in events {
        match event {
            Event::Tokens {
                at,
                model,
                tokens,
                key: dedup,
            } => {
                if tool == LocalTool::ClaudeCode {
                    store.add_claude_record(&dedup, &key, at, &model, &tokens)?;
                } else {
                    store.add_contribution(
                        &key,
                        tool as i32,
                        &model,
                        time::bucket_start(at),
                        &tokens,
                    )?;
                }
            }
            Event::Session { id, at, model } => {
                store.add_session(tool as i32, &id, &key, at, &model)?;
            }
            Event::RateLimits { at, value } => {
                if report
                    .codex_rate_limits
                    .as_ref()
                    .is_none_or(|(t, _)| *t <= at)
                {
                    report.codex_rate_limits = Some((at, value));
                }
            }
        }
    }
    cursor.offset = if compressed {
        size
    } else {
        cursor.offset + consumed as u64
    };
    cursor.size = size;
    cursor.mtime = modified;
    cursor.state = if tool == LocalTool::Codex {
        serde_json::to_string(&state).unwrap_or_default()
    } else {
        String::new()
    };
    store.put_file(&cursor)?;
    store.commit()?;
    Ok(true)
}
