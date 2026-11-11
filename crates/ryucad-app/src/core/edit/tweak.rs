//! The forms of the face tweaks (design 400, The app): a draft's angle,
//! each group's angle, `reverse` and `tangent` edited, its faces and
//! flipped faces each a checkbox to take it out, its hinge and pull shown;
//! a move's distance or angle, its groups' distances and `tangent` edited,
//! its `by` and what it goes along or about shown; a replacement's faces
//! checkboxes, each group's surface shown. And of the direct edits
//! (design 445, The app): a delete's faces and seeds checkboxes and its
//! heal a choice; a blend delete's size and tolerance edited and its
//! faces checkboxes; a resize's sizes and tolerance edited. Adding a
//! face, or changing what a tweak turns about, is an agent's work. Each
//! save is one `feature.edit` against the revision read.

use super::{Choice, Field, Flag, Picks, text, texts};
use ryucad::proto::{FeatureFields, Flip};
use serde_json::{Value, json};

/// Faces at a quantity of their own: a draft's group at its angle, a
/// move's at its distance, each face a checkbox to take it out.
#[derive(Clone, Debug, PartialEq)]
pub struct TweakGroup {
    pub quantity: Field,
    pub faces: Picks,
}

/// A replacement's group: the surface its faces take, shown, and the
/// faces, each a checkbox.
#[derive(Clone, Debug, PartialEq)]
pub struct WithGroup {
    pub with: String,
    pub faces: Picks,
}

/// A draft's form.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftForm {
    pub target: String,
    /// `neutral xy`, or `parting <edges>`: shown, never changed here.
    pub hinge: String,
    /// A parting line's pull, shown.
    pub pull: Option<String>,
    pub angle: Field,
    pub faces: Picks,
    pub groups: Vec<TweakGroup>,
    /// The drafted faces drafted against the pull: taken out with the
    /// faces they are among.
    pub flip: Picks,
    pub reverse: Flag,
    pub tangent: Flag,
}

/// A move face's form.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveFaceForm {
    pub target: String,
    /// `translate`, `rotate` or `offset`: shown.
    pub by: String,
    /// A translation's direction or a rotation's axis, shown.
    pub along: Option<String>,
    /// A translation's or an offset's distance.
    pub distance: Option<Field>,
    /// A rotation's angle.
    pub angle: Option<Field>,
    pub faces: Picks,
    /// An offset's faces at distances of their own.
    pub groups: Vec<TweakGroup>,
    pub tangent: Flag,
}

/// A replace face's form.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplaceFaceForm {
    pub target: String,
    pub groups: Vec<WithGroup>,
}

/// A delete face's form: its faces and seeds, each a checkbox to take it
/// out, and how it heals.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteFaceForm {
    pub target: String,
    pub faces: Picks,
    pub seeds: Picks,
    /// `auto`, `cap` or `grow`.
    pub heal: Choice,
}

/// A blend delete's form: the size it takes blends below and its
/// recognition tolerance (each empty when not written), and its faces,
/// each a checkbox to take it out.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteBlendForm {
    pub target: String,
    pub below: Field,
    pub tolerance: Field,
    pub faces: Picks,
}

/// One group of a resize: its sizes by their keys (`radius`, or a
/// chamfer's `d`, `d1`, `d2`, `angle`), edited; its chamfer form and the
/// face its first distance runs along, and the faces naming its blends,
/// shown.
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeGroup {
    pub sizes: Vec<(String, Field)>,
    pub form: Option<String>,
    pub face: Option<String>,
    pub faces: Vec<String>,
}

/// A resize blend's form.
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeBlendForm {
    pub target: String,
    pub groups: Vec<ResizeGroup>,
    pub tolerance: Field,
}

impl ResizeBlendForm {
    fn sizes(&self) -> impl Iterator<Item = &Field> {
        self.groups
            .iter()
            .flat_map(|g| g.sizes.iter().map(|(_, f)| f))
    }

    /// The groups as `feature.edit` takes them: every one, whole, as
    /// typed.
    fn groups_json(&self) -> Value {
        let gs: Vec<Value> = self
            .groups
            .iter()
            .map(|g| {
                let mut v = json!({"faces": g.faces});
                for (k, f) in &g.sizes {
                    v[k] = json!(f.text.trim());
                }
                if let Some(form) = &g.form {
                    v["form"] = json!(form);
                }
                if let Some(face) = &g.face {
                    v["face"] = json!(face);
                }
                v
            })
            .collect();
        json!(gs)
    }
}

