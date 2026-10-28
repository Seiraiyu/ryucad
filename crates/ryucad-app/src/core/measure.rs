//! Measurement as the window shows it: the Measure tool's picks and the
//! last `measure` answer, the Mass properties panel's `mass` answer, and
//! the Clearance dialog's `clearance` answer. Everything is read from the
//! daemon; nothing is worked out here.

use ryucad::error::Error;
use serde_json::Value;

/// A label and the value it reads, as a panel lists them.
pub type Line = (String, String);

/// A value of an answer as people read it: its text in the default unit,
/// and `(exact)` or its bound in SI units.
pub fn shown(v: &Value) -> String {
    let text = v["value"].as_str().unwrap_or_default();
    if v["exact"] == true {
        return format!("{text} (exact)");
    }
    let units = [
        ("bound_m", "m"),
        ("bound_m2", "m²"),
        ("bound_m3", "m³"),
        ("bound_rad", "rad"),
        ("bound_kg", "kg"),
        ("bound_kg_m2", "kg m²"),
    ];
    for (key, unit) in units {
        if let Some(b) = v[key].as_f64() {
            return format!("{text} (± {b:.2e} {unit})");
        }
    }
    text.to_owned()
}

fn point(v: &Value) -> Option<[f64; 3]> {
    let a = v["m"].as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

/// Adds a value (or a point) to `out` if the answer has it.
fn add(out: &mut Vec<Line>, label: &str, v: &Value) {
    if v.is_object() {
        let text = if v.get("m").is_some_and(Value::is_array) {
            v["value"].as_str().unwrap_or_default().to_owned()
        } else {
            shown(v)
        };
        out.push((label.to_owned(), text));
    }
}

/// The lines of a `measure` answer, in a fixed order.
fn lines(v: &Value) -> Vec<Line> {
    let mut out = Vec::new();
    for (label, key) in [
        ("Distance", "distance"),
        ("Angle", "angle"),
        ("Supplement", "supplement"),
        ("Point", "point"),
        ("Length", "length"),
        ("Area", "area"),
        ("Volume", "volume"),
        ("Start", "start"),
        ("End", "end"),
    ] {
        add(&mut out, label, &v[key]);
    }
    let shape = &v["shape"];
    if let Some(t) = shape["type"].as_str() {
        out.push(("Shape".to_owned(), t.to_owned()));
        for (label, key) in [
            ("Radius", "radius"),
            ("Major radius", "major"),
            ("Minor radius", "minor"),
            ("Angle", "angle"),
            ("Half-angle", "half_angle"),
            ("Centre", "centre"),
            ("Origin", "origin"),
            ("Apex", "apex"),
        ] {
            add(&mut out, label, &shape[key]);
        }
        for (label, key) in [
            ("Direction", "direction"),
            ("Axis", "axis"),
            ("Normal", "normal"),
        ] {
            if let Some(d) = shape[key].as_array() {
                let c: Vec<String> = d
                    .iter()
                    .map(|x| format!("{:.6}", x.as_f64().unwrap_or_default()))
                    .collect();
                out.push((label.to_owned(), format!("({})", c.join(", "))));
            }
        }
    }
    out
}

/// What the tool last measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub lines: Vec<Line>,
    /// Between two entities, their nearest points: where the dimension
    /// line is drawn.
    pub line: Option<([f64; 3], [f64; 3])>,
    /// The distance as the dimension line's label writes it.
    pub label: Option<String>,
}

impl Reading {
    pub fn of(v: &Value) -> Reading {
        let pts = v["points"].as_array();
        let line = pts.and_then(|p| Some((point(p.first()?)?, point(p.get(1)?)?)));
        Reading {
            lines: lines(v),
            line,
            label: v["distance"]["value"].as_str().map(str::to_owned),
        }
    }
}

/// Tools > Measure: while on, picks in the viewport are measured as they
/// are made: one entity's properties, then with a second the distance and
/// angle between them; a third starts again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tool {
    pub on: bool,
    /// The entities measured, as references: one or two.
    pub picks: Vec<String>,
    /// The answer for `picks`, once it has come.
    pub reading: Option<Reading>,
    pub error: Option<Error>,
}

impl Tool {
    /// Turns the tool on or off; off forgets what it measured.
    pub fn set(&mut self, on: bool) {
        *self = Tool {
            on,
            ..Tool::default()
        };
    }

    /// Takes a pick: the entities to measure now.
    pub fn pick(&mut self, entity: &str) -> Vec<String> {
        if self.picks.len() >= 2 || self.picks.iter().any(|p| p == entity) {
            self.picks.clear();
        }
        self.picks.push(entity.to_owned());
        self.reading = None;
        self.error = None;
        self.picks.clone()
    }

    /// The answer for `asked`; dropped when the picks have moved on.
    pub fn take(&mut self, asked: &[String], v: &Value) {
        if self.picks == asked {
            self.reading = Some(Reading::of(v));
            self.error = None;
        }
    }

    pub fn failed(&mut self, asked: &[String], e: Error) {
        if self.picks == asked {
            self.reading = None;
            self.error = Some(e);
        }
    }

