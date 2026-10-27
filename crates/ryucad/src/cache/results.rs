//! Feature results as the disk cache holds them: ryucad's own data
//! around ryukei's encodings of each body, its journal and every place a
//! join, cut or hole applied its body (see [`ryukei::codec`]).
//!
//! A body-making feature's entry is its [`Made`]: each solid's founder
//! and [`Built`] (the body, the journal, the origins as JSON, the hash),
//! the hash, what it applied, if anything, and an import's report. A tool's entry is its own
//! body's `Built`. Integers are little-endian, byte strings length-first.
//! Anything that doesn't read back exactly is `None`: a miss.

use crate::doc::id::Id;
use crate::doc::solid::Operation;
use crate::rebuild::{Applied, Built, Made};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(&(b.len() as u64).to_le_bytes());
        self.0.extend_from_slice(b);
    }

    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    fn flag(&mut self, f: bool) {
        self.0.push(u8::from(f));
    }

    fn built(&mut self, b: &Built) {
        self.bytes(&ryukei::codec::encode_body(&b.body));
        self.bytes(&ryukei::codec::encode_journal(&b.journal));
        self.bytes(&serde_json::to_vec(b.origins.as_ref()).expect("JSON values serialize"));
        self.str(&b.hash);
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let (n, rest) = self.0.split_first_chunk::<8>()?;
        let n = usize::try_from(u64::from_le_bytes(*n)).ok()?;
        if rest.len() < n {
            return None;
        }
        let (b, rest) = rest.split_at(n);
        self.0 = rest;
        Some(b)
    }

    fn str(&mut self) -> Option<&'a str> {
        std::str::from_utf8(self.bytes()?).ok()
    }

    fn id(&mut self) -> Option<Id> {
        Id::parse(self.str()?, "feature").ok()
    }

    fn flag(&mut self) -> Option<bool> {
        let (&f, rest) = self.0.split_first()?;
        self.0 = rest;
        match f {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn count(&mut self) -> Option<usize> {
        let (n, rest) = self.0.split_first_chunk::<4>()?;
        self.0 = rest;
        let n = u32::from_le_bytes(*n) as usize;
        // Every item takes at least eight bytes.
        (n.saturating_mul(8) <= self.0.len()).then_some(n)
    }

    fn built(&mut self) -> Option<Built> {
        let body = ryukei::codec::decode_body(self.bytes()?).ok()?;
        let journal = ryukei::codec::decode_journal(self.bytes()?).ok()?;
        let origins: BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(self.bytes()?).ok()?;
        Some(Built {
            body,
            journal,
            origins: Arc::new(origins),
            hash: self.str()?.to_owned(),
        })
    }

    fn end(&self) -> Option<()> {
        self.0.is_empty().then_some(())
    }
}

impl Writer {
    fn count(&mut self, n: usize) {
        self.0
            .extend_from_slice(&u32::try_from(n).expect("fewer than 2^32").to_le_bytes());
    }
}

/// A feature's solids as an entry's payload.
pub fn encode_made(m: &Made) -> Vec<u8> {
    let mut w = Writer::default();
    w.count(m.solids.len());
    for (id, b) in &m.solids {
        w.str(id.as_str());
        w.built(b);
    }
    w.str(&m.hash);
    match &m.applied {
        None => w.flag(false),
        Some(a) => {
            w.flag(true);
            w.built(&a.body);
            w.str(a.operation.as_str());
            w.count(a.places.len());
            for (point, r) in &a.places {
                match point {
                    None => w.flag(false),
                    Some(p) => {
                        w.flag(true);
                        w.str(p.as_str());
                    }
                }
                w.bytes(&ryukei::codec::encode_rigid(r));
            }
            w.str(&a.hash);
        }
    }
    match &m.import {
        None => w.flag(false),
        Some(v) => {
            w.flag(true);
            w.bytes(&serde_json::to_vec(v.as_ref()).expect("JSON values serialize"));
        }
    }
    w.0
}

/// The solids an entry's payload holds, or `None` if it doesn't decode.
pub fn decode_made(bytes: &[u8]) -> Option<Made> {
    let mut r = Reader(bytes);
    let n = r.count()?;
    let mut solids = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.id()?;
        solids.push((id, Arc::new(r.built()?)));
    }
    let hash = r.str()?.to_owned();
    let applied = if r.flag()? {
        let body = Arc::new(r.built()?);
        let operation = Operation::parse(r.str()?)?;
        let n = r.count()?;
        let mut places = Vec::with_capacity(n);
        for _ in 0..n {
            let point = if r.flag()? { Some(r.id()?) } else { None };
            let rigid = ryukei::codec::decode_rigid(r.bytes()?).ok()?;
            places.push((point, rigid));
        }
        let hash = r.str()?.to_owned();
        Some(Arc::new(Applied {
            body,
            operation,
            places,
            hash,
        }))
    } else {
        None
    };
    let import = if r.flag()? {
        Some(Arc::new(serde_json::from_slice(r.bytes()?).ok()?))
    } else {
        None
    };
    r.end()?;
    Some(Made {
        solids,
        hash,
        applied,
        import,
    })
}