/// A draft's, move face's or replace face's form, or a delete face's,
/// delete blend's or resize blend's.
#[derive(Clone, Debug, PartialEq)]
pub enum TweakForm {
    Draft(Box<DraftForm>),
    MoveFace(Box<MoveFaceForm>),
    ReplaceFace(Box<ReplaceFaceForm>),
    DeleteFace(Box<DeleteFaceForm>),
    DeleteBlend(Box<DeleteBlendForm>),
    ResizeBlend(Box<ResizeBlendForm>),
}

fn flag(name: &str, read: bool) -> Flag {
    Flag {
        name: name.to_owned(),
        read,
        chosen: read,
    }
}

/// `feature.show`'s groups, each quantity under `key`.
fn groups(v: &Value, key: &str) -> Vec<TweakGroup> {
    v["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, g)| TweakGroup {
            quantity: Field::new(&format!("{key} (group {})", i + 1), text(&g[key])),
            faces: Picks::new(&format!("faces (group {})", i + 1), texts(&g["faces"])),
        })
        .collect()
}

fn groups_dirty(gs: &[TweakGroup]) -> bool {
    gs.iter().any(|g| g.quantity.dirty() || g.faces.dirty())
}

fn revert_groups(gs: &mut [TweakGroup]) {
    for g in gs {
        g.quantity.text.clone_from(&g.quantity.read);
        g.faces.chosen.clone_from(&g.faces.read);
    }
}

/// The groups as `feature.edit` takes them: every one, whole, as typed;
/// a group left with no faces goes.
fn groups_json(gs: &[TweakGroup], key: &str) -> Value {
    let out: Vec<Value> = gs
        .iter()
        .filter(|g| !g.faces.chosen.is_empty())
        .map(|g| {
            let mut v = json!({"faces": g.faces.chosen});
            v[key] = json!(g.quantity.text.trim());
            v
        })
        .collect();
    json!(out)
}

impl TweakForm {
    /// From a `feature.show` of a feature of type `ty`; `None` for any
    /// other type.
    pub fn from_show(ty: &str, v: &Value) -> Option<TweakForm> {
        let target = text(&v["target"]);
        let tangent = flag("tangent", v["tangent"] != false);
        Some(match ty {
            "draft" => {
                let hinge = match v["neutral"].as_str() {
                    Some(n) => format!("neutral {n}"),
                    None => format!("parting {}", texts(&v["parting"]).join(", ")),
                };
                TweakForm::Draft(Box::new(DraftForm {
                    target,
                    hinge,
                    pull: v["pull"].as_str().map(str::to_owned),
                    angle: Field::new("angle", text(&v["angle"])),
                    faces: Picks::new("faces", texts(&v["faces"])),
                    groups: groups(v, "angle"),
                    flip: Picks::new("flipped faces", texts(&v["flip"])),
                    reverse: flag("reverse", v["reverse"] == true),
                    tangent,
                }))
            }
            "move_face" => {
                let by = text(&v["by"]);
                let along = ["direction", "axis"]
                    .into_iter()
                    .find_map(|k| v[k].as_str().map(|t| format!("{k} {t}")));
                TweakForm::MoveFace(Box::new(MoveFaceForm {
                    target,
                    along,
                    distance: (!v["distance"].is_null())
                        .then(|| Field::new("distance", text(&v["distance"]))),
                    angle: (!v["angle"].is_null()).then(|| Field::new("angle", text(&v["angle"]))),
                    faces: Picks::new("faces", texts(&v["faces"])),
                    groups: groups(v, "distance"),
                    tangent,
                    by,
                }))
            }
            "replace_face" => TweakForm::ReplaceFace(Box::new(ReplaceFaceForm {
                target,
                groups: v["groups"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(i, g)| WithGroup {
                        with: text(&g["with"]),
                        faces: Picks::new(&format!("faces (group {})", i + 1), texts(&g["faces"])),
                    })
                    .collect(),
            })),
            "delete_face" => TweakForm::DeleteFace(Box::new(DeleteFaceForm {
                target,
                faces: Picks::new("faces", texts(&v["faces"])),
                seeds: Picks::new("seeds", texts(&v["seeds"])),
                heal: Choice::new(
                    "heal",
                    &["auto", "cap", "grow"],
                    v["heal"].as_str().unwrap_or("auto").to_owned(),
                ),
            })),
            "delete_blend" => TweakForm::DeleteBlend(Box::new(DeleteBlendForm {
                target,
                below: Field::new("below", text(&v["below"])),
                tolerance: Field::new("tolerance", text(&v["tolerance"])),
                faces: Picks::new("faces", texts(&v["faces"])),
            })),
            "resize_blend" => {
                let gs: Vec<&Value> = v["groups"].as_array().into_iter().flatten().collect();
                let many = gs.len() > 1;
                let groups = gs
                    .into_iter()
                    .enumerate()
                    .map(|(i, g)| {
                        let named = |k: &str| {
                            if many {
                                format!("{k} (group {})", i + 1)
                            } else {
                                k.to_owned()
                            }
                        };
                        ResizeGroup {
                            sizes: ["radius", "d", "d1", "d2", "angle"]
                                .into_iter()
                                .filter(|k| !g[*k].is_null())
                                .map(|k| (k.to_owned(), Field::new(&named(k), text(&g[k]))))
                                .collect(),
                            form: g["form"].as_str().map(str::to_owned),
                            face: g["face"].as_str().map(str::to_owned),
                            faces: texts(&g["faces"]),
                        }
                    })
                    .collect();
                TweakForm::ResizeBlend(Box::new(ResizeBlendForm {
                    target,
                    groups,
                    tolerance: Field::new("tolerance", text(&v["tolerance"])),
                }))
            }
            _ => return None,
        })
    }

