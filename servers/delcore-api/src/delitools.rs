// SPDX-License-Identifier: FSL-1.1-ALv2

//! Delcore support for the DeliTools consumer service. The service registered
//! with the reserved catalog key below rates AI usage at call time, may use the
//! tenant-key authorization modes and requires the long reporting windows. Other
//! services keep their subscription behavior unchanged.
use crate::{Result, config::Config};
use sqlx::PgConnection;

/// Reserved stable catalog key and display name of the DeliTools service. The
/// key is bound to the service ID when the service is first registered.
pub const SERVICE_KEY: &str = "delitools";
/// Smallest reporting window, in seconds, delcore accepts while serving DeliTools:
/// DeliTools keeps unconfirmed usage reports for 30 days.
pub const MIN_REPORTING_WINDOW_SECONDS: i64 = 30 * 24 * 60 * 60;

/// Why a service registration touches the reserved DeliTools identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityViolation {
    /// The reserved key or name would be renamed or moved to another service.
    Immutable,
}

/// Check a service registration against the reserved identity. `existing_key` is
/// the stable key already bound to the supplied service ID, if any; `key` is the
/// key the registration resolves to. The reserved key always belongs to the
/// service named like it; no other service may take either the key or the name,
/// and the bound key cannot change.
pub fn check_service_identity(
    existing_key: Option<&str>,
    key: &str,
    name: &str,
) -> std::result::Result<(), IdentityViolation> {
    let reserved = key == SERVICE_KEY || existing_key == Some(SERVICE_KEY);
    if reserved {
        let stable = key == SERVICE_KEY && existing_key.is_none_or(|k| k == SERVICE_KEY);
        return if stable && name == SERVICE_KEY {
            Ok(())
        } else {
            Err(IdentityViolation::Immutable)
        };
    }
    if name.trim().eq_ignore_ascii_case(SERVICE_KEY) {
        return Err(IdentityViolation::Immutable);
    }
    Ok(())
}

/// Whether the service is the registered DeliTools service.
pub async fn is_delitools(db: &mut PgConnection, service_id: &str) -> Result<bool> {
    if service_id.is_empty() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM catalog_entities WHERE id=$1::text::uuid AND kind='service' AND scope='' AND key=$2)",
    )
    .bind(service_id)
    .bind(SERVICE_KEY)
    .fetch_one(db)
    .await?)
}

/// Whether a DeliTools service is registered in the database.
pub async fn is_registered(db: &mut PgConnection) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM catalog_entities WHERE kind='service' AND scope='' AND key=$1)",
    )
    .bind(SERVICE_KEY)
    .fetch_one(db)
    .await?)
}

/// Startup check: a registered DeliTools service requires the explicit switch,
/// whose configuration validation enforces the 30-day reporting windows. The
/// error carries no configuration values.
pub async fn ensure_configured(db: &mut PgConnection, config: &Config) -> anyhow::Result<()> {
    if !config.delitools_enabled && is_registered(db).await? {
        anyhow::bail!(
            "the DeliTools service is registered; set DELITOOLS_ENABLED=true with 30-day reporting windows"
        );
    }
    Ok(())
}

/// Whether a stored meter definition is priced at call time.
pub async fn meter_is_call_time(db: &mut PgConnection, meter_id: &str) -> Result<bool> {
    if meter_id.is_empty() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar::<_, Option<bool>>(
        "SELECT (cv.definition->>'value_source')::int=$2 FROM catalog_entities ce JOIN catalog_versions cv ON cv.id=ce.active_version_id WHERE ce.id=$1::text::uuid AND ce.kind='meter'",
    )
    .bind(meter_id)
    .bind(crate::events::VALUE_SOURCE_AI_TOKEN_USD)
    .fetch_optional(db)
    .await?
    .flatten()
    .unwrap_or(false))
}

/// Whether the service's meter with this stable key is priced at call time.
pub async fn meter_key_is_call_time(
    db: &mut PgConnection,
    service_id: &str,
    meter_key: &str,
) -> Result<bool> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT id::text FROM catalog_entities WHERE kind='meter' AND scope=$1 AND key=$2",
    )
    .bind(service_id)
    .bind(meter_key)
    .fetch_optional(&mut *db)
    .await?;
    match id {
        Some(id) => meter_is_call_time(db, &id).await,
        None => Ok(false),
    }
}

/// Whether any usage price version of the service names this meter key. Call-time
/// meters never carry usage prices, so such a meter cannot become call-time.
pub async fn meter_has_usage_prices(
    db: &mut PgConnection,
    service_id: &str,
    meter_key: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM prices WHERE service_id=$1::text::uuid AND meter=$2 AND kind=1)",
    )
    .bind(service_id)
    .bind(meter_key)
    .fetch_one(db)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserved_identity_is_bound_to_its_key_and_name() {
        // First registration and unchanged updates.
        assert!(check_service_identity(None, "delitools", "delitools").is_ok());
        assert!(check_service_identity(Some("delitools"), "delitools", "delitools").is_ok());
        // Renaming the reserved service, in any direction.
        for (existing, key, name) in [
            (Some("delitools"), "delitools", "DeliTools"),
            (Some("delitools"), "delitools", "other"),
            (Some("delitools"), "other", "delitools"),
            (Some("delitools"), "other", "other"),
            (None, "delitools", "other"),
        ] {
            assert_eq!(
                check_service_identity(existing, key, name),
                Err(IdentityViolation::Immutable),
                "{existing:?} {key} {name}"
            );
        }
        // Another service cannot move into the key or the name.
        for (existing, key, name) in [
            (Some("search"), "delitools", "delitools"),
            (Some("search"), "search", "delitools"),
            (Some("search"), "search", " DeliTools "),
            (None, "search", "delitools"),
        ] {
            assert_eq!(
                check_service_identity(existing, key, name),
                Err(IdentityViolation::Immutable),
                "{existing:?} {key} {name}"
            );
        }
        // Unrelated services are unaffected.
        assert!(check_service_identity(None, "search", "Search").is_ok());
        assert!(check_service_identity(Some("search"), "search", "Search v2").is_ok());
    }
}
