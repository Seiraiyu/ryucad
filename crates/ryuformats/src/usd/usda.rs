//! `usda`: a [`Layer`] as canonical text.
//!
//! The layer's metadata in a block of its own (`customLayerData`,
//! `defaultPrim`, `metersPerUnit`, `upAxis`), then each root prim; inside
//! a prim its metadata in parentheses, its attributes in order, then its
//! children, each after a blank line, indented four spaces a level.
//! Doubles are the shortest text that reads back to the same bits, floats
//! the same for `f32`, never in exponent form, `-0` as `0`. Strings and
//! tokens are quoted with `\`, `"` and control characters escaped. The
//! same layer always gives the same text; a value that isn't finite is
//! `InvalidData`, before anything of its attribute is written.

use super::layer::{Attribute, Dict, DictValue, Layer, Prim, Value, is_identifier};
use crate::text::{push_f32, push_f64};
use std::fmt::Write as _;
use std::io::{self, Write};

/// Writes `layer` as usda.
pub fn write(layer: &Layer, out: &mut dyn Write) -> io::Result<()> {
    let mut s = String::from("#usda 1.0\n(\n");
    if !layer.custom_layer_data.0.is_empty() {
        s.push_str("    customLayerData = ");
        dict(&mut s, &layer.custom_layer_data, 1);
        s.push('\n');
    }
    if let Some(p) = &layer.default_prim {
        s.push_str("    defaultPrim = ");
        quoted(&mut s, p);
        s.push('\n');
    }
    s.push_str("    metersPerUnit = ");
    number(&mut s, layer.meters_per_unit)?;
    let _ = writeln!(s, "\n    upAxis = \"{}\"\n)", layer.up_axis.as_str());
    out.write_all(s.as_bytes())?;
    for p in &layer.prims {
        out.write_all(b"\n")?;
        prim(p, 0, out)?;
    }
    Ok(())
}

/// The layer as a string, for tests and small layers.
pub fn to_string(layer: &Layer) -> io::Result<String> {
    let mut bytes = Vec::new();
    write(layer, &mut bytes)?;
    Ok(String::from_utf8(bytes).expect("usda is UTF-8"))
}

fn indent(s: &mut String, level: usize) {
    for _ in 0..level {
        s.push_str("    ");
    }
}

fn prim(p: &Prim, level: usize, out: &mut dyn Write) -> io::Result<()> {
    let mut s = String::new();
    indent(&mut s, level);
    s.push_str(p.specifier.as_str());
    if let Some(t) = &p.type_name {
        s.push(' ');
        s.push_str(t);
    }
    s.push(' ');
    quoted(&mut s, &p.name);
    let m = &p.meta;
    if !m.is_empty() {
        s.push_str(" (\n");
        if !m.api_schemas.is_empty() {
            indent(&mut s, level + 1);
            s.push_str("prepend apiSchemas = ");
            list(&mut s, &m.api_schemas, |s, t| quoted(s, t));
            s.push('\n');
        }
        if !m.custom_data.0.is_empty() {
            indent(&mut s, level + 1);
            s.push_str("customData = ");
            dict(&mut s, &m.custom_data, level + 1);
            s.push('\n');
        }
        if let Some(i) = m.instanceable {
            indent(&mut s, level + 1);
            let _ = writeln!(s, "instanceable = {i}");
        }
        if let Some(k) = &m.kind {
            indent(&mut s, level + 1);
            s.push_str("kind = ");
            quoted(&mut s, k);
            s.push('\n');
        }
        if !m.references.is_empty() {
            indent(&mut s, level + 1);
            s.push_str("prepend references = ");
            if let [one] = m.references.as_slice() {
                let _ = write!(s, "<{one}>");
            } else {
                list(&mut s, &m.references, |s, r| {
                    let _ = write!(s, "<{r}>");
                });
            }
            s.push('\n');
        }
        indent(&mut s, level);
        s.push(')');
    }
    s.push('\n');
    indent(&mut s, level);
    s.push_str("{\n");
    out.write_all(s.as_bytes())?;
    for a in &p.attributes {
        attribute(a, level + 1, out)?;
    }
    for (i, c) in p.children.iter().enumerate() {
        if i > 0 || !p.attributes.is_empty() {
            out.write_all(b"\n")?;
        }
        prim(c, level + 1, out)?;
    }
    let mut s = String::new();
    indent(&mut s, level);
    s.push_str("}\n");
    out.write_all(s.as_bytes())
}

