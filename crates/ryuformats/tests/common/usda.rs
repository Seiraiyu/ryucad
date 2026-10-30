//! A reader of the usda subset ryuformats writes, back into its layer
//! model, so a layer written, read and written again can be compared.
//! Panics on anything outside the subset.

use ryuformats::usd::layer::{
    Attribute, Dict, DictValue, Layer, Prim, PrimMeta, Specifier, UpAxis, Value,
};

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Num(String),
    Path(String),
    P(char),
}

fn lex(text: &str) -> Vec<Tok> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let x = c[i];
        if x.is_whitespace() {
            i += 1;
        } else if x == '#' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if x == '"' {
            let mut s = String::new();
            i += 1;
            while c[i] != '"' {
                if c[i] == '\\' {
                    i += 1;
                    match c[i] {
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        'x' => {
                            let h: String = c[i + 1..i + 3].iter().collect();
                            s.push(char::from(u8::from_str_radix(&h, 16).unwrap()));
                            i += 2;
                        }
                        e => s.push(e),
                    }
                } else {
                    s.push(c[i]);
                }
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if x == '<' {
            let end = i + c[i..].iter().position(|&y| y == '>').unwrap();
            out.push(Tok::Path(c[i + 1..end].iter().collect()));
            i = end + 1;
        } else if x == '-' || x.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < c.len()
                && (c[i].is_ascii_digit() || c[i] == '.' || c[i] == 'e' || c[i] == '-')
            {
                i += 1;
            }
            out.push(Tok::Num(c[start..i].iter().collect()));
        } else if x.is_ascii_alphabetic() || x == '_' {
            let start = i;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || "_:[]".contains(c[i])) {
                // `[]` belongs to a type name only when it closes one.
                if c[i] == '[' && c.get(i + 1) != Some(&']') {
                    break;
                }
                if c[i] == ']' && c[i - 1] != '[' {
                    break;
                }
                i += 1;
            }
            out.push(Tok::Word(c[start..i].iter().collect()));
        } else {
            out.push(Tok::P(x));
            i += 1;
        }
    }
    out
}

struct R {
    t: Vec<Tok>,
    i: usize,
}

