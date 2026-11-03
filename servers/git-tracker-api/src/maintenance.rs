// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Server, audit, repository};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::Value;
use server_common::{Fault, Result, integer, now, string, uuid};
use sha2::Sha256;
use sqlx::Row;
/// Returns the instant before which audit rows have expired, or `None` when `retention` reaches
/// back past the earliest representable timestamp. In that case no application-created audit row
/// can be older than the cutoff, so there is nothing to expire. Never substitute a sentinel or a
/// clamped value: the cutoff must stay the exact `now - retention` or be absent.
pub fn audit_cutoff(now: DateTime<Utc>, retention: chrono::Duration) -> Option<DateTime<Utc>> {
    now.checked_sub_signed(retention)
}
impl Server {
    pub async fn maintain(&self) -> Result<()> {
        self.maintain_at(now()).await
    }
    /// Runs one maintenance cycle as of `now`. Production uses [`Server::maintain`]; tests pass a
    /// fixed instant to exercise the audit retention boundary deterministically.
    pub async fn maintain_at(&self, now: DateTime<Utc>) -> Result<()> {
        // Stored webhook events first: a disconnect must not wait behind the slower synchronization.
        self.apply_webhooks().await?;
        let spaces: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT space_id FROM git_tracker_repositories ORDER BY space_id",
        )
        .fetch_all(&self.db)
        .await?;
        for batch in spaces.chunks(100) {
            let ids: Vec<_> = batch.iter().map(ToString::to_string).collect();
            let deleted = match self.authority.finalized(&ids).await {
                Ok(v) => v,
                Err(_) => {
                    tracing::warn!(category = "authorization", "tenant lifecycle unavailable");
                    continue;
                }
            };
            let deleted: Vec<_> = deleted.iter().map(|id| uuid(id)).collect::<Result<_>>()?;
            let mut tx = self.db.begin().await?;
            for table in [
                "git_tracker_repositories",
                "git_tracker_audit",
                "git_tracker_links",
            ] {
                sqlx::query(&format!("DELETE FROM {table} WHERE space_id=ANY($1)"))
                    .bind(&deleted)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        let ids: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT id FROM git_tracker_repositories ORDER BY id")
                .fetch_all(&self.db)
                .await?;
        for id in ids {
            let mut tx = self.db.begin().await?;
            let (repo, mut state) = repository(&mut tx, &id.to_string()).await?;
            if repo.connected {
                match self.github.metadata(&repo).await {
                    Ok(v) if integer(&v, "id") != 0 => {
                        let public = v["private"] == false;
                        let name = string(&v, "full_name");
                        if repo.public != public || repo.full_name != name {
                            sqlx::query("UPDATE git_tracker_repositories SET public=$2,full_name=$3 WHERE id=$1").bind(id).bind(public).bind(name).execute(&mut *tx).await?;
                            audit(
                                &mut tx,
                                &repo.space_id,
                                "repository_metadata_changed",
                                &server_common::id(),
                            )
                            .await?;
                        }
                    }
                    other => {
                        match other {
                            Err(fault) => self.record_failure(&fault),
                            Ok(_) => self
                                .metrics
                                .events
                                .with_label_values(&["github", "failure"])
                                .inc(),
                        }
                        tracing::warn!(category = "github", "repository synchronization failed");
                    }
                }
            }
            let count = state.interrupt(now);
            if count > 0 {
                sqlx::query("UPDATE git_tracker_repositories SET state=$2 WHERE id=$1")
                    .bind(id)
                    .bind(serde_json::to_value(state)?)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            if count > 0 {
                self.metrics
                    .events
                    .with_label_values(&["execution", "interrupted"])
                    .inc_by(count as u64);
                tracing::info!(count, "executions interrupted");
            }
        }
        match audit_cutoff(now, self.audit_retention) {
            Some(cutoff) => {
                sqlx::query("DELETE FROM git_tracker_audit WHERE created_at<$1")
                    .bind(cutoff)
                    .execute(&self.db)
                    .await?;
                // Only applied events expire; a pending event is retried until it applies. After
                // expiry a very late redelivery of the same delivery ID would apply again.
                sqlx::query(
                    "DELETE FROM git_tracker_webhook_events WHERE applied_at IS NOT NULL AND applied_at<$1",
                )
                .bind(cutoff)
                .execute(&self.db)
                .await?;
            }
            None => tracing::debug!(
                retention_seconds = self.audit_retention.num_seconds(),
                "audit expiration skipped: retention predates every representable timestamp"
            ),
        }
        Ok(())
    }
    /// Verifies a GitHub delivery and stores it durably, then returns. It takes no repository lock
    /// and makes no GitHub call, so the response stays within GitHub's delivery deadline however
    /// long uploads, reads or maintenance hold repository locks. [`Server::apply_webhooks`]
    /// applies the stored event.
    ///
    /// `delivery_id` is the `X-GitHub-Delivery` header. A redelivery of a stored delivery is
    /// acknowledged and ignored. A verified delivery without that header is rejected as invalid
    /// and stores nothing, because the identifier is what makes redelivery idempotent.
    pub async fn webhook(
        &self,
        event_type: &str,
        delivery_id: &str,
        signature: &str,
        raw: &[u8],
    ) -> Result<()> {
        let signature = signature
            .strip_prefix("sha256=")
            .ok_or_else(Fault::unauthenticated)?;
        let signature = hex::decode(signature).map_err(|_| Fault::unauthenticated())?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.webhook_secret.as_bytes())
            .map_err(|_| Fault::unavailable())?;
        mac.update(raw);
        mac.verify_slice(&signature)
            .map_err(|_| Fault::unauthenticated())?;
        if !valid_delivery_id(delivery_id) {
            return Err(Fault::invalid("X-GitHub-Delivery is missing or invalid"));
        }
        if !WebhookEvent::is_handled(event_type) {
            tracing::debug!(delivery_id, event_type, "webhook event type ignored");
            return Ok(());
        }
        let event = WebhookEvent::parse(event_type, raw)?;
        let stored = sqlx::query(
            "INSERT INTO git_tracker_webhook_events(id,delivery_id,event_type,action,installation_id,github_id,removed_ids,received_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (delivery_id) DO NOTHING",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(delivery_id)
        .bind(&event.event_type)
        .bind(&event.action)
        .bind(&event.installation_id)
        .bind(event.github_id)
        .bind(&event.removed_ids)
        .bind(now())
        .execute(&self.db)
        .await?
        .rows_affected();
        tracing::info!(
            delivery_id,
            event_type,
            action = event.action,
            duplicate = stored == 0,
            "webhook stored"
        );
        Ok(())
    }
    /// Applies stored webhook events that are still pending, oldest receipt first, and returns the
    /// number applied. Each event runs in its own transaction with a bounded lock wait, and
    /// `applied_at` commits with the event's effect, so an event takes effect at most once per
    /// commit and a crash or failure leaves it pending for the next call. A failed event is
    /// counted in the failure metric and retried on the next call; the remaining events still
    /// run, which is safe because every effect either sets a disconnected state or re-reads
    /// current GitHub metadata. Concurrent callers, including other replicas, never apply the same
    /// event twice.
    pub async fn apply_webhooks(&self) -> Result<usize> {
        let mut applied = 0;
        let mut after: Option<(DateTime<Utc>, uuid::Uuid)> = None;
        loop {
            let page: Vec<(uuid::Uuid, DateTime<Utc>)> = match after {
                None => sqlx::query_as(
                    "SELECT id,received_at FROM git_tracker_webhook_events WHERE applied_at IS NULL ORDER BY received_at,id LIMIT 100",
                )
                .fetch_all(&self.db)
                .await?,
                Some((at, id)) => sqlx::query_as(
                    "SELECT id,received_at FROM git_tracker_webhook_events WHERE applied_at IS NULL AND (received_at,id)>($1,$2) ORDER BY received_at,id LIMIT 100",
                )
                .bind(at)
                .bind(id)
                .fetch_all(&self.db)
                .await?,
            };
            let Some(&(last_id, last_at)) = page.last() else {
                return Ok(applied);
            };
            for (id, _) in page {
                match self.apply_webhook(id).await {
                    Ok(true) => applied += 1,
                    Ok(false) => {}
                    Err(fault) => {
                        self.record_failure(&fault);
                        tracing::warn!(
                            category = "webhook",
                            event_id = %id,
                            "webhook event stays pending"
                        );
                    }
                }
            }
            after = Some((last_at, last_id));
        }
    }
    /// Applies one pending event. It returns `false` when another caller already applied or holds
    /// the event.
    async fn apply_webhook(&self, id: uuid::Uuid) -> Result<bool> {
        let mut tx = self.db.begin().await?;
        sqlx::query("SELECT set_config('lock_timeout',$1,true)")
            .bind(format!("{}ms", WEBHOOK_LOCK_TIMEOUT.as_millis()))
            .execute(&mut *tx)
            .await?;
        let Some(row) = sqlx::query(
            "SELECT delivery_id,event_type,action,installation_id,github_id,removed_ids FROM git_tracker_webhook_events WHERE id=$1 AND applied_at IS NULL FOR UPDATE SKIP LOCKED",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(false);
        };
        let delivery_id: String = row.get("delivery_id");
        let event = WebhookEvent {
            event_type: row.get("event_type"),
            action: row.get("action"),
            installation_id: row.get("installation_id"),
            github_id: row.get("github_id"),
            removed_ids: row.get("removed_ids"),
        };
        let (installation, github, action) = (
            &event.installation_id,
            event.github_id,
            event.action.as_str(),
        );
        match event.event_type.as_str() {
            "installation" if matches!(action, "deleted" | "suspend") => {
                sqlx::query("UPDATE git_tracker_repositories SET connected=false,public=false WHERE installation_id=$1").bind(installation).execute(&mut *tx).await?;
            }
            "installation_repositories" => {
                for removed in &event.removed_ids {
                    sqlx::query("UPDATE git_tracker_repositories SET connected=false,public=false WHERE github_id=$1 AND installation_id=$2").bind(removed).bind(installation).execute(&mut *tx).await?;
                }
            }
            "repository" if action == "deleted" => {
                sqlx::query("UPDATE git_tracker_repositories SET connected=false,public=false WHERE github_id=$1").bind(github).execute(&mut *tx).await?;
            }
            "repository" => {
                if let Some(row)=sqlx::query("SELECT * FROM git_tracker_repositories WHERE github_id=$1 AND connected FOR UPDATE").bind(github).fetch_optional(&mut *tx).await?{
                // Fetch current metadata rather than apply potentially reordered webhook payloads.
                let repo=crate::repository_row(&row);let v=self.github.metadata(&repo).await?;
                sqlx::query("UPDATE git_tracker_repositories SET public=$2,full_name=$3 WHERE id=$1").bind(uuid(&repo.id)?).bind(v["private"]==false).bind(string(&v,"full_name")).execute(&mut *tx).await?;
            }
            }
            _ => {}
        }
        let spaces:Vec<uuid::Uuid>=sqlx::query_scalar("SELECT DISTINCT space_id FROM git_tracker_repositories WHERE ($1<>0 AND github_id=$1) OR ($2<>'0' AND installation_id=$2)").bind(github).bind(installation).fetch_all(&mut *tx).await?;
        for space in spaces {
            audit(
                &mut tx,
                &space.to_string(),
                "github_connection_event",
                &server_common::id(),
            )
            .await?
        }
        sqlx::query("UPDATE git_tracker_webhook_events SET applied_at=$2 WHERE id=$1")
            .bind(id)
            .bind(now())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(
            delivery_id,
            event_type = event.event_type,
            action = event.action,
            "webhook applied"
        );
        Ok(true)
    }
}
/// Longest wait for a row lock while applying one webhook event.
const WEBHOOK_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Reports whether `id` can identify a delivery: non-empty printable ASCII of bounded length.
/// GitHub sends a GUID.
fn valid_delivery_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_graphic())
}
/// The fields of a verified delivery that its effect needs.
#[derive(Debug, PartialEq, Eq)]
pub struct WebhookEvent {
    pub event_type: String,
    pub action: String,
    pub installation_id: String,
    pub github_id: i64,
    pub removed_ids: Vec<i64>,
}
impl WebhookEvent {
    /// Event types that change connection state.
    pub fn is_handled(event_type: &str) -> bool {
        matches!(
            event_type,
            "repository" | "installation" | "installation_repositories"
        )
    }
    pub fn parse(event_type: &str, raw: &[u8]) -> Result<Self> {
        let event: Value = serde_json::from_slice(raw)?;
        Ok(Self {
            event_type: event_type.to_owned(),
            action: string(&event, "action").to_owned(),
            installation_id: integer(&event["installation"], "id").to_string(),
            github_id: integer(&event["repository"], "id"),
            removed_ids: event["repositories_removed"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|removed| integer(removed, "id"))
                .collect(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().to_utc()
    }

    #[test]
    fn huge_accepted_retention_parses_and_converts() {
        // SAFETY: the key is read only by this test.
        unsafe { std::env::set_var("GIT_TRACKER_TEST_HUGE_RETENTION", "9999999999h") };
        let parsed = server_common::config::duration(
            "GIT_TRACKER_TEST_HUGE_RETENTION",
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(parsed.as_secs(), 35_999_999_996_400);
        assert_eq!(parsed.subsec_nanos(), 0);
        let retention = chrono::Duration::from_std(parsed).unwrap();
        assert_eq!(retention.num_seconds(), 35_999_999_996_400);
    }

    #[test]
    fn two_thousand_one_hundred_sixty_hours_is_ninety_days() {
        let retention = chrono::Duration::from_std(Duration::from_secs(2160 * 3600)).unwrap();
        assert_eq!(retention, chrono::Duration::days(90));
        assert_eq!(retention.num_seconds(), 7_776_000);
        assert_eq!(
            audit_cutoff(at("2026-10-22T00:00:00Z"), retention),
            Some(at("2026-07-24T00:00:00Z"))
        );
    }

    #[test]
    fn cutoff_is_absent_only_below_the_minimum_timestamp() {
        let now = at("2026-10-22T00:00:00Z");
        let exact = now - DateTime::<Utc>::MIN_UTC;
        assert_eq!(audit_cutoff(now, exact), Some(DateTime::<Utc>::MIN_UTC));
        assert_eq!(
            audit_cutoff(now, exact + chrono::Duration::microseconds(1)),
            None
        );
        let huge = chrono::Duration::from_std(Duration::from_secs(35_999_999_996_400)).unwrap();
        assert_eq!(audit_cutoff(now, huge), None);
    }

    #[test]
    fn webhook_payload_is_reduced_to_the_fields_its_effect_needs() {
        let raw = br#"{"action":"removed","installation":{"id":7},"repository":{"id":42},
            "repositories_removed":[{"id":1},{"id":"2"},{"name":"no-id"}],"sender":{"login":"x"}}"#;
        assert_eq!(
            WebhookEvent::parse("installation_repositories", raw).unwrap(),
            WebhookEvent {
                event_type: "installation_repositories".into(),
                action: "removed".into(),
                installation_id: "7".into(),
                github_id: 42,
                removed_ids: vec![1, 2, 0],
            }
        );
        let bare = WebhookEvent::parse("installation", br#"{"action":"deleted"}"#).unwrap();
        assert_eq!((bare.installation_id.as_str(), bare.github_id), ("0", 0));
        assert!(WebhookEvent::parse("installation", b"not json").is_err());
    }

    #[test]
    fn only_connection_events_are_stored() {
        for handled in ["repository", "installation", "installation_repositories"] {
            assert!(WebhookEvent::is_handled(handled));
        }
        for ignored in ["ping", "push", "installation_target", ""] {
            assert!(!WebhookEvent::is_handled(ignored));
        }
    }

    #[test]
    fn delivery_ids_are_bounded_printable_ascii() {
        assert!(valid_delivery_id("72d3162e-cc78-11e3-81ab-4c9367dc0958"));
        assert!(!valid_delivery_id(""));
        assert!(!valid_delivery_id("has space"));
        assert!(!valid_delivery_id(&"a".repeat(129)));
        assert!(valid_delivery_id(&"a".repeat(128)));
    }
}