fn attribute(a: &Attribute, level: usize, out: &mut dyn Write) -> io::Result<()> {
    if !a.value.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} holds a number that isn't finite", a.name),
        ));
    }
    let mut s = String::new();
    indent(&mut s, level);
    if a.custom {
        s.push_str("custom ");
    }
    if a.uniform {
        s.push_str("uniform ");
    }
    let _ = write!(s, "{} {} = ", a.value.type_name(), a.name);
    value(&mut s, &a.value);
    if let Some(i) = &a.interpolation {
        s.push_str(" (\n");
        indent(&mut s, level + 1);
        s.push_str("interpolation = ");
        quoted(&mut s, i);
        s.push('\n');
        indent(&mut s, level);
        s.push(')');
    }
    s.push('\n');
    out.write_all(s.as_bytes())
}

/// A finite double, else `InvalidData`.
fn number(s: &mut String, x: f64) -> io::Result<()> {
    if !x.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a number that isn't finite",
        ));
    }
    push_f64(s, x);
    Ok(())
}

/// `text` quoted, with `\`, `"` and control characters escaped.
pub(crate) fn quoted(s: &mut String, text: &str) {
    s.push('"');
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(s, "\\x{:02x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

fn list<T>(s: &mut String, items: &[T], mut one: impl FnMut(&mut String, &T)) {
    s.push('[');
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        one(s, x);
    }
    s.push(']');
}

fn tuple64<const N: usize>(s: &mut String, t: &[f64; N]) {
    s.push('(');
    for (i, &x) in t.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        push_f64(s, x);
    }
    s.push(')');
}

fn tuple32(s: &mut String, t: &[f32; 3]) {
    s.push('(');
    for (i, &x) in t.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        push_f32(s, x);
    }
    s.push(')');
}

fn value(s: &mut String, v: &Value) {
    match v {
        Value::Bool(b) => {
            let _ = write!(s, "{b}");
        }
        Value::Int(i) => {
            let _ = write!(s, "{i}");
        }
        Value::Double(x) => push_f64(s, *x),
        Value::Token(t) | Value::String(t) => quoted(s, t),
        Value::IntArray(a) => list(s, a, |s, i| {
            let _ = write!(s, "{i}");
        }),
        Value::UIntArray(a) => list(s, a, |s, i| {
            let _ = write!(s, "{i}");
        }),
        Value::DoubleArray(a) => list(s, a, |s, &x| push_f64(s, x)),
        Value::TokenArray(a) | Value::StringArray(a) => list(s, a, |s, t| quoted(s, t)),
        Value::Int2Array(a) => list(s, a, |s, p| {
            let _ = write!(s, "({}, {})", p[0], p[1]);
        }),
        Value::Double2Array(a) => list(s, a, tuple64),
        Value::Double3Array(a) | Value::Point3dArray(a) | Value::Vector3dArray(a) => {
            list(s, a, tuple64)
        }
        Value::Point3fArray(a) | Value::Normal3fArray(a) | Value::Float3Array(a) => {
            list(s, a, tuple32)
        }
        Value::Matrix4d(m) => {
            s.push_str("( ");
            for (i, row) in m.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                tuple64(s, row);
            }
            s.push_str(" )");
        }
    }
}

