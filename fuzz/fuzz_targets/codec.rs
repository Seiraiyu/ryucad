//! The body codec from bytes. Raw input, and an encoded body, journal or
//! mesh with the input's bytes written over part of it, must be refused
//! with [`Error::InvalidEncoding`], or decode to what encodes and decodes
//! again to the same thing: a body that passes the structural checks, a
//! journal, a mesh (input may list names out of order, or write `-0` in a
//! control point, which a body holds as `0`). Any panic, and any other
//! error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::check_topology;
use ryukei::codec::{
    decode_body, decode_journal, decode_mesh, encode_body, encode_journal, encode_mesh,
};
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use std::sync::OnceLock;

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

fn mesh_bytes(data: &[u8]) {
    match decode_mesh(data) {
        Ok(m) => assert_eq!(decode_mesh(&encode_mesh(&m)).unwrap(), m),
        Err(Error::InvalidEncoding { .. }) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
}

/// Encodings of a block, a torus and a sphere turned off the axes, the
/// journals that made them and their meshes, made once.
fn bases() -> &'static [(Vec<u8>, Vec<u8>, Vec<u8>)] {
    static BASES: OnceLock<Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>> = OnceLock::new();
    BASES.get_or_init(|| {
        let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
        [
            primitive::block(OpKey(1), 1.0, 2.0, 3.0, &t),
            primitive::torus(OpKey(1), 2.0, 0.5, &t),
            primitive::sphere(OpKey(1), 1.0, &t),
        ]
        .into_iter()
        .map(|r| {
            let (b, j) = r.unwrap();
            let m = tessellate(&b, &MeshParams::new(0.1, 0.8)).unwrap();
            (encode_body(&b), encode_journal(&j), encode_mesh(&m))
        })
        .collect()
    })
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
    mesh_bytes(data);
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let (b, j, m) = &bases()[usize::from(mode) % 3];
    match mode / 3 % 3 {
        0 => body_bytes(&mutated(b, rest)),
        1 => journal_bytes(&mutated(j, rest)),
        _ => mesh_bytes(&mutated(m, rest)),
    }
});