    /// What Copy puts on the clipboard: the picks, then a line per value.
    pub fn text(&self) -> String {
        let mut out: Vec<String> = self.picks.clone();
        if let Some(r) = &self.reading {
            out.extend(r.lines.iter().map(|(l, v)| format!("{l}: {v}")));
        }
        out.join("\n")
    }
}

/// The Mass properties panel's answer for one solid, part, occurrence or
/// assembly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mass {
    pub of: String,
    pub loaded: bool,
    pub lines: Vec<Line>,
    /// The centre of mass, drawn in the viewport.
    pub centre: Option<[f64; 3]>,
    /// Each part's row: its part, material and mass (or volume).
    pub parts: Vec<Line>,
    pub warnings: Vec<String>,
    pub error: Option<Error>,
}

impl Mass {
    pub fn asked(of: &str) -> Mass {
        Mass {
            of: of.to_owned(),
            ..Mass::default()
        }
    }

    pub fn take(&mut self, v: &Value) {
        let mut lines = Vec::new();
        for (label, key) in [("Mass", "mass"), ("Volume", "volume"), ("Area", "area")] {
            if v[key].is_object() {
                lines.push((label.to_owned(), shown(&v[key])));
            }
        }
        if let Some(c) = v["centre"]["value"].as_str() {
            lines.push(("Centre of mass".to_owned(), c.to_owned()));
        }
        if let Some(ps) = v["inertia"]["principal"].as_array() {
            for (k, p) in ps.iter().enumerate() {
                lines.push((format!("Principal moment {}", k + 1), shown(&p["moment"])));
            }
        }
        let parts = v["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let what = match (p["material"].as_str(), p["mass"]["value"].as_str()) {
                    (Some(m), Some(kg)) => format!("{m}, {kg}"),
                    (Some(m), None) => format!("{m} (not in the library)"),
                    (None, _) => format!(
                        "no material, {}",
                        p["volume"]["value"].as_str().unwrap_or("")
                    ),
                };
                (p["part"].as_str().unwrap_or_default().to_owned(), what)
            })
            .collect();
        let warnings = v["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| w["message"].as_str().map(str::to_owned))
            .collect();
        *self = Mass {
            of: std::mem::take(&mut self.of),
            loaded: true,
            lines,
            centre: point(&v["centre"]),
            parts,
            warnings,
            error: None,
        };
    }

    pub fn failed(&mut self, e: Error) {
        *self = Mass {
            of: std::mem::take(&mut self.of),
            loaded: true,
            error: Some(e),
            ..Mass::default()
        };
    }
}

/// Two solids of an assembly and how far apart they are.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    pub a: String,
    pub b: String,
    pub distance: String,
    pub points: [[f64; 3]; 2],
    /// At 0: touching or interfering.
    pub contact: bool,
}

impl Gap {
    /// How the dialog lists it: the solids under the assembly and the
    /// distance.
    pub fn label(&self, part: &str) -> String {
        let short = |s: &str| {
            s.strip_prefix(part)
                .and_then(|r| r.strip_prefix('/'))
                .unwrap_or(s)
                .to_owned()
        };
        let touch = if self.contact {
            " (touching or interfering)"
        } else {
            ""
        };
        format!(
            "{} and {}: {}{touch}",
            short(&self.a),
            short(&self.b),
            self.distance
        )
    }
}

/// Tools > Clearance: the shown assembly's last `clearance` answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clearance {
    pub part: String,
    pub loaded: bool,
    /// Closest first, as the daemon lists them.
    pub gaps: Vec<Gap>,
    pub unchecked: Vec<(String, String)>,
    pub error: Option<Error>,
    /// The gap last shown: its nearest points are drawn.
    pub shown: Option<usize>,
}

impl Clearance {
    pub fn asked(part: &str) -> Clearance {
        Clearance {
            part: part.to_owned(),
            ..Clearance::default()
        }
    }

    pub fn take(&mut self, v: &Value) {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        self.loaded = true;
        self.error = None;
        self.shown = None;
        self.gaps = v["clearances"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let p = c["points"].as_array()?;
                Some(Gap {
                    a: text(&c["a"]),
                    b: text(&c["b"]),
                    distance: shown(&c["distance"]),
                    points: [point(p.first()?)?, point(p.get(1)?)?],
                    contact: c["contact"] == true,
                })
            })
            .collect();
        self.unchecked = v["unchecked"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|u| (text(&u["a"]), text(&u["b"])))
            .collect();
    }

    pub fn failed(&mut self, e: Error) {
        *self = Clearance {
            part: std::mem::take(&mut self.part),
            loaded: true,
            error: Some(e),
            ..Clearance::default()
        };
    }
}

