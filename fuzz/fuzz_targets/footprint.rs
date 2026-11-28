#![no_main]
//! Footprint ops, then a symbol's, from bytes (ryueda 7.F): a batch drawn from a menu of
//! every op, each field from the bytes. A typed refusal, never ryueda's
//! own bug; or a footprint that checks, lints, draws and writes back the
//! same, never a panic.

use libfuzzer_sys::fuzz_target;
use serde_json::{Value, json};

struct Bytes<'a>(&'a [u8], usize);

impl Bytes<'_> {
    fn next(&mut self) -> usize {
        let b = self.0.get(self.1).copied().unwrap_or(0);
        self.1 += 1;
        usize::from(b)
    }
    /// A length: mostly 0 to 5 mm on a 10 µm grid, sometimes zero, negative,
    /// far, or either end of the range.
    fn len(&mut self) -> Value {
        match self.next() % 16 {
            0 => json!(0),
            1 => json!(-(self.next() as i64) * 10_000),
            2 => json!(1i64 << 40),
            // The ends of the range (7.F's review: a move by one overflowed).
            3 => json!(i64::MAX),
            4 => json!(i64::MIN),
            _ => json!((self.next() as i64 % 500) * 10_000),
        }
    }
    /// An object's ID: null for the next free one, a small one, or one
    /// at the end of `<prefix><u64>` (7.F's review: the next free ID
    /// overflowed past `p18446744073709551615`).
    fn id(&mut self, p: &str) -> Value {
        match self.next() % 6 {
            0 | 1 => Value::Null,
            2 => json!(format!("{p}{}", u64::MAX)),
            3 => json!(format!("{p}{}", u64::MAX - 1)),
            n => json!(format!("{p}{n}")),
        }
    }

    fn point(&mut self) -> Value {
        json!([self.len(), self.len()])
    }
    fn pick<'s>(&mut self, from: &[&'s str]) -> &'s str {
        from[self.next() % from.len()]
    }
}

const LAYERS: [&str; 9] = ["copper.top", "copper.bottom", "copper.all", "copper.inner", "mask.top", "paste.top", "silk.top", "courtyard.top", "nope"];
const SHAPES: [&str; 5] = ["rect", "circle", "oval", "roundrect", "blob"];
const KINDS: [&str; 5] = ["smd", "through", "connector", "npth", "glued"];
const TYPES: [&str; 6] = ["line", "rect", "circle", "polygon", "text", "arc"];

fn op(b: &mut Bytes) -> Value {
    let id = |b: &mut Bytes, p: &str| -> Value { b.id(p) };
    match b.next() % 12 {
        0 => {
            let shape = match b.pick(&SHAPES) {
                "roundrect" => json!({"type": "roundrect", "ratio_ppm": (b.next() as i64) * 4_000}),
                s => json!(s),
            };
            let mut v = json!({"op": "add", "pad": id(b, "p"), "kind": b.pick(&KINDS), "shape": shape,
                "at": b.point(), "size": [b.len(), b.len()],
                "layers": (0..1 + b.next() % 3).map(|_| b.pick(&LAYERS)).collect::<Vec<_>>()});
            if b.next() % 2 == 0 {
                v["number"] = json!(format!("{}", b.next() % 6));
            }
            if b.next() % 3 == 0 {
                v["drill"] = json!({"shape": "round", "diameter": b.len()});
            }
            v
        }
        1 => json!({"op": "set", "pad": id(b, "p"), "size": [b.len(), b.len()], "rotation": b.next() as i64 * 1_409}),
        2 => json!({"op": "move", "pad": id(b, "p"), "by": b.point()}),
        3 => json!({"op": "remove", "pad": id(b, "p")}),
        4 => {
            let t = b.pick(&TYPES);
            let mut v = json!({"op": "add", "graphic": id(b, "g"), "type": t, "layer": b.pick(&LAYERS), "width": b.len()});
            match t {
                "line" | "rect" => { v["start"] = b.point(); v["end"] = b.point(); }
                "circle" => { v["center"] = b.point(); v["radius"] = b.len(); }
                "polygon" => v["points"] = json!((0..b.next() % 6).map(|_| b.point()).collect::<Vec<_>>()),
                "arc" => { v["start"] = b.point(); v["mid"] = b.point(); v["end"] = b.point(); }
                _ => { v["text"] = json!("R"); v["at"] = b.point(); v["size"] = json!([b.len(), b.len()]); }
            }
            v
        }
        5 => json!({"op": "move", "graphic": id(b, "g"), "by": b.point()}),
        6 => json!({"op": "remove", "graphic": id(b, "g")}),
        7 => json!({"op": "add", "keepout": id(b, "k"),
            "polygon": (0..b.next() % 6).map(|_| b.point()).collect::<Vec<_>>(),
            "keeps": ["tracks"], "layers": [b.pick(&LAYERS)]}),
        8 => json!({"op": "remove", "keepout": id(b, "k")}),
        9 => json!({"op": b.pick(&["add", "set"]), "field": b.pick(&["reference", "value", "colour"]),
            "layer": b.pick(&LAYERS), "at": b.point(), "size": [b.len(), b.len()]}),
        10 => json!({"op": "remove", "field": b.pick(&["reference", "value"])}),
        _ => json!({"op": b.pick(&["add", "explode"]), "model": Value::Null, "path": "library/models/x.step"}),
    }
}

