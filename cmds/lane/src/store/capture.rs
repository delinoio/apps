// SPDX-License-Identifier: Apache-2.0
use super::domain::{heads, mapped};
use super::{
    Connection, Failure, OptionalExtension, Result, Store, Value, invalid, json, params, rows,
    strv, valid_id,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::collections::HashSet;

#[expect(
    clippy::unused_self,
    reason = "Store operations use the same instance-method interface under the caller transaction."
)]
impl Store {
    // These minimal identities survive owner deletion; image bytes and browser
    // metadata do not. Replays must never reassign an attachment UUID.
    pub(crate) fn remember_capture_identity(
        c: &Connection,
        task: &str,
        capture: &Value,
    ) -> Result<()> {
        crate::capture::validate_reference(capture)?;
        if !valid_id(task) {
            return Err(invalid("Invalid attachment owner."));
        }
        let attachment = strv(capture, "attachmentId");
        let expected = (
            task.to_owned(),
            strv(capture, "sha256").to_owned(),
            capture["width"].as_i64().unwrap(),
            capture["height"].as_i64().unwrap(),
            strv(capture, "byteSize").parse::<i64>().unwrap(),
        );
        for table in ["attachment_identities", "attachments"] {
            let old: Option<(String, String, i64, i64, i64)> = c
                .query_row(
                    &format!(
                        "SELECT task_id,sha256,width,height,byte_size FROM {table} WHERE id=?"
                    ),
                    [attachment],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            if old.is_some_and(|old| old != expected) {
                return Err(invalid("Immutable attachment owner or content changed."));
            }
        }
        c.execute("INSERT OR IGNORE INTO attachment_identities(id,task_id,sha256,width,height,byte_size) VALUES(?,?,?,?,?,?)", params![attachment,expected.0,expected.1,expected.2,expected.3,expected.4])?;
        Ok(())
    }

    pub(super) fn insert_captures(
        &self,
        c: &Connection,
        task: &str,
        images: &Value,
    ) -> Result<Value> {
        let images = images
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| invalid("At least one final image is required."))?;
        c.execute("INSERT INTO entities(id,kind) VALUES(?,2)", [task])?;
        let mut references = Vec::new();
        for image in images {
            let capture = &image["capture"];
            let png = STANDARD
                .decode(strv(image, "png"))
                .map_err(|_| invalid("Invalid PNG encoding."))?;
            crate::capture::validate_png(capture, &png)?;
            Self::remember_capture_identity(c, task, capture)?;
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM attachments WHERE id=?)",
                [strv(capture, "attachmentId")],
                |r| r.get(0),
            )?;
            if exists {
                return Err(invalid("Attachment identity is already in use."));
            }
            c.execute("INSERT INTO attachments(id,task_id,sha256,width,height,byte_size,png_base64) VALUES(?,?,?,?,?,?,?)", params![strv(capture,"attachmentId"),task,strv(capture,"sha256"),capture["width"].as_i64(),capture["height"].as_i64(),i64::try_from(png.len()).map_err(|_| invalid("PNG size cannot be represented."))?,STANDARD.encode(png)])?;
            references.push(capture.clone());
        }
        Ok(json!(references))
    }
    pub(super) fn validate_capture_references(
        &self,
        c: &Connection,
        task: &str,
        captures: &Value,
    ) -> Result<()> {
        if captures.is_null() {
            return Ok(());
        }
        let captures = captures
            .as_array()
            .ok_or_else(|| invalid("Invalid capture references."))?;
        let mut seen = HashSet::new();
        for capture in captures {
            let id = strv(capture, "attachmentId");
            if !valid_id(id) || !seen.insert(id) {
                return Err(invalid("Capture identities must be unique UUID v7 values."));
            }
            let stored: Option<(String,i64,i64,i64)> = c.query_row("SELECT sha256,width,height,byte_size FROM attachments WHERE id=? AND task_id=?",params![id,task], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let Some((hash, width, height, size)) = stored else {
                return Err(Failure::new(
                    "NOT_FOUND",
                    "Attachment does not belong to this task.",
                ));
            };
            if hash != strv(capture, "sha256")
                || Some(width) != capture["width"].as_i64()
                || Some(height) != capture["height"].as_i64()
                || strv(capture, "byteSize").parse::<i64>().ok() != Some(size)
            {
                return Err(invalid("Immutable attachment content cannot change."));
            }
            Self::remember_capture_identity(c, task, capture)?;
        }
        Ok(())
    }
    pub(super) fn get_attachment(&self, c: &Connection, q: &Value) -> Result<Value> {
        let task = strv(q, "taskId");
        let h = heads(c, task)?;
        if h[0]["kind"] != "ENTITY_KIND_TASK" {
            return Err(Failure::new("NOT_FOUND", "Task does not exist."));
        }
        if h[0]["deleted"] == true {
            return Ok(json!({"state":"ATTACHMENT_STATE_DELETED"}));
        }
        let id = strv(q, "attachmentId");
        if !valid_id(id) {
            return Err(invalid("Invalid attachment identity."));
        }
        let row: Option<Option<String>> = c
            .query_row(
                "SELECT png_base64 FROM attachments WHERE id=? AND task_id=?",
                params![id, task],
                |r| r.get(0),
            )
            .optional()?;
        match row {
            Some(Some(png)) => Ok(json!({"state":"ATTACHMENT_STATE_AVAILABLE","png":png})),
            Some(None) => Ok(json!({"state":"ATTACHMENT_STATE_WAITING_FOR_SYNC"})),
            None => Err(Failure::new(
                "NOT_FOUND",
                "Attachment does not belong to this task.",
            )),
        }
    }
    pub(super) fn validate_connection(
        &self,
        c: &Connection,
        id: &str,
        connection: &Value,
    ) -> Result<()> {
        let prefix = strv(connection, "prefix");
        if crate::capture::normalize_prefix(prefix)? != prefix {
            return Err(invalid("URL connection prefix is not normalized."));
        }
        let repository = mapped(c, strv(connection, "repositoryId"))?;
        let h = heads(c, &repository)?;
        if h[0]["kind"] != "ENTITY_KIND_REPOSITORY" || h[0]["deleted"] == true {
            return Err(Failure::new("DELETED", "Select an existing repository."));
        }
        if h.len() != 1 {
            return Err(Failure::new(
                "CONFLICT_READ_ONLY",
                "Resolve the repository conflict first.",
            ));
        }
        for raw in rows(
            c,
            "SELECT v.envelope_json FROM heads h JOIN versions v ON v.version_id=h.version_id JOIN entities e ON e.id=h.entity_id WHERE e.kind=3 AND e.deleted=0 AND e.id<>?",
            [id],
        )? {
            let other: Value = serde_json::from_str(&raw)?;
            if other["urlConnection"]["prefix"] == prefix {
                return Err(invalid("This URL prefix already has a connection."));
            }
        }
        Ok(())
    }
}
