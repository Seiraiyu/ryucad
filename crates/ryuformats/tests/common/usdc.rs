//! A reader of the usdc files ryuformats writes, back into its layer
//! model, written from OpenUSD's `crateFile.cpp` apart from the writer:
//! the bootstrap, the table of contents, the six structural sections
//! (LZ4 and integer coding undone), the path tree, and the value reps of
//! the types the writer uses. Panics on anything else.

use ryuformats::usd::layer::{
    Attribute, Dict, DictValue, Layer, Prim, PrimMeta, Specifier, UpAxis, Value,
};
use std::collections::HashMap;

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn f64_at(b: &[u8], at: usize) -> f64 {
    f64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// `TfFastCompression`'s framing undone: `n` bytes out.
fn unfast(packed: &[u8], n: usize) -> Vec<u8> {
    let chunks = packed[0];
    if chunks == 0 {
        return lz4_flex::block::decompress(&packed[1..], n).unwrap();
    }
    let mut out = Vec::with_capacity(n);
    let mut at = 1;
    for _ in 0..chunks {
        let size = i32::from_le_bytes(packed[at..at + 4].try_into().unwrap()) as usize;
        at += 4;
        let rest = n - out.len();
        out.extend(
            lz4_flex::block::decompress(&packed[at..at + size], rest.min(0x7E00_0000)).unwrap(),
        );
        at += size;
    }
    assert_eq!(out.len(), n);
    out
}

/// `Sdf_IntegerCompression`'s coding undone: `n` integers from coded
/// bytes.
fn decode_ints(coded: &[u8], n: usize) -> Vec<i32> {
    let common = i32::from_le_bytes(coded[..4].try_into().unwrap());
    let codes = &coded[4..4 + (2 * n).div_ceil(8)];
    let mut at = 4 + codes.len();
    let mut prev = 0i32;
    (0..n)
        .map(|k| {
            let d = match (codes[k / 4] >> (2 * (k % 4))) & 3 {
                0 => common,
                1 => {
                    at += 1;
                    i32::from(coded[at - 1] as i8)
                }
                2 => {
                    at += 2;
                    i32::from(i16::from_le_bytes(coded[at - 2..at].try_into().unwrap()))
                }
                _ => {
                    at += 4;
                    i32::from_le_bytes(coded[at - 4..at].try_into().unwrap())
                }
            };
            prev = prev.wrapping_add(d);
            prev
        })
        .collect()
}

/// Compressed integers at `at` (their size, then the bytes): the
/// integers and the offset past them.
fn compressed_ints(b: &[u8], at: usize, n: usize) -> (Vec<i32>, usize) {
    let size = u64_at(b, at) as usize;
    let packed = &b[at + 8..at + 8 + size];
    let coded = unfast(packed, 4 + (2 * n).div_ceil(8) + 4 * n);
    // The coded length isn't stored: decompression stops at the end.
    (decode_ints(&coded, n), at + 8 + size)
}

/// A crate file's tables, read.
struct Crate<'a> {
    b: &'a [u8],
    tokens: Vec<String>,
    strings: Vec<u32>,
    fields: Vec<(u32, u64)>,
    field_sets: Vec<u32>,
    paths: Vec<String>,
    specs: Vec<(u32, u32, u32)>,
}

