//! File > Export…: the shown part or assembly written as STEP, STL, 3MF
//! or USD. The window says which format and how, and where; the daemon
//! (on this machine) writes the file itself, as it does for an agent's
//! `ryucad export --out`, and answers what it wrote or why it refused.
//! Nothing is worked out here but the request and how to read the answer:
//! the options are checked by the daemon, as an agent's are.

use crate::core::model::Model;
use ryucad::error::Error;
use ryucad::proto::Request;
use serde_json::Value;

/// The formats the dialog offers. USD's three encodings are one format
/// with an encoding option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Step,
    Stl,
    ThreeMf,
    Usd,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Step, Format::Stl, Format::ThreeMf, Format::Usd];

    /// As the dialog's choice shows it.
    pub fn title(self) -> &'static str {
        match self {
            Format::Step => "STEP",
            Format::Stl => "STL",
            Format::ThreeMf => "3MF",
            Format::Usd => "USD",
        }
    }

    /// Whether its files hold meshes, and so take a chord and an angle.
    pub fn meshes(self) -> bool {
        matches!(self, Format::Stl | Format::ThreeMf)
    }
}

/// USD's encodings: text, Pixar's crate format, or a package.
pub const ENCODINGS: [&str; 3] = ["usda", "usdc", "usdz"];
/// The units STEP takes, and those the other formats take.
pub const STEP_UNITS: [&str; 2] = ["mm", "m"];
pub const UNITS: [&str; 3] = ["mm", "cm", "m"];
/// The schemas STEP is written in.
pub const SCHEMAS: [&str; 2] = ["ap242", "ap214"];
/// The daemon's defaults for a mesh's chord and angle, shown as typed.
pub const CHORD: &str = "0.01 mm";
pub const ANGLE: &str = "15 deg";

/// What the last export chose: the next one starts from it. The window
/// keeps it while it is open; it is never a preference.
#[derive(Clone, Debug, PartialEq)]
pub struct Choices {
    pub format: Format,
    pub schema: String,
    /// STEP's unit (`mm` or `m`).
    pub step_unit: String,
    /// The unit of STL, 3MF and USD (`mm`, `cm` or `m`).
    pub unit: String,
    /// A mesh's chord and angle as typed, with their units.
    pub chord: String,
    pub angle: String,
    /// STL as text, not binary.
    pub ascii: bool,
    /// USD's encoding, one of [`ENCODINGS`].
    pub encoding: String,
    /// What a USD file holds: B-rep, meshes, or both (never neither).
    pub brep: bool,
    pub mesh: bool,
}

impl Default for Choices {
    fn default() -> Choices {
        Choices {
            format: Format::Step,
            schema: SCHEMAS[0].to_owned(),
            step_unit: STEP_UNITS[0].to_owned(),
            unit: UNITS[0].to_owned(),
            chord: CHORD.to_owned(),
            angle: ANGLE.to_owned(),
            ascii: false,
            encoding: ENCODINGS[0].to_owned(),
            brep: true,
            mesh: true,
        }
    }
}

impl Choices {
    /// The extension files of the chosen format take.
    pub fn extension(&self) -> &str {
        match self.format {
            Format::Step => "step",
            Format::Stl => "stl",
            Format::ThreeMf => "3mf",
            Format::Usd => self.encoding.as_str(),
        }
    }

    /// The format's name in `export`: `step`, `stl`, `3mf` or one of
    /// USD's encodings.
    pub fn format_name(&self) -> &str {
        match self.format {
            Format::Usd => self.encoding.as_str(),
            _ => self.extension(),
        }
    }

    /// Whether the export writes meshes: STL, 3MF, and USD with them.
    pub fn writes_meshes(&self) -> bool {
        self.format.meshes() || (self.format == Format::Usd && self.mesh)
    }
}

/// What the daemon wrote: the file, its size and BLAKE3, and how many
/// triangles, where it wrote meshes.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub out: String,
    pub bytes: u64,
    pub blake3: String,
    pub triangles: Option<u64>,
}

/// A face the export couldn't write, as a refusal names it: its
/// reference (`<part>/<solid>/<name>`) and why, as a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    pub part: String,
    pub face: String,
    pub why: String,
}

/// File > Export…: the part or assembly exported, the choices, where to
/// write, and once answered what was written or why not.
#[derive(Clone, Debug, PartialEq)]
pub struct Export {
    pub part: String,
    pub choices: Choices,
    /// Where to write it, as typed or picked.
    pub path: String,
    pub done: Option<Written>,
    pub error: Option<Error>,
    /// The faces a refusal names, each to show.
    pub faults: Vec<Fault>,
    pub waiting: bool,
}

impl Export {
    /// Exporting `part` as `choices` say, the path empty.
    pub fn new(part: &str, choices: Choices) -> Export {
        Export {
            part: part.to_owned(),
            choices,
            path: String::new(),
            done: None,
            error: None,
            faults: Vec::new(),
            waiting: false,
        }
    }

