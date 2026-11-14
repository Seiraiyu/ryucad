//! Surface features' forms (design 388, The app): a fill's sides shown,
//! each edge side's tangency toggled, its tolerance and angle edited; an
//! extract's faces, each a checkbox to take it out; a sew's sheets, each a
//! checkbox to take it out, its tolerance and whether it makes a solid; a
//! thicken's source shown (its faces, each a checkbox to take out), its
//! front and back, and its groups' sides and faces. What they name is
//! never added here, as a fillet's edges aren't: that is an agent's work.
//! A thicken's operation and targets are the form's own, as an extrude's.

use super::{FeatureForm, Field, Flag, Picks, claimed, text, texts};
use ryucad::error::Error;
use ryucad::proto::FeatureFields;
use serde_json::{Value, json};

/// One side of a fill: an edge (with whether it is met tangentially) or
/// a curve, as text.
#[derive(Clone, Debug, PartialEq)]
pub struct FillSide {
    /// `edge p/wall/<hex>` or `curve p/s2/a1`, for people.
    pub text: String,
    /// What a request names: the edge's or curve's reference.
    pub reference: String,
    /// An edge side's tangency; none for a curve.
    pub tangent: Option<Flag>,
}

/// A fill's form.
#[derive(Clone, Debug, PartialEq)]
pub struct FillForm {
    pub sides: Vec<FillSide>,
    pub tolerance: Field,
    pub angle: Field,
}

impl FillForm {
    fn from_show(v: &Value) -> FillForm {
        let sides = v["sides"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, s)| match s["edge"].as_str() {
                Some(e) => {
                    let on = s["tangent"] == true;
                    FillSide {
                        text: format!("edge {e}"),
                        reference: e.to_owned(),
                        tangent: Some(Flag {
                            name: format!("tangent (side {})", i + 1),
                            read: on,
                            chosen: on,
                        }),
                    }
                }
                None => FillSide {
                    text: format!("curve {}", text(&s["curve"])),
                    reference: text(&s["curve"]),
                    tangent: None,
                },
            })
            .collect();
        FillForm {
            sides,
            tolerance: Field::new("tolerance", text(&v["tolerance"])),
            angle: Field::new("angle", text(&v["angle"])),
        }
    }

    fn sides_dirty(&self) -> bool {
        self.sides
            .iter()
            .any(|s| s.tangent.as_ref().is_some_and(Flag::dirty))
    }

    /// The sides as `feature.edit` takes them, whole, in order.
    pub fn sides_json(&self) -> Value {
        let out: Vec<Value> = self
            .sides
            .iter()
            .map(|s| match &s.tangent {
                Some(t) => json!({"edge": s.reference, "tangent": t.chosen}),
                None => json!({"curve": s.reference}),
            })
            .collect();
        json!(out)
    }
}

/// An extract's form.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractForm {
    pub faces: Picks,
}

/// A sew's form.
#[derive(Clone, Debug, PartialEq)]
pub struct SewForm {
    pub sheets: Picks,
    pub tolerance: Field,
    /// Whether a sew that closes up makes a solid.
    pub solid: Flag,
}

/// One group of a thicken's faces: its own front and back, and its faces.
#[derive(Clone, Debug, PartialEq)]
pub struct SideGroup {
    pub front: Field,
    pub back: Field,
    pub faces: Picks,
}

/// A thicken's form.
#[derive(Clone, Debug, PartialEq)]
pub struct ThickenForm {
    /// The sheet it thickens and consumes, by its founder; else none.
    pub sheet: Option<String>,
    /// The faces it thickens (of one body, left as it was), each a
    /// checkbox to take out; none for a sheet.
    pub faces: Option<Picks>,
    pub front: Field,
    pub back: Field,
    pub groups: Vec<SideGroup>,
}

impl ThickenForm {
    fn from_show(v: &Value) -> ThickenForm {
        let from = &v["from"];
        ThickenForm {
            sheet: from["sheet"].as_str().map(str::to_owned),
            faces: from["faces"]
                .is_array()
                .then(|| Picks::new("faces", texts(&from["faces"]))),
            front: Field::new("front", text(&v["front"])),
            back: Field::new("back", text(&v["back"])),
            groups: v["faces"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(i, g)| SideGroup {
                    front: Field::new(&format!("front (group {})", i + 1), text(&g["front"])),
                    back: Field::new(&format!("back (group {})", i + 1), text(&g["back"])),
                    faces: Picks::new(&format!("faces (group {})", i + 1), texts(&g["faces"])),
                })
                .collect(),
        }
    }