    /// Every field it edits, to place errors and keep what was typed.
    pub fn fields(&self) -> Vec<&Field> {
        match self {
            TweakForm::Draft(d) => std::iter::once(&d.angle)
                .chain(d.groups.iter().map(|g| &g.quantity))
                .collect(),
            TweakForm::MoveFace(m) => m
                .distance
                .iter()
                .chain(&m.angle)
                .chain(m.groups.iter().map(|g| &g.quantity))
                .collect(),
            TweakForm::ReplaceFace(_) => Vec::new(),
            TweakForm::DeleteFace(_) => Vec::new(),
            TweakForm::DeleteBlend(d) => vec![&d.below, &d.tolerance],
            TweakForm::ResizeBlend(r) => r.sizes().chain([&r.tolerance]).collect(),
        }
    }

    pub fn fields_mut(&mut self) -> Vec<&mut Field> {
        match self {
            TweakForm::Draft(d) => std::iter::once(&mut d.angle)
                .chain(d.groups.iter_mut().map(|g| &mut g.quantity))
                .collect(),
            TweakForm::MoveFace(m) => m
                .distance
                .iter_mut()
                .chain(&mut m.angle)
                .chain(m.groups.iter_mut().map(|g| &mut g.quantity))
                .collect(),
            TweakForm::ReplaceFace(_) => Vec::new(),
            TweakForm::DeleteFace(_) => Vec::new(),
            TweakForm::DeleteBlend(d) => vec![&mut d.below, &mut d.tolerance],
            TweakForm::ResizeBlend(r) => r
                .groups
                .iter_mut()
                .flat_map(|g| g.sizes.iter_mut().map(|(_, f)| f))
                .chain([&mut r.tolerance])
                .collect(),
        }
    }

    /// A group's quantity by its pointer's steps (`groups`, `0`,
    /// `angle`): a draft's or move's one quantity, a resize's size by its
    /// key.
    pub fn at_pointer(&mut self, steps: &[&str]) -> Option<&mut Field> {
        let ["groups", i, k] = steps else {
            return None;
        };
        let i: usize = i.parse().ok()?;
        match self {
            TweakForm::Draft(d) => Some(&mut d.groups.get_mut(i)?.quantity),
            TweakForm::MoveFace(m) => Some(&mut m.groups.get_mut(i)?.quantity),
            TweakForm::ResizeBlend(r) => r
                .groups
                .get_mut(i)?
                .sizes
                .iter_mut()
                .find(|(key, _)| key == k)
                .map(|(_, f)| f),
            TweakForm::ReplaceFace(_) | TweakForm::DeleteFace(_) | TweakForm::DeleteBlend(_) => {
                None
            }
        }
    }