    /// Makes the typed path's extension the chosen format's: one an
    /// export knows is replaced, any other kept and the format's added.
    /// A path still empty stays so.
    pub fn follow_format(&mut self) {
        let path = self.path.trim();
        if path.is_empty() {
            return;
        }
        let p = std::path::Path::new(path);
        let known = p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ryucad::export::Format::of_path(&format!("x.{e}")).is_some());
        let ext = self.choices.extension();
        self.path = if known {
            p.with_extension(ext).display().to_string()
        } else {
            format!("{path}.{ext}")
        };
    }

    /// The `export` request for the file at `out` (absolute): the working
    /// files, the format and the options it takes, nothing else.
    pub fn request(&self, out: &std::path::Path) -> Request {
        let c = &self.choices;
        let mesh = c.writes_meshes();
        let some = |s: &str| Some(s.trim().to_owned());
        let (unit, schema) = match c.format {
            Format::Step => (some(&c.step_unit), some(&c.schema)),
            _ => (some(&c.unit), None),
        };
        let usd = c.format == Format::Usd;
        Request::Export {
            part: self.part.clone(),
            rev: None,
            format: some(c.format_name()),
            out: Some(out.display().to_string()),
            schema,
            unit,
            time_stamp: None,
            file_name: None,
            chord: if mesh { some(&c.chord) } else { None },
            angle: if mesh { some(&c.angle) } else { None },
            max_edge: None,
            ascii: c.format == Format::Stl && c.ascii,
            brep: usd.then_some(c.brep),
            mesh: usd.then_some(c.mesh),
            pipe_tolerance: None,
        }
    }

    /// Takes the daemon's answer: what it wrote, or its refusal and the
    /// faces it names.
    pub fn answered(&mut self, answer: Result<Value, Error>) {
        self.waiting = false;
        match answer {
            Ok(v) => {
                let n = |v: &Value| v.as_u64().unwrap_or_default();
                let triangles = v["products"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| p["triangles"].as_u64())
                    .try_fold(0, |sum, t| t.map(|t| sum + t))
                    .filter(|_| v.get("mesh").is_some());
                self.done = Some(Written {
                    out: v["out"].as_str().unwrap_or_default().to_owned(),
                    bytes: n(&v["bytes"]),
                    blake3: v["blake3"].as_str().unwrap_or_default().to_owned(),
                    triangles,
                });
            }
            Err(e) => {
                self.faults = faults(&e);
                self.error = Some(e);
            }
        }
    }
}