impl Crate<'_> {
    fn string(&self, i: u32) -> String {
        self.tokens[self.strings[i as usize] as usize].clone()
    }

    fn token(&self, i: u64) -> String {
        self.tokens[i as usize].clone()
    }

    /// The fields of the set starting at `set`, by name.
    fn fields_of(&self, set: u32) -> Vec<(String, u64)> {
        self.field_sets[set as usize..]
            .iter()
            .take_while(|&&f| f != u32::MAX)
            .map(|&f| {
                let (t, r) = self.fields[f as usize];
                (self.tokens[t as usize].clone(), r)
            })
            .collect()
    }

    fn token_vector(&self, rep: u64) -> Vec<String> {
        assert_eq!(rep >> 48 & 0xff, 41, "a token vector");
        let at = payload(rep);
        let n = u64_at(self.b, at) as usize;
        (0..n)
            .map(|k| self.token(u64::from(u32_at(self.b, at + 8 + 4 * k))))
            .collect()
    }

    /// A prepended list op's items as `item` reads each, after its header
    /// byte and count.
    fn prepended<T>(&self, rep: u64, ty: u64, size: usize, item: impl Fn(usize) -> T) -> Vec<T> {
        assert_eq!(rep >> 48 & 0xff, ty);
        let at = payload(rep);
        assert_eq!(self.b[at], 1 << 5, "only prepended items");
        let n = u64_at(self.b, at + 1) as usize;
        (0..n).map(|k| item(at + 9 + size * k)).collect()
    }

    fn dictionary(&self, rep: u64) -> Dict {
        assert_eq!(rep >> 48 & 0xff, 31, "a dictionary");
        let mut at = payload(rep);
        let n = u64_at(self.b, at) as usize;
        at += 8;
        let mut d = Dict::default();
        for _ in 0..n {
            let key = self.string(u32_at(self.b, at));
            let offset = u64_at(self.b, at + 4) as usize;
            let r = u64_at(self.b, at + 4 + offset);
            at += 4 + offset + 8;
            let v = match r >> 48 & 0xff {
                10 => DictValue::String(self.string(r as u32)),
                9 => DictValue::Double(f64_at(self.b, payload(r))),
                31 => DictValue::Dict(self.dictionary(r)),
                t => panic!("a dictionary value of type {t}"),
            };
            d.0.push((key, v));
        }
        d
    }

    /// An array's elements' bytes and count: empty, written as it is, or
    /// integers compressed (returned as their bytes).
    fn array(&self, rep: u64, size: usize) -> (Vec<u8>, usize) {
        assert!(rep & (1 << 63) != 0, "an array");
        if payload(rep) == 0 {
            return (Vec::new(), 0);
        }
        let at = payload(rep);
        let n = u64_at(self.b, at) as usize;
        if rep & (1 << 61) != 0 {
            let (ints, _) = compressed_ints(self.b, at + 8, n);
            return (ints.iter().flat_map(|x| x.to_le_bytes()).collect(), n);
        }
        (self.b[at + 8..at + 8 + n * size].to_vec(), n)
    }

    fn value(&self, type_name: &str, rep: u64) -> Value {
        let ty = rep >> 48 & 0xff;
        let inline = rep & (1 << 62) != 0;
        let f64s = |bytes: &[u8]| -> Vec<f64> {
            bytes
                .chunks(8)
                .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
                .collect()
        };
        let f32s = |bytes: &[u8]| -> Vec<[f32; 3]> {
            bytes
                .chunks(12)
                .map(|c| [0, 1, 2].map(|k| f32_at(c, 4 * k)))
                .collect()
        };
        let i32s = |bytes: &[u8]| -> Vec<i32> {
            bytes
                .chunks(4)
                .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                .collect()
        };
        let triples =
            |v: Vec<f64>| -> Vec<[f64; 3]> { v.chunks(3).map(|c| [c[0], c[1], c[2]]).collect() };
        match (type_name, ty) {
            ("bool", 1) => {
                assert!(inline);
                Value::Bool(rep as u8 != 0)
            }
            ("int", 3) => Value::Int(rep as u32 as i32),
            ("double", 9) => Value::Double(f64_at(self.b, payload(rep))),
            ("token", 11) => Value::Token(self.token(rep & 0xffff_ffff)),
            ("string", 10) => Value::String(self.string(rep as u32)),
            ("int[]", 3) => Value::IntArray(i32s(&self.array(rep, 4).0)),
            ("uint[]", 4) => Value::UIntArray(
                i32s(&self.array(rep, 4).0)
                    .into_iter()
                    .map(|x| x as u32)
                    .collect(),
            ),
            ("double[]", 9) => Value::DoubleArray(f64s(&self.array(rep, 8).0)),
            ("token[]", 11) => Value::TokenArray(
                i32s(&self.array(rep, 4).0)
                    .into_iter()
                    .map(|i| self.token(i as u64))
                    .collect(),
            ),
            ("string[]", 10) => Value::StringArray(
                i32s(&self.array(rep, 4).0)
                    .into_iter()
                    .map(|i| self.string(i as u32))
                    .collect(),
            ),
            ("int2[]", 22) => Value::Int2Array(
                i32s(&self.array(rep, 8).0)
                    .chunks(2)
                    .map(|c| [c[0], c[1]])
                    .collect(),
            ),
            ("double2[]", 19) => Value::Double2Array(
                f64s(&self.array(rep, 16).0)
                    .chunks(2)
                    .map(|c| [c[0], c[1]])
                    .collect(),
            ),
            ("double3[]", 23) => Value::Double3Array(triples(f64s(&self.array(rep, 24).0))),
            ("point3d[]", 23) => Value::Point3dArray(triples(f64s(&self.array(rep, 24).0))),
            ("vector3d[]", 23) => Value::Vector3dArray(triples(f64s(&self.array(rep, 24).0))),
            ("point3f[]", 24) => Value::Point3fArray(f32s(&self.array(rep, 12).0)),
            ("normal3f[]", 24) => Value::Normal3fArray(f32s(&self.array(rep, 12).0)),
            ("float3[]", 24) => Value::Float3Array(f32s(&self.array(rep, 12).0)),
            ("matrix4d", 15) => {
                let at = payload(rep);
                let v = f64s(&self.b[at..at + 128]);
                Value::Matrix4d([0, 1, 2, 3].map(|r| [0, 1, 2, 3].map(|c| v[4 * r + c])))
            }
            (t, ty) => panic!("{t} as type {ty}"),
        }
    }
}

