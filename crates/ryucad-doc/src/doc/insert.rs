//! An `insert` feature: a part takes another part's solids into its own
//! history, pinned to `working` or a release label and placed in its own
//! frame (design 521).

use super::assembly::Rev;
use super::files::{Obj, Placement, invalid};
use super::id::{Id, ProjectId};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::Unit;
use serde_json::{Value, json};

/// The feature format that holds inserts, deletes of bodies and solid
/// paths (`<insert>/<source key>` in a solid field, or a reference with
/// `solid`): ryucad 25.
pub const FORMAT: u64 = 21;

/// An `insert` feature: the solids part `part` shows at `rev`, founded
/// in this part's frame at `placement` (the identity when `None`). Another
/// project's part is `Rev::Foreign`: the file writes its `project`
/// beside `rev`, as an instance's does.
#[derive(Clone, Debug, PartialEq)]
pub struct Insert {
    pub part: Id,
    pub rev: Rev,
    pub placement: Option<Placement>,
}

impl Insert {
    /// Its fields in a feature file, besides the header: `placement` only
    /// when it isn't the identity, `project` only for another project's.
    pub fn fields(&self) -> Value {
        let rev = match &self.rev {
            Rev::Foreign { label, .. } => Rev::Label(label.clone()),
            r => r.clone(),
        };
        let mut v = json!({"part": self.part, "rev": rev.to_json()});
        if let Some(p) = self.rev.project() {
            v["project"] = json!(p);
        }
        if let Some(p) = &self.placement {
            v["placement"] = p.to_json();
        }
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Insert> {
        let part = o.take("part")?;
        let part = part
            .as_str()
            .ok_or_else(|| invalid("/part", "part is the ID of the part it inserts"))?;
        let part = Id::parse(part, "part").map_err(|e| e.with("pointer", "/part"))?;
        let rev = Rev::from_json(&o.take("rev")?, "/rev")?;
        let rev = match o.take_opt("project") {
            None => rev,
            Some(p) => {
                let project = p
                    .as_str()
                    .ok_or_else(|| invalid("/project", "project is a dependency's project ID"))
                    .and_then(ProjectId::parse)
                    .map_err(|e| e.with("pointer", "/project"))?;
                match rev {
                    Rev::Label(label) => Rev::Foreign { project, label },
                    _ => {
                        return Err(Error::new(
                            Code::InstanceForeignWorking,
                            "another project's part is inserted at one of its official labels, never at working",
                        )
                        .with("pointer", "/project"));
                    }
                }
            }
        };
        let placement = match o.take_opt("placement") {
            None => None,
            Some(v) => {
                let p = Placement::from_json(&v, default, "/placement", env)?;
                (!p.is_identity()).then_some(p)
            }
        };
        Ok(Insert {
            part,
            rev,
            placement,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::Feature;
    use crate::expr::Env;
    use crate::units::Unit;
    use serde_json::json;

    fn read(v: Value) -> crate::error::Result<Feature> {
        Feature::from_json(&v, Unit::Mm, &Env::empty())
    }

    fn head(mut v: Value) -> Value {
        v["format"] = json!(FORMAT);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!("insert");
        v
    }

    #[test]
    fn inserts_round_trip_and_leave_an_identity_placement_out() {
        for v in [
            head(json!({"part": "pcb", "rev": "working"})),
            head(json!({"part": "pcb", "rev": {"label": "B"}})),
            head(
                json!({"part": "pcb", "rev": {"label": "B"}, "project": "0123456789abcdef0123456789abcdef"}),
            ),
            head(json!({"part": "pcb", "rev": "working",
                        "placement": {"origin": ["0 mm", "0 mm", "1.6 mm"], "x": [1, 0, 0], "z": [0, 0, 1]}})),
        ] {
            let f = read(v.clone()).unwrap();
            assert_eq!(f.type_name(), "insert");
            assert!(f.inserts());
            assert_eq!(f.to_json(), v, "{v}");
        }
        let f = read(head(json!({"part": "pcb", "rev": "working",
            "placement": {"origin": ["0 mm", "0 mm", "0 mm"], "x": [1, 0, 0], "z": [0, 0, 1]}})))
        .unwrap();
        assert!(f.to_json().get("placement").is_none());
        let i = f.insert().unwrap();
        assert_eq!(i.part.as_str(), "pcb");
        assert!(i.placement.is_none());
    }

    #[test]
    fn inserts_are_checked() {
        for (v, pointer) in [
            (head(json!({"rev": "working"})), "/part"),
            (head(json!({"part": "Pcb", "rev": "working"})), "/part"),
            (head(json!({"part": "pcb"})), "/rev"),
            (
                head(json!({"part": "pcb", "rev": {"label": "b"}})),
                "/rev/label",
            ),
            (
                head(
                    json!({"part": "pcb", "rev": "working", "project": "0123456789abcdef0123456789abcdef"}),
                ),
                "/project",
            ),
            (
                head(json!({"part": "pcb", "rev": "working", "placement": {"z": [0, 0, 0]}})),
                "/placement/origin",
            ),
            (
                head(json!({"part": "pcb", "rev": "working", "solids": []})),
                "/solids",
            ),
        ] {
            let e = read(v.clone()).unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{v}");
        }
    }
}
