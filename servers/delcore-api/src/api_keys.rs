// SPDX-License-Identifier: FSL-1.1-ALv2
//! Service-neutral API key primitives: the secret format, hashing, constant-time
//! verification and request shape validation. Nothing here knows about any
//! particular consumer service, capability, or product.
use crate::{Fault, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Every secret starts with this fixed marker so consumers can route on it.
pub const SECRET_PREFIX: &str = "dlk_";
const RANDOM_BYTES: usize = 32;
const KEY_ID_HEX: usize = 32;
const RANDOM_TEXT: usize = 43;
/// Total length of a well-formed secret: `dlk_` + 32 hex + `_` + 43 base64url.
pub const SECRET_LENGTH: usize = SECRET_PREFIX.len() + KEY_ID_HEX + 1 + RANDOM_TEXT;

pub const MAX_NAME_BYTES: usize = 100;
pub const MAX_GRANTS: usize = 16;
pub const MAX_CAPABILITIES: usize = 32;
pub const MAX_CAPABILITY_BYTES: usize = 100;
/// Bound on keys that are still usable per owner scope.
pub const MAX_ACTIVE_KEYS: i64 = 100;
/// Bound on key rows of any state per owner scope. Revoked and expired keys stay
/// for the transient retention period, so rapid create-and-revoke churn cannot
/// grow the table without limit.
pub const MAX_RETAINED_KEYS: i64 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerKind {
    Personal = 1,
    Tenant = 2,
}
impl OwnerKind {
    pub fn from_request(value: &Value) -> Result<Self> {
        match value.as_str() {
            Some("API_KEY_OWNER_KIND_PERSONAL") => Ok(Self::Personal),
            Some("API_KEY_OWNER_KIND_TENANT") => Ok(Self::Tenant),
            _ => match value.as_i64() {
                Some(1) => Ok(Self::Personal),
                Some(2) => Ok(Self::Tenant),
                _ => Err(Fault::invalid("owner kind must be personal or tenant")),
            },
        }
    }
    pub fn from_db(value: i32) -> Result<Self> {
        match value {
            1 => Ok(Self::Personal),
            2 => Ok(Self::Tenant),
            _ => Err(Fault::unavailable()),
        }
    }
}

/// A freshly generated secret. It leaves this value exactly once, in the
/// creation response. `Debug` is redacted so it cannot reach logs.
pub struct GeneratedSecret {
    pub key_id: Uuid,
    pub secret: String,
    pub hash: [u8; 32],
}
impl std::fmt::Debug for GeneratedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedSecret")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

/// A syntactically valid presented secret.
pub struct ParsedSecret {
    pub key_id: Uuid,
    pub hash: [u8; 32],
}
impl std::fmt::Debug for ParsedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedSecret").finish_non_exhaustive()
    }
}

fn hash_random(random: &[u8]) -> [u8; 32] {
    Sha256::digest(random).into()
}

/// Generates a secret for `key_id` from the operating system backed generator.
pub fn generate(key_id: Uuid) -> GeneratedSecret {
    let mut random = [0u8; RANDOM_BYTES];
    rand::rng().fill_bytes(&mut random);
    GeneratedSecret {
        key_id,
        secret: format!(
            "{SECRET_PREFIX}{}_{}",
            key_id.simple(),
            URL_SAFE_NO_PAD.encode(random)
        ),
        hash: hash_random(&random),
    }
}

/// Parses a presented secret. Any deviation from the canonical format, including
/// non-canonical base64 and non-v7 key IDs, is reported as `None` so callers
/// answer malformed and unknown secrets identically.
pub fn parse(secret: &str) -> Option<ParsedSecret> {
    if secret.len() != SECRET_LENGTH || !secret.is_ascii() {
        return None;
    }
    let rest = secret.strip_prefix(SECRET_PREFIX)?;
    let (id, random) = rest.split_once('_')?;
    if id.len() != KEY_ID_HEX
        || random.len() != RANDOM_TEXT
        || !id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let key_id = Uuid::parse_str(id).ok()?;
    if key_id.get_version_num() != 7 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(random).ok()?;
    // Reject encodings that alias the same bytes (non-zero trailing bits).
    if bytes.len() != RANDOM_BYTES || URL_SAFE_NO_PAD.encode(&bytes) != random {
        return None;
    }
    Some(ParsedSecret {
        key_id,
        hash: hash_random(&bytes),
    })
}