fn dict(s: &mut String, d: &Dict, level: usize) {
    s.push_str("{\n");
    for (k, v) in &d.0 {
        indent(s, level + 1);
        let ty = match v {
            DictValue::String(_) => "string",
            DictValue::Double(_) => "double",
            DictValue::Dict(_) => "dictionary",
        };
        s.push_str(ty);
        s.push(' ');
        if is_identifier(k) {
            s.push_str(k);
        } else {
            quoted(s, k);
        }
        s.push_str(" = ");
        match v {
            DictValue::String(t) => quoted(s, t),
            DictValue::Double(x) => push_f64(s, *x),
            DictValue::Dict(d) => dict(s, d, level + 1),
        }
        s.push('\n');
    }
    indent(s, level);
    s.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usd::layer::{Specifier, UpAxis};

    fn layer() -> Layer {
        let mut data = Dict::default();
        let mut ryucad = Dict::default();
        ryucad.string("version", "0.0.0").string("chord", "0.01 mm");
        data.dict("ryucad", ryucad);
        let mut root = Prim::new(Specifier::Def, Some("Xform"), "frame").unwrap();
        root.meta.kind = Some("assembly".into());
        let mut proto = Prim::new(Specifier::Class, None, "Prototypes").unwrap();
        let mut pin = Prim::new(Specifier::Class, Some("Xform"), "pin").unwrap();
        pin.meta
            .custom_data
            .string("part", "pin")
            .string("odd key", "a \"b\"\n");
        proto.children.push(pin);
        let mut a = Prim::new(Specifier::Def, Some("Xform"), "a").unwrap();
        a.meta.instanceable = Some(true);
        a.meta.references = vec!["/Prototypes/pin".into()];
        let mut m = [[0.0; 4]; 4];
        for (i, row) in m.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        m[3] = [10.0, -0.0, 0.5, 1.0];
        a.varying("xformOp:transform", Value::Matrix4d(m)).unwrap();
        a.uniform(
            "xformOpOrder",
            Value::TokenArray(vec!["xformOp:transform".into()]),
        )
        .unwrap();
        root.children.push(a);
        let mut mesh = Prim::new(Specifier::Def, Some("Mesh"), "mesh").unwrap();
        mesh.meta.api_schemas = vec!["BrepPointAPI:vertexPoint".into()];
        mesh.varying("points", Value::Point3fArray(vec![[0.1, 2.0, -3.5]]))
            .unwrap();
        mesh.varying("normals", Value::Normal3fArray(vec![[0.0, 0.0, 1.0]]))
            .unwrap()
            .interpolation("faceVarying");
        mesh.uniform("ryucad:faceName", Value::StringArray(vec!["ab".into()]))
            .unwrap()
            .custom();
        mesh.uniform("edge:vertexIndices", Value::Int2Array(vec![[0, 1]]))
            .unwrap();
        mesh.uniform("doubleSided", Value::Bool(false)).unwrap();
        root.children.push(mesh);
        Layer {
            default_prim: Some("frame".into()),
            meters_per_unit: 0.001,
            up_axis: UpAxis::Z,
            custom_layer_data: data,
            prims: vec![proto, root],
        }
    }

    #[test]
    fn a_layer_as_text() {
        let text = to_string(&layer()).unwrap();
        let want = r#"#usda 1.0
(
    customLayerData = {
        dictionary ryucad = {
            string version = "0.0.0"
            string chord = "0.01 mm"
        }
    }
    defaultPrim = "frame"
    metersPerUnit = 0.001
    upAxis = "Z"
)

class "Prototypes"
{
    class Xform "pin" (
        customData = {
            string part = "pin"
            string "odd key" = "a \"b\"\n"
        }
    )
    {
    }
}

def Xform "frame" (
    kind = "assembly"
)
{
    def Xform "a" (
        instanceable = true
        prepend references = </Prototypes/pin>
    )
    {
        matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (10, 0, 0.5, 1) )
        uniform token[] xformOpOrder = ["xformOp:transform"]
    }

    def Mesh "mesh" (
        prepend apiSchemas = ["BrepPointAPI:vertexPoint"]
    )
    {
        point3f[] points = [(0.1, 2, -3.5)]
        normal3f[] normals = [(0, 0, 1)] (
            interpolation = "faceVarying"
        )
        custom uniform string[] ryucad:faceName = ["ab"]
        uniform int2[] edge:vertexIndices = [(0, 1)]
        uniform bool doubleSided = false
    }
}
"#;
        assert_eq!(text, want);
    }

    #[test]
    fn numbers_that_arent_finite_are_refused() {
        let mut l = layer();
        l.prims[1].children[1]
            .uniform("extent", Value::Float3Array(vec![[f32::NAN, 0.0, 0.0]]))
            .unwrap();
        let e = to_string(&l).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        l = layer();
        l.meters_per_unit = f64::INFINITY;
        assert!(to_string(&l).is_err());
    }
}
