//! `usdc`: a [`Layer`] in Pixar's crate format, version 0.8.0 (what
//! OpenUSD writes new files as by default), from OpenUSD's
//! `pxr/usd/sdf/crateFile.cpp`.
//!
//! The file is an 88-byte bootstrap (`PXR-USDC`, the version, the table
//! of contents' offset), the values fields don't hold inline, the six
//! structural sections (`TOKENS`, `STRINGS`, `FIELDS`, `FIELDSETS`,
//! `PATHS`, `SPECS`, compressed as 0.4.0 and later compress them) and the
//! table of contents. Each spec (the layer, each prim, each attribute) is
//! a path and a set of fields, a field a token and a value: inline where
//! the format always inlines its type (booleans, integers, tokens,
//! strings, enums), else written once (equal values are shared) and
//! referred to by offset. Integer arrays of 16 or more are compressed as
//! OpenUSD compresses them; other arrays are written as they are, and no
//! double is inlined as a float, which the format allows but doesn't
//! require. Tokens, strings, paths, fields and field sets are numbered in
//! the order the layer first uses them, and dictionaries are written in
//! the order of their keys' bytes (OpenUSD keeps them so): the same layer
//! always gives the same bytes. A value that isn't finite is
//! `InvalidData`, as in usda.

use super::compress;
use super::layer::{Attribute, Dict, DictValue, Layer, Prim, Specifier, Value};
use std::collections::HashMap;
use std::io::{self, Write};

/// The version written: OpenUSD's default for new files.
pub const VERSION: [u8; 3] = [0, 8, 0];

/// The bootstrap's size: identifier, version, the table of contents'
/// offset and eight reserved `i64`s.
const BOOTSTRAP: usize = 88;

/// The type numbers (`crateDataTypes.h`) of what is written.
mod ty {
    pub const BOOL: u8 = 1;
    pub const INT: u8 = 3;
    pub const UINT: u8 = 4;
    pub const DOUBLE: u8 = 9;
    pub const STRING: u8 = 10;
    pub const TOKEN: u8 = 11;
    pub const MATRIX4D: u8 = 15;
    pub const VEC2D: u8 = 19;
    pub const VEC2I: u8 = 22;
    pub const VEC3D: u8 = 23;
    pub const VEC3F: u8 = 24;
    pub const DICTIONARY: u8 = 31;
    pub const TOKEN_LIST_OP: u8 = 32;
    pub const REFERENCE_LIST_OP: u8 = 35;
    pub const TOKEN_VECTOR: u8 = 41;
    pub const SPECIFIER: u8 = 42;
    pub const VARIABILITY: u8 = 44;
}

/// A value rep's flags.
const ARRAY: u64 = 1 << 63;
const INLINED: u64 = 1 << 62;
const COMPRESSED: u64 = 1 << 61;

/// Spec types (`SdfSpecType`).
const ATTRIBUTE: u32 = 1;
const PRIM: u32 = 6;
const PSEUDO_ROOT: u32 = 7;

/// A list op's header bit for prepended items.
const PREPENDED: u8 = 1 << 5;

/// Integer arrays shorter than this aren't compressed.
const MIN_COMPRESSED: usize = 16;

/// `x`'s bytes, `-0` as `0` (as usda writes it, so a layer reads the
/// same from either).
fn f64_bytes(x: f64) -> [u8; 8] {
    (x + 0.0).to_le_bytes()
}

/// The same for an `f32`.
fn f32_bytes(x: f32) -> [u8; 4] {
    (x + 0.0).to_le_bytes()
}

/// A value rep: its type, flags and payload (an offset, or an inline
/// value).
fn rep(ty: u8, flags: u64, payload: u64) -> u64 {
    flags | (u64::from(ty) << 48) | (payload & ((1 << 48) - 1))
}

/// A path's place in the tree: its parent, its last element (a token)
/// and whether that is a property's name, and its children in the order
/// they were first named.
struct PathNode {
    element: u32,
    property: bool,
    children: Vec<u32>,
}

