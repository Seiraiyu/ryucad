//! 3MF: the 3MF Consortium's core specification (1.3), with no
//! extensions. A package (a zip, [`crate::zip`]) of `[Content_Types].xml`,
//! `_rels/.rels` and `3D/3dmodel.model`.
//!
//! The model holds an object per product, numbered by a walk that writes
//! what an object uses before it: a product of one solid and nothing
//! placed is that solid's mesh object; any other is an object of
//! components, its solids' mesh objects first (named for their solids),
//! then each child's object with its placing as a `transform` in the
//! file's unit. Objects carry the product's name and part number
//! (`partnumber`, its identifier); a product placed many times is one
//! object, so instancing is kept. A product with nothing to show has no
//! object; the build has one item, the root's object, if it has one.
//! Vertices are placed and scaled in f64, rounded to `f32` once and
//! written as the shortest text that reads back to them.

use crate::scene::{Scene, Solid, Unit, placed_point};
use crate::text::{push_f32, push_f64};
use crate::zip::{self, Entry};
use ryukei::geom::Rigid;
use std::fmt::Write as _;
use std::io::{self, Write};

/// The 3MF core namespace, which version 1.3 keeps.
pub const CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

/// The namespace of ryucad's own metadata (`ryucad:chord`, …).
pub const RYUCAD: &str = "https://seiraiyu.com/ryucad/3mf/2026";

/// How to write a 3MF file: its unit and its metadata, by name in order
/// (`Title`, `Application`, `CreationDate`, or `ryucad:`-prefixed names).
#[derive(Clone, Debug, Default)]
pub struct ThreeMf {
    pub unit: Unit,
    pub metadata: Vec<(String, String)>,
}

/// Text made safe for an XML attribute or element: `&`, `<`, `>`, `"`
/// and `'` escaped, characters XML 1.0 can't hold replaced by U+FFFD.
fn escape(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => {
                let _ = write!(out, "&#{};", c as u32);
            }
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
}

/// A 3MF transform: the rotation's columns as rows, then the translation,
/// in `unit`.
fn transform(out: &mut String, place: &Rigid, unit: Unit) {
    let r = place.rotation_matrix();
    let t = place.translation_part();
    let k = unit.per_meter();
    let values = [
        r[0][0],
        r[1][0],
        r[2][0],
        r[0][1],
        r[1][1],
        r[2][1],
        r[0][2],
        r[1][2],
        r[2][2],
        t.x * k,
        t.y * k,
        t.z * k,
    ];
    for (i, v) in values.into_iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        push_f64(out, v);
    }
}

/// The walk numbering objects and writing them.
struct Objects<'a> {
    scene: &'a Scene,
    unit: Unit,
    /// Each product's object, once written (`Some(None)`: it has none).
    done: Vec<Option<Option<u32>>>,
    next: u32,
    xml: String,
}

