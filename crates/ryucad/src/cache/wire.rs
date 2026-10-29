//! `ryucad.cache.1`: what a shared cache server speaks, over HTTP/1.1,
//! for the disk cache's entries (the bytes of an entry file: header,
//! identity, payload hash, length, payload).
//!
//! | Request | Answer |
//! |---|---|
//! | `GET /v1/info` | `{"protocol": "ryucad.cache.1", "entries", "bytes", "write"}`: whether the token may write |
//! | `GET /v1/<kind>/<key>` | 200 and the entry's bytes, or 404 |
//! | `PUT /v1/<kind>/<key>` | 201 stored, 200 already held; 400 for an entry whose header or hash is wrong, 413 past 256 MiB |
//! | `POST /v1/has` | a JSON array of up to 10,000 names (`<kind>/<key>`); answers the array of those held |
//! | `POST /v1/get` | a JSON array of up to 256 names; answers, for each in order, its length as 8 bytes little-endian and its bytes, or `u64::MAX` for one not held |
//!
//! Kinds are the disk cache's folders (`features`, `tools`, `meshes`);
//! keys are 64 lowercase hex digits, anything else is 400. Every request
//! carries `Authorization: Bearer <token>`: none or a token the server
//! doesn't know is 401 (a server may let anyone read), a read token
//! putting 403. The server never interprets a payload beyond its header
//! and hash. ryucad holds the client and a fake server for its tests
//! (`tests/fake_cache`); the server is ryupdm-service's.

use super::Kind;

/// The protocol's name, in `GET /v1/info`.
pub const PROTOCOL: &str = "ryucad.cache.1";

/// The most names one `has` may ask about.
pub const MAX_HAS: usize = 10_000;

/// The most entries one `get` may ask for.
pub const MAX_GET: usize = 256;

/// The largest entry a server takes.
pub const MAX_ENTRY: u64 = 256 << 20;

/// What `get` answers for an entry the server doesn't hold.
pub const ABSENT: u64 = u64::MAX;

/// Whether `key` is a key: 64 lowercase hex digits.
pub fn valid_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// An entry's name in `has` and `get`: `<kind>/<key>`.
pub fn name(kind: Kind, key: &str) -> String {
    format!("{}/{key}", kind.as_str())
}

/// The kind and key a name gives, if it is one.
pub fn parse_name(name: &str) -> Option<(Kind, &str)> {
    let (kind, key) = name.split_once('/')?;
    Some((Kind::parse(kind)?, key)).filter(|(_, k)| valid_key(k))
}

/// An entry's path on the server.
pub fn path(kind: Kind, key: &str) -> String {
    format!("/v1/{}", name(kind, key))
}

/// `get`'s answer from the entries asked for, in order (`None` for one
/// not held).
pub fn encode_batch<'a>(entries: impl IntoIterator<Item = Option<&'a [u8]>>) -> Vec<u8> {
    let mut out = Vec::new();
    for e in entries {
        match e {
            Some(bytes) => {
                out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                out.extend_from_slice(bytes);
            }
            None => out.extend_from_slice(&ABSENT.to_le_bytes()),
        }
    }
    out
}

/// The `n` entries a `get` answered, if `bytes` holds exactly that.
pub fn decode_batch(mut bytes: &[u8], n: usize) -> Option<Vec<Option<Vec<u8>>>> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let (len, rest) = bytes.split_first_chunk::<8>()?;
        let len = u64::from_le_bytes(*len);
        if len == ABSENT {
            out.push(None);
            bytes = rest;
            continue;
        }
        let len = usize::try_from(len).ok().filter(|l| *l <= rest.len())?;
        out.push(Some(rest[..len].to_vec()));
        bytes = &rest[len..];
    }
    bytes.is_empty().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_keys() {
        let key = "ab".repeat(32);
        assert!(valid_key(&key));
        assert!(!valid_key(&key.to_uppercase()));
        assert!(!valid_key(&key[..63]));
        assert!(!valid_key(&format!("{}g", &key[..63])));
        assert_eq!(name(Kind::Mesh, &key), format!("meshes/{key}"));
        assert_eq!(path(Kind::Tool, &key), format!("/v1/tools/{key}"));
        assert_eq!(
            parse_name(&name(Kind::Feature, &key)),
            Some((Kind::Feature, &key[..]))
        );
        assert_eq!(parse_name(&format!("bodies/{key}")), None);
        assert_eq!(parse_name("features/x"), None);
        assert_eq!(parse_name(&key), None);
    }

    #[test]
    fn batches_go_both_ways() {
        let bytes = encode_batch([Some(&b"one"[..]), None, Some(&b""[..])]);
        assert_eq!(bytes.len(), 8 + 3 + 8 + 8);
        assert_eq!(
            decode_batch(&bytes, 3).unwrap(),
            vec![Some(b"one".to_vec()), None, Some(Vec::new())]
        );
        assert_eq!(decode_batch(&bytes, 2), None, "bytes left over");
        assert_eq!(decode_batch(&bytes, 4), None, "cut short");
        assert_eq!(decode_batch(&bytes[..10], 1), None);
        let mut huge = encode_batch([None]);
        huge[0] = 7;
        assert_eq!(decode_batch(&huge, 1), None, "a length past the end");
    }
}