fn payload(rep: u64) -> usize {
    (rep & ((1 << 48) - 1)) as usize
}

/// Reads a usdc file's layer. Panics on anything the writer doesn't
/// write.
pub fn read(b: &[u8]) -> Layer {
    assert_eq!(&b[..8], b"PXR-USDC");
    assert_eq!(&b[8..11], &[0, 8, 0]);
    let toc = u64_at(b, 16) as usize;
    let n = u64_at(b, toc) as usize;
    let mut sections = HashMap::new();
    for k in 0..n {
        let at = toc + 8 + 32 * k;
        let name: String = b[at..at + 16]
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as char)
            .collect();
        sections.insert(name, u64_at(b, at + 16) as usize);
    }
    let section = |name: &str| sections[name];

    let at = section("TOKENS");
    let count = u64_at(b, at) as usize;
    let size = u64_at(b, at + 8) as usize;
    let packed_size = u64_at(b, at + 16) as usize;
    let text = unfast(&b[at + 24..at + 24 + packed_size], size);
    let tokens: Vec<String> = text
        .split(|&c| c == 0)
        .take(count)
        .map(|t| String::from_utf8(t.to_vec()).unwrap())
        .collect();
    assert_eq!(tokens.len(), count);

    let at = section("STRINGS");
    let strings = (0..u64_at(b, at) as usize)
        .map(|k| u32_at(b, at + 8 + 4 * k))
        .collect();

    let at = section("FIELDS");
    let count = u64_at(b, at) as usize;
    let (names, after) = compressed_ints(b, at + 8, count);
    let size = u64_at(b, after) as usize;
    let reps = unfast(&b[after + 8..after + 8 + size], 8 * count);
    let fields = (0..count)
        .map(|k| (names[k] as u32, u64_at(&reps, 8 * k)))
        .collect();

    let at = section("FIELDSETS");
    let count = u64_at(b, at) as usize;
    let field_sets = compressed_ints(b, at + 8, count)
        .0
        .into_iter()
        .map(|x| x as u32)
        .collect();

    let at = section("PATHS");
    let count = u64_at(b, at) as usize;
    assert_eq!(u64_at(b, at + 8) as usize, count);
    let (index, after) = compressed_ints(b, at + 16, count);
    let (element, after) = compressed_ints(b, after, count);
    let (jump, _) = compressed_ints(b, after, count);
    let mut paths = vec![String::new(); count];
    // OpenUSD's `_BuildDecompressedPathsImpl`, a stack for its tasks.
    let mut todo = vec![(0usize, String::new())];
    while let Some((mut k, mut parent)) = todo.pop() {
        loop {
            let path = if parent.is_empty() {
                "/".to_owned()
            } else {
                let e = &tokens[element[k].unsigned_abs() as usize];
                if element[k] < 0 {
                    format!("{parent}.{e}")
                } else if parent == "/" {
                    format!("/{e}")
                } else {
                    format!("{parent}/{e}")
                }
            };
            paths[index[k] as usize] = path.clone();
            let (child, sibling) = (jump[k] > 0 || jump[k] == -1, jump[k] >= 0);
            if child && sibling {
                todo.push((k + jump[k] as usize, parent.clone()));
            }
            if child {
                parent = path;
            }
            if !child && !sibling {
                break;
            }
            k += 1;
        }
    }

    let at = section("SPECS");
    let count = u64_at(b, at) as usize;
    let (p, after) = compressed_ints(b, at + 8, count);
    let (s, after) = compressed_ints(b, after, count);
    let (t, _) = compressed_ints(b, after, count);
    let specs = (0..count)
        .map(|k| (p[k] as u32, s[k] as u32, t[k] as u32))
        .collect();

    let c = Crate {
        b,
        tokens,
        strings,
        fields,
        field_sets,
        paths,
        specs,
    };
    layer(&c)
}

/// A spec as read: its type and its fields by name.
type Spec = (u32, Vec<(String, u64)>);

