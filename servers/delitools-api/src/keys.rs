// SPDX-License-Identifier: FSL-1.1-ALv2
//! DeliTools API key material and capability sets.
//!
//! A key is `dtk_<32 hex digits of the key UUID v7>_<43 URL-safe base64 characters of 32 random
//! bytes>`. The key UUID lets the server find the row; the random part is the secret. Only a
//! domain-separated SHA-256 verification hash is stored. The secret is 256 bits of CSPRNG output,
//! so a fast hash is sufficient; verification compares in constant time.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use buffa::Enumeration;
use chrono::{DateTime, Utc};
use delino_protocol::delitools::v1::{Capability, KeyKind};
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const KEY_PREFIX: &str = "dtk_";
const SECRET_BYTES: usize = 32;
const HASH_DOMAIN: &[u8] = b"delitools-api-key-v1\0";
/// Total token length: prefix, 32 hex digits, separator and 43 base64 characters.
pub const TOKEN_LEN: usize = 4 + 32 + 1 + 43;

pub struct GeneratedKey {
    pub id: Uuid,
    /// The complete bearer token. Returned to the caller once and never stored.
    pub token: String,
    pub hash: [u8; 32],
}

pub fn hash_secret(secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(HASH_DOMAIN);
    hasher.update(secret.as_bytes());
    hasher.finalize().into()
}

