// SPDX-License-Identifier: FSL-1.1-ALv2
pub mod keys;
pub mod operator;

use crate::error::{Fault, Result};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Tenant IDs are canonical UUID v7 strings issued by delcore.
pub fn canonical_tenant(value: &str) -> Result<Uuid> {
    if !server_common::canonical_id(value) {
        return Err(Fault::invalid_input("tenantId must be a UUID v7"));
    }
    Ok(Uuid::parse_str(value).expect("canonical"))
}

/// Current time at the PostgreSQL timestamp resolution, so stored values compare exactly.
pub fn now_checked() -> DateTime<Utc> {
    server_common::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_ids_must_be_canonical_uuid_v7() {
        assert!(canonical_tenant(&Uuid::now_v7().to_string()).is_ok());
        for bad in [
            "",
            "not-a-uuid",
            "01900000-0000-4000-8000-000000000001",
            &Uuid::now_v7().simple().to_string(),
            &Uuid::now_v7().to_string().to_uppercase(),
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(canonical_tenant(bad).is_err(), "{bad}");
        }
    }
}