impl R {
    fn peek(&self) -> &Tok {
        &self.t[self.i]
    }
    fn next(&mut self) -> Tok {
        self.i += 1;
        self.t[self.i - 1].clone()
    }
    fn p(&mut self, x: char) {
        assert_eq!(self.next(), Tok::P(x));
    }
    fn is(&self, x: char) -> bool {
        self.i < self.t.len() && self.t[self.i] == Tok::P(x)
    }
    fn word(&mut self) -> String {
        match self.next() {
            Tok::Word(w) => w,
            t => panic!("a word, not {t:?}"),
        }
    }
    fn string(&mut self) -> String {
        match self.next() {
            Tok::Str(s) => s,
            t => panic!("a string, not {t:?}"),
        }
    }
    fn num(&mut self) -> String {
        match self.next() {
            Tok::Num(n) => n,
            t => panic!("a number, not {t:?}"),
        }
    }
    fn f64(&mut self) -> f64 {
        self.num().parse().unwrap()
    }
    fn list<T>(&mut self, mut one: impl FnMut(&mut R) -> T) -> Vec<T> {
        self.p('[');
        let mut v = Vec::new();
        while !self.is(']') {
            v.push(one(self));
            if self.is(',') {
                self.p(',');
            }
        }
        self.p(']');
        v
    }
    fn tuple<const N: usize, T: Default + Copy>(
        &mut self,
        mut one: impl FnMut(&mut R) -> T,
    ) -> [T; N] {
        self.p('(');
        let mut out = [T::default(); N];
        for (k, x) in out.iter_mut().enumerate() {
            if k > 0 {
                self.p(',');
            }
            *x = one(self);
        }
        self.p(')');
        out
    }
    fn dict(&mut self) -> Dict {
        self.p('{');
        let mut d = Dict::default();
        while !self.is('}') {
            let ty = self.word();
            let key = match self.next() {
                Tok::Word(w) => w,
                Tok::Str(s) => s,
                t => panic!("a key, not {t:?}"),
            };
            self.p('=');
            let v = match ty.as_str() {
                "string" => DictValue::String(self.string()),
                "double" => DictValue::Double(self.f64()),
                "dictionary" => DictValue::Dict(self.dict()),
                t => panic!("dictionary type {t}"),
            };
            d.0.push((key, v));
        }
        self.p('}');
        d
    }
    fn value(&mut self, ty: &str) -> Value {
        let f32s = |r: &mut R| r.tuple::<3, f32>(|r| r.num().parse().unwrap());
        let f64s = |r: &mut R| r.tuple::<3, f64>(|r| r.f64());
        match ty {
            "bool" => Value::Bool(self.word() == "true"),
            "int" => Value::Int(self.num().parse().unwrap()),
            "double" => Value::Double(self.f64()),
            "token" => Value::Token(self.string()),
            "string" => Value::String(self.string()),
            "int[]" => Value::IntArray(self.list(|r| r.num().parse().unwrap())),
            "uint[]" => Value::UIntArray(self.list(|r| r.num().parse().unwrap())),
            "double[]" => Value::DoubleArray(self.list(|r| r.f64())),
            "token[]" => Value::TokenArray(self.list(|r| r.string())),
            "string[]" => Value::StringArray(self.list(|r| r.string())),
            "int2[]" => {
                Value::Int2Array(self.list(|r| r.tuple::<2, i32>(|r| r.num().parse().unwrap())))
            }
            "double2[]" => Value::Double2Array(self.list(|r| r.tuple::<2, f64>(|r| r.f64()))),
            "double3[]" => Value::Double3Array(self.list(f64s)),
            "point3d[]" => Value::Point3dArray(self.list(f64s)),
            "vector3d[]" => Value::Vector3dArray(self.list(f64s)),
            "point3f[]" => Value::Point3fArray(self.list(f32s)),
            "normal3f[]" => Value::Normal3fArray(self.list(f32s)),
            "float3[]" => Value::Float3Array(self.list(f32s)),
            "matrix4d" => {
                Value::Matrix4d(self.tuple::<4, [f64; 4]>(|r| r.tuple::<4, f64>(|r| r.f64())))
            }
            t => panic!("type {t}"),
        }
    }
    fn prim(&mut self) -> Prim {
        let specifier = match self.word().as_str() {
            "def" => Specifier::Def,
            "class" => Specifier::Class,
            s => panic!("specifier {s}"),
        };
        let type_name = match self.peek() {
            Tok::Word(_) => Some(self.word()),
            _ => None,
        };
        let name = self.string();
        let mut meta = PrimMeta::default();
        if self.is('(') {
            self.p('(');
            while !self.is(')') {
                match self.word().as_str() {
                    "prepend" => match self.word().as_str() {
                        "apiSchemas" => {
                            self.p('=');
                            meta.api_schemas = self.list(|r| r.string());
                        }
                        "references" => {
                            self.p('=');
                            meta.references = if self.is('[') {
                                self.list(|r| match r.next() {
                                    Tok::Path(p) => p,
                                    t => panic!("{t:?}"),
                                })
                            } else {
                                match self.next() {
                                    Tok::Path(p) => vec![p],
                                    t => panic!("{t:?}"),
                                }
                            };
                        }
                        w => panic!("prepend {w}"),
                    },
                    "customData" => {
                        self.p('=');
                        meta.custom_data = self.dict();
                    }
                    "instanceable" => {
                        self.p('=');
                        meta.instanceable = Some(self.word() == "true");
                    }
                    "kind" => {
                        self.p('=');
                        meta.kind = Some(self.string());
                    }
                    w => panic!("prim metadata {w}"),
                }
            }
            self.p(')');
        }
        self.p('{');
        let mut attributes = Vec::new();
        let mut children = Vec::new();
        while !self.is('}') {
            match self.peek() {
                Tok::Word(w) if w == "def" || w == "class" => children.push(self.prim()),
                _ => {
                    let mut custom = false;
                    let mut uniform = false;
                    let mut ty = self.word();
                    if ty == "custom" {
                        custom = true;
                        ty = self.word();
                    }
                    if ty == "uniform" {
                        uniform = true;
                        ty = self.word();
                    }
                    let name = self.word();
                    self.p('=');
                    let value = self.value(&ty);
                    let mut interpolation = None;
                    if self.is('(') {
                        self.p('(');
                        assert_eq!(self.word(), "interpolation");
                        self.p('=');
                        interpolation = Some(self.string());
                        self.p(')');
                    }
                    attributes.push(Attribute {
                        name,
                        uniform,
                        custom,
                        value,
                        interpolation,
                    });
                }
            }
        }
        self.p('}');
        Prim {
            specifier,
            type_name,
            name,
            meta,
            attributes,
            children,
        }
    }
}

/// Reads usda as ryuformats writes it.
pub fn read(text: &str) -> Layer {
    assert!(text.starts_with("#usda 1.0\n"), "not usda 1.0");
    let mut r = R { t: lex(text), i: 0 };
    let mut layer = Layer {
        default_prim: None,
        meters_per_unit: 0.0,
        up_axis: UpAxis::Y,
        custom_layer_data: Dict::default(),
        prims: Vec::new(),
    };
    r.p('(');
    while !r.is(')') {
        let key = r.word();
        r.p('=');
        match key.as_str() {
            "customLayerData" => layer.custom_layer_data = r.dict(),
            "defaultPrim" => layer.default_prim = Some(r.string()),
            "metersPerUnit" => layer.meters_per_unit = r.f64(),
            "upAxis" => {
                layer.up_axis = match r.string().as_str() {
                    "Z" => UpAxis::Z,
                    _ => UpAxis::Y,
                }
            }
            k => panic!("layer metadata {k}"),
        }
    }
    r.p(')');
    while r.i < r.t.len() {
        layer.prims.push(r.prim());
    }
    layer
}
