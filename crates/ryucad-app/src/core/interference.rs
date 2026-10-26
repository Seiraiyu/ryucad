//! Interference as the window shows it: the last `interference` report of
//! the assembly it shows, read from the daemon, never worked out here.
//! Each pair that shares a volume is listed, and the faces of both sides
//! it touches are painted in the removed colour.

use ryucad::error::Error;
use serde_json::Value;

/// Two solids of an assembly's occurrences that share a volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Pair {
    /// Each side's solid, as `<asm>/<path>/<solid>`.
    pub a: String,
    pub b: String,
    /// The volume they share, estimated from a mesh.
    pub volume_m3: f64,
    /// The faces of each side the shared volume touches, as entity
    /// references.
    pub faces: Vec<String>,
}

impl Pair {
    /// How the panel lists it: the two solids under the assembly and the
    /// volume in cubic millimetres.
    pub fn label(&self, part: &str) -> String {
        let short = |s: &str| {
            s.strip_prefix(part)
                .and_then(|r| r.strip_prefix('/'))
                .unwrap_or(s)
                .to_owned()
        };
        format!(
            "{} and {}: {:.3} mm³",
            short(&self.a),
            short(&self.b),
            self.volume_m3 * 1e9
        )
    }
}

/// One assembly's interference: its pairs, how many only touch, and the
/// pairs that couldn't be checked; or why it couldn't be found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interference {
    pub part: String,
    /// The answer has come.
    pub loaded: bool,
    pub pairs: Vec<Pair>,
    pub touching: usize,
    pub unchecked: Vec<(String, String)>,
    pub error: Option<Error>,
}

impl Interference {
    /// Asked for assembly `part`; the answer hasn't come.
    pub fn asked(part: &str) -> Interference {
        Interference {
            part: part.to_owned(),
            ..Interference::default()
        }
    }

    /// From an `interference` answer.
    pub fn take(&mut self, v: &Value) {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        self.loaded = true;
        self.error = None;
        self.pairs = v["interferences"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| Pair {
                a: text(&i["a"]),
                b: text(&i["b"]),
                volume_m3: i["volume_m3"].as_f64().unwrap_or_default(),
                faces: ["a", "b"]
                    .iter()
                    .flat_map(|s| i["faces"][s].as_array().into_iter().flatten())
                    .map(text)
                    .collect(),
            })
            .collect();
        self.touching = v["touching"].as_array().map_or(0, Vec::len);
        self.unchecked = v["unchecked"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|u| (text(&u["a"]), text(&u["b"])))
            .collect();
    }

    /// The request failed: nothing is listed.
    pub fn failed(&mut self, e: Error) {
        *self = Interference {
            part: std::mem::take(&mut self.part),
            loaded: true,
            error: Some(e),
            ..Interference::default()
        };
    }

    /// Every face the pairs touch, as entity references: what is painted.
    pub fn faces(&self) -> impl Iterator<Item = &String> {
        self.pairs.iter().flat_map(|p| &p.faces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_report_reads_as_pairs_and_faces() {
        let mut i = Interference::asked("frame");
        assert!(!i.loaded);
        i.take(&json!({
            "part": "frame",
            "interferences": [{"a": "frame/p1/slab", "b": "frame/b1/shank",
                               "volume_m3": 1.4137e-7,
                               "faces": {"a": ["frame/p1/slab/aa"], "b": ["frame/b1/shank/bb"]}}],
            "touching": [{"a": "frame/p1/slab", "b": "frame/b2/shank"}],
            "unchecked": [{"a": "frame/x/s", "b": "frame/y/s", "kernel": "Uncertain"}],
        }));
        assert!(i.loaded);
        assert_eq!(i.pairs.len(), 1);
        assert_eq!(
            i.pairs[0].label("frame"),
            "p1/slab and b1/shank: 141.370 mm³"
        );
        let faces: Vec<&String> = i.faces().collect();
        assert_eq!(faces, ["frame/p1/slab/aa", "frame/b1/shank/bb"]);
        assert_eq!(i.touching, 1);
        assert_eq!(i.unchecked, [("frame/x/s".into(), "frame/y/s".into())]);
        i.failed(Error::new(ryucad::error::Code::WrongPartKind, "a part"));
        assert_eq!((i.part.as_str(), i.pairs.len()), ("frame", 0));
        assert!(i.error.is_some());
    }
}
