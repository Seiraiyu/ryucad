//! Sheet metal features' forms (design 346, The app): a base flange's
//! thickness, radius, allowance and relief (and a bent section's depth); a
//! flange's angle, length, measure, position, radius, relief, setback and
//! ends' values; an unfold's allowance and overrides; a refold's bends.
//! What they name (edges, ends, bends, the fixed face) is shown, each a
//! checkbox to take it out, never to add one, as a fillet's edges are. A
//! value the founder gives (a flange's radius and relief, an unfold's
//! allowance) reads as the founder's until it is changed here, when the
//! feature takes its own.

use super::{Choice, FeatureForm, Field, Flag, Picks, claimed, text, texts};
use ryucad::error::Error;
use ryucad::proto::FeatureFields;
use serde_json::{Value, json};

/// A bend allowance's form and value: a base flange's `k_factor` or
/// `deduction`, an unfold's or override's also `allowance`.
#[derive(Clone, Debug, PartialEq)]
pub struct AllowanceForm {
    pub form: Choice,
    pub value: Field,
}

impl AllowanceForm {
    fn from_show(name: &str, v: &Value, forms: &[&str]) -> AllowanceForm {
        let read = forms
            .iter()
            .find(|f| !v[**f].is_null())
            .copied()
            .unwrap_or(forms[0]);
        AllowanceForm {
            form: Choice::new(&format!("{name} form"), forms, read.to_owned()),
            value: Field::new(name, text(&v[read])),
        }
    }

    pub fn dirty(&self) -> bool {
        self.form.dirty() || self.value.dirty()
    }

    fn revert(&mut self) {
        self.form.chosen.clone_from(&self.form.read);
        self.value.text.clone_from(&self.value.read);
    }

    /// As `feature.edit` takes it: `{"k_factor": "0.44"}`.
    pub fn json(&self) -> Value {
        let mut v = json!({});
        v[self.form.chosen.as_str()] = json!(self.value.text.trim());
        v
    }
}

/// A relief: its kind and the sizes it takes (a tear its width alone).
#[derive(Clone, Debug, PartialEq)]
pub struct ReliefForm {
    pub kind: Choice,
    pub width: Field,
    pub depth: Field,
}

/// The kinds of relief, `none` first.
pub const RELIEFS: [&str; 4] = ["none", "rectangular", "obround", "tear"];

impl ReliefForm {
    fn from_show(name: &str, v: &Value) -> ReliefForm {
        ReliefForm {
            kind: Choice::new(
                &format!("{name} kind"),
                &RELIEFS,
                v["kind"].as_str().unwrap_or("none").to_owned(),
            ),
            width: Field::new(&format!("{name} width"), text(&v["width"])),
            depth: Field::new(&format!("{name} depth"), text(&v["depth"])),
        }
    }

    /// Whether its kind takes a width, and a depth.
    pub fn takes(&self) -> (bool, bool) {
        match self.kind.chosen.as_str() {
            "rectangular" | "obround" => (true, true),
            "tear" => (true, false),
            _ => (false, false),
        }
    }

    pub fn dirty(&self) -> bool {
        let (w, d) = self.takes();
        self.kind.dirty() || w && self.width.dirty() || d && self.depth.dirty()
    }

    fn revert(&mut self) {
        self.kind.chosen.clone_from(&self.kind.read);
        self.width.text.clone_from(&self.width.read);
        self.depth.text.clone_from(&self.depth.read);
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        [&self.width, &self.depth].into_iter()
    }

    fn fields_mut(&mut self) -> impl Iterator<Item = &mut Field> {
        [&mut self.width, &mut self.depth].into_iter()
    }

    /// As `feature.edit` takes it: `{"kind": "tear", "width": "1 mm"}`.
    pub fn json(&self) -> Value {
        let (w, d) = self.takes();
        let mut v = json!({"kind": self.kind.chosen});
        if w {
            v["width"] = json!(self.width.text.trim());
        }
        if d {
            v["depth"] = json!(self.depth.text.trim());
        }
        v
    }
}