fn layer(c: &Crate<'_>) -> Layer {
    let by_path: HashMap<&str, Spec> = c
        .specs
        .iter()
        .map(|&(p, s, t)| (c.paths[p as usize].as_str(), (t, c.fields_of(s))))
        .collect();
    let get = |path: &str| -> &Spec {
        by_path
            .get(path)
            .unwrap_or_else(|| panic!("no spec at {path}"))
    };
    let (kind, fields) = get("/");
    assert_eq!(*kind, 7, "the pseudo-root");
    let mut l = Layer {
        default_prim: None,
        meters_per_unit: 0.0,
        up_axis: UpAxis::Y,
        custom_layer_data: Dict::default(),
        prims: Vec::new(),
    };
    for (name, rep) in fields {
        match name.as_str() {
            "customLayerData" => l.custom_layer_data = c.dictionary(*rep),
            "defaultPrim" => l.default_prim = Some(c.token(rep & 0xffff_ffff)),
            "metersPerUnit" => l.meters_per_unit = f64_at(c.b, payload(*rep)),
            "upAxis" => {
                l.up_axis = match c.token(rep & 0xffff_ffff).as_str() {
                    "Z" => UpAxis::Z,
                    _ => UpAxis::Y,
                }
            }
            "primChildren" => {
                l.prims = c
                    .token_vector(*rep)
                    .iter()
                    .map(|n| prim(c, &get, &format!("/{n}"), n))
                    .collect()
            }
            other => panic!("the layer's {other}"),
        }
    }
    l
}

type Get<'a> = dyn Fn(&str) -> &'a Spec + 'a;

fn prim<'a>(c: &Crate<'_>, get: &Get<'a>, path: &str, name: &str) -> Prim {
    let (kind, fields) = get(path);
    assert_eq!(*kind, 6, "a prim at {path}");
    let mut p = Prim {
        specifier: Specifier::Def,
        type_name: None,
        name: name.to_owned(),
        meta: PrimMeta::default(),
        attributes: Vec::new(),
        children: Vec::new(),
    };
    for (f, rep) in fields {
        let rep = *rep;
        match f.as_str() {
            "specifier" => {
                p.specifier = match rep as u32 {
                    0 => Specifier::Def,
                    2 => Specifier::Class,
                    s => panic!("specifier {s}"),
                }
            }
            "typeName" => p.type_name = Some(c.token(rep & 0xffff_ffff)),
            "apiSchemas" => {
                p.meta.api_schemas =
                    c.prepended(rep, 32, 4, |at| c.token(u64::from(u32_at(c.b, at))))
            }
            "customData" => p.meta.custom_data = c.dictionary(rep),
            "instanceable" => p.meta.instanceable = Some(rep as u8 != 0),
            "kind" => p.meta.kind = Some(c.token(rep & 0xffff_ffff)),
            "references" => {
                p.meta.references = c.prepended(rep, 35, 32, |at| {
                    assert_eq!(c.string(u32_at(c.b, at)), "", "an internal reference");
                    assert_eq!((f64_at(c.b, at + 8), f64_at(c.b, at + 16)), (0.0, 1.0));
                    assert_eq!(u64_at(c.b, at + 24), 0, "no custom data");
                    c.paths[u32_at(c.b, at + 4) as usize].clone()
                })
            }
            "properties" => {
                p.attributes = c
                    .token_vector(rep)
                    .iter()
                    .map(|n| attribute(c, get, &format!("{path}.{n}"), n))
                    .collect()
            }
            "primChildren" => {
                p.children = c
                    .token_vector(rep)
                    .iter()
                    .map(|n| prim(c, get, &format!("{path}/{n}"), n))
                    .collect()
            }
            other => panic!("{path}'s {other}"),
        }
    }
    p
}

fn attribute<'a>(c: &Crate<'_>, get: &Get<'a>, path: &str, name: &str) -> Attribute {
    let (kind, fields) = get(path);
    assert_eq!(*kind, 1, "an attribute at {path}");
    let field = |f: &str| fields.iter().find(|(n, _)| n == f).map(|x| x.1);
    let type_name = c.token(field("typeName").expect("a type") & 0xffff_ffff);
    let mut a = Attribute::new(
        name,
        c.value(&type_name, field("default").expect("a value")),
    )
    .unwrap();
    a.uniform = field("variability").expect("a variability") as u8 == 1;
    a.custom = field("custom").expect("custom") as u8 != 0;
    a.interpolation = field("interpolation").map(|r| c.token(r & 0xffff_ffff));
    for (f, _) in fields {
        assert!(
            [
                "typeName",
                "default",
                "variability",
                "custom",
                "interpolation"
            ]
            .contains(&f.as_str()),
            "{path}'s {f}"
        );
    }
    a
}