/// The faces a refusal names: every face that wouldn't mesh
/// (`export_mesh_faults`), or the one face STEP or USD can't describe
/// (`export_unrepresentable`).
pub fn faults(e: &Error) -> Vec<Fault> {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    match e.code {
        ryucad::error::Code::ExportMeshFaults => e
            .details
            .get("faults")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|f| f["face"].is_string())
            .map(|f| Fault {
                part: s(&f["part"]),
                face: s(&f["face"]),
                why: match f["edge"].as_str() {
                    Some(edge) => format!("{} at edge {edge}", s(&f["fault"])),
                    None => s(&f["fault"]),
                },
            })
            .collect(),
        ryucad::error::Code::ExportUnrepresentable => e
            .details
            .get("entity")
            .and_then(Value::as_str)
            .map(|face| Fault {
                part: e.details.get("part").map(s).unwrap_or_default(),
                face: face.to_owned(),
                why: e
                    .details
                    .get("reason")
                    .map(s)
                    .unwrap_or_else(|| "can't be described".to_owned()),
            })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// The references that show `face` (a part's, `<part>/<solid>/<name>`)
/// where the window draws it: itself when its part is shown, else the
/// same face in every occurrence of that part in a shown assembly
/// (`<asm>/<path>/<solid>/<name>`).
pub fn shown_refs(model: &Model, face: &str) -> Vec<String> {
    let Some((part, rest)) = face.split_once('/') else {
        return Vec::new();
    };
    let shown = model.shown();
    if shown.contains(&part) {
        return vec![face.to_owned()];
    }
    shown
        .iter()
        .filter_map(|asm| model.parts.get(*asm))
        .filter(|p| p.assembly)
        .flat_map(|p| {
            p.occurrences
                .iter()
                .filter(|o| o.part == part)
                .map(move |o| format!("{}/{}/{rest}", p.id, o.path))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::error::Code;
    use serde_json::json;

    fn requested(e: &Export) -> Value {
        let r = e.request(std::path::Path::new("/tmp/out/p.x"));
        serde_json::to_value(&r).unwrap()["params"].clone()
    }

    #[test]
    fn each_format_asks_for_what_it_takes() {
        let mut e = Export::new("p", Choices::default());
        assert_eq!(
            requested(&e),
            json!({"part": "p", "format": "step", "out": "/tmp/out/p.x",
                   "schema": "ap242", "unit": "mm"})
        );
        e.choices.format = Format::Stl;
        e.choices.ascii = true;
        e.choices.unit = "cm".into();
        assert_eq!(
            requested(&e),
            json!({"part": "p", "format": "stl", "out": "/tmp/out/p.x", "unit": "cm",
                   "chord": "0.01 mm", "angle": "15 deg", "ascii": true})
        );
        e.choices.format = Format::ThreeMf;
        assert_eq!(
            requested(&e),
            json!({"part": "p", "format": "3mf", "out": "/tmp/out/p.x", "unit": "cm",
                   "chord": "0.01 mm", "angle": "15 deg"})
        );
        e.choices.format = Format::Usd;
        e.choices.encoding = "usdz".into();
        e.choices.chord = " 0.1 mm ".into();
        assert_eq!(
            requested(&e),
            json!({"part": "p", "format": "usdz", "out": "/tmp/out/p.x", "unit": "cm",
                   "chord": "0.1 mm", "angle": "15 deg", "brep": true, "mesh": true})
        );
        // Without meshes a USD export takes no chord or angle.
        e.choices.mesh = false;
        assert_eq!(
            requested(&e),
            json!({"part": "p", "format": "usdz", "out": "/tmp/out/p.x", "unit": "cm",
                   "brep": true, "mesh": false})
        );
    }

    #[test]
    fn the_path_follows_the_format() {
        let mut e = Export::new("p", Choices::default());
        e.follow_format();
        assert_eq!(e.path, "", "an empty path stays empty");
        e.path = "/tmp/a/frame.step".into();
        for (f, enc, want) in [
            (Format::Stl, "usda", "/tmp/a/frame.stl"),
            (Format::ThreeMf, "usda", "/tmp/a/frame.3mf"),
            (Format::Usd, "usdc", "/tmp/a/frame.usdc"),
            (Format::Usd, "usdz", "/tmp/a/frame.usdz"),
            (Format::Step, "usdz", "/tmp/a/frame.step"),
        ] {
            e.choices.format = f;
            e.choices.encoding = enc.into();
            e.follow_format();
            assert_eq!(e.path, want);
        }
        // An extension export doesn't know is kept, and the format's added.
        e.path = "/tmp/a/frame.v2".into();
        e.choices.format = Format::Stl;
        e.follow_format();
        assert_eq!(e.path, "/tmp/a/frame.v2.stl");
    }

    #[test]
    fn an_answer_says_what_was_written() {
        let mut e = Export::new("p", Choices::default());
        e.waiting = true;
        e.answered(Ok(json!({"out": "/tmp/p.stl", "bytes": 684, "blake3": "ab",
                             "mesh": {}, "products": [{"triangles": 12}, {"triangles": 2}]})));
        assert!(!e.waiting);
        assert_eq!(
            e.done,
            Some(Written {
                out: "/tmp/p.stl".into(),
                bytes: 684,
                blake3: "ab".into(),
                triangles: Some(14),
            })
        );
        // STEP writes no meshes, so no triangles.
        e.answered(Ok(json!({"out": "/tmp/p.step", "bytes": 9, "blake3": "cd",
                             "products": [{"product": "p"}]})));
        assert_eq!(e.done.unwrap().triangles, None);
    }

    #[test]
    fn a_refusal_names_its_faces() {
        let mut e = Export::new("frame", Choices::default());
        let err = Error::new(Code::ExportMeshFaults, "2 faces can't be meshed").with(
            "faults",
            json!([
                {"part": "pin", "solid": "shank", "face": "pin/shank/f1", "fault": "bad_domain"},
                {"part": "pin", "solid": "shank", "face": "pin/shank/f2",
                 "fault": "edge_off_surface", "edge": "pin/shank/e9"},
            ]),
        );
        e.answered(Err(err));
        assert_eq!(e.error.as_ref().unwrap().code, Code::ExportMeshFaults);
        assert_eq!(
            e.faults,
            [
                Fault {
                    part: "pin".into(),
                    face: "pin/shank/f1".into(),
                    why: "bad_domain".into()
                },
                Fault {
                    part: "pin".into(),
                    face: "pin/shank/f2".into(),
                    why: "edge_off_surface at edge pin/shank/e9".into()
                },
            ]
        );
        let err = Error::new(Code::ExportUnrepresentable, "can't")
            .with("part", "p")
            .with("entity", "p/b/f3")
            .with("reason", "seams");
        assert_eq!(
            faults(&err),
            [Fault {
                part: "p".into(),
                face: "p/b/f3".into(),
                why: "seams".into()
            }]
        );
        assert!(faults(&Error::new(Code::InvalidParam, "chord")).is_empty());
    }

    #[test]
    fn a_face_is_shown_in_its_part_or_its_occurrences() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "pin", "revision": "r", "features": [], "solids": []}));
        m.apply_part(&json!({"part": "frame", "kind": "assembly", "revision": "r",
            "features": [], "solids": [],
            "occurrences": [
                {"path": "p1", "part": "pin", "solids": [], "pose": {}},
                {"path": "two/p2", "part": "pin", "solids": [], "pose": {}},
                {"path": "base", "part": "plate", "solids": [], "pose": {}},
            ]}));
        m.focus = Some("frame".into());
        assert_eq!(
            shown_refs(&m, "pin/shank/f1"),
            ["frame/p1/shank/f1", "frame/two/p2/shank/f1"]
        );
        m.focus = Some("pin".into());
        assert_eq!(shown_refs(&m, "pin/shank/f1"), ["pin/shank/f1"]);
        assert!(shown_refs(&m, "nothing").is_empty());
    }
}
