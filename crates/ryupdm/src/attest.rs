//! Attestations: what the hosted service signs when a release record
//! becomes official under approval rules, carried as the message of the
//! annotated tag `ryupdm/<part>/<label>` (`…/<label>.obsolete` for an
//! obsolete record). A clone verifies them offline against the keys this
//! build trusts ([`HOSTED_KEYS`] and `ryupdm.json`'s `approvals.keys`).
//!
//! The message is the attestation's canonical JSON, then
//! `-----BEGIN RYUPDM SIGNATURE-----`, `{"ed25519": "<128 hex>", "key":
//! "<16 hex>"}` on one line and `-----END RYUPDM SIGNATURE-----`. The
//! signature is Ed25519 over `ryupdm.attestation.1\0` followed by the
//! canonical bytes, so the same attestation and key give the same tag.

use std::collections::BTreeMap;

/// The hosted service's public keys: `(ID, 64 hex)`. Never remove one:
/// old tags must still verify. A new key needs a ryucad release before the
/// service signs with it (design 261). Task "The hosted key" fills it.
pub const HOSTED_KEYS: &[(&str, &str)] = &[];

/// `bytes` as lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|c| format!("{c:02x}")).collect()
}

/// `N` bytes from exactly `2 N` lower-case hex digits.
pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != 2 * N {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (digit(b[2 * i])? << 4) | digit(b[2 * i + 1])?;
    }
    Some(out)
}

/// A public key's ID: the first 16 hex digits of the BLAKE3 hash of its
/// 32 bytes.
pub fn key_id(public: &[u8; 32]) -> String {
    hex(&blake3::hash(public).as_bytes()[..8])
}

/// The keys an attestation may name: the hosted ones and `extra`
/// (`approvals.keys` as read where the rules are read).
pub fn trusted(extra: &BTreeMap<String, [u8; 32]>) -> BTreeMap<String, [u8; 32]> {
    let mut out: BTreeMap<String, [u8; 32]> = HOSTED_KEYS
        .iter()
        .filter_map(|(id, k)| Some(((*id).to_owned(), unhex::<32>(k)?)))
        .collect();
    out.extend(extra.iter().map(|(id, k)| (id.clone(), *k)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_lower_case_only() {
        let b: [u8; 4] = [0, 0x0f, 0xa0, 0xff];
        assert_eq!(hex(&b), "000fa0ff");
        assert_eq!(unhex::<4>("000fa0ff"), Some(b));
        assert_eq!(unhex::<4>("000FA0FF"), None);
        assert_eq!(unhex::<4>("000fa0f"), None);
        assert_eq!(unhex::<4>("000fa0fg"), None);
    }

    #[test]
    fn a_key_id_is_the_start_of_its_hash() {
        let k = [1u8; 32];
        let id = key_id(&k);
        assert_eq!(id.len(), 16);
        assert_eq!(id, hex(&blake3::hash(&k).as_bytes()[..8]));
    }

    #[test]
    fn hosted_keys_are_well_formed_and_trusted() {
        for (id, key) in HOSTED_KEYS {
            let bytes = unhex::<32>(key).expect("64 hex");
            assert_eq!(key_id(&bytes), *id);
            assert!(ed25519_dalek::VerifyingKey::from_bytes(&bytes).is_ok());
        }
        let extra = BTreeMap::from([("x".to_owned(), [2u8; 32])]);
        let t = trusted(&extra);
        assert_eq!(t.len(), HOSTED_KEYS.len() + 1);
        assert_eq!(t["x"], [2u8; 32]);
    }
}