/// The solid, occurrence or part `mass` should weigh for a selection's
/// entity: an entity's solid (`<part>/<solid>`) or occurrence
/// (`<asm>/<path>`), dropping its name; a whole solid or occurrence as
/// it is.
pub fn weighed(entity: &str, assembly: bool) -> String {
    let parts: Vec<&str> = entity.split('/').collect();
    let named = parts
        .last()
        .is_some_and(|n| n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit()));
    match (named, assembly) {
        // `<asm>/<path>/<solid>/<name>`: the occurrence.
        (true, true) if parts.len() > 3 => parts[..parts.len() - 2].join("/"),
        // `<part>/<solid>/<name>`: the solid.
        (true, _) => parts[..parts.len() - 1].join("/"),
        _ => entity.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn two_picks_read_as_a_dimension() {
        let mut t = Tool::default();
        t.set(true);
        assert_eq!(t.pick("p/b/1"), ["p/b/1"]);
        assert_eq!(t.pick("p/b/2"), ["p/b/1", "p/b/2"]);
        let v = json!({
            "distance": {"value": "1 mm", "m": 0.001, "exact": true},
            "angle": {"value": "180 deg", "rad": std::f64::consts::PI, "exact": true},
            "points": [{"m": [0.0, 0.0, 0.0], "value": "(0 mm, 0 mm, 0 mm)"},
                       {"m": [0.001, 0.0, 0.0], "value": "(1 mm, 0 mm, 0 mm)"}],
        });
        // An answer for picks that have moved on is dropped.
        t.take(&["p/b/1".to_owned()], &v);
        assert!(t.reading.is_none());
        t.take(&t.picks.clone(), &v);
        let r = t.reading.as_ref().unwrap();
        assert_eq!(r.line, Some(([0.0; 3], [0.001, 0.0, 0.0])));
        assert_eq!(r.label.as_deref(), Some("1 mm"));
        assert_eq!(
            r.lines[0],
            ("Distance".to_owned(), "1 mm (exact)".to_owned())
        );
        assert_eq!(
            t.text(),
            "p/b/1\np/b/2\nDistance: 1 mm (exact)\nAngle: 180 deg (exact)"
        );
        // A third pick starts again; picking one twice too.
        assert_eq!(t.pick("p/b/3"), ["p/b/3"]);
        assert_eq!(t.pick("p/b/3"), ["p/b/3"]);
        t.set(false);
        assert!(t.picks.is_empty() && !t.on);
    }

    #[test]
    fn one_pick_lists_its_shape() {
        let v = json!({"kind": "edge",
            "length": {"value": "6.283185307 mm", "m": 0.006283185307179587, "exact": true},
            "start": {"m": [0.001, 0.0, 0.0], "value": "(1 mm, 0 mm, 0 mm)"},
            "end": {"m": [0.001, 0.0, 0.0], "value": "(1 mm, 0 mm, 0 mm)"},
            "shape": {"type": "arc", "radius": {"value": "1 mm", "m": 0.001, "exact": true},
                      "axis": [0.0, 0.0, 1.0],
                      "centre": {"m": [0.0, 0.0, 0.0], "value": "(0 mm, 0 mm, 0 mm)"}}});
        let r = Reading::of(&v);
        assert!(r.line.is_none());
        let labels: Vec<&str> = r.lines.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Length", "Start", "End", "Shape", "Radius", "Centre", "Axis"
            ]
        );
        assert_eq!(r.lines[6].1, "(0.000000, 0.000000, 1.000000)");
        let b = json!({"value": "2 mm^2", "m2": 2e-6, "bound_m2": 1.5e-12});
        assert_eq!(shown(&b), "2 mm^2 (± 1.50e-12 m²)");
    }

    #[test]
    fn mass_and_clearance_answers() {
        let mut m = Mass::asked("plate");
        m.take(&json!({
            "mass": {"value": "0.19625 kg", "kg": 0.19625, "bound_kg": 1e-6},
            "volume": {"value": "25000 mm^3", "m3": 2.5e-5, "bound_m3": 1e-10},
            "centre": {"m": [0.05, 0.025, 0.0025], "value": "(50 mm, 25 mm, 2.5 mm)"},
            "parts": [{"part": "plate", "material": "steel",
                       "mass": {"value": "0.19625 kg"}}],
        }));
        assert_eq!(m.centre, Some([0.05, 0.025, 0.0025]));
        assert_eq!(
            m.parts,
            [("plate".to_owned(), "steel, 0.19625 kg".to_owned())]
        );
        assert_eq!(m.lines[0].0, "Mass");
        let mut c = Clearance::asked("frame");
        c.take(
            &json!({"clearances": [{"a": "frame/p1/slab", "b": "frame/b1/shank",
            "distance": {"value": "0 mm", "m": 0.0, "exact": true}, "contact": true,
            "points": [{"m": [0.0, 0.0, 0.0]}, {"m": [0.0, 0.0, 0.0]}]}], "unchecked": []}),
        );
        assert_eq!(
            c.gaps[0].label("frame"),
            "p1/slab and b1/shank: 0 mm (exact) (touching or interfering)"
        );
        assert_eq!(
            weighed("p/b/0123456789abcdef0123456789abcdef", false),
            "p/b"
        );
        assert_eq!(
            weighed("a/i/b/0123456789abcdef0123456789abcdef", true),
            "a/i"
        );
        assert_eq!(weighed("a/i", true), "a/i");
    }
}