/// A base flange's form: its sketch shown; its thickness, inner radius,
/// allowance and default relief edited, a bent section's depth too, and
/// which way it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseFlangeForm {
    pub sketch: String,
    pub thickness: Field,
    pub radius: Field,
    pub allowance: AllowanceForm,
    pub relief: ReliefForm,
    /// A bent section's depth; `None` for a plate.
    pub depth: Option<Field>,
    pub reverse: Flag,
    /// A bent section's symmetry about its sketch.
    pub symmetric: Option<Flag>,
}

fn flag(name: &str, on: bool) -> Flag {
    Flag {
        name: name.to_owned(),
        read: on,
        chosen: on,
    }
}

impl BaseFlangeForm {
    fn from_show(v: &Value) -> BaseFlangeForm {
        let section = !v["depth"].is_null();
        BaseFlangeForm {
            sketch: text(&v["sketch"]),
            thickness: Field::new("thickness", text(&v["thickness"])),
            radius: Field::new("radius", text(&v["radius"])),
            allowance: AllowanceForm::from_show(
                "allowance",
                &v["allowance"],
                &["k_factor", "deduction"],
            ),
            relief: ReliefForm::from_show("relief", &v["relief"]),
            depth: section.then(|| Field::new("depth", text(&v["depth"]))),
            reverse: flag("reverse", v["reverse"] == true),
            symmetric: section.then(|| flag("symmetric", v["symmetric"] == true)),
        }
    }
}

/// One end a flange names: its vertex, and its own setback and relief.
#[derive(Clone, Debug, PartialEq)]
pub struct EndForm {
    pub vertex: String,
    /// Its setback; empty for the flange's.
    pub setback: Field,
    /// Its own relief; `None` for the flange's.
    pub relief: Option<ReliefForm>,
}

/// A flange's form: its target and edges shown, each edge (and each end)
/// a checkbox to take it out; its angle, length, measure, position,
/// radius, relief and setback edited, and each end's setback and relief.
#[derive(Clone, Debug, PartialEq)]
pub struct FlangeForm {
    pub target: String,
    pub edges: Picks,
    /// The sides edges name, `{edge, side}`, kept for the edges kept.
    pub sides: Vec<(String, String)>,
    pub angle: Field,
    pub length: Field,
    pub measured: Choice,
    pub position: Choice,
    pub radius: Field,
    pub relief: ReliefForm,
    pub setback: Field,
    /// What the founder gives, not the flange: `radius`, `relief`.
    pub inherited: Vec<String>,
    pub ends: Vec<EndForm>,
    /// The ends kept, by vertex.
    pub kept: Picks,
}

impl FlangeForm {
    fn from_show(v: &Value) -> FlangeForm {
        let mut edges = Vec::new();
        let mut sides = Vec::new();
        for e in v["edges"].as_array().into_iter().flatten() {
            match e {
                Value::Object(o) => {
                    let edge = text(&o["edge"]);
                    sides.push((edge.clone(), text(&o["side"])));
                    edges.push(edge);
                }
                other => edges.push(text(other)),
            }
        }
        let ends: Vec<EndForm> = v["ends"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, e)| EndForm {
                vertex: text(&e["vertex"]),
                setback: Field::new(&format!("setback (end {})", i + 1), text(&e["setback"])),
                relief: e["relief"].is_object().then(|| {
                    ReliefForm::from_show(&format!("relief (end {})", i + 1), &e["relief"])
                }),
            })
            .collect();
        let vertices = ends.iter().map(|e| e.vertex.clone()).collect();
        FlangeForm {
            target: text(&v["target"]),
            edges: Picks::new("edges", edges),
            sides,
            angle: Field::new("angle", text(&v["angle"])),
            length: Field::new("length", text(&v["length"])),
            measured: Choice::new(
                "measured",
                &["outer_mould", "inner_mould", "tangent"],
                v["measured"].as_str().unwrap_or("outer_mould").to_owned(),
            ),
            position: Choice::new(
                "position",
                &["material_inside", "material_outside", "bend_outside"],
                v["position"]
                    .as_str()
                    .unwrap_or("material_inside")
                    .to_owned(),
            ),
            radius: Field::new("radius", text(&v["radius"])),
            relief: ReliefForm::from_show("relief", &v["relief"]),
            setback: Field::new("setback", text(&v["setback"])),
            inherited: texts(&v["inherited"]),
            ends,
            kept: Picks::new("ends", vertices),
        }
    }

    /// Whether `field` (`radius`, `relief`) is the founder's.
    pub fn inherits(&self, field: &str) -> bool {
        self.inherited.iter().any(|f| f == field)
    }

    fn ends_dirty(&self) -> bool {
        self.kept.dirty()
            || self
                .ends
                .iter()
                .any(|e| e.setback.dirty() || e.relief.as_ref().is_some_and(ReliefForm::dirty))
    }

    /// The ends kept, whole, as `feature.edit` takes them.
    fn ends_json(&self) -> Value {
        let out: Vec<Value> = self
            .ends
            .iter()
            .filter(|e| self.kept.chosen.contains(&e.vertex))
            .map(|e| {
                let mut v = json!({"vertex": e.vertex});
                if !e.setback.text.trim().is_empty() {
                    v["setback"] = json!(e.setback.text.trim());
                }
                if let Some(r) = &e.relief {
                    v["relief"] = r.json();
                }
                v
            })
            .collect();
        json!(out)
    }
}