pub fn generate() -> GeneratedKey {
    let id = Uuid::now_v7();
    let mut bytes = [0_u8; SECRET_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    let secret = URL_SAFE_NO_PAD.encode(bytes);
    GeneratedKey {
        id,
        token: format!("{KEY_PREFIX}{}_{secret}", id.simple()),
        hash: hash_secret(&secret),
    }
}

/// Parse a presented token into the key ID and the hash of its secret part.
pub fn parse(token: &str) -> Option<(Uuid, [u8; 32])> {
    let rest = token.strip_prefix(KEY_PREFIX)?;
    if token.len() != TOKEN_LEN || !rest.is_ascii() || rest.as_bytes()[32] != b'_' {
        return None;
    }
    let hex = &rest[..32];
    if !hex
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let id = Uuid::parse_str(hex)
        .ok()
        .filter(|id| id.get_version_num() == 7)?;
    let secret = &rest[33..];
    if URL_SAFE_NO_PAD.decode(secret).ok()?.len() != SECRET_BYTES {
        return None;
    }
    Some((id, hash_secret(secret)))
}

/// Constant-time comparison of a stored verification hash with a presented one.
pub fn verify(stored: &[u8], presented: &[u8; 32]) -> bool {
    stored.len() == presented.len() && bool::from(stored.ct_eq(presented))
}

/// A key is usable until it is revoked or reaches its optional expiry instant.
pub fn is_usable(
    revoked_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    revoked_at.is_none() && expires_at.is_none_or(|expiry| expiry > now)
}

/// Capability set of a key or a user. Backed by a two-bit mask in declaration order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities(u8);

impl Capabilities {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(0b11);

    fn bit(capability: Capability) -> Option<u8> {
        match capability {
            Capability::CAPABILITY_TRANSLATOR => Some(0b01),
            Capability::CAPABILITY_EXTRACTOR => Some(0b10),
            Capability::CAPABILITY_UNSPECIFIED => None,
        }
    }
    pub fn of(capability: Capability) -> Self {
        Self(Self::bit(capability).unwrap_or(0))
    }
    /// Build a set from the wire list. Unspecified, unknown and duplicate values are rejected.
    pub fn from_wire(values: &[buffa::EnumValue<Capability>]) -> Option<Self> {
        let mut set = 0_u8;
        for value in values {
            let bit = Self::bit(value.as_known()?)?;
            if set & bit != 0 {
                return None;
            }
            set |= bit;
        }
        (set != 0).then_some(Self(set))
    }
    pub fn from_database(values: &[i16]) -> Self {
        let mut set = 0;
        for value in values {
            if let Some(bit) = Capability::from_i32(i32::from(*value)).and_then(Self::bit) {
                set |= bit;
            }
        }
        Self(set)
    }
    pub fn to_database(self) -> Vec<i16> {
        self.to_list()
            .into_iter()
            .map(|c| c as i32 as i16)
            .collect()
    }
    pub fn to_list(self) -> Vec<Capability> {
        [
            Capability::CAPABILITY_TRANSLATOR,
            Capability::CAPABILITY_EXTRACTOR,
        ]
        .into_iter()
        .filter(|c| self.contains(*c))
        .collect()
    }
    pub fn contains(self, capability: Capability) -> bool {
        Self::bit(capability).is_some_and(|bit| self.0 & bit != 0)
    }
    /// Personal-key permission: key capabilities intersected with what the user may currently do.
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

pub fn kind_to_db(kind: KeyKind) -> i16 {
    kind as i32 as i16
}
pub fn kind_from_db(value: i16) -> Option<KeyKind> {
    KeyKind::from_i32(i32::from(value)).filter(|k| *k != KeyKind::KEY_KIND_UNSPECIFIED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn generated_tokens_parse_and_verify() {
        let key = generate();
        assert_eq!(key.token.len(), TOKEN_LEN);
        let (id, hash) = parse(&key.token).expect("generated token parses");
        assert_eq!(id, key.id);
        assert_eq!(id.get_version_num(), 7);
        assert!(verify(&key.hash, &hash));
        let other = generate();
        assert!(!verify(&other.hash, &hash));
        assert_ne!(key.token, other.token);
    }

    #[test]
    fn hash_is_domain_separated_and_never_the_secret() {
        let key = generate();
        let secret = key.token.rsplit('_').next().unwrap();
        assert_ne!(key.hash.to_vec(), secret.as_bytes());
        let plain: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        assert_ne!(key.hash, plain);
        assert!(!verify(&key.hash[..31], &key.hash));
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        let key = generate();
        let t = &key.token;
        let upper = t.replacen(&t[4..8], &t[4..8].to_uppercase(), 1);
        let cases = [
            String::new(),
            "dtk_".into(),
            t.trim_start_matches("dtk_").into(),
            format!("{t}x"),
            t[..t.len() - 1].to_owned(),
            t.replacen('_', "-", 2),
            format!("dtk_{}", "0".repeat(TOKEN_LEN - 4)),
            t.replacen("dtk_", "dbk_", 1),
            // A version 4 UUID in the ID position.
            format!(
                "dtk_{}_{}",
                Uuid::from_u128(0x0190_0000_0000_4000_8000_0000_0000_0001).simple(),
                t.rsplit('_').next().unwrap()
            ),
            // Non-base64 secret characters.
            format!("{}{}", &t[..t.len() - 1], "!"),
        ];
        for case in cases {
            assert!(parse(&case).is_none(), "{case}");
        }
        // Uppercase hex digits are not canonical.
        if upper != *t {
            assert!(parse(&upper).is_none());
        }
    }

    #[test]
    fn expiry_and_revocation_decide_usability() {
        let now = Utc::now();
        assert!(is_usable(None, None, now));
        assert!(is_usable(None, Some(now + Duration::seconds(1)), now));
        assert!(
            !is_usable(None, Some(now), now),
            "expiry instant is exclusive"
        );
        assert!(!is_usable(None, Some(now - Duration::seconds(1)), now));
        assert!(!is_usable(Some(now), None, now));
        assert!(!is_usable(
            Some(now - Duration::days(1)),
            Some(now + Duration::days(1)),
            now
        ));
    }

    #[test]
    fn capability_sets_validate_wire_values() {
        use buffa::EnumValue;
        let t = EnumValue::from(Capability::CAPABILITY_TRANSLATOR);
        let e = EnumValue::from(Capability::CAPABILITY_EXTRACTOR);
        let both = Capabilities::from_wire(&[t, e]).unwrap();
        assert_eq!(both, Capabilities::ALL);
        assert!(Capabilities::from_wire(&[]).is_none());
        assert!(Capabilities::from_wire(&[t, t]).is_none());
        assert!(
            Capabilities::from_wire(&[EnumValue::from(Capability::CAPABILITY_UNSPECIFIED)])
                .is_none()
        );
        assert!(Capabilities::from_wire(&[EnumValue::from(99)]).is_none());
        assert_eq!(
            Capabilities::from_wire(&[e]).unwrap().to_list(),
            vec![Capability::CAPABILITY_EXTRACTOR]
        );
    }

    #[test]
    fn personal_permission_is_the_intersection_of_key_and_user() {
        let translator = Capabilities::of(Capability::CAPABILITY_TRANSLATOR);
        let extractor = Capabilities::of(Capability::CAPABILITY_EXTRACTOR);
        assert_eq!(Capabilities::ALL.intersect(translator), translator);
        assert_eq!(translator.intersect(extractor), Capabilities::NONE);
        assert!(translator.intersect(extractor).is_empty());
        assert!(Capabilities::ALL.intersect(Capabilities::NONE).is_empty());
        assert!(translator.contains(Capability::CAPABILITY_TRANSLATOR));
        assert!(!translator.contains(Capability::CAPABILITY_UNSPECIFIED));
    }

    #[test]
    fn database_round_trip_is_canonical() {
        for set in [
            Capabilities::of(Capability::CAPABILITY_TRANSLATOR),
            Capabilities::of(Capability::CAPABILITY_EXTRACTOR),
            Capabilities::ALL,
        ] {
            assert_eq!(Capabilities::from_database(&set.to_database()), set);
        }
        assert_eq!(Capabilities::ALL.to_database(), vec![1, 2]);
        assert_eq!(
            kind_from_db(kind_to_db(KeyKind::KEY_KIND_SERVICE)),
            Some(KeyKind::KEY_KIND_SERVICE)
        );
        assert_eq!(kind_from_db(0), None);
    }
}