impl Objects<'_> {
    fn id(&mut self) -> u32 {
        self.next += 1;
        self.next
    }

    /// An object's opening tag.
    fn open(&mut self, id: u32, name: &str, number: Option<&str>) {
        let _ = write!(self.xml, "  <object id=\"{id}\" type=\"model\" name=\"");
        escape(&mut self.xml, name);
        self.xml.push('"');
        if let Some(n) = number {
            self.xml.push_str(" partnumber=\"");
            escape(&mut self.xml, n);
            self.xml.push('"');
        }
        self.xml.push_str(">\n");
    }

    /// A mesh object for `s`, named `name`.
    fn mesh(&mut self, s: &Solid, name: &str, number: Option<&str>) -> u32 {
        let id = self.id();
        self.open(id, name, number);
        let x = &mut self.xml;
        x.push_str("   <mesh>\n    <vertices>\n");
        for &p in &s.mesh.positions {
            let v = placed_point(&Rigid::IDENTITY, self.unit, p).map(|c| c as f32);
            x.push_str("     <vertex x=\"");
            push_f32(x, v[0]);
            x.push_str("\" y=\"");
            push_f32(x, v[1]);
            x.push_str("\" z=\"");
            push_f32(x, v[2]);
            x.push_str("\"/>\n");
        }
        x.push_str("    </vertices>\n    <triangles>\n");
        for [a, b, c] in s.mesh.welded_triangles() {
            let _ = writeln!(x, "     <triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>");
        }
        x.push_str("    </triangles>\n   </mesh>\n  </object>\n");
        id
    }

    /// Product `k`'s object, written with everything it uses first.
    fn product(&mut self, k: usize) -> Option<u32> {
        if let Some(done) = self.done[k] {
            return done;
        }
        let scene = self.scene;
        let p = &scene.products[k];
        let mut placed = Vec::new();
        for c in &p.children {
            if let Some(id) = self.product(c.product) {
                placed.push((id, c.place));
            }
        }
        let solids: Vec<&Solid> = p
            .solids
            .iter()
            .filter(|s| !s.mesh.triangles.is_empty())
            .collect();
        let id = match (solids.as_slice(), placed.is_empty()) {
            ([], true) => None,
            ([only], true) => Some(self.mesh(only, &p.name, Some(&p.id))),
            _ => {
                let own: Vec<u32> = solids.iter().map(|s| self.mesh(s, &s.name, None)).collect();
                let id = self.id();
                self.open(id, &p.name, Some(&p.id));
                self.xml.push_str("   <components>\n");
                for o in own {
                    let _ = writeln!(self.xml, "    <component objectid=\"{o}\"/>");
                }
                for (o, place) in placed {
                    let _ = write!(self.xml, "    <component objectid=\"{o}\" transform=\"");
                    transform(&mut self.xml, &place, self.unit);
                    self.xml.push_str("\"/>\n");
                }
                self.xml.push_str("   </components>\n  </object>\n");
                Some(id)
            }
        };
        self.done[k] = Some(id);
        id
    }
}