/// One allowance override of an unfold: its bend, and its allowance.
#[derive(Clone, Debug, PartialEq)]
pub struct OverrideForm {
    pub bend: String,
    pub allowance: AllowanceForm,
}

/// An unfold's form: its target and fixed face shown (the fixed face a
/// checkbox to fall back on the kernel's default flange); its bends
/// (`only`) and overrides each a checkbox to take out; its allowance and
/// each override's edited.
#[derive(Clone, Debug, PartialEq)]
pub struct UnfoldForm {
    pub target: String,
    /// The side face held still; `None` for the kernel's default flange.
    pub fixed: Option<String>,
    pub keep_fixed: Flag,
    /// The bends it unfolds, each by a face of it; none for every bend.
    pub only: Picks,
    pub allowance: AllowanceForm,
    /// Whether the allowance is the founder's.
    pub inherited: bool,
    pub overrides: Vec<OverrideForm>,
    /// The overrides kept, by bend.
    pub kept: Picks,
}

const ALLOWANCES: [&str; 3] = ["k_factor", "deduction", "allowance"];

impl UnfoldForm {
    fn from_show(v: &Value) -> UnfoldForm {
        let overrides: Vec<OverrideForm> = v["bends"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, b)| OverrideForm {
                bend: text(&b["bend"]),
                allowance: AllowanceForm::from_show(
                    &format!("allowance (override {})", i + 1),
                    b,
                    &ALLOWANCES,
                ),
            })
            .collect();
        let bends = overrides.iter().map(|o| o.bend.clone()).collect();
        let fixed = v["fixed"].as_str().map(str::to_owned);
        UnfoldForm {
            target: text(&v["target"]),
            keep_fixed: flag("fixed face", fixed.is_some()),
            fixed,
            only: Picks::new("bends", texts(&v["only"])),
            allowance: AllowanceForm::from_show(
                "allowance",
                &v["allowance"],
                &["k_factor", "deduction"],
            ),
            inherited: texts(&v["inherited"]).iter().any(|f| f == "allowance"),
            overrides,
            kept: Picks::new("overrides", bends),
        }
    }

    fn overrides_dirty(&self) -> bool {
        self.kept.dirty() || self.overrides.iter().any(|o| o.allowance.dirty())
    }

    fn overrides_json(&self) -> Value {
        let out: Vec<Value> = self
            .overrides
            .iter()
            .filter(|o| self.kept.chosen.contains(&o.bend))
            .map(|o| {
                let mut v = o.allowance.json();
                v["bend"] = json!(o.bend);
                v
            })
            .collect();
        json!(out)
    }
}

/// A refold's form: its target and unfold shown, the bends it folds each
/// a checkbox to take out.
#[derive(Clone, Debug, PartialEq)]
pub struct RefoldForm {
    pub target: String,
    pub unfold: String,
    pub only: Picks,
}