    fn groups_dirty(&self) -> bool {
        self.groups
            .iter()
            .any(|g| g.front.dirty() || g.back.dirty() || g.faces.dirty())
    }

    /// The groups as `feature.edit` takes them (`face_sides`): every one,
    /// whole, as typed; a group left with no faces goes.
    pub fn groups_json(&self) -> Value {
        let out: Vec<Value> = self
            .groups
            .iter()
            .filter(|g| !g.faces.chosen.is_empty())
            .map(|g| {
                json!({"front": g.front.text.trim(), "back": g.back.text.trim(),
                       "faces": g.faces.chosen})
            })
            .collect();
        json!(out)
    }
}

/// A fill's, extract's, sew's or thicken's form.
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceForm {
    Fill(FillForm),
    Extract(ExtractForm),
    Sew(SewForm),
    Thicken(ThickenForm),
}

impl SurfaceForm {
    /// From a `feature.show` of a feature of type `ty`; `None` for any
    /// other type.
    pub fn from_show(ty: &str, v: &Value) -> Option<SurfaceForm> {
        Some(match ty {
            "fill" => SurfaceForm::Fill(FillForm::from_show(v)),
            "extract" => SurfaceForm::Extract(ExtractForm {
                faces: Picks::new("faces", texts(&v["faces"])),
            }),
            "sew" => {
                // Left out, a sew that closes up makes a solid.
                let solid = v["solid"] != false;
                SurfaceForm::Sew(SewForm {
                    sheets: Picks::new("sheets", texts(&v["sheets"])),
                    tolerance: Field::new("tolerance", text(&v["tolerance"])),
                    solid: Flag {
                        name: "solid".to_owned(),
                        read: solid,
                        chosen: solid,
                    },
                })
            }
            "thicken" => SurfaceForm::Thicken(ThickenForm::from_show(v)),
            _ => return None,
        })
    }

    pub fn fields(&self) -> Vec<&Field> {
        match self {
            SurfaceForm::Fill(f) => vec![&f.tolerance, &f.angle],
            SurfaceForm::Extract(_) => Vec::new(),
            SurfaceForm::Sew(s) => vec![&s.tolerance],
            SurfaceForm::Thicken(t) => {
                let mut out = vec![&t.front, &t.back];
                for g in &t.groups {
                    out.extend([&g.front, &g.back]);
                }
                out
            }
        }
    }

    pub fn fields_mut(&mut self) -> Vec<&mut Field> {
        match self {
            SurfaceForm::Fill(f) => vec![&mut f.tolerance, &mut f.angle],
            SurfaceForm::Extract(_) => Vec::new(),
            SurfaceForm::Sew(s) => vec![&mut s.tolerance],
            SurfaceForm::Thicken(t) => {
                let mut out = vec![&mut t.front, &mut t.back];
                for g in &mut t.groups {
                    out.extend([&mut g.front, &mut g.back]);
                }
                out
            }
        }
    }