/// Compares two digests without early exit, so timing does not reveal how many
/// leading bytes of a guessed secret matched.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// A validated grant request: one service with a sorted, unique capability list.
#[derive(Debug, PartialEq, Eq)]
pub struct GrantRequest {
    pub service_id: Uuid,
    pub capabilities: Vec<String>,
}

fn capability_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CAPABILITY_BYTES
        && value.chars().all(|c| !c.is_control())
}

/// Validates the shape of requested grants. Catalog membership is checked by the
/// caller against the registered services.
pub fn parse_grants(value: &Value) -> Result<Vec<GrantRequest>> {
    let list = value
        .as_array()
        .filter(|list| !list.is_empty() && list.len() <= MAX_GRANTS)
        .ok_or_else(|| Fault::invalid(format!("supply 1 to {MAX_GRANTS} grants")))?;
    let mut services = BTreeSet::new();
    let mut out = Vec::new();
    for grant in list {
        let service = grant["serviceId"].as_str().unwrap_or("");
        if !crate::rpc::canonical(service) {
            return Err(Fault::invalid("grant service ID must be a UUID v7"));
        }
        let service_id = Uuid::parse_str(service).map_err(|_| Fault::invalid("invalid UUID"))?;
        if !services.insert(service_id) {
            return Err(Fault::invalid("duplicate grant service"));
        }
        let names = grant["capabilities"]
            .as_array()
            .filter(|names| !names.is_empty() && names.len() <= MAX_CAPABILITIES)
            .ok_or_else(|| {
                Fault::invalid(format!(
                    "supply 1 to {MAX_CAPABILITIES} capabilities per grant"
                ))
            })?;
        let mut capabilities = BTreeSet::new();
        for name in names {
            let name = name.as_str().unwrap_or("");
            if !capability_text(name) {
                return Err(Fault::invalid("invalid grant capability"));
            }
            if !capabilities.insert(name.to_owned()) {
                return Err(Fault::invalid("duplicate grant capability"));
            }
        }
        out.push(GrantRequest {
            service_id,
            capabilities: capabilities.into_iter().collect(),
        });
    }
    Ok(out)
}

