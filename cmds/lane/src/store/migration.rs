// SPDX-License-Identifier: Apache-2.0
use super::{
    Connection, Failure, Path, Result, Value, compatible, invalid, json, lock, meta, valid_id,
};
use rusqlite::OpenFlags;

/// Explicitly upgrade one existing space. Never open or create a default space.
pub fn migrate(root: &Path, space: &str) -> Result<Value> {
    if !valid_id(space) {
        return Err(invalid("Migration requires an explicit space UUID."));
    }
    let registry = Connection::open_with_flags(
        root.join("registry.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    compatible(&registry, None)?;
    let _handles = lock(
        &root.join("locks").join(format!("{space}.handles.lock")),
        true,
    )?;
    let exists: bool = registry.query_row(
        "SELECT EXISTS(SELECT 1 FROM spaces WHERE id=?)",
        [space],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(Failure::new("NOT_FOUND", "Space does not exist."));
    }
    let unavailable: bool = registry.query_row(
        "SELECT EXISTS(SELECT 1 FROM registry_meta WHERE key=? AND value='1')",
        [format!("unavailable:{space}")],
        |r| r.get(0),
    )?;
    if unavailable {
        return Err(Failure::new("DELETED", "Space is unavailable."));
    }
    let db = Connection::open_with_flags(
        root.join("spaces").join(space).join("lane.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    // SQLite requires foreign_keys to be disabled outside the transaction when
    // rebuilding a referenced table. The exclusive lifetime lock drains clients.
    db.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA synchronous=FULL; BEGIN EXCLUSIVE")?;
    let result = (|| {
        let app: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let schema: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if app != 1_279_348_293 || !matches!(schema, 1 | 2) {
            return Err(Failure::new(
                "UNSUPPORTED_FORMAT",
                "No supported migration for this database.",
            ));
        }
        if meta(&db, "space_id")? != space || meta(&db, "format_version")? != schema.to_string() {
            return Err(Failure::new(
                "UNSUPPORTED_FORMAT",
                "Incompatible space format.",
            ));
        }
        if schema == 1 {
            db.execute_batch(include_str!(
                "../../../../apps/lane/native/store/migrate-1-to-2.sql"
            ))?;
        }
        let mut check = db.prepare("PRAGMA foreign_key_check")?;
        if check.query([])?.next()?.is_some() {
            return Err(invalid("Migration found invalid foreign keys."));
        }
        db.execute_batch("COMMIT")?;
        Ok(json!({"spaceId":space,"schemaVersion":2,"formatVersion":2,"migrated":schema == 1}))
    })();
    if result.is_err() {
        let _ = db.execute_batch("ROLLBACK");
    }
    result
}