/// A crate file as it is built: the bytes so far (the bootstrap left
/// blank, then values) and the tables the sections are written from.
struct Crate {
    data: Vec<u8>,
    tokens: Vec<String>,
    token_index: HashMap<String, u32>,
    strings: Vec<u32>,
    string_index: HashMap<String, u32>,
    paths: Vec<PathNode>,
    path_index: HashMap<String, u32>,
    fields: Vec<(u32, u64)>,
    field_index: HashMap<(u32, u64), u32>,
    field_sets: Vec<u32>,
    field_set_index: HashMap<Vec<u32>, u32>,
    specs: Vec<[u32; 3]>,
    /// Values written once, by their rep's type and flags and bytes.
    written: HashMap<(u64, Vec<u8>), u64>,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{what} holds a number that isn't finite"),
    )
}

impl Crate {
    fn new() -> Crate {
        let mut c = Crate {
            data: vec![0; BOOTSTRAP],
            tokens: Vec::new(),
            token_index: HashMap::new(),
            strings: Vec::new(),
            string_index: HashMap::new(),
            paths: Vec::new(),
            path_index: HashMap::new(),
            fields: Vec::new(),
            field_index: HashMap::new(),
            field_sets: Vec::new(),
            field_set_index: HashMap::new(),
            specs: Vec::new(),
            written: HashMap::new(),
        };
        // OpenUSD's first token: one no property can be named, so token 0
        // is never a property's (paths mark those by negating the index).
        c.token(";-)");
        c
    }

    fn tell(&self) -> u64 {
        self.data.len() as u64
    }

    fn align(&mut self, n: usize) {
        while !self.data.len().is_multiple_of(n) {
            self.data.push(0);
        }
    }

    fn u32(&mut self, x: u32) {
        self.data.extend_from_slice(&x.to_le_bytes());
    }

    fn u64(&mut self, x: u64) {
        self.data.extend_from_slice(&x.to_le_bytes());
    }

    fn token(&mut self, t: &str) -> u32 {
        if let Some(&i) = self.token_index.get(t) {
            return i;
        }
        let i = self.tokens.len() as u32;
        self.tokens.push(t.to_owned());
        self.token_index.insert(t.to_owned(), i);
        i
    }

    fn string(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.string_index.get(s) {
            return i;
        }
        let i = self.strings.len() as u32;
        let t = self.token(s);
        self.strings.push(t);
        self.string_index.insert(s.to_owned(), i);
        i
    }