const PIN_TYPES: [&str; 4] = ["passive", "input", "power_in", "blob"];
const SIDES: [&str; 5] = ["left", "right", "up", "down", "east"];
const SYM_TYPES: [&str; 6] = ["rect", "circle", "arc", "polyline", "text", "bezier"];

/// A symbol op (7.F's review: a symbol's moves, next graphic ID and arc
/// check overflowed at the ends of the range).
fn symbol_op(b: &mut Bytes) -> Value {
    let pin = |b: &mut Bytes| json!(format!("{}", b.next() % 4));
    match b.next() % 7 {
        0 => json!({"op": "add", "pin": pin(b), "type": b.pick(&PIN_TYPES), "at": b.point(),
            "side": b.pick(&SIDES), "length": b.len()}),
        1 => json!({"op": "move", "pin": pin(b), "by": b.point()}),
        2 => json!({"op": "remove", "pin": pin(b)}),
        3 => {
            let t = b.pick(&SYM_TYPES);
            let mut v = json!({"op": "add", "graphic": t});
            if b.next() % 2 == 0 {
                v["id"] = b.id("g");
            }
            match t {
                "rect" => { v["start"] = b.point(); v["end"] = b.point(); }
                "circle" => { v["center"] = b.point(); v["radius"] = b.len(); }
                "arc" => { v["start"] = b.point(); v["mid"] = b.point(); v["end"] = b.point(); }
                "polyline" => v["points"] = json!((0..b.next() % 5).map(|_| b.point()).collect::<Vec<_>>()),
                "bezier" => v["points"] = json!((0..4).map(|_| b.point()).collect::<Vec<_>>()),
                _ => { v["text"] = json!("A"); v["at"] = b.point(); v["size"] = b.len(); }
            }
            v
        }
        4 => json!({"op": "move", "graphic": b.id("g"), "by": b.point()}),
        5 => json!({"op": "set", "graphic": b.id("g"), "radius": b.len()}),
        _ => json!({"op": "remove", "graphic": b.id("g")}),
    }
}

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes(data, 0);
    let ops: Vec<Value> = (0..b.next() % 12).map(|_| op(&mut b)).collect();
    let mut f: ryueda::footprint::Footprint = serde_json::from_value(json!({"name": "F"})).unwrap();
    let no = |_: &str| -> ryueda::error::Result<String> {
        Err(ryueda::error::Error::new(ryueda::error::Code::NotFound, "no models in the fuzzer"))
    };
    match ryueda::fpedit::apply(&mut f, &ops, ryueda::units::Unit::Mm, &no) {
        Ok(_) => {
            // As ryueda does: a footprint failing its own checks is never
            // drawn or linted (the lint returns nothing for it).
            let faulty = f.check().iter().any(|x| x.is_error());
            let _ = ryueda::fplint::lint(&f);
            if !faulty {
                let _ = ryueda::board::shapes::footprint_alone(&f);
            }
            let back: ryueda::footprint::Footprint =
                serde_json::from_value(serde_json::to_value(&f).unwrap()).unwrap();
            assert_eq!(back, f, "a footprint writes back the same");
        }
        Err(e) => {
            ryueda::error::details::check(&e);
            assert_ne!(e.code, ryueda::error::Code::Internal, "{e:?}");
        }
    }
    // A symbol's ops from the bytes left: a typed refusal, or a symbol
    // that checks and writes back the same.
    let ops: Vec<Value> = (0..b.next() % 8).map(|_| symbol_op(&mut b)).collect();
    let mut s: ryueda::symbol::Symbol = serde_json::from_value(json!({"name": "S", "units": 1})).unwrap();
    match ryueda::edit::apply(&mut s, &ops, ryueda::units::Unit::Mm) {
        Ok(_) => {
            let _ = s.check();
            let back: ryueda::symbol::Symbol =
                serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
            assert_eq!(back, s, "a symbol writes back the same");
        }
        Err(e) => {
            ryueda::error::details::check(&e);
            assert_ne!(e.code, ryueda::error::Code::Internal, "{e:?}");
        }
    }
});
