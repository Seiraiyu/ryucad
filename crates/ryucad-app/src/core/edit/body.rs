//! A shell's, offset's, mirror's or split's form in the property panel (ryucad's `body` kind).

use super::*;

/// One group of a shell's or offset's faces: its own thickness or
/// distance, and its faces, each one a checkbox to take it out (adding
/// one is an agent's work), as a fillet's edges are.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceGroup {
    pub quantity: Field,
    pub faces: Picks,
}

/// A shell's or offset's groups as `feature.show` gives them, each
/// group's quantity under `key` (`thickness` or `distance`).
fn face_groups(v: &Value, key: &str) -> Vec<FaceGroup> {
    v["faces"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, g)| FaceGroup {
            quantity: Field::new(&format!("{key} (group {})", i + 1), text(&g[key])),
            faces: Picks::new(&format!("faces (group {})", i + 1), texts(&g["faces"])),
        })
        .collect()
}

fn groups_dirty(groups: &[FaceGroup]) -> bool {
    groups.iter().any(|g| g.quantity.dirty() || g.faces.dirty())
}

fn revert_groups(groups: &mut [FaceGroup]) {
    for g in groups {
        g.quantity.text.clone_from(&g.quantity.read);
        g.faces.chosen.clone_from(&g.faces.read);
    }
}

/// The groups as `feature.edit` takes them: every one, whole, as typed; a
/// group left with no faces goes.
fn groups_json(groups: &[FaceGroup], key: &str) -> Value {
    let out: Vec<Value> = groups
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

/// A shell's form (design 327): its target shown; its thickness and
/// direction edited; its open faces and its groups' faces each a checkbox
/// to take out, its groups' thicknesses edited.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellForm {
    pub target: String,
    pub thickness: Field,
    /// `inward` or `outward`.
    pub direction: Choice,
    pub open: Picks,
    pub groups: Vec<FaceGroup>,
}

impl ShellForm {
    /// From a `feature.show` of a shell.
    pub(super) fn from_show(v: &Value) -> ShellForm {
        let direction = v["direction"].as_str().unwrap_or("inward").to_owned();
        ShellForm {
            target: text(&v["target"]),
            thickness: Field::new("thickness", text(&v["thickness"])),
            direction: Choice::new("direction", &["inward", "outward"], direction),
            open: Picks::new("open faces", texts(&v["open"])),
            groups: face_groups(v, "thickness"),
        }
    }

    pub(super) fn fields(&self) -> impl Iterator<Item = &Field> {
        std::iter::once(&self.thickness).chain(self.groups.iter().map(|g| &g.quantity))
    }

    pub fn dirty(&self) -> bool {
        self.thickness.dirty()
            || self.direction.dirty()
            || self.open.dirty()
            || groups_dirty(&self.groups)
    }

    pub(super) fn revert(&mut self) {
        self.thickness.text.clone_from(&self.thickness.read);
        self.direction.chosen.clone_from(&self.direction.read);
        self.open.chosen.clone_from(&self.open.read);
        revert_groups(&mut self.groups);
    }

    /// What changed, as `feature.edit` takes it: the thickness typed, the
    /// direction chosen, the open faces left (none: `[]`), the groups
    /// whole.
    pub(super) fn given(&self, out: &mut FeatureFields) {
        if self.thickness.dirty() {
            out.thickness = Some(json!(self.thickness.text.trim()));
        }
        if self.direction.dirty() {
            out.direction = Some(self.direction.chosen.clone());
        }
        if self.open.dirty() {
            out.open = Some(self.open.chosen.clone());
        }
        if groups_dirty(&self.groups) {
            out.faces = Some(groups_json(&self.groups, "thickness"));
        }
    }
}

/// An offset's form (design 327): its target shown; its distance edited;
/// its groups' distances edited and their faces each a checkbox to take
/// out. `OffsetForm` is a mate connector's (question 148).
#[derive(Clone, Debug, PartialEq)]
pub struct BodyOffsetForm {
    pub target: String,
    pub distance: Field,
    pub groups: Vec<FaceGroup>,
}

impl BodyOffsetForm {
    /// From a `feature.show` of an offset.
    pub(super) fn from_show(v: &Value) -> BodyOffsetForm {
        BodyOffsetForm {
            target: text(&v["target"]),
            distance: Field::new("distance", text(&v["distance"])),
            groups: face_groups(v, "distance"),
        }
    }

    pub(super) fn fields(&self) -> impl Iterator<Item = &Field> {
        std::iter::once(&self.distance).chain(self.groups.iter().map(|g| &g.quantity))
    }

    pub fn dirty(&self) -> bool {
        self.distance.dirty() || groups_dirty(&self.groups)
    }

    pub(super) fn revert(&mut self) {
        self.distance.text.clone_from(&self.distance.read);
        revert_groups(&mut self.groups);
    }

    pub(super) fn given(&self, out: &mut FeatureFields) {
        if self.distance.dirty() {
            out.distance = Some(json!(self.distance.text.trim()));
        }
        if groups_dirty(&self.groups) {
            out.faces = Some(groups_json(&self.groups, "distance"));
        }
    }
}

/// A mirror's form (design 327): the solid it copies and its plane shown,
/// never changed here; its operation and targets are the form's own, as
/// an extrude's.
#[derive(Clone, Debug, PartialEq)]
pub struct MirrorForm {
    pub solid: String,
    pub plane: String,
}

/// A split's form (design 327): its target and plane or tool shown;
/// whether it is flipped and which sides it keeps edited.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitForm {
    pub target: String,
    pub plane: String,
    /// A split by a tool (ryucad 18): `body parting`, or `faces …`; none
    /// for a split by a plane.
    pub tool: Option<String>,
    pub flip: Flag,
    /// `both` or `behind`.
    pub keep: Choice,
}

impl SplitForm {
    pub(super) fn from_show(v: &Value) -> SplitForm {
        let flip = v["flip"] == true;
        let keep = v["keep"].as_str().unwrap_or("both").to_owned();
        let tool = &v["tool"];
        let tool = match (tool["body"].as_str(), tool["faces"].as_array()) {
            (Some(b), _) => Some(format!("body {b}")),
            (None, Some(fs)) => Some(format!("faces {}", texts(&json!(fs)).join(", "))),
            (None, None) => None,
        };
        SplitForm {
            target: text(&v["target"]),
            plane: text(&v["plane"]),
            tool,
            flip: Flag {
                name: "flip".to_owned(),
                read: flip,
                chosen: flip,
            },
            keep: Choice::new("keep", &["both", "behind"], keep),
        }
    }

    pub fn dirty(&self) -> bool {
        self.flip.dirty() || self.keep.dirty()
    }

    pub(super) fn revert(&mut self) {
        self.flip.chosen = self.flip.read;
        self.keep.chosen.clone_from(&self.keep.read);
    }

    pub(super) fn given(&self, out: &mut FeatureFields) {
        if self.flip.dirty() {
            out.flip = Some(self.flip.chosen.into());
        }
        if self.keep.dirty() {
            out.keep = Some(self.keep.chosen.clone());
        }
    }
}