/// A sheet metal feature's form, by its type.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetMetalForm {
    BaseFlange(Box<BaseFlangeForm>),
    Flange(Box<FlangeForm>),
    Unfold(Box<UnfoldForm>),
    Refold(Box<RefoldForm>),
}

impl SheetMetalForm {
    /// From a `feature.show` of a feature of type `ty`; `None` for any
    /// other type.
    pub fn from_show(ty: &str, v: &Value) -> Option<SheetMetalForm> {
        Some(match ty {
            "base_flange" => SheetMetalForm::BaseFlange(Box::new(BaseFlangeForm::from_show(v))),
            "flange" => SheetMetalForm::Flange(Box::new(FlangeForm::from_show(v))),
            "unfold" => SheetMetalForm::Unfold(Box::new(UnfoldForm::from_show(v))),
            "refold" => SheetMetalForm::Refold(Box::new(RefoldForm {
                target: text(&v["target"]),
                unfold: text(&v["unfold"]),
                only: Picks::new("bends", texts(&v["only"])),
            })),
            _ => return None,
        })
    }

    /// Every field it edits, to place errors and keep what was typed.
    pub fn fields(&self) -> Vec<&Field> {
        let mut out: Vec<&Field> = Vec::new();
        match self {
            SheetMetalForm::BaseFlange(b) => {
                out.extend([&b.thickness, &b.radius, &b.allowance.value]);
                out.extend(b.relief.fields());
                out.extend(&b.depth);
            }
            SheetMetalForm::Flange(f) => {
                out.extend([&f.angle, &f.length, &f.radius, &f.setback]);
                out.extend(f.relief.fields());
                for e in &f.ends {
                    out.push(&e.setback);
                    out.extend(e.relief.iter().flat_map(ReliefForm::fields));
                }
            }
            SheetMetalForm::Unfold(u) => {
                out.push(&u.allowance.value);
                out.extend(u.overrides.iter().map(|o| &o.allowance.value));
            }
            SheetMetalForm::Refold(_) => {}
        }
        out
    }

    pub fn fields_mut(&mut self) -> Vec<&mut Field> {
        let mut out: Vec<&mut Field> = Vec::new();
        match self {
            SheetMetalForm::BaseFlange(b) => {
                out.extend([&mut b.thickness, &mut b.radius, &mut b.allowance.value]);
                out.extend(b.relief.fields_mut());
                out.extend(&mut b.depth);
            }
            SheetMetalForm::Flange(f) => {
                out.extend([&mut f.angle, &mut f.length, &mut f.radius, &mut f.setback]);
                out.extend(f.relief.fields_mut());
                for e in &mut f.ends {
                    out.push(&mut e.setback);
                    out.extend(e.relief.iter_mut().flat_map(ReliefForm::fields_mut));
                }
            }
            SheetMetalForm::Unfold(u) => {
                out.push(&mut u.allowance.value);
                out.extend(u.overrides.iter_mut().map(|o| &mut o.allowance.value));
            }
            SheetMetalForm::Refold(_) => {}
        }
        out
    }