    /// The index of `path` (`/`, `/a/b` or `/a/b.c:d`), its ancestors
    /// numbered first.
    fn path(&mut self, path: &str) -> u32 {
        if let Some(&i) = self.path_index.get(path) {
            return i;
        }
        let (parent, element, property) = if path == "/" {
            (None, "", false)
        } else if let Some((prim, name)) = path.rsplit_once('.') {
            (Some(prim), name, true)
        } else {
            let (up, name) = path.rsplit_once('/').expect("an absolute path");
            (Some(if up.is_empty() { "/" } else { up }), name, false)
        };
        let parent = parent.map(|p| self.path(p));
        let element = self.token(element);
        let i = self.paths.len() as u32;
        self.paths.push(PathNode {
            element,
            property,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            self.paths[p as usize].children.push(i);
        }
        self.path_index.insert(path.to_owned(), i);
        i
    }

    /// Writes `bytes` once for each distinct `(kind, bytes)`, aligned to
    /// eight bytes first if `aligned`: the offset they are at.
    fn put(&mut self, kind: u64, aligned: bool, bytes: Vec<u8>) -> u64 {
        if let Some(&at) = self.written.get(&(kind, bytes.clone())) {
            return at;
        }
        if aligned {
            self.align(8);
        }
        let at = self.tell();
        self.data.extend_from_slice(&bytes);
        self.written.insert((kind, bytes), at);
        at
    }

    /// A value written once, not inline: its rep.
    fn stored(&mut self, ty: u8, bytes: Vec<u8>) -> u64 {
        let kind = rep(ty, 0, 0);
        rep(ty, 0, self.put(kind, false, bytes))
    }

    /// An array of fixed-size elements, written as it is: aligned, its
    /// length as a `u64`, then its elements' bytes.
    fn array(&mut self, ty: u8, len: usize, elements: Vec<u8>) -> u64 {
        if len == 0 {
            return rep(ty, ARRAY, 0);
        }
        let mut bytes = (len as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(&elements);
        let kind = rep(ty, ARRAY, 0);
        rep(ty, ARRAY, self.put(kind, true, bytes))
    }

    /// An array of 32-bit integers: compressed from 16 on, as OpenUSD
    /// writes them (unaligned, its length, then the compressed size and
    /// bytes).
    fn int_array(&mut self, ty: u8, ints: &[i32]) -> u64 {
        if ints.is_empty() {
            return rep(ty, ARRAY, 0);
        }
        let mut bytes = (ints.len() as u64).to_le_bytes().to_vec();
        let flags = if ints.len() < MIN_COMPRESSED {
            for x in ints {
                bytes.extend_from_slice(&x.to_le_bytes());
            }
            ARRAY
        } else {
            let packed = compress::ints(ints);
            bytes.extend_from_slice(&(packed.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&packed);
            ARRAY | COMPRESSED
        };
        // Equal arrays share their bytes whether or not compressed.
        let kind = rep(ty, flags, 0);
        rep(ty, flags, self.put(kind, false, bytes))
    }

    fn tokens_bytes(&mut self, tokens: &[String]) -> Vec<u8> {
        let mut bytes = (tokens.len() as u64).to_le_bytes().to_vec();
        for t in tokens {
            let i = self.token(t);
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        bytes
    }

    fn token_vector(&mut self, tokens: &[String]) -> u64 {
        let bytes = self.tokens_bytes(tokens);
        self.stored(ty::TOKEN_VECTOR, bytes)
    }

    fn value(&mut self, v: &Value) -> u64 {
        fn f64s<const N: usize>(items: &[[f64; N]]) -> Vec<u8> {
            items.iter().flatten().flat_map(|&x| f64_bytes(x)).collect()
        }
        match v {
            Value::Bool(b) => rep(ty::BOOL, INLINED, u64::from(*b)),
            Value::Int(i) => rep(ty::INT, INLINED, u64::from(*i as u32)),
            Value::Double(x) => self.stored(ty::DOUBLE, f64_bytes(*x).to_vec()),
            Value::Token(t) => {
                let i = self.token(t);
                rep(ty::TOKEN, INLINED, u64::from(i))
            }
            Value::String(s) => {
                let i = self.string(s);
                rep(ty::STRING, INLINED, u64::from(i))
            }
            Value::IntArray(a) => self.int_array(ty::INT, a),
            Value::UIntArray(a) => {
                let a: Vec<i32> = a.iter().map(|&x| x as i32).collect();
                self.int_array(ty::UINT, &a)
            }
            Value::DoubleArray(a) => {
                let bytes = a.iter().flat_map(|&x| f64_bytes(x)).collect();
                self.array(ty::DOUBLE, a.len(), bytes)
            }
            Value::TokenArray(a) => {
                let mut bytes = Vec::with_capacity(a.len() * 4);
                for t in a {
                    bytes.extend_from_slice(&self.token(t).to_le_bytes());
                }
                self.array(ty::TOKEN, a.len(), bytes)
            }
            Value::StringArray(a) => {
                let mut bytes = Vec::with_capacity(a.len() * 4);
                for s in a {
                    bytes.extend_from_slice(&self.string(s).to_le_bytes());
                }
                self.array(ty::STRING, a.len(), bytes)
            }
            Value::Int2Array(a) => {
                let bytes = a.iter().flatten().flat_map(|x| x.to_le_bytes()).collect();
                self.array(ty::VEC2I, a.len(), bytes)
            }
            Value::Double2Array(a) => self.array(ty::VEC2D, a.len(), f64s(a)),
            Value::Double3Array(a) | Value::Point3dArray(a) | Value::Vector3dArray(a) => {
                self.array(ty::VEC3D, a.len(), f64s(a))
            }
            Value::Point3fArray(a) | Value::Normal3fArray(a) | Value::Float3Array(a) => {
                let bytes = a.iter().flatten().flat_map(|&x| f32_bytes(x)).collect();
                self.array(ty::VEC3F, a.len(), bytes)
            }
            Value::Matrix4d(m) => self.stored(ty::MATRIX4D, f64s(m)),
        }
    }

    /// A dictionary: written where it stands (its values may write
    /// their own), its rep.
    fn dictionary(&mut self, d: &Dict) -> io::Result<u64> {
        let at = self.tell();
        self.dictionary_body(d)?;
        Ok(rep(ty::DICTIONARY, 0, at))
    }

    /// A dictionary's entries in their keys' order: the count, then each
    /// key (a string) and value. A value is an offset past the data it
    /// writes, that data, then its rep.
    fn dictionary_body(&mut self, d: &Dict) -> io::Result<()> {
        let mut entries: Vec<&(String, DictValue)> = d.0.iter().collect();
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        entries.dedup_by(|a, b| a.0 == b.0);
        self.u64(entries.len() as u64);
        for (k, v) in entries {
            let key = self.string(k);
            self.u32(key);
            let start = self.data.len();
            self.u64(0);
            let r = match v {
                DictValue::String(s) => {
                    let i = self.string(s);
                    rep(ty::STRING, INLINED, u64::from(i))
                }
                DictValue::Double(x) => {
                    if !x.is_finite() {
                        return Err(bad(k));
                    }
                    let at = self.tell();
                    self.data.extend_from_slice(&f64_bytes(*x));
                    rep(ty::DOUBLE, 0, at)
                }
                DictValue::Dict(inner) => self.dictionary(inner)?,
            };
            let offset = (self.data.len() - start) as u64;
            self.data[start..start + 8].copy_from_slice(&offset.to_le_bytes());
            self.u64(r);
        }
        Ok(())
    }

    /// A list op of prepended tokens (`apiSchemas`).
    fn token_list_op(&mut self, tokens: &[String]) -> u64 {
        let mut bytes = vec![PREPENDED];
        bytes.extend(self.tokens_bytes(tokens));
        self.stored(ty::TOKEN_LIST_OP, bytes)
    }

    /// A list op of prepended internal references: each an empty asset
    /// path, the prim path, the identity layer offset and no custom data.
    fn reference_list_op(&mut self, paths: &[String]) -> u64 {
        let mut bytes = vec![PREPENDED];
        bytes.extend_from_slice(&(paths.len() as u64).to_le_bytes());
        for p in paths {
            let asset = self.string("");
            let path = self.path(p);
            bytes.extend_from_slice(&asset.to_le_bytes());
            bytes.extend_from_slice(&path.to_le_bytes());
            bytes.extend_from_slice(&0f64.to_le_bytes());
            bytes.extend_from_slice(&1f64.to_le_bytes());
            bytes.extend_from_slice(&0u64.to_le_bytes());
        }
        self.stored(ty::REFERENCE_LIST_OP, bytes)
    }

    fn field(&mut self, name: &str, rep: u64) -> u32 {
        let f = (self.token(name), rep);
        if let Some(&i) = self.field_index.get(&f) {
            return i;
        }
        let i = self.fields.len() as u32;
        self.fields.push(f);
        self.field_index.insert(f, i);
        i
    }

    /// A spec of `path` with `fields`: the fields first, as OpenUSD adds
    /// them, then the path.
    fn spec(&mut self, path: &str, kind: u32, fields: Vec<u32>) {
        let set = match self.field_set_index.get(&fields) {
            Some(&i) => i,
            None => {
                let i = self.field_sets.len() as u32;
                self.field_sets.extend_from_slice(&fields);
                self.field_sets.push(u32::MAX);
                self.field_set_index.insert(fields, i);
                i
            }
        };
        let p = self.path(path);
        self.specs.push([p, set, kind]);
    }

    fn layer(&mut self, l: &Layer) -> io::Result<()> {
        if !l.meters_per_unit.is_finite() {
            return Err(bad("metersPerUnit"));
        }
        let mut fields = Vec::new();
        if !l.custom_layer_data.0.is_empty() {
            let r = self.dictionary(&l.custom_layer_data)?;
            fields.push(self.field("customLayerData", r));
        }
        if let Some(p) = &l.default_prim {
            let r = self.value(&Value::Token(p.clone()));
            fields.push(self.field("defaultPrim", r));
        }
        let r = self.value(&Value::Double(l.meters_per_unit));
        fields.push(self.field("metersPerUnit", r));
        let r = self.value(&Value::Token(l.up_axis.as_str().to_owned()));
        fields.push(self.field("upAxis", r));
        if !l.prims.is_empty() {
            let names: Vec<String> = l.prims.iter().map(|p| p.name.clone()).collect();
            let r = self.token_vector(&names);
            fields.push(self.field("primChildren", r));
        }
        self.spec("/", PSEUDO_ROOT, fields);
        for p in &l.prims {
            self.prim(p, "")?;
        }
        Ok(())
    }

    fn prim(&mut self, p: &Prim, parent: &str) -> io::Result<()> {
        let path = format!("{parent}/{}", p.name);
        let mut fields = Vec::new();
        let specifier = match p.specifier {
            Specifier::Def => 0,
            Specifier::Class => 2,
        };
        fields.push(self.field("specifier", rep(ty::SPECIFIER, INLINED, specifier)));
        if let Some(t) = &p.type_name {
            let r = self.value(&Value::Token(t.clone()));
            fields.push(self.field("typeName", r));
        }
        let m = &p.meta;
        if !m.api_schemas.is_empty() {
            let r = self.token_list_op(&m.api_schemas);
            fields.push(self.field("apiSchemas", r));
        }
        if !m.custom_data.0.is_empty() {
            let r = self.dictionary(&m.custom_data)?;
            fields.push(self.field("customData", r));
        }
        if let Some(i) = m.instanceable {
            fields.push(self.field("instanceable", rep(ty::BOOL, INLINED, u64::from(i))));
        }
        if let Some(k) = &m.kind {
            let r = self.value(&Value::Token(k.clone()));
            fields.push(self.field("kind", r));
        }
        if !m.references.is_empty() {
            let r = self.reference_list_op(&m.references);
            fields.push(self.field("references", r));
        }
        if !p.attributes.is_empty() {
            let names: Vec<String> = p.attributes.iter().map(|a| a.name.clone()).collect();
            let r = self.token_vector(&names);
            fields.push(self.field("properties", r));
        }
        if !p.children.is_empty() {
            let names: Vec<String> = p.children.iter().map(|c| c.name.clone()).collect();
            let r = self.token_vector(&names);
            fields.push(self.field("primChildren", r));
        }
        self.spec(&path, PRIM, fields);
        for a in &p.attributes {
            self.attribute(a, &path)?;
        }
        for c in &p.children {
            self.prim(c, &path)?;
        }
        Ok(())
    }

    fn attribute(&mut self, a: &Attribute, prim: &str) -> io::Result<()> {
        if !a.value.is_finite() {
            return Err(bad(&a.name));
        }
        let mut fields = Vec::new();
        let r = self.value(&Value::Token(a.value.type_name().to_owned()));
        fields.push(self.field("typeName", r));
        fields.push(self.field("custom", rep(ty::BOOL, INLINED, u64::from(a.custom))));
        fields.push(self.field(
            "variability",
            rep(ty::VARIABILITY, INLINED, u64::from(a.uniform)),
        ));
        let r = self.value(&a.value);
        fields.push(self.field("default", r));
        if let Some(i) = &a.interpolation {
            let r = self.value(&Value::Token(i.clone()));
            fields.push(self.field("interpolation", r));
        }
        self.spec(&format!("{prim}.{}", a.name), ATTRIBUTE, fields);
        Ok(())
    }

    /// Compressed integers as a section holds them: their compressed
    /// size, then the bytes.
    fn compressed(&mut self, packed: &[u8]) {
        self.u64(packed.len() as u64);
        self.data.extend_from_slice(packed);
    }

    /// The paths as three integer lists, depth first from the root: each
    /// path's index, its element's token (negated for a property), and
    /// where its next sibling is (`n` ahead, past its subtree), 0 for a
    /// sibling next and no child, -1 for a child and no sibling, -2 for
    /// neither.
    fn path_lists(&self) -> (Vec<u32>, Vec<i32>, Vec<i32>) {
        let n = self.paths.len();
        // Each subtree's size, children always numbered after parents.
        let mut size = vec![1i32; n];
        for node in (0..n).rev() {
            let s: i32 = self.paths[node]
                .children
                .iter()
                .map(|&c| size[c as usize])
                .sum();
            size[node] += s;
        }
        let (mut index, mut element, mut jump) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        // (path, whether a sibling follows it); the root has none.
        let mut stack = vec![(0u32, false)];
        while let Some((node, sibling)) = stack.pop() {
            let p = &self.paths[node as usize];
            index.push(node);
            let e = p.element as i32;
            element.push(if p.property { -e } else { e });
            let child = !p.children.is_empty();
            jump.push(match (child, sibling) {
                (true, true) => size[node as usize],
                (false, true) => 0,
                (true, false) => -1,
                (false, false) => -2,
            });
            let last = p.children.len();
            for (k, &c) in p.children.iter().enumerate().rev() {
                stack.push((c, k + 1 < last));
            }
        }
        (index, element, jump)
    }

    /// The sections and the table of contents after the values, and the
    /// bootstrap before them.
    fn finish(mut self) -> Vec<u8> {
        let mut toc: Vec<(&str, u64, u64)> = Vec::new();

        let start = self.tell();
        self.u64(self.tokens.len() as u64);
        let mut text = Vec::new();
        for t in &self.tokens {
            text.extend_from_slice(t.as_bytes());
            text.push(0);
        }
        self.u64(text.len() as u64);
        let packed = compress::fast(&text);
        self.compressed(&packed);
        toc.push(("TOKENS", start, self.tell() - start));

        let start = self.tell();
        self.u64(self.strings.len() as u64);
        for i in 0..self.strings.len() {
            self.u32(self.strings[i]);
        }
        toc.push(("STRINGS", start, self.tell() - start));

        let start = self.tell();
        self.u64(self.fields.len() as u64);
        let tokens: Vec<u32> = self.fields.iter().map(|f| f.0).collect();
        self.compressed(&compress::uints(&tokens));
        let reps: Vec<u8> = self.fields.iter().flat_map(|f| f.1.to_le_bytes()).collect();
        let packed = compress::fast(&reps);
        self.compressed(&packed);
        toc.push(("FIELDS", start, self.tell() - start));

        let start = self.tell();
        self.u64(self.field_sets.len() as u64);
        let packed = compress::uints(&self.field_sets);
        self.compressed(&packed);
        toc.push(("FIELDSETS", start, self.tell() - start));

        let start = self.tell();
        let (index, element, jump) = self.path_lists();
        self.u64(self.paths.len() as u64);
        self.u64(index.len() as u64);
        self.compressed(&compress::uints(&index));
        self.compressed(&compress::ints(&element));
        self.compressed(&compress::ints(&jump));
        toc.push(("PATHS", start, self.tell() - start));

        let start = self.tell();
        self.u64(self.specs.len() as u64);
        for k in 0..3 {
            let column: Vec<u32> = self.specs.iter().map(|s| s[k]).collect();
            self.compressed(&compress::uints(&column));
        }
        toc.push(("SPECS", start, self.tell() - start));

        let toc_at = self.tell();
        self.u64(toc.len() as u64);
        for (name, start, size) in toc {
            let mut n = [0u8; 16];
            n[..name.len()].copy_from_slice(name.as_bytes());
            self.data.extend_from_slice(&n);
            self.u64(start);
            self.u64(size);
        }
        self.data[..8].copy_from_slice(b"PXR-USDC");
        self.data[8..11].copy_from_slice(&VERSION);
        self.data[16..24].copy_from_slice(&toc_at.to_le_bytes());
        self.data
    }
}

/// `layer` as usdc bytes.
pub fn to_bytes(layer: &Layer) -> io::Result<Vec<u8>> {
    let mut c = Crate::new();
    c.layer(layer)?;
    Ok(c.finish())
}

/// Writes `layer` as usdc.
pub fn write(layer: &Layer, out: &mut dyn Write) -> io::Result<()> {
    out.write_all(&to_bytes(layer)?)
}