/// A tool's own body as an entry's payload.
pub fn encode_built(b: &Built) -> Vec<u8> {
    let mut w = Writer::default();
    w.built(b);
    w.0
}

/// The tool body an entry's payload holds, or `None`.
pub fn decode_built(bytes: &[u8]) -> Option<Built> {
    let mut r = Reader(bytes);
    let b = r.built()?;
    r.end()?;
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    /// A plate with a hole and a row of three more: a solid, a hole that
    /// applied its own body at a point, and a pattern.
    fn made() -> Vec<Made> {
        let (_dir, mut s) = fresh();
        let add = |s: &mut crate::session::Session, v: serde_json::Value| {
            call(s, "feature.add", v).unwrap();
        };
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        add(
            &mut s,
            json!({"part": "p", "type": "box", "id": "plate",
                   "params": {"x": 40, "y": 30, "z": 5}}),
        );
        add(
            &mut s,
            json!({"part": "p", "type": "sketch", "id": "s",
                   "placement": {"origin": [0, 0, 5]}}),
        );
        let ops = json!([
            {"op": "add", "type": "point", "at": [10, 10]},
            {"op": "add", "type": "fix", "a": "p1"},
        ]);
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": ops}),
        )
        .unwrap();
        add(
            &mut s,
            json!({"part": "p", "type": "hole", "id": "h", "sketch": "s",
                   "diameter": 3, "depth": "through_all"}),
        );
        add(
            &mut s,
            json!({"part": "p", "type": "pattern", "id": "row", "features": ["h"],
                   "direction": "x", "spacing": 5, "count": 3}),
        );
        let r = s.rebuilt(&Id::parse("p", "part").unwrap()).unwrap();
        r.features
            .iter()
            .filter_map(|(id, o)| match o {
                Ok(crate::rebuild::Output::Solids(m)) => Some(m.as_ref().clone()),
                Ok(_) => None,
                Err(e) => panic!("{id}: {e:?}"),
            })
            .collect()
    }

    fn same(a: &Made, b: &Made) {
        assert_eq!(a.hash, b.hash);
        assert_eq!(a.solids.len(), b.solids.len());
        for ((ia, ba), (ib, bb)) in a.solids.iter().zip(&b.solids) {
            assert_eq!(ia, ib);
            assert_eq!(ba.body.dump(), bb.body.dump());
            assert_eq!(ba.journal, bb.journal);
            assert_eq!(ba.origins, bb.origins);
            assert_eq!(ba.hash, bb.hash);
        }
        match (&a.applied, &b.applied) {
            (None, None) => {}
            (Some(x), Some(y)) => {
                assert_eq!(x.body.body.dump(), y.body.body.dump());
                assert_eq!(x.operation, y.operation);
                assert_eq!(x.places, y.places);
                assert_eq!(x.hash, y.hash);
            }
            _ => panic!("applied differs"),
        }
    }

    #[test]
    fn solids_and_what_they_applied_come_back_exactly() {
        let made = made();
        assert_eq!(made.len(), 3, "the plate, the hole and the pattern");
        assert!(made[1].applied.is_some());
        for m in &made {
            let bytes = encode_made(m);
            let back = decode_made(&bytes).unwrap();
            same(m, &back);
            assert_eq!(encode_made(&back), bytes);
        }
        let tool = &made[1].applied.as_ref().unwrap().body;
        let back = decode_built(&encode_built(tool)).unwrap();
        assert_eq!(back.body.dump(), tool.body.dump());
        assert_eq!(back.hash, tool.hash);
    }

    #[test]
    fn anything_else_is_a_miss() {
        let m = &made()[1];
        let bytes = encode_made(m);
        for n in (0..bytes.len()).step_by(97) {
            assert!(decode_made(&bytes[..n]).is_none(), "{n}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode_made(&longer).is_none());
        assert!(decode_built(&bytes).is_none());
    }
}