    /// The field at a pointer whose last step repeats: an end's or an
    /// override's (`/ends/0/setback`, `/bends/1/k_factor`), a relief's
    /// size (`/relief/width`, `/ends/0/relief/depth`).
    pub fn at_pointer(&mut self, steps: &[&str]) -> Option<&mut Field> {
        fn relief<'a>(r: &'a mut ReliefForm, k: &str) -> Option<&'a mut Field> {
            match k {
                "width" => Some(&mut r.width),
                "depth" => Some(&mut r.depth),
                _ => None,
            }
        }
        match (self, steps) {
            (SheetMetalForm::BaseFlange(b), ["relief", k]) => relief(&mut b.relief, k),
            (SheetMetalForm::BaseFlange(b), ["allowance", ..]) => Some(&mut b.allowance.value),
            (SheetMetalForm::Flange(f), ["relief", k]) => relief(&mut f.relief, k),
            (SheetMetalForm::Flange(f), ["ends", i, "setback"]) => {
                Some(&mut f.ends.get_mut(i.parse::<usize>().ok()?)?.setback)
            }
            (SheetMetalForm::Flange(f), ["ends", i, "relief", k]) => relief(
                f.ends.get_mut(i.parse::<usize>().ok()?)?.relief.as_mut()?,
                k,
            ),
            (SheetMetalForm::Unfold(u), ["allowance", ..]) => Some(&mut u.allowance.value),
            (SheetMetalForm::Unfold(u), ["bends", i, ..]) => Some(
                &mut u
                    .overrides
                    .get_mut(i.parse::<usize>().ok()?)?
                    .allowance
                    .value,
            ),
            _ => None,
        }
    }

    pub fn dirty(&self) -> bool {
        match self {
            SheetMetalForm::BaseFlange(b) => {
                b.thickness.dirty()
                    || b.radius.dirty()
                    || b.allowance.dirty()
                    || b.relief.dirty()
                    || b.depth.as_ref().is_some_and(Field::dirty)
                    || b.reverse.dirty()
                    || b.symmetric.as_ref().is_some_and(Flag::dirty)
            }
            SheetMetalForm::Flange(f) => {
                f.edges.dirty()
                    || [&f.angle, &f.length, &f.radius, &f.setback]
                        .iter()
                        .any(|x| x.dirty())
                    || f.measured.dirty()
                    || f.position.dirty()
                    || f.relief.dirty()
                    || f.ends_dirty()
            }
            SheetMetalForm::Unfold(u) => {
                u.keep_fixed.dirty() || u.only.dirty() || u.allowance.dirty() || u.overrides_dirty()
            }
            SheetMetalForm::Refold(r) => r.only.dirty(),
        }
    }

    pub fn revert(&mut self) {
        for f in self.fields_mut() {
            f.text.clone_from(&f.read);
        }
        match self {
            SheetMetalForm::BaseFlange(b) => {
                b.allowance.revert();
                b.relief.revert();
                b.reverse.chosen = b.reverse.read;
                if let Some(s) = &mut b.symmetric {
                    s.chosen = s.read;
                }
            }
            SheetMetalForm::Flange(f) => {
                f.edges.chosen.clone_from(&f.edges.read);
                f.measured.chosen.clone_from(&f.measured.read);
                f.position.chosen.clone_from(&f.position.read);
                f.relief.revert();
                f.kept.chosen.clone_from(&f.kept.read);
                for e in &mut f.ends {
                    if let Some(r) = &mut e.relief {
                        r.revert();
                    }
                }
            }
            SheetMetalForm::Unfold(u) => {
                u.keep_fixed.chosen = u.keep_fixed.read;
                u.only.chosen.clone_from(&u.only.read);
                u.allowance.revert();
                u.kept.chosen.clone_from(&u.kept.read);
                for o in &mut u.overrides {
                    o.allowance.revert();
                }
            }
            SheetMetalForm::Refold(r) => r.only.chosen.clone_from(&r.only.read),
        }
    }

    /// What changed, as `feature.edit` takes it: each value typed or
    /// chosen; lists (edges, ends, bends, overrides) whole, as kept.
    pub fn given(&self, out: &mut FeatureFields) {
        let q = |f: &Field| f.dirty().then(|| json!(f.text.trim()));
        match self {
            SheetMetalForm::BaseFlange(b) => {
                out.thickness = q(&b.thickness);
                out.radius = q(&b.radius);
                out.depth = b.depth.as_ref().and_then(q);
                if b.allowance.dirty() {
                    out.allowance = Some(b.allowance.json());
                }
                if b.relief.dirty() {
                    out.relief = Some(b.relief.json());
                }
                if b.reverse.dirty() {
                    out.reverse = Some(b.reverse.chosen);
                }
                if let Some(s) = b.symmetric.as_ref().filter(|s| s.dirty()) {
                    out.symmetric = Some(s.chosen);
                }
            }
            SheetMetalForm::Flange(f) => {
                if f.edges.dirty() {
                    out.edges = Some(f.edges.chosen.clone());
                    if !f.sides.is_empty() {
                        let kept: Vec<Value> = f
                            .sides
                            .iter()
                            .filter(|(e, _)| f.edges.chosen.contains(e))
                            .map(|(e, s)| json!({"edge": e, "side": s}))
                            .collect();
                        out.sides = Some(json!(kept));
                    }
                }
                out.angle = q(&f.angle);
                out.length = q(&f.length);
                out.radius = q(&f.radius);
                out.setback = q(&f.setback);
                if f.measured.dirty() {
                    out.measured = Some(f.measured.chosen.clone());
                }
                if f.position.dirty() {
                    out.position = Some(f.position.chosen.clone());
                }
                if f.relief.dirty() {
                    out.relief = Some(f.relief.json());
                }
                if f.ends_dirty() {
                    out.ends = Some(f.ends_json());
                }
            }
            SheetMetalForm::Unfold(u) => {
                if u.keep_fixed.dirty() {
                    out.fixed_face = Some(match (&u.fixed, u.keep_fixed.chosen) {
                        (Some(f), true) => f.clone(),
                        _ => String::new(),
                    });
                }
                if u.only.dirty() {
                    out.only = Some(u.only.chosen.clone());
                }
                if u.allowance.dirty() {
                    out.allowance = Some(u.allowance.json());
                }
                if u.overrides_dirty() {
                    out.bends = Some(u.overrides_json());
                }
            }
            SheetMetalForm::Refold(r) => {
                if r.only.dirty() {
                    out.only = Some(r.only.chosen.clone());
                }
            }
        }
    }
}

