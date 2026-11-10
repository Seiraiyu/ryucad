//! Feature results as the disk cache holds them: ryucad's own data
//! around ryukei's encodings of each body, its journal and every place a
//! join, cut or hole applied its body (see [`ryukei::codec`]).
//!
//! A body-making feature's entry is its [`Made`]: each solid's founder
//! and [`Built`] (the body, the journal, the origins as JSON, the hash),
//! the hash, what it applied, if anything, a feature's report (an
//! import's, a fill's, a draft's angles), and an unfold's flat, written only when it has one (so every other entry keeps
//! its bytes). A tool's entry is its own
//! body's `Built`. Integers are little-endian, byte strings length-first.
//! Anything that doesn't read back exactly is `None`: a miss.

use crate::doc::id::Id;
use crate::doc::solid::Operation;
use crate::rebuild::{Applied, Built, Flat, FlatBend, Made};
use ryukei::name::Name;
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

    fn name(&mut self) -> Option<Name> {
        let (n, rest) = self.0.split_first_chunk::<16>()?;
        self.0 = rest;
        Some(Name::from_u128(u128::from_le_bytes(*n)))
    }

    fn names(&mut self) -> Option<Vec<Name>> {
        let n = self.count()?;
        (0..n).map(|_| self.name()).collect()
    }

    fn flat(&mut self) -> Option<Flat> {
        let unfold = self.id()?;
        let map = ryukei::codec::decode_flat_map(self.bytes()?).ok()?;
        let pattern = ryukei::codec::decode_flat_pattern(self.bytes()?).ok()?;
        let n = self.count()?;
        let mut bends = Vec::with_capacity(n);
        for _ in 0..n {
            bends.push(FlatBend {
                name: self.name()?,
                inner: self.names()?,
                outer: self.names()?,
                allowance: self.str()?.to_owned(),
            });
        }
        let partial = self.flag()?;
        Some(Flat {
            unfold,
            map,
            pattern,
            bends,
            partial,
        })
    }

    fn built(&mut self) -> Option<Built> {
        let body = ryukei::codec::decode_body(self.bytes()?).ok()?;
        let journal = ryukei::codec::decode_journal(self.bytes()?).ok()?;
        let origins: BTreeMap<String, &serde_json::value::RawValue> =
            serde_json::from_slice(self.bytes()?).ok()?;
        Some(Built {
            body,
            journal,
            origins: Arc::new(crate::rebuild::Origins::from_raw(origins)?),
            hash: self.str()?.to_owned(),
            reference: false,
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

    fn name(&mut self, n: Name) {
        self.0.extend_from_slice(&n.to_u128().to_le_bytes());
    }

    fn names(&mut self, ns: &[Name]) {
        self.count(ns.len());
        for n in ns {
            self.name(*n);
        }
    }

    fn flat(&mut self, f: &Flat) {
        self.str(f.unfold.as_str());
        self.bytes(&ryukei::codec::encode_flat_map(&f.map));
        self.bytes(&ryukei::codec::encode_flat_pattern(&f.pattern));
        self.count(f.bends.len());
        for b in &f.bends {
            self.name(b.name);
            self.names(&b.inner);
            self.names(&b.outer);
            self.str(&b.allowance);
        }
        self.flag(f.partial);
    }
}

/// What a failure's payload starts with: a count no `encode_made` writes
/// (a feature's solids number fewer than 2^32 - 1, and every item takes
/// eight bytes), so `decode_made` refuses it and an earlier build, which
/// never shares an identity with this one anyway, reads it as a bad entry.
const FAILURE: &[u8] = b"\xff\xff\xff\xffryucad.failure.1\n";

/// A feature's refusal as an entry's payload: the error as JSON (#503).
pub fn encode_failure(e: &crate::error::Error) -> Vec<u8> {
    let mut out = FAILURE.to_vec();
    out.extend(serde_json::to_vec(e).expect("an error writes as JSON"));
    out
}

/// A refusal [`encode_failure`] wrote, or `None` for anything else.
pub fn decode_failure(bytes: &[u8]) -> Option<crate::error::Error> {
    serde_json::from_slice(bytes.strip_prefix(FAILURE)?).ok()
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
    match &m.report {
        None => w.flag(false),
        Some(v) => {
            w.flag(true);
            w.bytes(&serde_json::to_vec(v.as_ref()).expect("JSON values serialize"));
        }
    }
    // A flat only where there is one: every other entry keeps its bytes.
    if let Some(f) = &m.flat {
        w.flat(f);
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
    let import: Option<Arc<serde_json::Value>> = if r.flag()? {
        Some(Arc::new(serde_json::from_slice(r.bytes()?).ok()?))
    } else {
        None
    };
    let flat = if r.0.is_empty() {
        None
    } else {
        Some(Arc::new(r.flat()?))
    };
    r.end()?;
    // A reference's entry says so in its report; nothing else holds one
    // (nothing acts on a reference), so the bytes don't need to.
    let reference = import.as_ref().is_some_and(|v| v["mode"] == "reference");
    let solids = if reference {
        solids
            .into_iter()
            .map(|(id, b)| {
                let mut b = Arc::try_unwrap(b).unwrap_or_else(|b| (*b).clone());
                b.reference = true;
                (id, Arc::new(b))
            })
            .collect()
    } else {
        solids
    };
    Some(Made {
        solids,
        hash,
        applied,
        report: import,
        flat,
        encoded: Default::default(),
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
    fn a_drafts_entry_keeps_its_report_and_is_no_reference() {
        let body = ryukei::primitive::block(
            ryukei::name::OpKey::from_bytes(b"t"),
            0.01,
            0.01,
            0.01,
            &ryukei::geom::Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        let id = Id::parse("d", "feature").unwrap();
        let b = crate::rebuild::finish(
            &id,
            "draft",
            (body, Default::default()),
            crate::rebuild::Origins::default(),
        )
        .unwrap();
        let rep = json!({"target": "b", "hinge": "parting",
            "angles": [{"face": "0123456789abcdef0123456789abcdef",
                        "least": 0.0349, "greatest": 0.0351, "asked": 0.0349}]});
        let made = Made {
            report: Some(Arc::new(rep.clone())),
            ..Made::new(vec![(Id::parse("b", "feature").unwrap(), Arc::new(b))])
        };
        let back = decode_made(&encode_made(&made)).unwrap();
        assert_eq!(back.report.as_deref(), Some(&rep));
        assert!(!back.solids[0].1.reference);
    }

    #[test]
    fn a_references_entry_comes_back_a_reference() {
        let body = ryukei::primitive::block(
            ryukei::name::OpKey::from_bytes(b"t"),
            0.01,
            0.01,
            0.01,
            &ryukei::geom::Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        let id = Id::parse("i", "feature").unwrap();
        let mut b = crate::rebuild::finish_reference(
            &id,
            (body, Default::default()),
            crate::rebuild::Origins::default(),
        )
        .unwrap();
        assert!(b.reference);
        let made = Made {
            report: Some(Arc::new(json!({"mode": "reference"}))),
            ..Made::new(vec![(id.clone(), Arc::new(b.clone()))])
        };
        let back = decode_made(&encode_made(&made)).unwrap();
        assert!(back.solids[0].1.reference);
        // The same entry without the report's mode is a solid's.
        b.reference = false;
        let plain = Made::new(vec![(id, Arc::new(b))]);
        let back = decode_made(&encode_made(&plain)).unwrap();
        assert!(!back.solids[0].1.reference);
        // A reference's bytes are a solid's but for the report.
        assert_eq!(
            encode_built(&back.solids[0].1),
            encode_built(&made.solids[0].1)
        );
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
