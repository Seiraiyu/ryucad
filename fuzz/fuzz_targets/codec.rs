//! The body codec from bytes. Raw input, and an encoded body or journal
//! with the input's bytes written over part of it, must be refused with
//! [`Error::InvalidEncoding`], or decode to a body that passes the
//! structural checks and whose encoding decodes to the same body and
//! encoding again (input may list names out of order, or write `-0` in a
//! control point, which a body holds as `0`). Any panic, and any other
//! error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::check_topology;
use ryukei::codec::{decode_body, decode_journal, encode_body, encode_journal};
use ryukei::geom::Rigid;
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei_testkit::blend_bytes::body;

fn body_bytes(data: &[u8]) {
    match decode_body(data) {
        Ok(b) => {
            assert_eq!(check_topology(&b), []);
            let bytes = encode_body(&b);
            let again = decode_body(&bytes).unwrap();
            assert_eq!(again.dump(), b.dump());
            assert_eq!(encode_body(&again), bytes);
        }
        Err(Error::InvalidEncoding { .. }) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
}

fn journal_bytes(data: &[u8]) {
    match decode_journal(data) {
        Ok(j) => assert_eq!(encode_journal(&j), data),
        Err(Error::InvalidEncoding { .. }) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
}

/// `base` with `patch` written over it from the offset its first two
/// bytes give (past the magic), or cut there when `patch` is empty.
fn mutated(base: &[u8], data: &[u8]) -> Vec<u8> {
    let Some((at, patch)) = data.split_first_chunk::<2>() else {
        return base.to_vec();
    };
    let at = (usize::from(u16::from_le_bytes(*at)) % base.len().max(1)).max(13);
    let mut out = base.to_vec();
    if patch.is_empty() {
        out.truncate(at);
    }
    for (i, &b) in patch.iter().enumerate() {
        if let Some(x) = out.get_mut(at + i) {
            *x = b;
        }
    }
    out
}

fuzz_target!(|data: &[u8]| {
    body_bytes(data);
    journal_bytes(data);
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let base = match mode % 3 {
        0 => primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
            .map(|(b, j)| (encode_body(&b), encode_journal(&j)))
            .unwrap(),
        1 => primitive::torus(OpKey(1), 2.0, 0.5, &Rigid::IDENTITY)
            .map(|(b, j)| (encode_body(&b), encode_journal(&j)))
            .unwrap(),
        _ => match body(rest) {
            Some((b, tail)) => {
                let bytes = encode_body(&b);
                body_bytes(&mutated(&bytes, tail));
                return;
            }
            None => return,
        },
    };
    body_bytes(&mutated(&base.0, rest));
    journal_bytes(&mutated(&base.1, rest));
});