    /// The field at a pointer into a thicken's groups
    /// (`/faces/1/front`): their last steps repeat the thicken's own.
    pub fn at_pointer(&mut self, steps: &[&str]) -> Option<&mut Field> {
        let SurfaceForm::Thicken(t) = self else {
            return None;
        };
        match steps {
            ["faces", i, side @ ("front" | "back")] => {
                let g = t.groups.get_mut(i.parse::<usize>().ok()?)?;
                Some(if *side == "front" {
                    &mut g.front
                } else {
                    &mut g.back
                })
            }
            _ => None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.fields().into_iter().any(Field::dirty)
            || match self {
                SurfaceForm::Fill(f) => f.sides_dirty(),
                SurfaceForm::Extract(x) => x.faces.dirty(),
                SurfaceForm::Sew(s) => s.sheets.dirty() || s.solid.dirty(),
                SurfaceForm::Thicken(t) => {
                    t.faces.as_ref().is_some_and(Picks::dirty) || t.groups_dirty()
                }
            }
    }

    pub fn revert(&mut self) {
        for f in self.fields_mut() {
            f.text.clone_from(&f.read);
        }
        match self {
            SurfaceForm::Fill(f) => {
                for t in f.sides.iter_mut().filter_map(|s| s.tangent.as_mut()) {
                    t.chosen = t.read;
                }
            }
            SurfaceForm::Extract(x) => x.faces.chosen.clone_from(&x.faces.read),
            SurfaceForm::Sew(s) => {
                s.sheets.chosen.clone_from(&s.sheets.read);
                s.solid.chosen = s.solid.read;
            }
            SurfaceForm::Thicken(t) => {
                if let Some(p) = &mut t.faces {
                    p.chosen.clone_from(&p.read);
                }
                for g in &mut t.groups {
                    g.faces.chosen.clone_from(&g.faces.read);
                }
            }
        }
    }

    /// What changed, as `feature.edit` takes it: a fill's tolerance and
    /// angle typed and its sides whole when a tangency changed; an
    /// extract's faces left; a sew's sheets left, tolerance and solid; a
    /// thicken's faces left (`from_faces`), front, back and groups whole
    /// (`face_sides`).
    pub fn given(&self, out: &mut FeatureFields) {
        let typed = |f: &Field| f.dirty().then(|| json!(f.text.trim()));
        match self {
            SurfaceForm::Fill(f) => {
                out.tolerance = typed(&f.tolerance);
                out.angle = typed(&f.angle);
                if f.sides_dirty() {
                    out.sides = Some(f.sides_json());
                }
            }
            SurfaceForm::Extract(x) => {
                if x.faces.dirty() {
                    out.faces = Some(json!(x.faces.chosen));
                }
            }
            SurfaceForm::Sew(s) => {
                out.tolerance = typed(&s.tolerance);
                if s.sheets.dirty() {
                    out.sheets = Some(s.sheets.chosen.clone());
                }
                if s.solid.dirty() {
                    out.make_solid = Some(s.solid.chosen);
                }
            }
            SurfaceForm::Thicken(t) => {
                out.front = typed(&t.front);
                out.back = typed(&t.back);
                if let Some(p) = t.faces.as_ref().filter(|p| p.dirty()) {
                    out.from_faces = Some(p.chosen.clone());
                }
                if t.groups_dirty() {
                    out.face_sides = Some(t.groups_json());
                }
            }
        }
    }
}

impl FeatureForm for SurfaceForm {
    fn read(ty: &str, v: &Value) -> Option<SurfaceForm> {
        SurfaceForm::from_show(ty, v)
    }
    fn kind_fields(&self) -> Vec<&Field> {
        self.fields()
    }
    fn kind_fields_mut(&mut self) -> Vec<&mut Field> {
        self.fields_mut()
    }
    fn kind_dirty(&self) -> bool {
        self.dirty()
    }
    fn kind_revert(&mut self) {
        self.revert();
    }
    fn kind_given(&self, out: &mut FeatureFields) -> Result<(), Error> {
        self.given(out);
        Ok(())
    }
    fn kind_at_pointer(&mut self, steps: &[&str]) -> Option<&mut Field> {
        if claimed(steps) {
            return None;
        }
        self.at_pointer(steps)
    }
}

/// The tree's icon of a type this kind owns.
pub(crate) fn icon(ty: &str, _op: Option<&str>, _form: Option<&str>) -> Option<&'static str> {
    Some(match ty {
        "fill" => "fill",
        "extract" => "extract",
        "sew" => "sew",
        "thicken" => "thicken",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDGE: &str = "p/wall/0123456789abcdef0123456789abcdef";

    #[test]
    fn a_fills_tangency_tolerance_and_angle_are_edited() {
        let v = json!({"sides": [{"edge": EDGE, "tangent": false}, {"curve": "p/s2/a1"}],
                       "tolerance": {"value": "0.001 mm", "m": 1e-6},
                       "angle": {"value": "1 deg", "rad": 0.017}});
        let Some(SurfaceForm::Fill(mut f)) = SurfaceForm::from_show("fill", &v) else {
            panic!("a fill's form");
        };
        assert_eq!(f.sides[0].text, format!("edge {EDGE}"));
        assert_eq!(f.sides[1].text, "curve p/s2/a1");
        assert!(f.sides[1].tangent.is_none());
        assert_eq!(f.tolerance.read, "0.001 mm");
        let mut form = SurfaceForm::Fill(f.clone());
        assert!(!form.dirty());
        f.sides[0].tangent.as_mut().unwrap().chosen = true;
        f.tolerance.text = "0.002 mm".into();
        form = SurfaceForm::Fill(f);
        assert!(form.dirty());
        let mut out = FeatureFields::default();
        form.given(&mut out);
        assert_eq!(out.tolerance, Some(json!("0.002 mm")));
        assert_eq!(out.angle, None);
        assert_eq!(
            out.sides,
            Some(json!([{"edge": EDGE, "tangent": true}, {"curve": "p/s2/a1"}]))
        );
        form.revert();
        assert!(!form.dirty());
    }

    #[test]
    fn an_extracts_faces_and_a_sews_sheets_are_taken_out() {
        let v = json!({"faces": [EDGE, "p/base/ffffffffffffffffffffffffffffffff"]});
        let Some(SurfaceForm::Extract(mut x)) = SurfaceForm::from_show("extract", &v) else {
            panic!("an extract's form");
        };
        x.faces.toggle(EDGE);
        let mut out = FeatureFields::default();
        SurfaceForm::Extract(x).given(&mut out);
        assert_eq!(
            out.faces,
            Some(json!(["p/base/ffffffffffffffffffffffffffffffff"]))
        );
        // A sew written without `solid` makes one.
        let v = json!({"sheets": ["a", "b", "c"], "tolerance": {"value": "0.01 mm"}});
        let Some(SurfaceForm::Sew(mut s)) = SurfaceForm::from_show("sew", &v) else {
            panic!("a sew's form");
        };
        assert!(s.solid.read);
        s.sheets.toggle("b");
        s.solid.chosen = false;
        let mut out = FeatureFields::default();
        SurfaceForm::Sew(s).given(&mut out);
        assert_eq!(out.sheets, Some(vec!["a".to_owned(), "c".to_owned()]));
        assert_eq!(out.make_solid, Some(false));
        assert_eq!(out.tolerance, None);
    }

    #[test]
    fn a_thickens_sides_faces_and_groups_are_edited() {
        let other = "p/base/ffffffffffffffffffffffffffffffff";
        let v = json!({"from": {"faces": [EDGE, other]},
                       "front": {"value": "3 mm"}, "back": {"value": "0 mm"},
                       "faces": [{"front": {"value": "1 mm"}, "back": {"value": "0 mm"},
                                  "faces": [other]}],
                       "operation": "join", "targets": ["base"]});
        let Some(SurfaceForm::Thicken(mut t)) = SurfaceForm::from_show("thicken", &v) else {
            panic!("a thicken's form");
        };
        assert_eq!(t.sheet, None);
        assert_eq!(t.front.read, "3 mm");
        t.front.text = "4 mm".into();
        t.faces.as_mut().unwrap().toggle(EDGE);
        t.groups[0].back.text = "1 mm".into();
        let mut form = SurfaceForm::Thicken(t);
        let mut out = FeatureFields::default();
        form.given(&mut out);
        assert_eq!(out.front, Some(json!("4 mm")));
        assert_eq!(out.back, None);
        assert_eq!(out.from_faces, Some(vec![other.to_owned()]));
        assert_eq!(
            out.face_sides,
            Some(json!([{"front": "1 mm", "back": "1 mm", "faces": [other]}]))
        );
        // A refusal at a group's side lands on it, not on the thicken's own.
        let f = form.at_pointer(&["faces", "0", "back"]).unwrap();
        assert_eq!(f.name, "back (group 1)");
        // From a sheet: no faces to take out.
        let v = json!({"from": {"sheet": "skin"}, "front": {"value": "2 mm"},
                       "back": {"value": "0 mm"}, "operation": "new"});
        let Some(SurfaceForm::Thicken(t)) = SurfaceForm::from_show("thicken", &v) else {
            panic!("a thicken's form");
        };
        assert_eq!(
            (t.sheet.as_deref(), t.faces.is_none()),
            (Some("skin"), true)
        );
        assert!(SurfaceForm::from_show("extrude", &v).is_none());
    }
}