impl FeatureForm for SheetMetalForm {
    fn read(ty: &str, v: &Value) -> Option<SheetMetalForm> {
        SheetMetalForm::from_show(ty, v)
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
        "base_flange" => "base-flange",
        "flange" => "edge-flange",
        "unfold" => "unfold",
        "refold" => "refold",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `feature.show`'s answers for a plate, a flange on its edge with an
    /// end set back and relieved, an unfold of its one bend with an
    /// override, and a refold, as 16.B's daemon gives them.
    fn shows() -> [Value; 4] {
        [
            json!({"allowance":{"k_factor":{"n":0.44,"value":"0.44"}},"id":"plate",
                "radius":{"m":0.001,"value":"1 mm"},"relief":{"kind":"none"},"reverse":false,
                "revision":"r1","sketch":"s","thickness":{"m":0.0015,"value":"1.5 mm"},
                "type":"base_flange"}),
            json!({"angle":{"rad":std::f64::consts::FRAC_PI_2,"value":"90 deg"},"edges":["p/plate/e1"],
                "ends":[{"relief":{"depth":{"m":0.001,"value":"1 mm"},"kind":"rectangular",
                         "width":{"m":0.001,"value":"1 mm"}},"setback":{"m":0.002,"value":"2 mm"},
                         "vertex":"p/plate/v1"}],
                "id":"f","inherited":["radius","relief"],"length":{"m":0.02,"value":"20 mm"},
                "measured":"outer_mould","position":"material_inside",
                "radius":{"m":0.001,"value":"1 mm"},"relief":{"kind":"none"},"revision":"r1",
                "setback":{"m":0.0,"value":"0 mm"},"target":"plate","type":"flange"}),
            json!({"allowance":{"k_factor":{"n":0.44,"value":"0.44"}},
                "bends":[{"bend":"p/plate/b1","k_factor":{"n":0.3,"value":"0.3"}}],"id":"u",
                "inherited":["allowance"],"only":["p/plate/b2"],"revision":"r1",
                "target":"plate","type":"unfold"}),
            json!({"id":"r","revision":"r1","target":"plate","type":"refold","unfold":"u"}),
        ]
    }

    fn given(f: &SheetMetalForm) -> Value {
        let mut out = FeatureFields::default();
        f.given(&mut out);
        serde_json::to_value(out).unwrap()
    }

    #[test]
    fn a_base_flanges_settings_are_edited_and_sent_alone() {
        let [plate, ..] = shows();
        let mut f = SheetMetalForm::from_show("base_flange", &plate).unwrap();
        let SheetMetalForm::BaseFlange(b) = &mut f else {
            panic!("a base flange")
        };
        assert_eq!(
            (
                b.thickness.read.as_str(),
                b.allowance.form.read.as_str(),
                b.allowance.value.read.as_str()
            ),
            ("1.5 mm", "k_factor", "0.44")
        );
        assert!(b.depth.is_none() && b.symmetric.is_none());
        assert!(!f.dirty());
        let SheetMetalForm::BaseFlange(b) = &mut f else {
            unreachable!()
        };
        b.allowance.value.text = "0.5".into();
        b.relief.kind.chosen = "tear".into();
        b.relief.width.text = "1 mm".into();
        assert!(f.dirty());
        assert_eq!(
            given(&f),
            json!({"allowance": {"k_factor": "0.5"}, "relief": {"kind": "tear", "width": "1 mm"}})
        );
        f.revert();
        assert!(!f.dirty());
        assert_eq!(given(&f), json!({}));
    }

    #[test]
    fn a_flanges_sizes_and_ends_are_sent_and_the_founders_shown() {
        let [_, flange, ..] = shows();
        let mut f = SheetMetalForm::from_show("flange", &flange).unwrap();
        let SheetMetalForm::Flange(x) = &mut f else {
            panic!("a flange")
        };
        assert!(x.inherits("radius") && x.inherits("relief") && !x.inherits("angle"));
        assert_eq!(x.ends[0].setback.read, "2 mm");
        assert_eq!(x.ends[0].relief.as_ref().unwrap().kind.read, "rectangular");
        x.angle.text = "120 deg".into();
        x.measured.chosen = "tangent".into();
        x.ends[0].setback.text = "3 mm".into();
        assert_eq!(
            given(&f),
            json!({"angle": "120 deg", "measured": "tangent",
                   "ends": [{"vertex": "p/plate/v1", "setback": "3 mm",
                             "relief": {"kind": "rectangular", "width": "1 mm", "depth": "1 mm"}}]})
        );
        // Taking the end out sends the ends left: none.
        let SheetMetalForm::Flange(x) = &mut f else {
            unreachable!()
        };
        x.kept.toggle("p/plate/v1");
        assert_eq!(given(&f)["ends"], json!([]));
        // An end's refusal lands on its own field.
        let field = f.at_pointer(&["ends", "0", "setback"]).unwrap();
        assert_eq!(field.name, "setback (end 1)");
    }

    #[test]
    fn an_unfolds_fixed_face_bends_and_overrides_are_taken_out_and_edited() {
        let [_, _, unfold, _] = shows();
        let mut f = SheetMetalForm::from_show("unfold", &unfold).unwrap();
        let SheetMetalForm::Unfold(u) = &mut f else {
            panic!("an unfold")
        };
        assert!(u.inherited && u.fixed.is_none() && !u.keep_fixed.read);
        assert_eq!(u.overrides[0].allowance.form.read, "k_factor");
        u.overrides[0].allowance.form.chosen = "deduction".into();
        u.overrides[0].allowance.value.text = "2.2 mm".into();
        u.only.toggle("p/plate/b2");
        assert_eq!(
            given(&f),
            json!({"only": [], "bends": [{"bend": "p/plate/b1", "deduction": "2.2 mm"}]})
        );
        assert_eq!(
            f.at_pointer(&["bends", "0", "deduction"]).unwrap().name,
            "allowance (override 1)"
        );
        // A fixed face read is kept until its box is cleared.
        let mut fixed = unfold.clone();
        fixed["fixed"] = json!("p/plate/side");
        let mut f = SheetMetalForm::from_show("unfold", &fixed).unwrap();
        let SheetMetalForm::Unfold(u) = &mut f else {
            unreachable!()
        };
        u.keep_fixed.chosen = false;
        assert_eq!(given(&f), json!({"fixed_face": ""}));
    }

    #[test]
    fn a_refold_shows_its_unfold_and_sends_its_bends() {
        let [.., refold] = shows();
        let f = SheetMetalForm::from_show("refold", &refold).unwrap();
        let SheetMetalForm::Refold(r) = &f else {
            panic!("a refold")
        };
        assert_eq!((r.target.as_str(), r.unfold.as_str()), ("plate", "u"));
        assert!(r.only.options.is_empty());
        assert!(SheetMetalForm::from_show("shell", &refold).is_none());
    }
}