    pub fn dirty(&self) -> bool {
        match self {
            TweakForm::Draft(d) => {
                d.angle.dirty()
                    || d.faces.dirty()
                    || groups_dirty(&d.groups)
                    || d.flip.dirty()
                    || d.reverse.dirty()
                    || d.tangent.dirty()
            }
            TweakForm::MoveFace(m) => {
                self.fields().iter().any(|f| f.dirty())
                    || m.faces.dirty()
                    || m.groups.iter().any(|g| g.faces.dirty())
                    || m.tangent.dirty()
            }
            TweakForm::ReplaceFace(r) => r.groups.iter().any(|g| g.faces.dirty()),
            TweakForm::DeleteFace(x) => x.faces.dirty() || x.seeds.dirty() || x.heal.dirty(),
            TweakForm::DeleteBlend(d) => d.below.dirty() || d.tolerance.dirty() || d.faces.dirty(),
            TweakForm::ResizeBlend(_) => self.fields().iter().any(|f| f.dirty()),
        }
    }

    pub fn revert(&mut self) {
        for f in self.fields_mut() {
            f.text.clone_from(&f.read);
        }
        match self {
            TweakForm::Draft(d) => {
                d.faces.chosen.clone_from(&d.faces.read);
                revert_groups(&mut d.groups);
                d.flip.chosen.clone_from(&d.flip.read);
                d.reverse.chosen = d.reverse.read;
                d.tangent.chosen = d.tangent.read;
            }
            TweakForm::MoveFace(m) => {
                m.faces.chosen.clone_from(&m.faces.read);
                revert_groups(&mut m.groups);
                m.tangent.chosen = m.tangent.read;
            }
            TweakForm::ReplaceFace(r) => {
                for g in &mut r.groups {
                    g.faces.chosen.clone_from(&g.faces.read);
                }
            }
            TweakForm::DeleteFace(x) => {
                x.faces.chosen.clone_from(&x.faces.read);
                x.seeds.chosen.clone_from(&x.seeds.read);
                x.heal.chosen.clone_from(&x.heal.read);
            }
            TweakForm::DeleteBlend(d) => {
                d.faces.chosen.clone_from(&d.faces.read);
            }
            // Its sizes and tolerance went back with every field.
            TweakForm::ResizeBlend(_) => {}
        }
    }

    /// A draft's flipped faces still drafted: a face taken out of its
    /// list or group leaves the flips with it (a flip of a face not
    /// drafted is refused).
    fn flipped(d: &DraftForm) -> Vec<String> {
        let drafted = |f: &String| {
            d.faces.chosen.contains(f) || d.groups.iter().any(|g| g.faces.chosen.contains(f))
        };
        d.flip
            .chosen
            .iter()
            .filter(|f| drafted(f))
            .cloned()
            .collect()
    }

