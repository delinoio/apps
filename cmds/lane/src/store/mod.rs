// SPDX-License-Identifier: Apache-2.0
mod capture;
mod domain;
mod migration;
pub use migration::migrate;
pub mod normalize;
mod query;
pub mod sync;
use crate::{
    error::{Failure, Result, invalid},
    protocol::{hash, id, strv, valid_id},
};
#[cfg(unix)]
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
pub struct Store {
    pub root: PathBuf,
    pub space: String,
    pub device: String,
    pub registry: Connection,
    db: std::cell::RefCell<Option<Connection>>,
    handles: std::cell::RefCell<Option<File>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub issuer: String,
    pub account: String,
    pub tenant: String,
    pub enabled: bool,
}
pub fn private(path: &Path, directory: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )?;
    }
    #[cfg(windows)]
    crate::windows::protect(path, directory)?;
    Ok(())
}
pub fn mkdir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    private(path, true)
}
pub fn default_root() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let root = Some(crate::windows::data_root()?);
    #[cfg(target_os = "macos")]
    let root =
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let root = std::env::var_os("XDG_DATA_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    let root = root.filter(|p| p.is_absolute()).ok_or_else(|| {
        Failure::new(
            "DEPENDENCY_UNAVAILABLE",
            "OS data directory is unavailable.",
        )
    })?;
    Ok(root.join("io.delino.lane"))
}
pub fn lock(path: &Path, exclusive: bool) -> Result<File> {
    mkdir(path.parent().ok_or_else(|| invalid("Invalid lock path."))?)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    private(path, false)?;
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        #[cfg(windows)]
        let result = crate::windows::try_lock(&file, exclusive);
        #[cfg(unix)]
        let result = if exclusive {
            FileExt::try_lock_exclusive(&file)
        } else {
            FileExt::try_lock_shared(&file)
        };
        match result {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        if Instant::now() >= until {
            return Err(Failure::new(
                "STORAGE_BUSY",
                "Space handles are busy; close other clients and retry.",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
pub fn transaction<T>(
    c: &Connection,
    write: bool,
    f: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    c.execute_batch(if write { "BEGIN IMMEDIATE" } else { "BEGIN" })?;
    let r = f(c).and_then(|v| {
        c.execute_batch("COMMIT")?;
        Ok(v)
    });
    if r.is_err() {
        let _ = c.execute_batch("ROLLBACK");
    }
    r
}
fn compatible(c: &Connection, schema: Option<&str>) -> Result<()> {
    let app: i64 = c.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let is_store: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='store_meta')",
        [],
        |r| r.get(0),
    )?;
    if app == 1_279_348_293 && version == if is_store { 2 } else { 1 } {
        return Ok(());
    }
    let count: i64 = c.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    if app == 0
        && version == 0
        && count == 0
        && let Some(schema) = schema
    {
        c.execute_batch(schema)?;
        c.execute_batch(if schema.contains("CREATE TABLE store_meta") {
            "PRAGMA application_id=1279348293; PRAGMA user_version=2"
        } else {
            "PRAGMA application_id=1279348293; PRAGMA user_version=1"
        })?;
        return Ok(());
    }
    Err(Failure::new(
        "UNSUPPORTED_FORMAT",
        "Update Lane to open this database.",
    ))
}
fn open_db(path: &Path, schema: &str) -> Result<Connection> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    private(path, false)?;
    drop(file);
    let c = Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(5))?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON;")?;
    transaction(&c, true, |c| compatible(c, Some(schema)))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match c.query_row::<String, _, _>("PRAGMA journal_mode=WAL", [], |r| r.get(0)) {
            Ok(mode) if mode == "wal" => break,
            Err(e)
                if matches!(
                    e.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.into()),
            _ => {
                return Err(Failure::new(
                    "UNSUPPORTED_FORMAT",
                    "WAL mode is unavailable.",
                ));
            }
        }
    }
    Ok(c)
}
pub fn meta(c: &Connection, key: &str) -> Result<String> {
    Ok(
        c.query_row("SELECT value FROM store_meta WHERE key=?", [key], |r| {
            r.get(0)
        })
        .optional()?
        .unwrap_or_default(),
    )
}
pub fn put_meta(c: &Connection, key: &str, value: &str) -> Result<()> {
    c.execute("INSERT INTO store_meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
    Ok(())
}
pub fn rows(c: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<String>> {
    Ok(c.prepare(sql)?
        .query_map(p, |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}
impl Store {
    pub fn open(root: &Path, selected: &str, removal: bool) -> Result<Self> {
        if !selected.is_empty() && !valid_id(selected) {
            return Err(invalid("Invalid space ID."));
        }
        mkdir(root)?;
        let registry = open_db(
            &root.join("registry.sqlite"),
            include_str!("../../../../apps/lane/native/store/registry.sql"),
        )?;
        let (space, device) = transaction(&registry, true, |c| {
            let mut space = selected.to_owned();
            if space.is_empty() {
                space = c
                    .query_row(
                        "SELECT value FROM registry_meta WHERE key='active_space_id'",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or_default();
                if space.is_empty() {
                    space = c
                        .query_row("SELECT id FROM spaces WHERE account_key='local'", [], |r| {
                            r.get(0)
                        })
                        .optional()?
                        .unwrap_or_default();
                    if space.is_empty() {
                        space = id();
                        c.execute(
                            "INSERT INTO spaces(id,account_key) VALUES(?,'local')",
                            [&space],
                        )?;
                    }
                    c.execute(
                        "INSERT OR REPLACE INTO registry_meta VALUES('active_space_id',?)",
                        [&space],
                    )?;
                    c.execute(
                        "INSERT OR IGNORE INTO registry_meta VALUES('device_id',?)",
                        [id()],
                    )?;
                }
            }
            let unavailable: i64 = c.query_row(
                "SELECT count(*) FROM registry_meta WHERE key=? AND value='1'",
                [format!("unavailable:{space}")],
                |r| r.get(0),
            )?;
            if unavailable > 0 && !removal {
                return Err(Failure::new("DELETED", "Space is unavailable."));
            }
            let n: i64 = c.query_row("SELECT count(*) FROM spaces WHERE id=?", [&space], |r| {
                r.get(0)
            })?;
            if n != 1 && !(removal && unavailable > 0) {
                return Err(Failure::new("NOT_FOUND", "Space does not exist."));
            }
            let device: String = c.query_row(
                "SELECT value FROM registry_meta WHERE key='device_id'",
                [],
                |r| r.get(0),
            )?;
            Ok((space, device))
        })?;
        let s = Self {
            root: root.into(),
            space,
            device,
            registry,
            db: std::cell::RefCell::new(None),
            handles: std::cell::RefCell::new(None),
        };
        if removal && s.unavailable()? {
            return Ok(s);
        }
        s.handles.replace(Some(lock(&s.lock_path(), false)?));
        if s.unavailable()? {
            return Err(Failure::new("DELETED", "Space is unavailable."));
        }
        mkdir(&root.join("spaces"))?;
        let dir = root.join("spaces").join(&s.space);
        mkdir(&dir)?;
        let db = open_db(
            &dir.join("lane.sqlite"),
            include_str!("../../../../apps/lane/native/store/schema.sql"),
        )?;
        transaction(&db, true, |c| {
            for (k, v) in [
                ("space_id", s.space.as_str()),
                ("device_id", &s.device),
                ("format_version", "2"),
                ("account_epoch", ""),
                ("pull_cursor", ""),
                ("notification_enabled", "0"),
                ("notification_content", "DETAIL"),
                ("sync_owner", ""),
                ("sync_owner_expires_at_us", "0"),
            ] {
                c.execute(
                    "INSERT OR IGNORE INTO store_meta VALUES(?,?)",
                    params![k, v],
                )?;
            }
            Ok(())
        })?;
        s.db.replace(Some(db));
        s.with(false, |_| Ok(()))?;
        Ok(s)
    }
    pub fn lock_path(&self) -> PathBuf {
        self.root
            .join("locks")
            .join(format!("{}.handles.lock", self.space))
    }
    pub fn unavailable(&self) -> Result<bool> {
        Ok(self.registry.query_row::<i64, _, _>(
            "SELECT count(*) FROM registry_meta WHERE key=? AND value='1'",
            [format!("unavailable:{}", self.space)],
            |r| r.get(0),
        )? > 0)
    }
    pub fn with<T>(&self, write: bool, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        if self.unavailable()? {
            self.db.borrow_mut().take();
            self.handles.borrow_mut().take();
            return Err(Failure::new("DELETED", "Space is unavailable."));
        }
        let connection = self.db.borrow();
        let c = connection
            .as_ref()
            .ok_or_else(|| Failure::new("DELETED", "Space is unavailable."))?;
        transaction(c, write, |c| {
            compatible(c, None)?;
            compatible(&self.registry, None)?;
            if self.unavailable()? || meta(c, "removed")? == "1" {
                return Err(Failure::new("DELETED", "Space is unavailable."));
            }
            if meta(c, "space_id")? != self.space
                || meta(c, "device_id")? != self.device
                || meta(c, "format_version")? != "2"
            {
                return Err(Failure::new(
                    "UNSUPPORTED_FORMAT",
                    "Incompatible space format.",
                ));
            }
            let n:i64=c.query_row("SELECT count(*) FROM heads h LEFT JOIN versions v ON v.version_id=h.version_id WHERE v.version_id IS NULL OR h.entity_id<>v.entity_id",[],|r|r.get(0))?;
            if n != 0 {
                return Err(invalid("Invalid head linkage."));
            }
            f(c)
        })
    }
    pub fn identity(&self) -> Result<Identity> {
        Ok(self.registry.query_row("SELECT coalesce(issuer,''),coalesce(account_id,''),coalesce(tenant_id,''),sync_enabled FROM spaces WHERE id=?",[&self.space],|r|Ok(Identity{
issuer:r.get(0)?,account:r.get(1)?,tenant:r.get(2)?,enabled:r.get(3)?}
))?)
    }
    pub fn active(&self) -> Result<String> {
        Ok(self.registry.query_row(
            "SELECT value FROM registry_meta WHERE key='active_space_id'",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        transaction(&self.registry, true, |c| {
            if self.unavailable()? {
                return Err(Failure::new("DELETED", "Space is unavailable."));
            }
            if c.execute(
                "UPDATE spaces SET sync_enabled=? WHERE id=?",
                params![enabled, self.space],
            )? != 1
            {
                return Err(Failure::new("DELETED", "Space is unavailable."));
            }
            Ok(())
        })
    }
    pub fn select_account(&self, issuer: &str, account: &str, tenant: &str) -> Result<Self> {
        if issuer.is_empty() || account.is_empty() || tenant.is_empty() {
            return Err(invalid("Approved account identity is required."));
        }
        let key = hash(format!("{issuer}\0{account}\0{tenant}"));
        let space = transaction(&self.registry, true, |c| {
            let existing: Option<String> = c
                .query_row("SELECT id FROM spaces WHERE account_key=?", [&key], |r| {
                    r.get(0)
                })
                .optional()?;
            let space = existing.unwrap_or_else(id);
            c.execute("INSERT OR IGNORE INTO spaces(id,account_key,issuer,account_id,tenant_id) VALUES(?,?,?,?,?)",params![space,key,issuer,account,tenant])?;
            c.execute("UPDATE spaces SET sync_enabled=0 WHERE id=?", [&space])?;
            c.execute(
                "UPDATE registry_meta SET value=? WHERE key='active_space_id'",
                [&space],
            )?;
            Ok(space)
        })?;
        Self::open(&self.root, &space, false)
    }
    pub fn remove(&mut self) -> Result<()> {
        transaction(&self.registry, true, |c| {
            c.execute(
                "INSERT OR REPLACE INTO registry_meta VALUES(?,'1')",
                [format!("unavailable:{}", self.space)],
            )?;
            c.execute(
                "UPDATE spaces SET sync_enabled=0,account_key=? WHERE id=?",
                params![format!("removed:{}", self.space), self.space],
            )?;
            Ok(())
        })?;
        if let Some(db) = self.db.borrow_mut().take() {
            let result = transaction(&db, true, |c| {
                c.execute_batch("DELETE FROM attachments; DELETE FROM attachment_identities; DELETE FROM heads; DELETE FROM versions; DELETE FROM entities; DELETE FROM receipts; DELETE FROM outbox; DELETE FROM remote_objects; DELETE FROM notification_receipts; DELETE FROM repository_mappings; DELETE FROM store_meta WHERE key NOT IN ('space_id','device_id','format_version'); INSERT INTO changes(change_kind) VALUES(3); INSERT OR REPLACE INTO store_meta VALUES('removed','1');")?;
                Ok(())
            });
            drop(db);
            self.handles.borrow_mut().take();
            result?;
        }
        let _lock = lock(&self.lock_path(), true)?;
        let path = self.root.join("spaces").join(&self.space);
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        transaction(&self.registry, true, |c| {
            c.execute("DELETE FROM spaces WHERE id=?", [&self.space])?;
            c.execute(
                "DELETE FROM registry_meta WHERE key='active_space_id' AND value=?",
                [&self.space],
            )?;
            Ok(())
        })
    }
    pub fn preferences(&self, c: &Connection) -> Result<Value> {
        Ok(json!({
        "enabled":meta(c,"notification_enabled")?=="1","content":if meta(c,"notification_content")?=="GENERIC" {
        "NOTIFICATION_CONTENT_GENERIC"}
        else{
        "NOTIFICATION_CONTENT_DETAIL"}
        ,"permission":"NOTIFICATION_PERMISSION_UNSUPPORTED"}
        ))
    }
    pub fn invoke(&self, method: &str, q: &Value) -> Result<Value> {
        match method {
            "GetNotificationPreferences"=>self.with(false,|c|self.preferences(c)),
            "SetNotificationPreferences"=>self.with(true,|c|{
let content=match strv(q,"content"){
""|"NOTIFICATION_CONTENT_DETAIL"=>"DETAIL","NOTIFICATION_CONTENT_GENERIC"=>"GENERIC",_=>return Err(invalid("Invalid notification content."))}
;
put_meta(c,"notification_enabled",if q["enabled"]==true{
"1"}
else{
"0"}
)?;
put_meta(c,"notification_content",content)?;
c.execute("INSERT INTO changes(change_kind) VALUES(3)",[])?;
self.preferences(c)}
),
            "GetLocalStatus"=>self.with(false,|c|{
let identity=self.identity()?;
let count:i64=c.query_row("SELECT (SELECT count(*) FROM outbox) + (SELECT count(*) FROM attachments WHERE synchronized=0 OR png_base64 IS NULL)",[],|r|r.get(0))?;
let mut stmt=c.prepare("SELECT request_id,entity_id,error_reason FROM outbox WHERE error_reason IS NOT NULL ORDER BY request_id")?;
let items=stmt.query_map([],|r|Ok(json!({
"requestId":r.get::<_,String>(0)?,"entityId":r.get::<_,String>(1)?,"reason":r.get::<_,String>(2)?}
)))?.collect::<std::result::Result<Vec<_>,_>>()?;
let mut out=json!({
"spaceId":self.space,"deviceId":self.device,"accountId":identity.account,"syncEnabled":identity.enabled,"connected":false,"reconnectRequired":meta(c,"reconnect_required")?=="1","syncErrorReason":meta(c,"sync_error_reason")?,"pendingItemCount":count.to_string(),"itemErrors":items,"notificationPreferences":self.preferences(c)?}
);
if let Ok(us)=meta(c,"last_sync_at_us")?.parse::<i64>()&& let Some(t)=chrono::DateTime::from_timestamp_micros(us){
out["lastSyncAt"]=t.to_rfc3339_opts(chrono::SecondsFormat::Micros,true).into();
}
Ok(out)}
),
            "CreateCaptureTask"|"CreateURLConnection"|"SaveURLConnection"|"DeleteURLConnection"|"CreateRepository"|"SaveRepository"|"DeleteRepository"|"CreateTask"|"SaveTask"|"SetProgress"|"SetProposal"|"RestorePrompt"|"DeleteTask"|"ResolveConflict"=>self.mutate(method,q),
            "GetAttachment"=>self.with(false, |c| self.get_attachment(c,q)),
            _=>self.with(false,|c|self.query(c,method,q)),
        }
    }
}
