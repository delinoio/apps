// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Server, audit, repository};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::Value;
use server_common::{Fault, Result, integer, now, string, uuid};
use sha2::Sha256;
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
            }
            None => tracing::debug!(
                retention_seconds = self.audit_retention.num_seconds(),
                "audit expiration skipped: retention predates every representable timestamp"
            ),
        }
        Ok(())
    }
    pub async fn webhook(&self, event_type: &str, signature: &str, raw: &[u8]) -> Result<()> {
        let signature = signature
            .strip_prefix("sha256=")
            .ok_or_else(Fault::unauthenticated)?;
        let signature = hex::decode(signature).map_err(|_| Fault::unauthenticated())?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.webhook_secret.as_bytes())
            .map_err(|_| Fault::unavailable())?;
        mac.update(raw);
        mac.verify_slice(&signature)
            .map_err(|_| Fault::unauthenticated())?;
        let event: Value = serde_json::from_slice(raw)?;
        let installation = integer(&event["installation"], "id").to_string();
        let github = integer(&event["repository"], "id");
        let action = string(&event, "action");
        let mut tx = self.db.begin().await?;
        match event_type {
            "installation" if matches!(action, "deleted" | "suspend") => {
                sqlx::query("UPDATE git_tracker_repositories SET connected=false,public=false WHERE installation_id=$1").bind(&installation).execute(&mut *tx).await?;
            }
            "installation_repositories" => {
                for removed in event["repositories_removed"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    sqlx::query("UPDATE git_tracker_repositories SET connected=false,public=false WHERE github_id=$1 AND installation_id=$2").bind(integer(removed,"id")).bind(&installation).execute(&mut *tx).await?;
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
        if matches!(
            event_type,
            "repository" | "installation" | "installation_repositories"
        ) {
            let spaces:Vec<uuid::Uuid>=sqlx::query_scalar("SELECT DISTINCT space_id FROM git_tracker_repositories WHERE ($1<>0 AND github_id=$1) OR ($2<>'0' AND installation_id=$2)").bind(github).bind(&installation).fetch_all(&mut *tx).await?;
            for space in spaces {
                audit(
                    &mut tx,
                    &space.to_string(),
                    "github_connection_event",
                    &server_common::id(),
                )
                .await?
            }
        }
        tx.commit().await?;
        Ok(())
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
}