    /// What changed, as `feature.edit` takes it: each value typed or
    /// flag chosen; faces, groups and flips whole, as kept.
    pub fn given(&self, out: &mut FeatureFields) {
        let typed = |f: &Field| json!(f.text.trim());
        match self {
            TweakForm::Draft(d) => {
                if d.angle.dirty() {
                    out.angle = Some(typed(&d.angle));
                }
                if d.faces.dirty() {
                    out.faces = Some(json!(d.faces.chosen));
                }
                if groups_dirty(&d.groups) {
                    out.groups = Some(groups_json(&d.groups, "angle"));
                }
                let flip = Self::flipped(d);
                if flip != d.flip.read {
                    out.flip = Some(Flip::Faces(flip));
                }
                if d.reverse.dirty() {
                    out.reverse = Some(d.reverse.chosen);
                }
                if d.tangent.dirty() {
                    out.tangent = Some(d.tangent.chosen);
                }
            }
            TweakForm::MoveFace(m) => {
                if let Some(f) = m.distance.as_ref().filter(|f| f.dirty()) {
                    out.distance = Some(typed(f));
                }
                if let Some(f) = m.angle.as_ref().filter(|f| f.dirty()) {
                    out.angle = Some(typed(f));
                }
                if m.faces.dirty() {
                    out.faces = Some(json!(m.faces.chosen));
                }
                if groups_dirty(&m.groups) {
                    out.groups = Some(groups_json(&m.groups, "distance"));
                }
                if m.tangent.dirty() {
                    out.tangent = Some(m.tangent.chosen);
                }
            }
            TweakForm::ReplaceFace(r) => {
                if r.groups.iter().any(|g| g.faces.dirty()) {
                    let gs: Vec<Value> = r
                        .groups
                        .iter()
                        .filter(|g| !g.faces.chosen.is_empty())
                        .map(|g| json!({"with": g.with, "faces": g.faces.chosen}))
                        .collect();
                    out.groups = Some(json!(gs));
                }
            }
            TweakForm::DeleteFace(x) => {
                if x.faces.dirty() {
                    out.faces = Some(json!(x.faces.chosen));
                }
                if x.seeds.dirty() {
                    out.seeds = Some(x.seeds.chosen.clone());
                }
                if x.heal.dirty() {
                    out.heal = Some(x.heal.chosen.clone());
                }
            }
            TweakForm::DeleteBlend(d) => {
                // Cleared, `""` takes the field out of the file.
                if d.below.dirty() {
                    out.below = Some(typed(&d.below));
                }
                if d.tolerance.dirty() {
                    out.tolerance = Some(typed(&d.tolerance));
                }
                if d.faces.dirty() {
                    out.faces = Some(json!(d.faces.chosen));
                }
            }
            TweakForm::ResizeBlend(r) => {
                if r.sizes().any(Field::dirty) {
                    out.groups = Some(r.groups_json());
                }
                if r.tolerance.dirty() {
                    out.tolerance = Some(typed(&r.tolerance));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `feature.show` answers of 17.B's daemon, taken while planning
    // (names shortened).
    fn draft() -> Value {
        json!({"angle":{"rad":0.03490658503988659,"value":"2 deg"},
               "faces":["box/base/e","box/base/w","box/base/s"],
               "flip":["box/base/s"],
               "groups":[{"angle":{"rad":0.08726646259971647,"value":"5 deg"},
                          "faces":["box/base/n"]}],
               "id":"d","neutral":"xy","reverse":false,"revision":"r1",
               "tangent":true,"target":"base","type":"draft"})
    }

    fn given(t: &TweakForm) -> FeatureFields {
        let mut out = FeatureFields::default();
        t.given(&mut out);
        out
    }

    #[test]
    fn a_drafts_angles_and_flags_are_edited_and_sent_alone() {
        let mut t = TweakForm::from_show("draft", &draft()).unwrap();
        let TweakForm::Draft(d) = &mut t else {
            panic!()
        };
        assert_eq!(d.hinge, "neutral xy");
        assert_eq!(
            (d.angle.read.as_str(), d.groups[0].quantity.read.as_str()),
            ("2 deg", "5 deg")
        );
        assert!(d.tangent.read && !d.reverse.read);
        d.angle.text = "3 deg".into();
        d.groups[0].quantity.text = "6 deg".into();
        d.reverse.chosen = true;
        assert!(t.dirty());
        let out = given(&t);
        assert_eq!(out.angle, Some(json!("3 deg")));
        assert_eq!(
            out.groups,
            Some(json!([{"angle": "6 deg", "faces": ["box/base/n"]}]))
        );
        assert_eq!((out.reverse, out.tangent), (Some(true), None));
        // The faces and flips weren't touched: not sent.
        assert!(out.faces.is_none() && out.flip.is_none());
        // A refusal at a group's angle lands on it.
        assert_eq!(
            t.at_pointer(&["groups", "0", "angle"]).unwrap().name,
            "angle (group 1)"
        );
        t.revert();
        assert!(!t.dirty());
    }

    #[test]
    fn taking_a_flipped_face_out_takes_its_flip_too() {
        let mut t = TweakForm::from_show("draft", &draft()).unwrap();
        let TweakForm::Draft(d) = &mut t else {
            panic!()
        };
        d.faces.toggle("box/base/s");
        let out = given(&t);
        assert_eq!(out.faces, Some(json!(["box/base/e", "box/base/w"])));
        assert_eq!(out.flip, Some(Flip::Faces(Vec::new())));
        // A parting-line draft shows its edges and pull.
        let mut v = draft();
        v["neutral"] = Value::Null;
        v["parting"] = json!(["box/base/k1", "box/base/k2"]);
        v["pull"] = json!("z");
        let TweakForm::Draft(d) = TweakForm::from_show("draft", &v).unwrap() else {
            panic!()
        };
        assert_eq!(d.hinge, "parting box/base/k1, box/base/k2");
        assert_eq!(d.pull.as_deref(), Some("z"));
    }

    #[test]
    fn a_moves_distance_or_angle_groups_and_tangent_are_sent() {
        let v = json!({"by":"offset","distance":{"m":0.001,"value":"1 mm"},
            "faces":["box/base/t"],"groups":[{"distance":{"m":0.002,"value":"2 mm"},
            "faces":["box/base/s","box/base/n"]}],"id":"m","revision":"r1",
            "tangent":true,"target":"base","type":"move_face"});
        let mut t = TweakForm::from_show("move_face", &v).unwrap();
        let TweakForm::MoveFace(m) = &mut t else {
            panic!()
        };
        assert_eq!((m.by.as_str(), m.along.as_deref()), ("offset", None));
        assert!(m.angle.is_none());
        m.distance.as_mut().unwrap().text = "1.5 mm".into();
        m.groups[0].faces.toggle("box/base/n");
        m.tangent.chosen = false;
        let out = given(&t);
        assert_eq!(out.distance, Some(json!("1.5 mm")));
        assert_eq!(
            out.groups,
            Some(json!([{"distance": "2 mm", "faces": ["box/base/s"]}]))
        );
        assert_eq!(out.tangent, Some(false));
        assert!(out.faces.is_none() && out.angle.is_none());
        // A rotation: its axis shown, its angle edited.
        let v = json!({"angle":{"rad":0.017453292519943295,"value":"1 deg"},"axis":"x",
            "by":"rotate","faces":["box/base/e"],"id":"m2","revision":"r1",
            "tangent":true,"target":"base","type":"move_face"});
        let mut t = TweakForm::from_show("move_face", &v).unwrap();
        let TweakForm::MoveFace(m) = &mut t else {
            panic!()
        };
        assert_eq!(m.along.as_deref(), Some("axis x"));
        assert!(m.distance.is_none());
        m.angle.as_mut().unwrap().text = "2 deg".into();
        assert_eq!(given(&t).angle, Some(json!("2 deg")));
    }

    #[test]
    fn a_replacements_faces_are_taken_out_and_its_surfaces_kept() {
        let v = json!({"groups":[{"faces":["box/base/t","box/base/b"],"with":"box/lid"}],
            "id":"r","revision":"r1","target":"base","type":"replace_face"});
        let mut t = TweakForm::from_show("replace_face", &v).unwrap();
        assert!(t.fields().is_empty());
        let TweakForm::ReplaceFace(r) = &mut t else {
            panic!()
        };
        assert_eq!(r.groups[0].with, "box/lid");
        r.groups[0].faces.toggle("box/base/b");
        assert_eq!(
            given(&t).groups,
            Some(json!([{"with": "box/lid", "faces": ["box/base/t"]}]))
        );
        assert!(TweakForm::from_show("shell", &v).is_none());
    }

    #[test]
    fn a_deletes_faces_and_seeds_are_taken_out_and_its_heal_chosen() {
        let v = json!({"faces":["box/base/a","box/base/b"],"heal":"cap",
            "id":"x","revision":"r1","seeds":["box/hole/s"],"target":"base",
            "type":"delete_face"});
        let mut t = TweakForm::from_show("delete_face", &v).unwrap();
        assert!(t.fields().is_empty() && !t.dirty());
        let TweakForm::DeleteFace(x) = &mut t else {
            panic!()
        };
        assert_eq!((x.target.as_str(), x.heal.read.as_str()), ("base", "cap"));
        assert_eq!(x.heal.options, ["auto", "cap", "grow"]);
        x.faces.toggle("box/base/b");
        x.heal.chosen = "grow".into();
        let out = given(&t);
        assert_eq!(out.faces, Some(json!(["box/base/a"])));
        assert_eq!(out.heal.as_deref(), Some("grow"));
        assert!(out.seeds.is_none());
        // Every seed taken out goes as an empty list.
        let TweakForm::DeleteFace(x) = &mut t else {
            panic!()
        };
        x.seeds.toggle("box/hole/s");
        assert_eq!(given(&t).seeds, Some(Vec::new()));
        t.revert();
        assert!(!t.dirty());
        // A delete by seeds alone, healing as it likes: no faces, auto.
        let v = json!({"heal":"auto","id":"x","revision":"r1",
            "seeds":["box/hole/s"],"target":"base","type":"delete_face"});
        let TweakForm::DeleteFace(x) = TweakForm::from_show("delete_face", &v).unwrap() else {
            panic!()
        };
        assert!(x.faces.options.is_empty());
        assert_eq!(x.heal.read, "auto");
    }

    #[test]
    fn a_blend_deletes_size_and_tolerance_are_edited_and_cleared() {
        let v = json!({"below":{"m":0.002,"value":"2 mm"},"faces":["box/round/f"],
            "id":"db","revision":"r1","target":"base","type":"delete_blend"});
        let mut t = TweakForm::from_show("delete_blend", &v).unwrap();
        let TweakForm::DeleteBlend(d) = &mut t else {
            panic!()
        };
        assert_eq!(
            (d.below.read.as_str(), d.tolerance.read.as_str()),
            ("2 mm", "")
        );
        assert_eq!(
            t.fields()
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["below", "tolerance"]
        );
        let TweakForm::DeleteBlend(d) = &mut t else {
            panic!()
        };
        d.below.text = "1.5 mm".into();
        d.tolerance.text = "0.02 mm".into();
        let out = given(&t);
        assert_eq!(
            (out.below, out.tolerance),
            (Some(json!("1.5 mm")), Some(json!("0.02 mm")))
        );
        assert!(out.faces.is_none());
        // Below cleared, the face taken out: below goes, faces are none.
        let TweakForm::DeleteBlend(d) = &mut t else {
            panic!()
        };
        d.below.text = String::new();
        d.tolerance.text.clone_from(&d.tolerance.read);
        d.faces.toggle("box/round/f");
        let out = given(&t);
        assert_eq!(out.below, Some(json!("")));
        assert_eq!(out.faces, Some(json!([])));
        assert!(out.tolerance.is_none());
    }

    #[test]
    fn a_resizes_sizes_are_edited_and_its_groups_sent_whole() {
        let v = json!({"groups":[
                {"faces":["box/round/f"],"radius":{"m":0.003,"value":"3 mm"}},
                {"d1":{"m":0.001,"value":"1 mm"},"d2":{"m":0.002,"value":"2 mm"},
                 "face":"box/base/t","faces":["box/bevel/c"],"form":"distances"}],
            "id":"r","revision":"r1","target":"base","type":"resize_blend"});
        let mut t = TweakForm::from_show("resize_blend", &v).unwrap();
        assert_eq!(
            t.fields()
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            [
                "radius (group 1)",
                "d1 (group 2)",
                "d2 (group 2)",
                "tolerance"
            ]
        );
        let TweakForm::ResizeBlend(r) = &mut t else {
            panic!()
        };
        assert_eq!(r.groups[1].form.as_deref(), Some("distances"));
        assert_eq!(r.groups[1].face.as_deref(), Some("box/base/t"));
        assert_eq!(r.groups[0].faces, ["box/round/f"]);
        r.groups[0].sizes[0].1.text = "2 mm".into();
        let out = given(&t);
        assert_eq!(
            out.groups,
            Some(json!([
                {"faces": ["box/round/f"], "radius": "2 mm"},
                {"faces": ["box/bevel/c"], "form": "distances", "face": "box/base/t",
                 "d1": "1 mm", "d2": "2 mm"}]))
        );
        assert!(out.tolerance.is_none() && out.faces.is_none() && out.radius.is_none());
        // A refusal at a group's size lands on it.
        assert_eq!(
            t.at_pointer(&["groups", "1", "d2"]).unwrap().name,
            "d2 (group 2)"
        );
        assert!(t.at_pointer(&["groups", "1", "radius"]).is_none());
        t.revert();
        assert!(!t.dirty());
        // One group: its sizes are named plainly.
        let v = json!({"groups":[{"faces":["box/round/f"],
            "radius":{"m":0.003,"value":"3 mm"}}],
            "id":"r","revision":"r1","target":"base","type":"resize_blend"});
        let t = TweakForm::from_show("resize_blend", &v).unwrap();
        assert_eq!(t.fields()[0].name, "radius");
    }
}