/// Validates a key display name.
pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(|c| c.is_control())
    {
        return Err(Fault::invalid(format!(
            "name must contain 1 to {MAX_NAME_BYTES} bytes without control characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id() -> Uuid {
        Uuid::now_v7()
    }

    #[test]
    fn generated_secret_has_the_documented_shape_and_parses_back() {
        let key = id();
        let generated = generate(key);
        assert_eq!(generated.secret.len(), SECRET_LENGTH);
        assert!(generated.secret.starts_with("dlk_"));
        assert!(generated.secret.contains(&key.simple().to_string()));
        let parsed = parse(&generated.secret).unwrap();
        assert_eq!(parsed.key_id, key);
        assert_eq!(parsed.hash, generated.hash);
        assert_ne!(generate(key).secret, generated.secret);
    }

    #[test]
    fn debug_output_never_contains_the_secret() {
        let generated = generate(id());
        let text = format!("{generated:?}");
        let random = &generated.secret[generated.secret.len() - 43..];
        assert!(!text.contains(random));
        assert!(!format!("{:?}", parse(&generated.secret).unwrap()).contains(random));
    }

    #[test]
    fn malformed_secrets_are_rejected() {
        let good = generate(id()).secret;
        assert!(parse(&good).is_some());
        for bad in [
            String::new(),
            "dlk_".into(),
            good.replacen("dlk_", "dlx_", 1),
            good[..good.len() - 1].to_owned(),
            format!("{good}A"),
            good.replacen('_', "-", 2),
            good.to_uppercase(),
            // Non-hex and non-v7 key IDs.
            format!("dlk_{}_{}", "g".repeat(32), &good[good.len() - 43..]),
            format!(
                "dlk_00000000000040008000000000000000_{}",
                &good[good.len() - 43..]
            ),
            // Characters outside the base64url alphabet.
            format!("{}+", &good[..good.len() - 1]),
            format!("{}é", &good[..good.len() - 2]),
        ] {
            assert!(parse(&bad).is_none(), "accepted {bad}");
        }
    }

    #[test]
    fn non_canonical_base64_cannot_alias_a_secret() {
        let good = generate(id()).secret;
        // The last character carries four data bits and two trailing zero bits.
        // Only the 16 canonical characters may verify, each with distinct bytes;
        // the 48 aliases of those bytes must be rejected.
        let head = &good[..good.len() - 1];
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let hashes: BTreeSet<[u8; 32]> = alphabet
            .chars()
            .filter_map(|c| parse(&format!("{head}{c}")))
            .map(|parsed| parsed.hash)
            .collect();
        let accepted = alphabet
            .chars()
            .filter(|c| parse(&format!("{head}{c}")).is_some())
            .count();
        assert_eq!(accepted, 16);
        assert_eq!(hashes.len(), accepted);
    }

    #[test]
    fn hash_only_covers_the_random_part() {
        let key = id();
        let generated = generate(key);
        let random = URL_SAFE_NO_PAD
            .decode(&generated.secret[generated.secret.len() - 43..])
            .unwrap();
        assert_eq!(generated.hash, <[u8; 32]>::from(Sha256::digest(&random)));
    }

    #[test]
    fn constant_time_comparison_matches_equality() {
        let a = [7u8; 32];
        let mut b = a;
        assert!(constant_time_eq(&a, &b));
        b[31] ^= 1;
        assert!(!constant_time_eq(&a, &b));
        b = a;
        b[0] ^= 0x80;
        assert!(!constant_time_eq(&a, &b));
        assert!(!constant_time_eq(&a, &a[..31]));
    }

    #[test]
    fn owner_kind_accepts_names_and_numbers_only() {
        assert_eq!(
            OwnerKind::from_request(&json!("API_KEY_OWNER_KIND_PERSONAL")).unwrap(),
            OwnerKind::Personal
        );
        assert_eq!(
            OwnerKind::from_request(&json!(2)).unwrap(),
            OwnerKind::Tenant
        );
        for bad in [
            json!("API_KEY_OWNER_KIND_UNSPECIFIED"),
            json!(0),
            json!(3),
            json!(null),
            json!("tenant"),
        ] {
            assert!(OwnerKind::from_request(&bad).is_err());
        }
    }

    #[test]
    fn grants_are_normalized_and_bounded() {
        let service = id().to_string();
        let grants = parse_grants(&json!([
            {"serviceId":service,"capabilities":["b","a"]}
        ]))
        .unwrap();
        assert_eq!(grants[0].capabilities, ["a", "b"]);
        let other = id().to_string();
        assert_eq!(
            parse_grants(&json!([
                {"serviceId":service,"capabilities":["a"]},
                {"serviceId":other,"capabilities":["a"]}
            ]))
            .unwrap()
            .len(),
            2
        );
        let many: Vec<_> = (0..=MAX_GRANTS)
            .map(|_| json!({"serviceId":id().to_string(),"capabilities":["a"]}))
            .collect();
        let too_many_caps: Vec<_> = (0..=MAX_CAPABILITIES).map(|i| format!("c{i}")).collect();
        for bad in [
            json!(null),
            json!([]),
            json!({}),
            json!(many),
            json!([{"serviceId":"nope","capabilities":["a"]}]),
            json!([{"serviceId":"00000000-0000-0000-0000-000000000000","capabilities":["a"]}]),
            json!([{"serviceId":service,"capabilities":[]}]),
            json!([{"serviceId":service}]),
            json!([{"serviceId":service,"capabilities":[""]}]),
            json!([{"serviceId":service,"capabilities":["a","a"]}]),
            json!([{"serviceId":service,"capabilities":[1]}]),
            json!([{"serviceId":service,"capabilities":["bad\ncap"]}]),
            json!([{"serviceId":service,"capabilities":["x".repeat(MAX_CAPABILITY_BYTES + 1)]}]),
            json!([{"serviceId":service,"capabilities":too_many_caps}]),
            json!([
                {"serviceId":service,"capabilities":["a"]},
                {"serviceId":service,"capabilities":["b"]}
            ]),
        ] {
            assert!(parse_grants(&bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn names_are_bounded_text() {
        assert!(validate_name("CI key").is_ok());
        assert!(validate_name(&"é".repeat(50)).is_ok());
        for bad in ["", "   ", "tab\there", &"x".repeat(MAX_NAME_BYTES + 1)] {
            assert!(validate_name(bad).is_err());
        }
    }
}
