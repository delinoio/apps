// SPDX-License-Identifier: Apache-2.0
//! The local SQLite store `<data dir>/collector.db`. It holds token counts, model identifiers,
//! timestamps, hashed session identifiers and paths only; never prompts or response bodies.
use crate::{
    error::{Failure, Result},
    keys::DataKey,
    pb::ErrorReason,
    time,
};
use rusqlite::{Connection, OptionalExtension as _, params};
use std::{collections::BTreeMap, path::Path};
pub const SCHEMA_VERSION: i32 = 1;
const SCHEMA: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE secrets(name TEXT PRIMARY KEY, key_id TEXT NOT NULL, nonce BLOB NOT NULL, ciphertext BLOB NOT NULL);
CREATE TABLE source_paths(tool INTEGER NOT NULL, path TEXT NOT NULL, user_added INTEGER NOT NULL, enabled INTEGER NOT NULL, PRIMARY KEY(tool, path));
CREATE TABLE files(path TEXT PRIMARY KEY, tool INTEGER NOT NULL, identity TEXT NOT NULL, size INTEGER NOT NULL, mtime INTEGER NOT NULL, offset INTEGER NOT NULL, skipped INTEGER NOT NULL, state TEXT NOT NULL);
CREATE TABLE contributions(path TEXT NOT NULL, tool INTEGER NOT NULL, model TEXT NOT NULL, bucket INTEGER NOT NULL, input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL, PRIMARY KEY(path, tool, model, bucket));
CREATE TABLE claude_records(key TEXT NOT NULL, path TEXT NOT NULL, total INTEGER NOT NULL, at INTEGER NOT NULL, model TEXT NOT NULL, input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL, PRIMARY KEY(key, path));
CREATE TABLE sessions(tool INTEGER NOT NULL, session TEXT NOT NULL, path TEXT NOT NULL, started INTEGER NOT NULL, model TEXT NOT NULL, PRIMARY KEY(tool, session, path));
CREATE TABLE buckets(tool INTEGER NOT NULL, model TEXT NOT NULL, bucket INTEGER NOT NULL, sessions INTEGER NOT NULL, input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL, version INTEGER NOT NULL, uploaded_version INTEGER NOT NULL, PRIMARY KEY(tool, model, bucket));
CREATE TABLE connections(id TEXT PRIMARY KEY, provider INTEGER NOT NULL, display_name TEXT NOT NULL, scope_id TEXT NOT NULL, lease_epoch INTEGER NOT NULL, lease_held INTEGER NOT NULL, failures INTEGER NOT NULL, next_attempt INTEGER NOT NULL, last_status INTEGER NOT NULL, failure_class INTEGER NOT NULL, last_success INTEGER NOT NULL, last_error_correlation TEXT NOT NULL, credential_marker TEXT NOT NULL, auth_stopped INTEGER NOT NULL, last_active_observed INTEGER NOT NULL, last_manual INTEGER NOT NULL, account_fingerprint TEXT NOT NULL);
CREATE TABLE pending_snapshots(connection_id TEXT PRIMARY KEY, epoch INTEGER NOT NULL, snapshot BLOB NOT NULL);
CREATE TABLE widget_snapshots(widget_id TEXT PRIMARY KEY, position INTEGER NOT NULL, snapshot BLOB NOT NULL);
";
/// Token counts normalized to the protocol meaning: uncached input, output including
/// reasoning, cache reads and cache writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}
impl Tokens {
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
    const fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BucketValue {
    pub sessions: u64,
    pub tokens: Tokens,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BucketRow {
    pub tool: i32,
    pub model: String,
    pub bucket: i64,
    pub value: BucketValue,
    pub version: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileCursor {
    pub path: String,
    pub tool: i32,
    pub identity: String,
    pub size: u64,
    pub mtime: i64,
    pub offset: u64,
    pub skipped: u64,
    pub state: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionRow {
    pub id: String,
    pub provider: i32,
    pub display_name: String,
    /// Vercel team ID; empty for other providers.
    pub scope_id: String,
    pub lease_epoch: u64,
    pub lease_held: bool,
    pub failures: u32,
    pub next_attempt: i64,
    pub last_status: i32,
    pub failure_class: i32,
    pub last_success: i64,
    pub last_error_correlation: String,
    /// Digest of the local CLI credential observed when authentication expired.
    pub credential_marker: String,
    pub auth_stopped: bool,
    pub last_active_observed: i64,
    pub last_manual: i64,
    /// The connection's provider account fingerprint when it was added or registered on this
    /// device, or empty when the provider exposes no stable account. Local CLI credentials are
    /// used only while they still belong to this account.
    pub account_fingerprint: String,
}
#[must_use]
pub fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}
fn uint(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}
/// Create a directory readable only by the current user.
pub fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub struct Store {
    conn: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            private_dir(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0 => {
                conn.execute_batch(&format!(
                    "BEGIN; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
                ))?;
            }
            SCHEMA_VERSION => {}
            _ => {
                return Err(Failure::local(
                    ErrorReason::Internal,
                    "The local store has an unsupported schema version; update the collector.",
                ));
            }
        }
        Ok(Self { conn })
    }
    pub fn meta(&self, key: &str) -> Result<String> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?
            .unwrap_or_default())
    }
    pub fn meta_i64(&self, key: &str) -> Result<i64> {
        Ok(self.meta(key)?.parse().unwrap_or(0))
    }
    pub fn put_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
    // Secrets
    pub fn put_secret(&self, key: &DataKey, name: &str, plaintext: &[u8]) -> Result<()> {
        let (nonce, ciphertext) = key.seal(name, plaintext)?;
        self.conn.execute(
            "INSERT INTO secrets(name, key_id, nonce, ciphertext) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET key_id = excluded.key_id, nonce = excluded.nonce, ciphertext = excluded.ciphertext",
            params![name, key.id, nonce, ciphertext],
        )?;
        Ok(())
    }
    pub fn secret(&self, key: &DataKey, name: &str) -> Result<Option<Vec<u8>>> {
        let row: Option<(String, Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT key_id, nonce, ciphertext FROM secrets WHERE name = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        row.map(|(_, nonce, ciphertext)| key.open(name, &nonce, &ciphertext))
            .transpose()
    }
    pub fn has_secret(&self, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM secrets WHERE name = ?1", [name], |_| Ok(()))
            .optional()?
            .is_some())
    }
    pub fn delete_secret(&self, name: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM secrets WHERE name = ?1", [name])?;
        Ok(())
    }
    pub fn secret_count(&self) -> Result<u32> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM secrets", [], |r| r.get(0))?)
    }
    /// Re-encrypt every secret from `old` to `new` and record the new key ID in one
    /// transaction. `fail` aborts before the commit (test fault injection).
    pub fn reencrypt(&mut self, old: &DataKey, new: &DataKey, fail: bool) -> Result<u32> {
        let tx = self.conn.transaction()?;
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> = tx
            .prepare("SELECT name, nonce, ciphertext FROM secrets ORDER BY name")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut count = 0;
        for (index, (name, nonce, ciphertext)) in rows.iter().enumerate() {
            let plain = old.open(name, nonce, ciphertext)?;
            if fail && index + 1 == rows.len() {
                return Err(Failure::internal("Injected rotation failure."));
            }
            let (nonce, ciphertext) = new.seal(name, &plain)?;
            tx.execute(
                "UPDATE secrets SET key_id = ?2, nonce = ?3, ciphertext = ?4 WHERE name = ?1",
                params![name, new.id, nonce, ciphertext],
            )?;
            count += 1;
        }
        if fail && rows.is_empty() {
            return Err(Failure::internal("Injected rotation failure."));
        }
        for key in ["key_id", "pending_key_id"] {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, new.id],
            )?;
        }
        tx.commit()?;
        Ok(count)
    }
    // Source paths
    /// Stored path rows: `(tool, path, user_added, enabled)`.
    pub fn path_rows(&self) -> Result<Vec<(i32, String, bool, bool)>> {
        Ok(self
            .conn
            .prepare(
                "SELECT tool, path, user_added, enabled FROM source_paths ORDER BY tool, path",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn put_path(&self, tool: i32, path: &str, user_added: bool, enabled: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO source_paths(tool, path, user_added, enabled) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(tool, path) DO UPDATE SET user_added = excluded.user_added, enabled = excluded.enabled",
            params![tool, path, user_added, enabled],
        )?;
        Ok(())
    }
    pub fn delete_path(&self, tool: i32, path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM source_paths WHERE tool = ?1 AND path = ?2",
            params![tool, path],
        )?;
        Ok(())
    }
    // File cursors and contributions
    pub fn file(&self, path: &str) -> Result<Option<FileCursor>> {
        Ok(self
            .conn
            .query_row(
                "SELECT path, tool, identity, size, mtime, offset, skipped, state FROM files WHERE path = ?1",
                [path],
                |r| {
                    Ok(FileCursor {
                        path: r.get(0)?,
                        tool: r.get(1)?,
                        identity: r.get(2)?,
                        size: uint(r.get(3)?),
                        mtime: r.get(4)?,
                        offset: uint(r.get(5)?),
                        skipped: uint(r.get(6)?),
                        state: r.get(7)?,
                    })
                },
            )
            .optional()?)
    }
    pub fn begin(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(())
    }
    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }
    pub fn rollback(&self) {
        let _ = self.conn.execute_batch("ROLLBACK");
    }
    pub fn put_file(&self, f: &FileCursor) -> Result<()> {
        self.conn.execute(
            "INSERT INTO files(path, tool, identity, size, mtime, offset, skipped, state) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path) DO UPDATE SET tool = excluded.tool, identity = excluded.identity, size = excluded.size,
             mtime = excluded.mtime, offset = excluded.offset, skipped = excluded.skipped, state = excluded.state",
            params![f.path, f.tool, f.identity, int(f.size), f.mtime, int(f.offset), int(f.skipped), f.state],
        )?;
        Ok(())
    }
    /// Remove a file's stored contributions before it is read again from the start.
    pub fn reset_file(&self, path: &str) -> Result<()> {
        for table in ["contributions", "claude_records", "sessions"] {
            self.conn
                .execute(&format!("DELETE FROM {table} WHERE path = ?1"), [path])?;
        }
        Ok(())
    }
    pub fn add_contribution(
        &self,
        path: &str,
        tool: i32,
        model: &str,
        bucket: i64,
        t: &Tokens,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO contributions(path, tool, model, bucket, input, output, cache_read, cache_write) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path, tool, model, bucket) DO UPDATE SET input = input + excluded.input, output = output + excluded.output,
             cache_read = cache_read + excluded.cache_read, cache_write = cache_write + excluded.cache_write",
            params![path, tool, model, bucket, int(t.input), int(t.output), int(t.cache_read), int(t.cache_write)],
        )?;
        Ok(())
    }
    /// Record a Claude Code token entry; within one file the entry with the larger total wins.
    pub fn add_claude_record(
        &self,
        key: &str,
        path: &str,
        at: i64,
        model: &str,
        t: &Tokens,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO claude_records(key, path, total, at, model, input, output, cache_read, cache_write) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(key, path) DO UPDATE SET total = excluded.total, at = excluded.at, model = excluded.model, input = excluded.input,
             output = excluded.output, cache_read = excluded.cache_read, cache_write = excluded.cache_write WHERE excluded.total > total",
            params![key, path, int(t.total()), at, model, int(t.input), int(t.output), int(t.cache_read), int(t.cache_write)],
        )?;
        Ok(())
    }
    /// Record a session start seen in a file; the earliest start and the first known model win.
    pub fn add_session(
        &self,
        tool: i32,
        session: &str,
        path: &str,
        started: i64,
        model: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions(tool, session, path, started, model) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(tool, session, path) DO UPDATE SET started = MIN(started, excluded.started),
             model = CASE WHEN model = 'unknown' THEN excluded.model ELSE model END",
            params![tool, session, path, started, model],
        )?;
        Ok(())
    }
    /// The current aggregate of every bucket: Codex contributions deduplicated by session,
    /// deduplicated Claude Code entries (largest total per key across files) and sessions counted
    /// in their start bucket.
    ///
    /// A Codex rollout holds one session, so several files with the same session are copies of
    /// it: a rollout moved to `archived_sessions`, or compressed. Only the file with the largest
    /// token total counts; the session comes from the file cursor's parser state.
    pub fn aggregate(&self) -> Result<BTreeMap<(i32, String, i64), BucketValue>> {
        let mut out: BTreeMap<(i32, String, i64), BucketValue> = BTreeMap::new();
        let mut tokens = self.conn.prepare(
            "WITH codex_files AS (
               SELECT c.path, COALESCE(NULLIF(CASE WHEN json_valid(f.state) THEN json_extract(f.state, '$.session') END, ''), c.path) AS session,
                 SUM(c.input + c.output + c.cache_read + c.cache_write) AS total
               FROM contributions c LEFT JOIN files f ON f.path = c.path GROUP BY c.path
             ), counted AS (
               SELECT path FROM (SELECT path, ROW_NUMBER() OVER (PARTITION BY session ORDER BY total DESC, path) AS rn FROM codex_files) WHERE rn = 1
             )
             SELECT tool, model, bucket, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write) FROM contributions
             WHERE path IN (SELECT path FROM counted) GROUP BY tool, model, bucket
             UNION ALL
             SELECT 2, model, (at / 900) * 900 AS b, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write) FROM
             (SELECT *, ROW_NUMBER() OVER (PARTITION BY key ORDER BY total DESC, path) AS rn FROM claude_records) WHERE rn = 1 GROUP BY model, b",
        )?;
        let rows = tokens.query_map([], |r| {
            Ok((
                (
                    r.get::<_, i32>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ),
                Tokens {
                    input: uint(r.get(3)?),
                    output: uint(r.get(4)?),
                    cache_read: uint(r.get(5)?),
                    cache_write: uint(r.get(6)?),
                },
            ))
        })?;
        for row in rows {
            let (key, t) = row?;
            out.entry(key).or_default().tokens.add(&t);
        }
        let mut sessions = self.conn.prepare(
            "SELECT tool, model, (started / 900) * 900 AS b, COUNT(*) FROM
             (SELECT *, ROW_NUMBER() OVER (PARTITION BY tool, session ORDER BY started, path) AS rn FROM sessions) WHERE rn = 1 GROUP BY tool, model, b",
        )?;
        let rows = sessions.query_map([], |r| {
            Ok((
                (
                    r.get::<_, i32>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ),
                uint(r.get(3)?),
            ))
        })?;
        for row in rows {
            let (key, n) = row?;
            out.entry(key).or_default().sessions += n;
        }
        Ok(out)
    }
    /// Write changed aggregates to the bucket table with new versions; buckets whose sources
    /// vanished become zero so the server receives the correction. Returns the changed count.
    pub fn refresh_buckets(&self, now_micros: u64, cutoff: i64) -> Result<usize> {
        let current = self.aggregate()?;
        let mut stored: BTreeMap<(i32, String, i64), (BucketValue, u64)> = BTreeMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT tool, model, bucket, sessions, input, output, cache_read, cache_write, version FROM buckets",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    (
                        r.get::<_, i32>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ),
                    (
                        BucketValue {
                            sessions: uint(r.get(3)?),
                            tokens: Tokens {
                                input: uint(r.get(4)?),
                                output: uint(r.get(5)?),
                                cache_read: uint(r.get(6)?),
                                cache_write: uint(r.get(7)?),
                            },
                        },
                        uint(r.get(8)?),
                    ),
                ))
            })?;
            for row in rows {
                let (k, v) = row?;
                stored.insert(k, v);
            }
        }
        let mut changed = 0;
        let mut keys: Vec<_> = current.keys().cloned().collect();
        keys.extend(stored.keys().cloned());
        keys.sort();
        keys.dedup();
        for key in keys {
            if key.2 < cutoff {
                continue;
            }
            let value = current.get(&key).cloned().unwrap_or_default();
            let previous = stored.get(&key);
            if previous.is_some_and(|(v, _)| *v == value) {
                continue;
            }
            let version = time::next_version(previous.map_or(0, |(_, v)| *v), now_micros);
            self.conn.execute(
                "INSERT INTO buckets(tool, model, bucket, sessions, input, output, cache_read, cache_write, version, uploaded_version)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)
                 ON CONFLICT(tool, model, bucket) DO UPDATE SET sessions = excluded.sessions, input = excluded.input, output = excluded.output,
                 cache_read = excluded.cache_read, cache_write = excluded.cache_write, version = excluded.version",
                params![key.0, key.1, key.2, int(value.sessions), int(value.tokens.input), int(value.tokens.output),
                    int(value.tokens.cache_read), int(value.tokens.cache_write), int(version)],
            )?;
            changed += 1;
        }
        Ok(changed)
    }
    /// Buckets whose current version has not been uploaded, oldest first.
    pub fn pending_buckets(&self, limit: usize) -> Result<Vec<BucketRow>> {
        Ok(self
            .conn
            .prepare(
                "SELECT tool, model, bucket, sessions, input, output, cache_read, cache_write, version FROM buckets
                 WHERE version > uploaded_version ORDER BY bucket, tool, model LIMIT ?1",
            )?
            .query_map([int(limit as u64)], |r| {
                Ok(BucketRow {
                    tool: r.get(0)?,
                    model: r.get(1)?,
                    bucket: r.get(2)?,
                    value: BucketValue {
                        sessions: uint(r.get(3)?),
                        tokens: Tokens {
                            input: uint(r.get(4)?),
                            output: uint(r.get(5)?),
                            cache_read: uint(r.get(6)?),
                            cache_write: uint(r.get(7)?),
                        },
                    },
                    version: uint(r.get(8)?),
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// `(count, oldest bucket start)` of buckets waiting for upload.
    pub fn pending_summary(&self) -> Result<(u64, i64)> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), COALESCE(MIN(bucket), 0) FROM buckets WHERE version > uploaded_version",
            [],
            |r| Ok((uint(r.get(0)?), r.get(1)?)),
        )?)
    }
    pub fn mark_uploaded(&self, row: &BucketRow) -> Result<()> {
        self.conn.execute(
            "UPDATE buckets SET uploaded_version = MAX(uploaded_version, ?4) WHERE tool = ?1 AND model = ?2 AND bucket = ?3",
            params![row.tool, row.model, row.bucket, int(row.version)],
        )?;
        Ok(())
    }
    /// Delete buckets, contributions, deduplication keys and session records older than `cutoff`.
    pub fn prune(&self, cutoff: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM buckets WHERE bucket < ?1", [cutoff])?;
        self.conn
            .execute("DELETE FROM contributions WHERE bucket < ?1", [cutoff])?;
        self.conn
            .execute("DELETE FROM claude_records WHERE at < ?1", [cutoff])?;
        self.conn
            .execute("DELETE FROM sessions WHERE started < ?1", [cutoff])?;
        Ok(())
    }
    /// Skipped records per tool across retained files.
    pub fn skipped(&self, tool: i32) -> Result<u64> {
        Ok(uint(self.conn.query_row(
            "SELECT COALESCE(SUM(skipped), 0) FROM files WHERE tool = ?1",
            [tool],
            |r| r.get(0),
        )?))
    }
    // Connections
    pub fn connections(&self) -> Result<Vec<ConnectionRow>> {
        Ok(self
            .conn
            .prepare(
                "SELECT id, provider, display_name, scope_id, lease_epoch, lease_held, failures, next_attempt, last_status, failure_class,
                 last_success, last_error_correlation, credential_marker, auth_stopped, last_active_observed, last_manual, account_fingerprint FROM connections ORDER BY id",
            )?
            .query_map([], |r| {
                Ok(ConnectionRow {
                    id: r.get(0)?,
                    provider: r.get(1)?,
                    display_name: r.get(2)?,
                    scope_id: r.get(3)?,
                    lease_epoch: uint(r.get(4)?),
                    lease_held: r.get(5)?,
                    failures: r.get(6)?,
                    next_attempt: r.get(7)?,
                    last_status: r.get(8)?,
                    failure_class: r.get(9)?,
                    last_success: r.get(10)?,
                    last_error_correlation: r.get(11)?,
                    credential_marker: r.get(12)?,
                    auth_stopped: r.get(13)?,
                    last_active_observed: r.get(14)?,
                    last_manual: r.get(15)?,
                    account_fingerprint: r.get(16)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn connection(&self, id: &str) -> Result<Option<ConnectionRow>> {
        Ok(self.connections()?.into_iter().find(|c| c.id == id))
    }
    pub fn put_connection(&self, c: &ConnectionRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO connections(id, provider, display_name, scope_id, lease_epoch, lease_held, failures, next_attempt, last_status,
             failure_class, last_success, last_error_correlation, credential_marker, auth_stopped, last_active_observed, last_manual, account_fingerprint)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET provider = excluded.provider, display_name = excluded.display_name, scope_id = excluded.scope_id,
             lease_epoch = excluded.lease_epoch, lease_held = excluded.lease_held, failures = excluded.failures, next_attempt = excluded.next_attempt,
             last_status = excluded.last_status, failure_class = excluded.failure_class, last_success = excluded.last_success,
             last_error_correlation = excluded.last_error_correlation, credential_marker = excluded.credential_marker,
             auth_stopped = excluded.auth_stopped, last_active_observed = excluded.last_active_observed, last_manual = excluded.last_manual,
             account_fingerprint = excluded.account_fingerprint",
            params![c.id, c.provider, c.display_name, c.scope_id, int(c.lease_epoch), c.lease_held, c.failures, c.next_attempt, c.last_status,
                c.failure_class, c.last_success, c.last_error_correlation, c.credential_marker, c.auth_stopped, c.last_active_observed, c.last_manual, c.account_fingerprint],
        )?;
        Ok(())
    }
    pub fn delete_connection(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM connections WHERE id = ?1", [id])?;
        self.conn.execute(
            "DELETE FROM pending_snapshots WHERE connection_id = ?1",
            [id],
        )?;
        self.delete_secret(&format!("connection:{id}"))
    }
    pub fn put_pending_snapshot(&self, id: &str, epoch: u64, snapshot: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pending_snapshots(connection_id, epoch, snapshot) VALUES(?1, ?2, ?3)
             ON CONFLICT(connection_id) DO UPDATE SET epoch = excluded.epoch, snapshot = excluded.snapshot",
            params![id, int(epoch), snapshot],
        )?;
        Ok(())
    }
    pub fn pending_snapshot(&self, id: &str) -> Result<Option<(u64, Vec<u8>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT epoch, snapshot FROM pending_snapshots WHERE connection_id = ?1",
                [id],
                |r| Ok((uint(r.get(0)?), r.get(1)?)),
            )
            .optional()?)
    }
    pub fn delete_pending_snapshot(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM pending_snapshots WHERE connection_id = ?1",
            [id],
        )?;
        Ok(())
    }
    // Native widget snapshots
    pub fn put_widgets(&mut self, items: &[(String, Vec<u8>)], replace_all: bool) -> Result<()> {
        let tx = self.conn.transaction()?;
        if replace_all {
            tx.execute("DELETE FROM widget_snapshots", [])?;
        }
        let base: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), 0) FROM widget_snapshots",
            [],
            |r| r.get(0),
        )?;
        for (index, (id, bytes)) in items.iter().enumerate() {
            tx.execute(
                "INSERT INTO widget_snapshots(widget_id, position, snapshot) VALUES(?1, ?2, ?3)
                 ON CONFLICT(widget_id) DO UPDATE SET snapshot = excluded.snapshot",
                params![id, base + int(index as u64) + 1, bytes],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn widgets(&self) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .conn
            .prepare("SELECT snapshot FROM widget_snapshots ORDER BY position")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
}