/// The model part, `3D/3dmodel.model`.
pub fn model(scene: &Scene, opts: &ThreeMf) -> String {
    let mut xml = String::new();
    let _ = writeln!(xml, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        xml,
        "<model unit=\"{}\" xml:lang=\"en-US\" xmlns=\"{CORE}\" xmlns:ryucad=\"{RYUCAD}\">",
        opts.unit.name()
    );
    for (name, value) in &opts.metadata {
        xml.push_str(" <metadata name=\"");
        escape(&mut xml, name);
        xml.push_str("\">");
        escape(&mut xml, value);
        xml.push_str("</metadata>\n");
    }
    xml.push_str(" <resources>\n");
    let mut objects = Objects {
        scene,
        unit: opts.unit,
        done: vec![None; scene.products.len()],
        next: 0,
        xml,
    };
    let root = if scene.products.is_empty() {
        None
    } else {
        objects.product(0)
    };
    let mut xml = objects.xml;
    xml.push_str(" </resources>\n");
    match root {
        Some(id) => {
            let _ = writeln!(xml, " <build>\n  <item objectid=\"{id}\"/>\n </build>");
        }
        None => xml.push_str(" <build/>\n"),
    }
    xml.push_str("</model>\n");
    xml
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">
 <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>
 <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>
</Types>
";

const RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">
 <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>
</Relationships>
";

/// Writes `scene` as a 3MF package.
pub fn write(scene: &Scene, opts: &ThreeMf, out: &mut dyn Write) -> io::Result<()> {
    let model = model(scene, opts);
    let entries = [
        Entry {
            name: "[Content_Types].xml",
            data: CONTENT_TYPES.as_bytes(),
        },
        Entry {
            name: "_rels/.rels",
            data: RELS.as_bytes(),
        },
        Entry {
            name: "3D/3dmodel.model",
            data: model.as_bytes(),
        },
    ];
    zip::write(&entries, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::tests::{at, block, frame};
    use crate::scene::{Child, Product};

    fn options() -> ThreeMf {
        ThreeMf {
            unit: Unit::Millimeter,
            metadata: vec![
                ("Title".into(), "Frame & <co>".into()),
                ("ryucad:chord".into(), "0.01 mm".into()),
            ],
        }
    }

    #[test]
    fn an_assembly_is_objects_of_components() {
        let xml = model(&frame(), &options());
        let lines: Vec<&str> = xml.lines().map(str::trim).collect();
        assert_eq!(
            lines[1],
            format!(
                "<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE}\" xmlns:ryucad=\"{RYUCAD}\">"
            )
        );
        assert_eq!(
            lines[2],
            "<metadata name=\"Title\">Frame &amp; &lt;co&gt;</metadata>"
        );
        assert_eq!(
            lines[3],
            "<metadata name=\"ryucad:chord\">0.01 mm</metadata>"
        );
        let objects: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| l.starts_with("<object"))
            .collect();
        // The pin once, then the frame's plate, then the frame.
        assert_eq!(
            objects,
            [
                "<object id=\"1\" type=\"model\" name=\"Pin\" partnumber=\"P-1\">",
                "<object id=\"2\" type=\"model\" name=\"plate\">",
                "<object id=\"3\" type=\"model\" name=\"Frame\" partnumber=\"F-1\">",
            ]
        );
        let components: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| l.starts_with("<component "))
            .collect();
        assert_eq!(
            components,
            [
                "<component objectid=\"2\"/>",
                "<component objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 0 0 10\"/>",
                "<component objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 50 0 10\"/>",
            ]
        );
        assert!(xml.ends_with(" <build>\n  <item objectid=\"3\"/>\n </build>\n</model>\n"));
        // The pin's vertices in millimetres.
        assert!(
            xml.contains("<vertex x=\"10\" y=\"10\" z=\"20\"/>"),
            "{xml}"
        );
    }

    #[test]
    fn transforms_take_rotations_by_columns() {
        use ryukei::geom::UnitVector3;
        let quarter = at(0.001, 0.002, 0.003).then_after(&Rigid::rotation(
            UnitVector3::Z,
            core::f64::consts::FRAC_PI_2,
        ));
        let mut s = String::new();
        transform(&mut s, &quarter, Unit::Meter);
        let v: Vec<f64> = s.split(' ').map(|x| x.parse().unwrap()).collect();
        // x goes to y: the first row is the image of x.
        assert!((v[1] - 1.0).abs() < 1e-15 && v[0].abs() < 1e-15, "{s}");
        assert!((v[3] + 1.0).abs() < 1e-15, "{s}");
        assert_eq!(&v[9..], [0.001, 0.002, 0.003]);
    }

    #[test]
    fn a_part_of_one_solid_is_its_mesh() {
        let one = Scene {
            products: vec![Product {
                id: "PN-7".into(),
                name: "Spacer".into(),
                part: "p".into(),
                solids: vec![Solid {
                    name: "a".into(),
                    mesh: block(0.01, 0.01, 0.01),
                }],
                children: vec![],
            }],
        };
        let xml = model(&one, &ThreeMf::default());
        assert!(xml.contains(
            "<object id=\"1\" type=\"model\" name=\"Spacer\" partnumber=\"PN-7\">\n   <mesh>"
        ));
        assert!(!xml.contains("<components>"));
        assert!(xml.contains("<item objectid=\"1\"/>"));
        assert_eq!(xml.matches("<vertex ").count(), 8);
        assert_eq!(xml.matches("<triangle ").count(), 12);
    }

    #[test]
    fn what_shows_nothing_has_no_object() {
        let empty = Scene {
            products: vec![
                Product {
                    part: "a".into(),
                    children: vec![Child {
                        product: 1,
                        place: Rigid::IDENTITY,
                        name: "e".into(),
                    }],
                    ..Product::default()
                },
                Product::default(),
            ],
        };
        let xml = model(&empty, &ThreeMf::default());
        assert!(!xml.contains("<object"));
        assert!(xml.contains(" <build/>\n"));
        assert!(model(&Scene::default(), &ThreeMf::default()).contains(" <build/>\n"));
    }

    #[test]
    fn escaping() {
        let mut s = String::new();
        escape(&mut s, "a&b<c>\"d'e\tf\u{1}g");
        assert_eq!(s, "a&amp;b&lt;c&gt;&quot;d&apos;e&#9;f\u{fffd}g");
    }
}
