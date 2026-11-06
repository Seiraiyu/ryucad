//! Editing a feature's parameters and placement, what it does to solids,
//! an extrude's extent, a boolean's solids, a fillet's or chamfer's sizes
//! and edges, a hole's sizes, depth and points, a pattern's spacings,
//! counts, skips and seeds, and an instance's revision, pose and whether
//! it is fixed: the property panel's form.
//! Values stay as written (quantities with their units). A save is an
//! ordinary `feature.edit` with the revision the form was read at, so a
//! human's stale edit fails with `conflict` like an agent's; the form then
//! reloads and lists what changed instead of overwriting it.

use ryucad::error::Error;
use ryucad::proto::{FeatureFields, PlacementArg, Request};
use ryucad::units::Unit;
use serde_json::{Map, Value, json};

/// One editable value: its name, as read, and as typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub read: String,
    pub text: String,
    /// What the daemon last said it is, in the project's unit, when what
    /// was read isn't a literal (`12 mm` for `thickness + 4 mm`).
    pub value: Option<String>,
    /// Why the daemon refused it, placed on it by its pointer.
    pub error: Option<String>,
}

impl Field {
    pub fn new(name: &str, read: String) -> Field {
        Field {
            name: name.to_owned(),
            text: read.clone(),
            read,
            value: None,
            error: None,
        }
    }

    /// The same, with the daemon's value beside it.
    pub fn valued(mut self, value: Option<String>) -> Field {
        self.value = value;
        self
    }

    pub fn dirty(&self) -> bool {
        self.text.trim() != self.read
    }

    /// What is wrong with what was typed as an expression, as it is typed:
    /// `expected ')' at byte 8`. Empty text, and text unchanged from what
    /// was read, are left to the daemon.
    pub fn syntax(&self) -> Option<String> {
        let t = self.text.trim();
        if t.is_empty() || !self.dirty() {
            return None;
        }
        let e = ryucad::expr::Expr::parse(t).err()?;
        let why = e
            .message
            .rsplit(": ")
            .next()
            .unwrap_or(&e.message)
            .to_owned();
        Some(match e.details["position"].as_u64() {
            Some(at) => format!("{why} at byte {at}"),
            None => why,
        })
    }
}

/// A quantity's value as the daemon gives it (`{"value", "m"}`, or
/// `"rad"` for an angle), shown beside its field when what was written
/// isn't a literal: in `unit`, or in degrees.
pub fn shown_value(q: &Value, unit: Unit) -> Option<String> {
    if q["expr"] != true {
        return None;
    }
    let trim = |x: f64| {
        let s = format!("{x:.6}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" {
            "0".to_owned()
        } else {
            s.to_owned()
        }
    };
    if let Some(m) = q["m"].as_f64() {
        return Some(format!("{} {}", trim(m / unit.factor()), unit.as_str()));
    }
    if let Some(r) = q["rad"].as_f64() {
        return Some(format!("{} deg", trim(r.to_degrees())));
    }
    None
}

/// The field a daemon's refusal names: the last step of its `pointer`
/// (`/params/length`), else its `param`, `field` or `name`.
pub fn field_of(e: &Error) -> Option<String> {
    let d = &e.details;
    if let Some(p) = d["pointer"].as_str() {
        return p
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
    }
    ["param", "field", "parameter", "name"]
        .iter()
        .find_map(|k| d[*k].as_str().map(str::to_owned))
}

/// One of a list: as read, and as chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub name: String,
    pub options: Vec<String>,
    pub read: String,
    pub chosen: String,
}

impl Choice {
    pub fn new(name: &str, options: &[&str], read: String) -> Choice {
        Choice {
            name: name.to_owned(),
            options: options.iter().map(|o| (*o).to_owned()).collect(),
            chosen: read.clone(),
            read,
        }
    }

    pub fn dirty(&self) -> bool {
        self.chosen != self.read
    }
}

/// Some of a list, in the order chosen: as read, and as chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Picks {
    pub name: String,
    pub options: Vec<String>,
    pub read: Vec<String>,
    pub chosen: Vec<String>,
}

impl Picks {
    pub fn new(name: &str, read: Vec<String>) -> Picks {
        Picks {
            name: name.to_owned(),
            options: read.clone(),
            chosen: read.clone(),
            read,
        }
    }

    pub fn dirty(&self) -> bool {
        self.chosen != self.read
    }

    /// Picks `option`, last, or unpicks it.
    pub fn toggle(&mut self, option: &str) {
        match self.chosen.iter().position(|o| o == option) {
            Some(i) => {
                self.chosen.remove(i);
            }
            None => self.chosen.push(option.to_owned()),
        }
    }
}

/// True or false: as read, and as chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Flag {
    pub name: String,
    pub read: bool,
    pub chosen: bool,
}

impl Flag {
    pub fn dirty(&self) -> bool {
        self.chosen != self.read
    }
}

/// An extrude's extent: its type, its length (blind or symmetric) and
/// whether through all goes each way.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtentForm {
    pub ty: Choice,
    pub length: Field,
    pub both: Flag,
}

impl ExtentForm {
    pub fn dirty(&self) -> bool {
        self.ty.dirty() || self.length.dirty() || self.both.dirty()
    }
}

/// One group of a fillet's or chamfer's edges: its sizes (a fillet's
/// radius; a chamfer's `d`, `d1`, `d2` or `angle`, as its form takes), its
/// form and face as read, and its edges. Edges can be removed, not added:
/// choosing one is an agent's work.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupForm {
    /// Each size by its key in the file, with its field.
    pub sizes: Vec<(String, Field)>,
    /// A chamfer's form: `equal`, `distances` or `distance_angle`.
    pub form: Option<String>,
    /// The face a chamfer's first distance runs along.
    pub face: Option<String>,
    pub edges: Picks,
}

/// A fillet's or chamfer's groups, and whether it grows its edges along
/// tangent chains.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendForm {
    pub groups: Vec<GroupForm>,
    pub propagate: Flag,
}

impl BlendForm {
    /// From a `feature.show` of a fillet or chamfer.
    fn from_show(v: &Value) -> BlendForm {
        let groups: Vec<&Value> = v["groups"].as_array().into_iter().flatten().collect();
        let many = groups.len() > 1;
        let groups = groups
            .into_iter()
            .enumerate()
            .map(|(i, g)| {
                let named = |key: &str| {
                    if many {
                        format!("{key} (group {})", i + 1)
                    } else {
                        key.to_owned()
                    }
                };
                let sizes = ["radius", "d", "d1", "d2", "angle"]
                    .into_iter()
                    .filter(|k| !g[*k].is_null())
                    .map(|k| (k.to_owned(), Field::new(&named(k), text(&g[k]))))
                    .collect();
                let edges = g["edges"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(text)
                    .collect();
                GroupForm {
                    sizes,
                    form: g["form"].as_str().map(str::to_owned),
                    face: g["face"].as_str().map(str::to_owned),
                    edges: Picks::new(&named("edges"), edges),
                }
            })
            .collect();
        let propagate = v["propagate"] != false;
        BlendForm {
            groups,
            propagate: Flag {
                name: "propagate".to_owned(),
                read: propagate,
                chosen: propagate,
            },
        }
    }

    fn sizes(&self) -> impl Iterator<Item = &Field> {
        self.groups
            .iter()
            .flat_map(|g| g.sizes.iter().map(|(_, f)| f))
    }

    /// Whether a group's sizes or edges changed: then all of them go.
    fn groups_dirty(&self) -> bool {
        self.sizes().any(Field::dirty) || self.groups.iter().any(|g| g.edges.dirty())
    }

    pub fn dirty(&self) -> bool {
        self.groups_dirty() || self.propagate.dirty()
    }

    fn revert(&mut self) {
        for g in &mut self.groups {
            for (_, f) in &mut g.sizes {
                f.text.clone_from(&f.read);
            }
            g.edges.chosen.clone_from(&g.edges.read);
        }
        self.propagate.chosen = self.propagate.read;
    }

    /// The groups as `feature.edit` takes them: every one, as typed.
    fn groups_json(&self) -> Value {
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|g| {
                let mut v = json!({"edges": g.edges.chosen});
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
        json!(groups)
    }
}

/// A hole's sizes (its diameter, its counterbore's or countersink's, a
/// drill point's angle), its depth (blind and its length, through all, or
/// up to a target, shown), its form as read, and its points. Points can
/// be taken out, not added; with none left it drills at every point no
/// curve uses.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleForm {
    /// `simple`, `counterbore` or `countersink`.
    pub form: String,
    /// Each size by its place in the file (`diameter`,
    /// `counterbore.depth`, `bottom.drill`, …), with its field.
    pub sizes: Vec<(String, Field)>,
    /// `blind` or `through_all`; `up_to` as read, to keep.
    pub depth: Choice,
    pub length: Field,
    /// An up-to depth's target.
    pub target: Option<String>,
    pub points: Picks,
}

impl HoleForm {
    /// From a `feature.show` of a hole.
    fn from_show(v: &Value) -> HoleForm {
        let mut sizes = vec![(
            "diameter".to_owned(),
            Field::new("diameter", text(&v["diameter"])),
        )];
        for (group, keys) in [
            ("counterbore", ["diameter", "depth"]),
            ("countersink", ["diameter", "angle"]),
        ] {
            for k in keys {
                let x = &v[group][k];
                if !x.is_null() {
                    let name = format!("{group} {k}");
                    sizes.push((format!("{group}.{k}"), Field::new(&name, text(x))));
                }
            }
        }
        if !v["bottom"]["drill"].is_null() {
            let read = text(&v["bottom"]["drill"]);
            sizes.push(("bottom.drill".to_owned(), Field::new("drill point", read)));
        }
        let (kind, length) = match &v["depth"] {
            Value::String(s) => (s.clone(), String::new()),
            d if !d["blind"].is_null() => ("blind".to_owned(), text(&d["blind"])),
            _ => ("up_to".to_owned(), String::new()),
        };
        let options: &[&str] = if kind == "up_to" {
            &["blind", "through_all", "up_to"]
        } else {
            &["blind", "through_all"]
        };
        let points = v["points"]
            .as_array()
            .into_iter()
            .flatten()
            .map(text)
            .collect();
        HoleForm {
            form: text(&v["form"]),
            sizes,
            depth: Choice::new("depth", options, kind),
            length: Field::new("depth", length),
            target: v["depth"]["up_to"]["target"].as_str().map(str::to_owned),
            points: Picks::new("points", points),
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        self.sizes.iter().map(|(_, f)| f).chain([&self.length])
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty) || self.depth.dirty() || self.points.dirty()
    }

    fn revert(&mut self) {
        for (_, f) in &mut self.sizes {
            f.text.clone_from(&f.read);
        }
        self.length.text.clone_from(&self.length.read);
        self.depth.chosen.clone_from(&self.depth.read);
        self.points.chosen.clone_from(&self.points.read);
    }

    /// What changed, as `feature.edit` takes it.
    fn given(&self, out: &mut FeatureFields) {
        let typed = |key: &str| {
            self.sizes
                .iter()
                .find(|(k, f)| k == key && f.dirty())
                .map(|(_, f)| json!(f.text.trim()))
        };
        out.diameter = typed("diameter");
        for (group, keys, slot) in [
            ("counterbore", ["diameter", "depth"], &mut out.counterbore),
            ("countersink", ["diameter", "angle"], &mut out.countersink),
        ] {
            let m: Map<String, Value> = keys
                .iter()
                .filter_map(|k| Some(((*k).to_owned(), typed(&format!("{group}.{k}"))?)))
                .collect();
            *slot = (!m.is_empty()).then_some(Value::Object(m));
        }
        out.bottom = typed("bottom.drill").map(|a| json!({ "drill": a }));
        if self.depth.dirty() || self.length.dirty() {
            out.depth = match self.depth.chosen.as_str() {
                "blind" => Some(json!({"blind": self.length.text.trim()})),
                "through_all" => Some(json!("through_all")),
                _ => None,
            };
        }
        if self.points.dirty() {
            out.points = Some(self.points.chosen.clone());
        }
    }
}

/// A pattern's spacings, counts and angle, its directions' reversals, its
/// skips (as JSON: `[2]`, or `[[1, 1]]` with two directions), its seeds,
/// and its form, directions, axis or plane as read; an instance pattern's
/// the same, its seeds instances and its directions connectors. Seeds can
/// be taken out, not added.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternForm {
    /// Whether it is an instance pattern: its seeds are `instances`.
    pub instances: bool,
    /// `linear`, `circular` or `mirror`.
    pub form: String,
    /// Each size by its key in the file, with its field.
    pub sizes: Vec<(String, Field)>,
    pub reverse: Vec<(String, Flag)>,
    pub skip: Field,
    /// Its direction, second direction, axis or plane, by key.
    pub refs: Vec<(String, String)>,
    pub seeds: Picks,
}

impl PatternForm {
    /// From a `feature.show` of a pattern.
    fn from_show(v: &Value) -> PatternForm {
        let sizes = ["spacing", "count", "spacing2", "count2", "angle"]
            .into_iter()
            .filter(|k| !v[*k].is_null())
            .map(|k| (k.to_owned(), Field::new(k, text(&v[k]))))
            .collect();
        let linear = v["form"] == "linear";
        let reverse = ["reverse", "reverse2"]
            .into_iter()
            .filter(|k| linear && (*k == "reverse" || !v["direction2"].is_null()))
            .map(|k| {
                let on = v[k] == true;
                let flag = Flag {
                    name: k.to_owned(),
                    read: on,
                    chosen: on,
                };
                (k.to_owned(), flag)
            })
            .collect();
        let skip = match &v["skip"] {
            Value::Null => "[]".to_owned(),
            x => x.to_string(),
        };
        // A reference as text; a connector as the text requests give it.
        let refs = ["direction", "direction2", "axis", "plane"]
            .into_iter()
            .filter_map(|k| {
                let t = v[k].as_str().or_else(|| v[k]["text"].as_str())?;
                Some((k.to_owned(), t.to_owned()))
            })
            .collect();
        let instances = v["type"] == "instance_pattern";
        let seeds = v[if instances { "instances" } else { "features" }]
            .as_array()
            .into_iter()
            .flatten()
            .map(text)
            .collect();
        PatternForm {
            instances,
            form: text(&v["form"]),
            sizes,
            reverse,
            skip: Field::new("skip", skip),
            refs,
            seeds: Picks::new("seeds", seeds),
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        self.sizes.iter().map(|(_, f)| f).chain([&self.skip])
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty)
            || self.reverse.iter().any(|(_, f)| f.dirty())
            || self.seeds.dirty()
    }

    fn revert(&mut self) {
        for (_, f) in &mut self.sizes {
            f.text.clone_from(&f.read);
        }
        for (_, f) in &mut self.reverse {
            f.chosen = f.read;
        }
        self.skip.text.clone_from(&self.skip.read);
        self.seeds.chosen.clone_from(&self.seeds.read);
    }

    /// What changed, as `feature.edit` takes it; skips that aren't JSON
    /// are refused before anything is sent.
    fn given(&self, out: &mut FeatureFields) -> Result<(), Error> {
        let typed = |key: &str| {
            self.sizes
                .iter()
                .find(|(k, f)| k == key && f.dirty())
                .map(|(_, f)| json!(f.text.trim()))
        };
        out.spacing = typed("spacing");
        out.count = typed("count");
        out.spacing2 = typed("spacing2");
        out.count2 = typed("count2");
        out.angle = typed("angle");
        for (k, f) in &self.reverse {
            if f.dirty() {
                match k.as_str() {
                    "reverse" => out.reverse = Some(f.chosen),
                    _ => out.reverse2 = Some(f.chosen),
                }
            }
        }
        if self.skip.dirty() {
            let t = self.skip.text.trim();
            let v: Value =
                serde_json::from_str(if t.is_empty() { "[]" } else { t }).map_err(|_| {
                    Error::new(
                        ryucad::error::Code::InvalidParam,
                        format!("skip is a list of instances, such as [2] or [[1, 1]], not {t:?}"),
                    )
                })?;
            out.skip = Some(v);
        }
        if self.seeds.dirty() {
            let seeds = Some(self.seeds.chosen.clone());
            if self.instances {
                out.instances = seeds;
            } else {
                out.features = seeds;
            }
        }
        Ok(())
    }
}

/// A connector's offset as typed: its origin's three quantities, its
/// turn about z and whether it is turned over.
#[derive(Clone, Debug, PartialEq)]
pub struct OffsetForm {
    pub origin: [Field; 3],
    pub rotate: Field,
    pub flip: Flag,
}

impl OffsetForm {
    /// From a connector as `feature.show` gives it; `side` is `a` or `b`.
    fn from_connector(side: &str, c: &Value) -> OffsetForm {
        let o = &c["offset"];
        OffsetForm {
            origin: [0, 1, 2].map(|i| {
                Field::new(
                    &format!("{side} offset {}", ["x", "y", "z"][i]),
                    text(&o["origin"][i]),
                )
            }),
            rotate: Field::new(&format!("{side} turn"), text(&o["rotate"])),
            flip: Flag {
                name: format!("{side} turned over"),
                read: o["flip"] == true,
                chosen: o["flip"] == true,
            },
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        self.origin.iter().chain([&self.rotate])
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty) || self.flip.dirty()
    }

    fn revert(&mut self) {
        for f in self.origin.iter_mut().chain([&mut self.rotate]) {
            f.text.clone_from(&f.read);
        }
        self.flip.chosen = self.flip.read;
    }

    /// The whole offset as typed, as `feature.edit` takes it: what is
    /// left empty is left out (`{}`: none).
    fn json(&self) -> Value {
        let mut v = json!({});
        if self.origin.iter().any(|f| !f.text.trim().is_empty()) {
            let q = |f: &Field| match f.text.trim() {
                "" => json!("0 mm"),
                t => json!(t),
            };
            v["origin"] = json!(self.origin.each_ref().map(q));
        }
        if !self.rotate.text.trim().is_empty() {
            v["rotate"] = json!(self.rotate.text.trim());
        }
        if self.flip.chosen {
            v["flip"] = json!(true);
        }
        v
    }
}

/// A mate's form, its driven angle and distance, each connector's offset,
/// and its connectors as read (shown, never changed here).
#[derive(Clone, Debug, PartialEq)]
pub struct MateForm {
    pub form: Choice,
    pub angle: Field,
    pub distance: Field,
    pub a: OffsetForm,
    pub b: OffsetForm,
    /// Each connector by side, as requests give it.
    pub connectors: Vec<(String, String)>,
}

impl MateForm {
    /// From a `feature.show` of a mate.
    fn from_show(v: &Value) -> MateForm {
        MateForm {
            form: Choice::new(
                "form",
                &[
                    "fastened",
                    "revolute",
                    "slider",
                    "cylindrical",
                    "planar",
                    "ball",
                ],
                text(&v["form"]),
            ),
            angle: Field::new("angle", text(&v["angle"])),
            distance: Field::new("distance", text(&v["distance"])),
            a: OffsetForm::from_connector("a", &v["a"]),
            b: OffsetForm::from_connector("b", &v["b"]),
            connectors: ["a", "b"]
                .into_iter()
                .map(|k| (k.to_owned(), text(&v[k]["text"])))
                .collect(),
        }
    }

    /// Whether the chosen form takes a driven angle, and a distance.
    pub fn takes(&self) -> (bool, bool) {
        let f = self.form.chosen.as_str();
        (
            matches!(f, "revolute" | "cylindrical" | "planar"),
            matches!(f, "slider" | "cylindrical" | "planar"),
        )
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        [&self.angle, &self.distance]
            .into_iter()
            .chain(self.a.fields())
            .chain(self.b.fields())
    }

    pub fn dirty(&self) -> bool {
        self.form.dirty()
            || self.angle.dirty()
            || self.distance.dirty()
            || self.a.dirty()
            || self.b.dirty()
    }

    fn revert(&mut self) {
        self.form.chosen.clone_from(&self.form.read);
        for f in [&mut self.angle, &mut self.distance] {
            f.text.clone_from(&f.read);
        }
        self.a.revert();
        self.b.revert();
    }

    /// What changed, as `feature.edit` takes it: an angle or distance
    /// emptied lets it go, an offset goes whole.
    fn given(&self, out: &mut FeatureFields) {
        if self.form.dirty() {
            out.form = Some(self.form.chosen.clone());
        }
        if self.angle.dirty() {
            out.angle = Some(json!(self.angle.text.trim()));
        }
        if self.distance.dirty() {
            out.distance = Some(json!(self.distance.text.trim()));
        }
        if self.a.dirty() {
            out.a_offset = Some(self.a.json());
        }
        if self.b.dirty() {
            out.b_offset = Some(self.b.json());
        }
    }
}

/// A value someone else changed while the form was being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub name: String,
    /// What it is now.
    pub theirs: String,
    /// What this window tried to save.
    pub yours: String,
}

/// A datum's, path's or helix's form (design 297): how it is built and
/// what from, shown as requests write it, never changed here (as a
/// mate's connectors are); its quantities, a point's coordinates, a
/// plane's or axis's flip, a path's reverse and a helix's hand, edited.
#[derive(Clone, Debug, PartialEq)]
pub struct DatumForm {
    /// `plane`, `axis`, `point`, `path` or `helix`.
    pub kind: String,
    /// A datum's construction (`offset`, `angle`, …); empty for a path or
    /// a helix.
    pub by: String,
    /// What it is built from, by field.
    pub refs: Vec<(String, String)>,
    /// Its quantities, by field.
    pub sizes: Vec<Field>,
    /// A point's coordinates, x, y and z.
    pub coordinates: Option<[Field; 3]>,
    /// A plane's or an axis's flip, a path's reverse.
    pub flag: Option<Flag>,
    /// A helix's hand.
    pub hand: Option<Choice>,
}

/// The quantities a datum, path or helix may have, in the order shown.
const DATUM_SIZES: [&str; 8] = [
    "distance", "angle", "fraction", "radius", "pitch", "turns", "height", "taper",
];

/// What a datum, path or helix may be built from, in the order shown.
const DATUM_REFS: [&str; 17] = [
    "from", "about", "through", "axis", "point", "points", "plane", "curve", "at", "between", "of",
    "in", "onto", "origin", "x", "start", "segments",
];

impl DatumForm {
    /// From a `feature.show` of a datum, a path or a helix.
    fn from_show(v: &Value) -> DatumForm {
        let kind = text(&v["type"]);
        let one = |r: &Value| match r {
            Value::Object(o) if o.contains_key("curve") => match o.get("propagate") {
                Some(Value::Bool(true)) => format!("{} (propagated)", text(&o["curve"])),
                _ => text(&o["curve"]),
            },
            r => text(r),
        };
        // A point's `at` is its coordinates; a plane normal to a curve's
        // is where along it.
        let coordinates = v["at"].as_array().map(|at| {
            [0, 1, 2].map(|i| {
                Field::new(
                    &format!("{} coordinate", ["x", "y", "z"][i]),
                    text(at.get(i).unwrap_or(&Value::Null)),
                )
            })
        });
        let refs = DATUM_REFS
            .into_iter()
            .filter(|k| !(*k == "at" && coordinates.is_some()))
            .filter_map(|k| {
                let shown = match &v[k] {
                    Value::Null => return None,
                    Value::Array(a) => a.iter().map(one).collect::<Vec<_>>().join(", "),
                    r => one(r),
                };
                Some((k.to_owned(), shown))
            })
            .collect();
        let sizes = DATUM_SIZES
            .into_iter()
            .filter(|k| !v[k].is_null())
            .map(|k| Field::new(k, text(&v[k])))
            .collect();
        let flag = match kind.as_str() {
            "plane" | "axis" => Some("flip"),
            "path" => Some("reverse"),
            _ => None,
        }
        .map(|name| Flag {
            name: name.to_owned(),
            read: v[name] == true,
            chosen: v[name] == true,
        });
        let hand = (kind == "helix").then(|| {
            let read = v["hand"].as_str().unwrap_or("right");
            Choice::new("hand", &["right", "left"], read.to_owned())
        });
        DatumForm {
            by: text(&v["by"]),
            kind,
            refs,
            sizes,
            coordinates,
            flag,
            hand,
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        self.sizes.iter().chain(self.coordinates.iter().flatten())
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty)
            || self.flag.as_ref().is_some_and(Flag::dirty)
            || self.hand.as_ref().is_some_and(Choice::dirty)
    }

    fn revert(&mut self) {
        for f in self
            .sizes
            .iter_mut()
            .chain(self.coordinates.iter_mut().flatten())
        {
            f.text.clone_from(&f.read);
        }
        if let Some(f) = &mut self.flag {
            f.chosen = f.read;
        }
        if let Some(h) = &mut self.hand {
            h.chosen.clone_from(&h.read);
        }
    }

    /// What changed, as `feature.edit` takes it: each quantity typed, a
    /// point's three coordinates together.
    fn given(&self, out: &mut FeatureFields) {
        for f in self.sizes.iter().filter(|f| f.dirty()) {
            let v = Some(json!(f.text.trim()));
            match f.name.as_str() {
                "distance" => out.distance = v,
                "angle" => out.angle = v,
                "fraction" => out.fraction = v,
                "radius" => out.radius = v,
                "pitch" => out.pitch = v,
                "turns" => out.turns = v,
                "height" => out.height = v,
                "taper" => out.taper = v,
                _ => {}
            }
        }
        if let Some(c) = self
            .coordinates
            .as_ref()
            .filter(|c| c.iter().any(Field::dirty))
        {
            out.coordinates = Some(c.iter().map(|f| json!(f.text.trim())).collect());
        }
        if let Some(f) = self.flag.as_ref().filter(|f| f.dirty()) {
            match self.kind.as_str() {
                "path" => out.reverse = Some(f.chosen),
                _ => out.flip = Some(f.chosen),
            }
        }
        if let Some(h) = self.hand.as_ref().filter(|h| h.dirty()) {
            out.hand = Some(h.chosen.clone());
        }
    }
}

/// A sweep's form (design 323): its profile, path and a binormal law's
/// direction shown as requests write them, never changed here (as a
/// mate's connectors are); its law, twist and scale edited. Its operation
/// and targets are the form's own, as an extrude's.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepForm {
    /// What it carries: `sketch s1 (loops p7, l3)` or `face p/base/<hex>`.
    pub profile: String,
    /// What it follows, as text.
    pub path: String,
    /// `minimal` or `binormal`.
    pub law: Choice,
    /// A binormal law's direction, as text.
    pub direction: Option<String>,
    pub twist: Field,
    pub scale: Field,
}

/// A section or profile as one line: its sketch or face, and the loops it
/// names (`sketch s1 (loops p7, l3)`).
fn section_text(v: &Value) -> String {
    let mut out = match (&v["sketch"], &v["face"]) {
        (Value::String(s), _) => format!("sketch {s}"),
        (_, f) if !f.is_null() => format!("face {}", text(f)),
        _ => String::new(),
    };
    if let Some(l) = v["loops"].as_array().filter(|l| !l.is_empty()) {
        let names: Vec<String> = l.iter().map(text).collect();
        out.push_str(&format!(" (loops {})", names.join(", ")));
    }
    out
}

impl SweepForm {
    /// From a `feature.show` of a sweep.
    fn from_show(v: &Value) -> SweepForm {
        let law = v["law"].as_str().unwrap_or("minimal").to_owned();
        SweepForm {
            profile: section_text(&v["profile"]),
            path: text(&v["path"]),
            law: Choice::new("law", &["minimal", "binormal"], law),
            direction: v["direction"].as_str().map(str::to_owned),
            twist: Field::new("twist", text(&v["twist"])),
            scale: Field::new("scale", text(&v["scale"])),
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        [&self.twist, &self.scale].into_iter()
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty) || self.law.dirty()
    }

    fn revert(&mut self) {
        for f in [&mut self.twist, &mut self.scale] {
            f.text.clone_from(&f.read);
        }
        self.law.chosen.clone_from(&self.law.read);
    }

    /// What changed, as `feature.edit` takes it: a twist or scale typed
    /// (emptied, `""`: none), and a law chosen.
    fn given(&self, out: &mut FeatureFields) {
        if self.twist.dirty() {
            out.twist = Some(json!(self.twist.text.trim()));
        }
        if self.scale.dirty() {
            out.scale = Some(json!(self.scale.text.trim()));
        }
        if self.law.dirty() {
            out.law = Some(self.law.chosen.clone());
        }
    }
}

/// A loft end's tangent: its direction shown, its weight edited.
#[derive(Clone, Debug, PartialEq)]
pub struct TangentForm {
    /// `normal`, or a reference as text.
    pub direction: String,
    pub weight: Field,
}

/// A loft's form (design 323): its sections, in order, and its tangents'
/// directions shown, never changed here; whether it closes and its
/// tangents' weights edited. Its operation and targets are the form's
/// own, as an extrude's.
#[derive(Clone, Debug, PartialEq)]
pub struct LoftForm {
    /// Each section as one line, in order.
    pub sections: Vec<String>,
    pub closed: Flag,
    pub start: Option<TangentForm>,
    pub end: Option<TangentForm>,
}

impl LoftForm {
    /// From a `feature.show` of a loft. A tangent written without a
    /// weight weighs 1.
    fn from_show(v: &Value) -> LoftForm {
        let tangent = |k: &str, name: &str| {
            let t = &v[k];
            t.is_object().then(|| {
                let w = match &t["weight"] {
                    Value::Null => "1".to_owned(),
                    w => text(w),
                };
                TangentForm {
                    direction: text(&t["direction"]),
                    weight: Field::new(name, w),
                }
            })
        };
        LoftForm {
            sections: v["sections"]
                .as_array()
                .into_iter()
                .flatten()
                .map(section_text)
                .collect(),
            closed: Flag {
                name: "closed".to_owned(),
                read: v["closed"] == true,
                chosen: v["closed"] == true,
            },
            start: tangent("start_tangent", "start weight"),
            end: tangent("end_tangent", "end weight"),
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        self.start.iter().chain(&self.end).map(|t| &t.weight)
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty) || self.closed.dirty()
    }

    fn revert(&mut self) {
        for t in self.start.iter_mut().chain(&mut self.end) {
            t.weight.text.clone_from(&t.weight.read);
        }
        self.closed.chosen = self.closed.read;
    }

    /// What changed, as `feature.edit` takes it: whether it closes, and
    /// each weight typed.
    fn given(&self, out: &mut FeatureFields) {
        if self.closed.dirty() {
            out.closed = Some(self.closed.chosen);
        }
        if let Some(t) = self.start.as_ref().filter(|t| t.weight.dirty()) {
            out.start_weight = Some(json!(t.weight.text.trim()));
        }
        if let Some(t) = self.end.as_ref().filter(|t| t.weight.dirty()) {
            out.end_weight = Some(json!(t.weight.text.trim()));
        }
    }
}

/// Strings of a list as `feature.show` gives them.
fn texts(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().map(text).collect()
}

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
    fn from_show(v: &Value) -> ShellForm {
        let direction = v["direction"].as_str().unwrap_or("inward").to_owned();
        ShellForm {
            target: text(&v["target"]),
            thickness: Field::new("thickness", text(&v["thickness"])),
            direction: Choice::new("direction", &["inward", "outward"], direction),
            open: Picks::new("open faces", texts(&v["open"])),
            groups: face_groups(v, "thickness"),
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        std::iter::once(&self.thickness).chain(self.groups.iter().map(|g| &g.quantity))
    }

    pub fn dirty(&self) -> bool {
        self.thickness.dirty()
            || self.direction.dirty()
            || self.open.dirty()
            || groups_dirty(&self.groups)
    }

    fn revert(&mut self) {
        self.thickness.text.clone_from(&self.thickness.read);
        self.direction.chosen.clone_from(&self.direction.read);
        self.open.chosen.clone_from(&self.open.read);
        revert_groups(&mut self.groups);
    }

    /// What changed, as `feature.edit` takes it: the thickness typed, the
    /// direction chosen, the open faces left (none: `[]`), the groups
    /// whole.
    fn given(&self, out: &mut FeatureFields) {
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
    fn from_show(v: &Value) -> BodyOffsetForm {
        BodyOffsetForm {
            target: text(&v["target"]),
            distance: Field::new("distance", text(&v["distance"])),
            groups: face_groups(v, "distance"),
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        std::iter::once(&self.distance).chain(self.groups.iter().map(|g| &g.quantity))
    }

    pub fn dirty(&self) -> bool {
        self.distance.dirty() || groups_dirty(&self.groups)
    }

    fn revert(&mut self) {
        self.distance.text.clone_from(&self.distance.read);
        revert_groups(&mut self.groups);
    }

    fn given(&self, out: &mut FeatureFields) {
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

/// A split's form (design 327): its target and plane shown; whether its
/// plane is flipped and which sides it keeps edited.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitForm {
    pub target: String,
    pub plane: String,
    pub flip: Flag,
    /// `both` or `behind`.
    pub keep: Choice,
}

impl SplitForm {
    fn from_show(v: &Value) -> SplitForm {
        let flip = v["flip"] == true;
        let keep = v["keep"].as_str().unwrap_or("both").to_owned();
        SplitForm {
            target: text(&v["target"]),
            plane: text(&v["plane"]),
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

    fn revert(&mut self) {
        self.flip.chosen = self.flip.read;
        self.keep.chosen.clone_from(&self.keep.read);
    }

    fn given(&self, out: &mut FeatureFields) {
        if self.flip.dirty() {
            out.flip = Some(self.flip.chosen);
        }
        if self.keep.dirty() {
            out.keep = Some(self.keep.chosen.clone());
        }
    }
}

/// The form for one feature.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub part: String,
    pub feature: String,
    pub ty: String,
    /// The part revision the values were read at.
    pub revision: String,
    /// The unit origins are shown in: the project's default.
    pub unit: Unit,
    pub params: Vec<Field>,
    /// The origin's three quantities, then the z and x axes' six numbers.
    pub origin: [Field; 3],
    pub z: [Field; 3],
    pub x: [Field; 3],
    /// A primitive's, extrude's or revolve's operation and the solids it
    /// acts on.
    pub operation: Option<Choice>,
    pub targets: Option<Picks>,
    /// An extrude's extent.
    pub extent: Option<ExtentForm>,
    /// A boolean's operation, target and tools.
    pub op: Option<Choice>,
    pub target: Option<Choice>,
    pub tools: Option<Picks>,
    /// A fillet's or chamfer's groups.
    pub blend: Option<BlendForm>,
    /// A hole's sizes, depth and points.
    pub hole: Option<HoleForm>,
    /// A pattern's or an instance pattern's spacings, counts, skips and
    /// seeds.
    pub pattern: Option<PatternForm>,
    /// A mate's form, driven values and offsets.
    pub mate: Option<MateForm>,
    /// A datum's, path's or helix's sizes, flip or reverse and hand.
    pub datum: Option<DatumForm>,
    /// A sweep's law, twist and scale.
    pub sweep: Option<SweepForm>,
    /// A loft's closing and tangents' weights.
    pub loft: Option<LoftForm>,
    /// A shell's thickness, direction, open faces and groups.
    pub shell: Option<ShellForm>,
    /// An offset's distance and groups.
    pub offset: Option<BodyOffsetForm>,
    /// A mirror's solid and plane, shown.
    pub mirror: Option<MirrorForm>,
    /// A split's flip and what it keeps.
    pub split: Option<SplitForm>,
    /// An instance's revision (`working` or a label) and whether it is
    /// fixed; its pose is the placement, its origin in [`Form::unit`].
    pub rev: Option<Choice>,
    pub fixed: Option<Flag>,
    /// For an instance of another project's part: that project's ID.
    /// Its revision offers only its official labels (never `working`).
    pub foreign: Option<String>,
    /// An import's file, hash, product and body: shown, never edited
    /// (`ryucad import` writes imports).
    pub import: Option<crate::core::model::ImportInfo>,
    /// What changed under a save that conflicted.
    pub changed: Vec<Changed>,
    /// Why the last save failed, other than a conflict.
    pub error: Option<Error>,
    /// A save is on its way.
    pub saving: bool,
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        // A quantity as reported: `{"value", "m"}`.
        Value::Object(o) => o.get("value").map_or_else(String::new, text),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `x` to `places` decimals, trailing zeros dropped, so a field never
/// hides an exponent (`6.123233995736766e-17` reads `0`; issue #108).
fn decimals(x: f64, places: usize) -> String {
    let s = format!("{x:.places$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    match s {
        "-0" | "" => "0".to_owned(),
        s => s.to_owned(),
    }
}

/// A pose's axis component as the form shows it: to 12 decimals (a
/// millionth of a microradian).
fn pose_number(v: &Value) -> String {
    match v.as_f64() {
        Some(x) => decimals(x, 12),
        None => text(v),
    }
}

/// Decimals that show a length in `unit` to a picometre.
fn places(unit: Unit) -> usize {
    match unit {
        Unit::M => 12,
        Unit::Cm => 10,
        Unit::Mm => 9,
        Unit::Um => 6,
        Unit::In | Unit::Ft => 11,
    }
}

fn triple(names: [&str; 3], v: &Value) -> [Field; 3] {
    [0, 1, 2].map(|i| Field::new(names[i], text(&v[i])))
}

impl Form {
    /// The form for a `feature.show` answer, origins in meters.
    pub fn from_show(part: &str, v: &Value) -> Form {
        Form::from_show_in(part, v, Unit::M)
    }

    /// The form for a `feature.show` answer, origins in `unit`.
    pub fn from_show_in(part: &str, v: &Value, unit: Unit) -> Form {
        let params = v["params"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, q)| Field::new(k, text(q)).valued(shown_value(q, unit)))
            .collect();
        let ty = text(&v["type"]);
        let instance = ty == "instance";
        let foreign = v["project"]
            .as_str()
            .filter(|_| instance)
            .map(str::to_owned);
        // An instance's pose is its placement, the origin in the
        // project's unit (issue #108).
        let pose = instance.then(|| {
            let p = &v["pose"];
            let o = |i: usize| match p["origin_m"][i].as_f64() {
                Some(m) => format!(
                    "{} {}",
                    decimals(m / unit.factor(), places(unit)),
                    unit.as_str()
                ),
                None => text(&p["origin_m"][i]),
            };
            let axis = |a: &Value| -> Vec<String> { (0..3).map(|i| pose_number(&a[i])).collect() };
            json!({"origin": [o(0), o(1), o(2)], "z": axis(&p["z"]), "x": axis(&p["x"])})
        });
        let p = pose.as_ref().unwrap_or(&v["placement"]);
        let strings =
            |v: &Value| -> Vec<String> { v.as_array().into_iter().flatten().map(text).collect() };
        let acts = matches!(
            ty.as_str(),
            "extrude" | "revolve" | "sweep" | "loft" | "mirror"
        ) || v["params"].is_object() && v["placement"].is_object();
        let operation = acts.then(|| {
            let read = v["operation"].as_str().unwrap_or("new").to_owned();
            Choice::new("operation", &["new", "join", "cut", "intersect"], read)
        });
        // A hole and a pattern act on solids too, always cutting or doing
        // what their seeds do.
        let repeats = matches!(ty.as_str(), "hole" | "pattern");
        let targets = (acts || repeats).then(|| Picks::new("targets", strings(&v["targets"])));
        let extent = (ty == "extrude").then(|| {
            let e = &v["extent"];
            ExtentForm {
                ty: Choice::new(
                    "extent",
                    &["blind", "symmetric", "up_to", "through_all"],
                    text(&e["type"]),
                ),
                length: Field::new("length", text(&e["length"])),
                both: Flag {
                    name: "both ways".to_owned(),
                    read: e["both"] == true,
                    chosen: e["both"] == true,
                },
            }
        });
        let boolean = ty == "boolean";
        let blend = matches!(ty.as_str(), "fillet" | "chamfer").then(|| BlendForm::from_show(v));
        let hole = (ty == "hole").then(|| HoleForm::from_show(v));
        let pattern = matches!(ty.as_str(), "pattern" | "instance_pattern")
            .then(|| PatternForm::from_show(v));
        let mate = (ty == "mate").then(|| MateForm::from_show(v));
        let datum = crate::core::datums::kind_of(&ty).map(|_| DatumForm::from_show(v));
        let sweep = (ty == "sweep").then(|| SweepForm::from_show(v));
        let loft = (ty == "loft").then(|| LoftForm::from_show(v));
        let shell = (ty == "shell").then(|| ShellForm::from_show(v));
        let offset = (ty == "offset").then(|| BodyOffsetForm::from_show(v));
        let mirror = (ty == "mirror").then(|| MirrorForm {
            solid: text(&v["solid"]),
            plane: text(&v["plane"]),
        });
        let split = (ty == "split").then(|| SplitForm::from_show(v));
        Form {
            part: part.to_owned(),
            feature: text(&v["id"]),
            revision: text(&v["revision"]),
            unit,
            params,
            origin: triple(["origin x", "origin y", "origin z"], &p["origin"]),
            z: triple(["z x", "z y", "z z"], &p["z"]),
            x: triple(["x x", "x y", "x z"], &p["x"]),
            operation,
            targets,
            extent,
            op: boolean
                .then(|| Choice::new("op", &["unite", "subtract", "intersect"], text(&v["op"]))),
            target: boolean.then(|| Choice::new("target", &[], text(&v["target"]))),
            tools: boolean.then(|| Picks::new("tools", strings(&v["tools"]))),
            blend,
            hole,
            pattern,
            mate,
            datum,
            sweep,
            loft,
            shell,
            offset,
            mirror,
            split,
            rev: instance.then(|| {
                let read = match v["rev"]["label"].as_str() {
                    Some(l) => l.to_owned(),
                    None => "working".to_owned(),
                };
                let first: &[&str] = if foreign.is_some() { &[] } else { &["working"] };
                let mut c = Choice::new("rev", first, read.clone());
                if !c.options.contains(&read) {
                    c.options.push(read);
                }
                c
            }),
            foreign,
            fixed: instance.then(|| Flag {
                name: "fixed".to_owned(),
                read: v["fixed"] == true,
                chosen: v["fixed"] == true,
            }),
            import: (ty == "import").then(|| {
                crate::core::model::ImportInfo::from_json(&json!({
                    "file": v["file"]["path"], "blake3": v["file"]["blake3"],
                    "product": v["product"], "body": v["body"]}))
            }),
            ty,
            changed: Vec::new(),
            error: None,
            saving: false,
        }
    }

    /// Offers `solids` (by founder) to pick targets, tools and a target
    /// from, besides those already chosen; never the feature itself.
    pub fn offer(&mut self, solids: &[String]) {
        let me = self.feature.clone();
        let add = |options: &mut Vec<String>| {
            for s in solids {
                if *s != me && !options.contains(s) {
                    options.push(s.clone());
                }
            }
        };
        for picks in [&mut self.targets, &mut self.tools].into_iter().flatten() {
            add(&mut picks.options);
        }
        if let Some(t) = &mut self.target {
            if !t.read.is_empty() && !t.options.contains(&t.read) {
                t.options.push(t.read.clone());
            }
            add(&mut t.options);
        }
    }

    /// Offers release `labels` of an instance's part, besides `working`
    /// and the one it reads.
    pub fn offer_revs(&mut self, labels: &[String]) {
        if let Some(r) = &mut self.rev {
            for l in labels {
                if !r.options.contains(l) {
                    r.options.push(l.clone());
                }
            }
        }
    }

    /// Whether the feature has a placement to edit.
    pub fn placed(&self) -> bool {
        self.origin.iter().any(|f| !f.read.is_empty())
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        self.params
            .iter()
            .chain(&self.origin)
            .chain(&self.z)
            .chain(&self.x)
            .chain(self.blend.iter().flat_map(BlendForm::sizes))
            .chain(self.hole.iter().flat_map(HoleForm::fields))
            .chain(self.pattern.iter().flat_map(PatternForm::fields))
            .chain(self.mate.iter().flat_map(MateForm::fields))
            .chain(self.datum.iter().flat_map(DatumForm::fields))
            .chain(self.sweep.iter().flat_map(SweepForm::fields))
            .chain(self.loft.iter().flat_map(LoftForm::fields))
            .chain(self.shell.iter().flat_map(ShellForm::fields))
            .chain(self.offset.iter().flat_map(BodyOffsetForm::fields))
    }

    /// Every field, to place errors and keep what was typed.
    fn fields_mut(&mut self) -> Vec<&mut Field> {
        let mut out: Vec<&mut Field> = Vec::new();
        out.extend(self.params.iter_mut());
        out.extend(self.origin.iter_mut());
        out.extend(self.z.iter_mut());
        out.extend(self.x.iter_mut());
        if let Some(e) = &mut self.extent {
            out.push(&mut e.length);
        }
        if let Some(b) = &mut self.blend {
            for g in &mut b.groups {
                out.extend(g.sizes.iter_mut().map(|(_, f)| f));
            }
        }
        if let Some(h) = &mut self.hole {
            out.extend(h.sizes.iter_mut().map(|(_, f)| f));
            out.push(&mut h.length);
        }
        if let Some(p) = &mut self.pattern {
            out.extend(p.sizes.iter_mut().map(|(_, f)| f));
            out.push(&mut p.skip);
        }
        if let Some(m) = &mut self.mate {
            out.push(&mut m.angle);
            out.push(&mut m.distance);
            for o in [&mut m.a, &mut m.b] {
                out.extend(o.origin.iter_mut());
                out.push(&mut o.rotate);
            }
        }
        if let Some(d) = &mut self.datum {
            out.extend(d.sizes.iter_mut());
            out.extend(d.coordinates.iter_mut().flatten());
        }
        if let Some(w) = &mut self.sweep {
            out.push(&mut w.twist);
            out.push(&mut w.scale);
        }
        if let Some(l) = &mut self.loft {
            out.extend(l.start.iter_mut().chain(&mut l.end).map(|t| &mut t.weight));
        }
        if let Some(s) = &mut self.shell {
            out.push(&mut s.thickness);
            out.extend(s.groups.iter_mut().map(|g| &mut g.quantity));
        }
        if let Some(o) = &mut self.offset {
            out.push(&mut o.distance);
            out.extend(o.groups.iter_mut().map(|g| &mut g.quantity));
        }
        out
    }

    /// A save refused for a reason other than a conflict: placed on the
    /// field its pointer names, else for the form's top.
    pub fn refuse(&mut self, e: Error) {
        for f in self.fields_mut() {
            f.error = None;
        }
        // A group's size or a mate's offset by its whole pointer, since
        // their last steps repeat (`/groups/1/radius`, `/b/offset/rotate`).
        if let Some(f) = e.details["pointer"]
            .as_str()
            .and_then(|p| self.at_pointer(p))
        {
            f.error = Some(e.message.clone());
            self.error = None;
            return;
        }
        let name = field_of(&e);
        let mut placed = false;
        for f in self.fields_mut() {
            if !placed && name.as_deref() == Some(f.name.as_str()) {
                f.error = Some(e.message.clone());
                placed = true;
            }
        }
        self.error = (!placed).then_some(e);
    }

    /// The field at a pointer into a blend's groups or a mate's offsets.
    fn at_pointer(&mut self, pointer: &str) -> Option<&mut Field> {
        let steps: Vec<&str> = pointer.strip_prefix('/')?.split('/').collect();
        match steps.as_slice() {
            ["groups", i, k] => {
                let i: usize = i.parse().ok()?;
                let g = self.blend.as_mut()?.groups.get_mut(i)?;
                g.sizes.iter_mut().find(|(key, _)| key == k).map(|(_, f)| f)
            }
            [side @ ("a" | "b"), "offset", rest @ ..] => {
                let m = self.mate.as_mut()?;
                let o = if *side == "a" { &mut m.a } else { &mut m.b };
                match rest {
                    ["origin", n] => o.origin.get_mut(n.parse::<usize>().ok()?),
                    ["rotate", ..] => Some(&mut o.rotate),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// After a conflict, what this window typed goes back over the values
    /// read again, so it can be saved over them on purpose.
    pub fn keep_mine(&mut self) {
        let changed = std::mem::take(&mut self.changed);
        for c in &changed {
            if let Some(f) = self.fields_mut().into_iter().find(|f| f.name == c.name) {
                f.text.clone_from(&c.yours);
                continue;
            }
            for ch in [
                &mut self.operation,
                &mut self.op,
                &mut self.target,
                &mut self.rev,
            ]
            .into_iter()
            .flatten()
            {
                if ch.name == c.name && ch.options.contains(&c.yours) {
                    ch.chosen.clone_from(&c.yours);
                }
            }
        }
    }

    /// Whether anything was typed or chosen.
    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty)
            || [&self.operation, &self.op, &self.target, &self.rev]
                .into_iter()
                .flatten()
                .any(Choice::dirty)
            || self.fixed.as_ref().is_some_and(Flag::dirty)
            || [&self.targets, &self.tools]
                .into_iter()
                .flatten()
                .any(Picks::dirty)
            || self.extent.as_ref().is_some_and(ExtentForm::dirty)
            || self.blend.as_ref().is_some_and(BlendForm::dirty)
            || self.hole.as_ref().is_some_and(HoleForm::dirty)
            || self.pattern.as_ref().is_some_and(PatternForm::dirty)
            || self.mate.as_ref().is_some_and(MateForm::dirty)
            || self.datum.as_ref().is_some_and(DatumForm::dirty)
            || self.sweep.as_ref().is_some_and(SweepForm::dirty)
            || self.loft.as_ref().is_some_and(LoftForm::dirty)
            || self.shell.as_ref().is_some_and(ShellForm::dirty)
            || self.offset.as_ref().is_some_and(BodyOffsetForm::dirty)
            || self.split.as_ref().is_some_and(SplitForm::dirty)
    }

    /// Everything typed or chosen goes back as it was read.
    pub fn revert(&mut self) {
        for f in self
            .params
            .iter_mut()
            .chain(&mut self.origin)
            .chain(&mut self.z)
            .chain(&mut self.x)
        {
            f.text.clone_from(&f.read);
        }
        for c in [
            &mut self.operation,
            &mut self.op,
            &mut self.target,
            &mut self.rev,
        ]
        .into_iter()
        .flatten()
        {
            c.chosen.clone_from(&c.read);
        }
        if let Some(f) = &mut self.fixed {
            f.chosen = f.read;
        }
        for p in [&mut self.targets, &mut self.tools].into_iter().flatten() {
            p.chosen.clone_from(&p.read);
        }
        if let Some(e) = &mut self.extent {
            e.ty.chosen.clone_from(&e.ty.read);
            e.length.text.clone_from(&e.length.read);
            e.both.chosen = e.both.read;
        }
        if let Some(b) = &mut self.blend {
            b.revert();
        }
        if let Some(h) = &mut self.hole {
            h.revert();
        }
        if let Some(p) = &mut self.pattern {
            p.revert();
        }
        if let Some(m) = &mut self.mate {
            m.revert();
        }
        if let Some(d) = &mut self.datum {
            d.revert();
        }
        if let Some(w) = &mut self.sweep {
            w.revert();
        }
        if let Some(l) = &mut self.loft {
            l.revert();
        }
        if let Some(s) = &mut self.shell {
            s.revert();
        }
        if let Some(o) = &mut self.offset {
            o.revert();
        }
        if let Some(s) = &mut self.split {
            s.revert();
        }
        // A refusal was about a value now put back (#532).
        for f in self.fields_mut() {
            f.error = None;
        }
        self.error = None;
    }

    /// The fields besides parameters and placement that changed, as
    /// `feature.edit` takes them.
    fn fields_given(&self) -> Result<FeatureFields, Error> {
        let chosen =
            |c: &Option<Choice>| c.as_ref().filter(|c| c.dirty()).map(|c| c.chosen.clone());
        let picked = |p: &Option<Picks>| p.as_ref().filter(|p| p.dirty()).map(|p| p.chosen.clone());
        // Back to `new`, a feature has no targets to give.
        let operation = chosen(&self.operation);
        let targets = match operation.as_deref() {
            Some("new") => None,
            _ => picked(&self.targets),
        };
        let extent = self.extent.as_ref().filter(|e| e.dirty()).map(|e| {
            let mut v = json!({"type": e.ty.chosen});
            match e.ty.chosen.as_str() {
                "blind" | "symmetric" => v["length"] = json!(e.length.text.trim()),
                "through_all" => v["both"] = json!(e.both.chosen),
                _ => {}
            }
            v
        });
        let blend = self.blend.as_ref();
        let mut out = FeatureFields {
            operation,
            targets,
            extent,
            op: chosen(&self.op),
            target: chosen(&self.target),
            tools: picked(&self.tools),
            groups: blend
                .filter(|b| b.groups_dirty())
                .map(BlendForm::groups_json),
            propagate: blend
                .filter(|b| b.propagate.dirty())
                .map(|b| b.propagate.chosen),
            rev: chosen(&self.rev),
            fixed: self.fixed.as_ref().filter(|f| f.dirty()).map(|f| f.chosen),
            ..FeatureFields::default()
        };
        if let Some(h) = &self.hole {
            h.given(&mut out);
        }
        if let Some(p) = &self.pattern {
            p.given(&mut out)?;
        }
        if let Some(m) = &self.mate {
            m.given(&mut out);
        }
        if let Some(d) = &self.datum {
            d.given(&mut out);
        }
        if let Some(w) = &self.sweep {
            w.given(&mut out);
        }
        if let Some(l) = &self.loft {
            l.given(&mut out);
        }
        if let Some(s) = &self.shell {
            s.given(&mut out);
        }
        if let Some(o) = &self.offset {
            o.given(&mut out);
        }
        if let Some(s) = &self.split {
            s.given(&mut out);
        }
        Ok(out)
    }

    /// The `feature.edit` that saves what was typed, guarded by the
    /// revision the form was read at.
    pub fn request(&self) -> Result<Request, Error> {
        let params: Map<String, Value> = self
            .params
            .iter()
            .filter(|f| f.dirty())
            .map(|f| (f.name.clone(), json!(f.text.trim())))
            .collect();
        let quantities = |fs: &[Field; 3]| fs.clone().map(|f| json!(f.text.trim()));
        let numbers = |fs: &[Field; 3]| -> Result<[f64; 3], Error> {
            let mut out = [0.0; 3];
            for (o, f) in out.iter_mut().zip(fs) {
                *o = f.text.trim().parse().map_err(|_| {
                    Error::new(
                        ryucad::error::Code::InvalidPlacement,
                        format!("{} must be a number, not {:?}", f.name, f.text),
                    )
                })?;
            }
            Ok(out)
        };
        let any = |fs: &[Field; 3]| fs.iter().any(Field::dirty);
        let placement = PlacementArg {
            origin: any(&self.origin).then(|| quantities(&self.origin)),
            z: if any(&self.z) {
                Some(numbers(&self.z)?)
            } else {
                None
            },
            x: if any(&self.x) {
                Some(numbers(&self.x)?)
            } else {
                None
            },
        };
        let moved = placement != PlacementArg::default();
        Ok(Request::FeatureEdit {
            part: self.part.clone(),
            id: self.feature.clone(),
            params,
            placement: moved.then_some(placement),
            fields: self.fields_given()?,
            expressions: Default::default(),
            expected_revision: Some(self.revision.clone()),
        })
    }

    /// The form read again after a conflict: the new values, with what
    /// changed under what was typed listed.
    pub fn reloaded(&self, v: &Value) -> Form {
        let mut fresh = Form::from_show_in(&self.part, v, self.unit);
        // What was offered stays offered.
        let offered: Vec<String> = [&self.targets, &self.tools]
            .into_iter()
            .flatten()
            .flat_map(|p| p.options.clone())
            .chain(self.target.iter().flat_map(|t| t.options.clone()))
            .collect();
        fresh.offer(&offered);
        fresh.changed = self
            .fields()
            .filter(|f| f.dirty())
            .filter_map(|mine| {
                let now = fresh.fields().find(|f| f.name == mine.name)?;
                (now.read != mine.read).then(|| Changed {
                    name: mine.name.clone(),
                    theirs: now.read.clone(),
                    yours: mine.text.trim().to_owned(),
                })
            })
            .collect();
        let choices = |f: &Form| -> Vec<(String, String, String)> {
            let edges = f
                .blend
                .iter()
                .flat_map(|b| b.groups.iter().map(|g| &g.edges))
                .chain(f.hole.iter().map(|h| &h.points))
                .chain(f.pattern.iter().map(|p| &p.seeds));
            let depth = f.hole.as_ref().map(|h| &h.depth);
            let form = f.mate.as_ref().map(|m| &m.form);
            let hand = f.datum.as_ref().and_then(|d| d.hand.as_ref());
            let law = f.sweep.as_ref().map(|w| &w.law);
            let flag = f.datum.as_ref().and_then(|d| d.flag.as_ref());
            let closed = f.loft.as_ref().map(|l| &l.closed);
            [&f.operation, &f.op, &f.target, &f.rev]
                .into_iter()
                .flatten()
                .chain(depth)
                .chain(form)
                .chain(hand)
                .chain(law)
                .map(|c| (c.name.clone(), c.read.clone(), c.chosen.clone()))
                .chain(
                    f.fixed
                        .iter()
                        .chain(flag)
                        .chain(closed)
                        .map(|x| (x.name.clone(), x.read.to_string(), x.chosen.to_string())),
                )
                .chain(
                    [&f.targets, &f.tools]
                        .into_iter()
                        .flatten()
                        .chain(edges)
                        .map(|p| (p.name.clone(), p.read.join(", "), p.chosen.join(", "))),
                )
                .collect()
        };
        let now = choices(&fresh);
        for (name, read, chosen) in choices(self) {
            if read == chosen {
                continue;
            }
            if let Some((_, theirs, _)) = now.iter().find(|(n, r, _)| *n == name && *r != read) {
                fresh.changed.push(Changed {
                    name,
                    theirs: theirs.clone(),
                    yours: chosen,
                });
            }
        }
        fresh
    }
}

/// The form for one of a sketch's dimensions: its value as written,
/// saved with `sketch.edit` against the revision it was read at.
#[derive(Clone, Debug, PartialEq)]
pub struct DimForm {
    pub part: String,
    pub sketch: String,
    pub constraint: String,
    /// `length`, `radius` and so on.
    pub ty: String,
    /// The part revision the value was read at.
    pub revision: String,
    pub value: Field,
    /// What changed under a save that conflicted.
    pub changed: Option<Changed>,
    /// Why the last save failed, other than a conflict.
    pub error: Option<Error>,
    /// A save is on its way.
    pub saving: bool,
}

impl DimForm {
    /// The form for `constraint` in a `sketch.get` answer, if it is there
    /// and has a value.
    pub fn from_get(part: &str, sketch: &str, constraint: &str, v: &Value) -> Option<DimForm> {
        let c = v["constraints"].get(constraint)?;
        let value = c["value"]["value"].as_str()?;
        Some(DimForm {
            part: part.to_owned(),
            sketch: sketch.to_owned(),
            constraint: constraint.to_owned(),
            ty: text(&c["type"]),
            revision: text(&v["revision"]),
            value: Field::new(constraint, value.to_owned()),
            changed: None,
            error: None,
            saving: false,
        })
    }

    pub fn dirty(&self) -> bool {
        self.value.dirty()
    }

    /// The `sketch.edit` that sets what was typed, guarded by the revision
    /// the form was read at.
    pub fn request(&self) -> Request {
        Request::SketchEdit {
            part: self.part.clone(),
            sketch: self.sketch.clone(),
            ops: vec![json!({"op": "set", "id": self.constraint,
                             "value": self.value.text.trim()})],
            expected_revision: Some(self.revision.clone()),
        }
    }

    /// The form read again after a conflict, saying what changed under
    /// what was typed; `None` if the dimension is gone.
    pub fn reloaded(&self, v: &Value) -> Option<DimForm> {
        let mut fresh = DimForm::from_get(&self.part, &self.sketch, &self.constraint, v)?;
        if self.dirty() && fresh.value.read != self.value.read {
            fresh.changed = Some(Changed {
                name: self.constraint.clone(),
                theirs: fresh.value.read.clone(),
                yours: self.value.text.trim().to_owned(),
            });
        }
        Some(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shells_thickness_direction_open_faces_and_groups_are_edited() {
        let (a, b, c) = (
            "p/b/".to_owned() + &"a".repeat(32),
            "p/b/".to_owned() + &"b".repeat(32),
            "p/b/".to_owned() + &"c".repeat(32),
        );
        let shown = json!({"id": "sh", "type": "shell", "revision": "r1", "target": "b",
                           "thickness": {"value": "2 mm", "m": 0.002}, "direction": "inward",
                           "open": [a],
                           "faces": [{"thickness": {"value": "4 mm", "m": 0.004}, "faces": [b, c]}]});
        let mut f = Form::from_show("p", &shown);
        let s = f.shell.as_ref().unwrap();
        assert_eq!(s.target, "b");
        assert_eq!(
            (s.thickness.read.as_str(), s.direction.read.as_str()),
            ("2 mm", "inward")
        );
        assert_eq!(s.open.read, std::slice::from_ref(&a));
        assert_eq!(s.groups[0].quantity.read, "4 mm");
        assert_eq!(s.groups[0].faces.read, [b.clone(), c.clone()]);
        // A shell names its target: no operation, no targets.
        assert!(f.operation.is_none() && f.targets.is_none());
        assert!(!f.dirty());
        let s = f.shell.as_mut().unwrap();
        s.thickness.text = "3 mm".into();
        s.direction.chosen = "outward".into();
        s.open.toggle(&a);
        s.groups[0].faces.toggle(&b);
        let g = given(&f);
        assert_eq!(g.thickness, Some(json!("3 mm")));
        assert_eq!(g.direction.as_deref(), Some("outward"));
        // Taking out the last open face writes none.
        assert_eq!(g.open, Some(Vec::<String>::new()));
        assert_eq!(g.faces, Some(json!([{"thickness": "4 mm", "faces": [c]}])));
        f.revert();
        assert!(!f.dirty());
        // A group left with no faces goes; only what changed is sent.
        let s = f.shell.as_mut().unwrap();
        s.groups[0].faces.chosen.clear();
        let g = given(&f);
        assert_eq!(g.faces, Some(json!([])));
        assert_eq!((g.thickness, g.direction, g.open), (None, None, None));
    }

    #[test]
    fn an_offsets_distance_and_groups_are_edited() {
        let a = "p/b/".to_owned() + &"a".repeat(32);
        let shown = json!({"id": "o", "type": "offset", "revision": "r1", "target": "b",
                           "distance": {"value": "-1 mm", "m": -0.001},
                           "faces": [{"distance": {"value": "2 mm", "m": 0.002}, "faces": [a]}]});
        let mut f = Form::from_show("p", &shown);
        let o = f.offset.as_ref().unwrap();
        assert_eq!(
            (o.target.as_str(), o.distance.read.as_str()),
            ("b", "-1 mm")
        );
        assert_eq!(o.groups[0].quantity.name, "distance (group 1)");
        let o = f.offset.as_mut().unwrap();
        o.groups[0].quantity.text = "wall".into();
        let g = given(&f);
        assert_eq!(g.distance, None);
        assert_eq!(g.faces, Some(json!([{"distance": "wall", "faces": [a]}])));
        // An offset with no groups and its distance edited.
        let shown = json!({"id": "o", "type": "offset", "revision": "r1", "target": "b",
                           "distance": {"value": "1 mm", "m": 0.001}});
        let mut f = Form::from_show("p", &shown);
        assert!(f.offset.as_ref().unwrap().groups.is_empty());
        f.offset.as_mut().unwrap().distance.text = "0.5 mm".into();
        assert_eq!(given(&f).distance, Some(json!("0.5 mm")));
    }

    #[test]
    fn a_mirrors_operation_is_edited_and_its_solid_and_plane_shown() {
        let shown = json!({"id": "m", "type": "mirror", "revision": "r1", "solid": "half",
                           "plane": "yz", "operation": "join", "targets": ["half"]});
        let mut f = Form::from_show("p", &shown);
        let m = f.mirror.as_ref().unwrap();
        assert_eq!((m.solid.as_str(), m.plane.as_str()), ("half", "yz"));
        assert_eq!(f.operation.as_ref().unwrap().read, "join");
        assert_eq!(f.targets.as_ref().unwrap().read, ["half"]);
        f.operation.as_mut().unwrap().chosen = "new".into();
        let g = given(&f);
        assert_eq!(g.operation.as_deref(), Some("new"));
        assert_eq!((g.plane, g.solid), (None, None));
    }

    #[test]
    fn a_splits_flip_and_keep_are_edited_and_its_plane_shown() {
        let shown = json!({"id": "sp", "type": "split", "revision": "r1", "target": "b",
                           "plane": "p/cut", "flip": false, "keep": "both"});
        let mut f = Form::from_show("p", &shown);
        let s = f.split.as_ref().unwrap();
        assert_eq!((s.target.as_str(), s.plane.as_str()), ("b", "p/cut"));
        assert_eq!((s.flip.read, s.keep.read.as_str()), (false, "both"));
        assert!(f.operation.is_none() && f.targets.is_none());
        let s = f.split.as_mut().unwrap();
        s.flip.chosen = true;
        s.keep.chosen = "behind".into();
        let g = given(&f);
        assert_eq!((g.flip, g.keep.as_deref()), (Some(true), Some("behind")));
        f.revert();
        let g = given(&f);
        assert_eq!((g.flip, g.keep), (None, None));
    }

    #[test]
    fn a_foreign_instance_offers_only_its_dependencys_labels() {
        let v = json!({"id": "b1", "type": "instance", "revision": "r1", "part": "m6-bolt",
                       "project": "aa", "rev": {"label": "B"},
                       "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}});
        let mut f = Form::from_show("frame", &v);
        assert_eq!(f.foreign.as_deref(), Some("aa"));
        assert_eq!(f.rev.as_ref().unwrap().options, ["B"]);
        f.offer_revs(&["C".into(), "B".into()]);
        assert_eq!(f.rev.as_ref().unwrap().options, ["B", "C"]);
        f.rev.as_mut().unwrap().chosen = "C".into();
        let Request::FeatureEdit { fields, .. } = f.request().unwrap() else {
            panic!()
        };
        assert_eq!(fields.rev.as_deref(), Some("C"));
        // A local instance still offers working.
        let mut local = v.clone();
        local.as_object_mut().unwrap().remove("project");
        let f = Form::from_show("frame", &local);
        assert_eq!(f.foreign, None);
        assert_eq!(f.rev.as_ref().unwrap().options, ["working", "B"]);
    }

    fn got(width: &str, revision: &str) -> Value {
        json!({"revision": revision, "constraints": {
            "d1": {"type": "length", "a": "l1", "value": {"value": width, "m": 0.02}},
            "c1": {"type": "horizontal", "a": "l1"}}})
    }

    #[test]
    fn a_dimension_saves_through_sketch_edit() {
        let mut f = DimForm::from_get("p", "s", "d1", &got("width", "r1")).unwrap();
        assert_eq!((f.ty.as_str(), f.value.read.as_str()), ("length", "width"));
        assert!(DimForm::from_get("p", "s", "c1", &got("width", "r1")).is_none());
        assert!(DimForm::from_get("p", "s", "d9", &got("width", "r1")).is_none());
        f.value.text = " 2 * width ".into();
        assert!(f.dirty());
        let Request::SketchEdit {
            ops,
            expected_revision,
            ..
        } = f.request()
        else {
            panic!()
        };
        assert_eq!(
            ops,
            [json!({"op": "set", "id": "d1", "value": "2 * width"})]
        );
        assert_eq!(expected_revision.as_deref(), Some("r1"));
        // Changed underneath: what it is now, and what was typed.
        let again = f.reloaded(&got("25 mm", "r2")).unwrap();
        assert_eq!(again.value.text, "25 mm");
        assert_eq!(
            again.changed,
            Some(Changed {
                name: "d1".into(),
                theirs: "25 mm".into(),
                yours: "2 * width".into()
            })
        );
    }

    #[test]
    fn an_import_is_shown_not_edited() {
        let f = Form::from_show(
            "p",
            &json!({"id": "body", "type": "import", "revision": "r1", "body": 1,
                    "file": {"path": "imports/bracket.step", "blake3": "ab12"},
                    "product": {"id": "BRK-100", "name": "Bracket", "ordinal": 1}}),
        );
        let i = f.import.as_ref().unwrap();
        assert_eq!(i.file, "imports/bracket.step");
        assert_eq!(i.blake3, "ab12");
        assert_eq!(i.product, Some(("BRK-100".into(), "Bracket".into(), 1)));
        assert_eq!(i.body, 1);
        assert!(!f.dirty());
        assert!(!f.placed());
        let none = Form::from_show(
            "p",
            &json!({"id": "body", "type": "import", "revision": "r1", "body": 0,
                    "file": {"path": "imports/loose.step", "blake3": "ab12"}}),
        );
        assert_eq!(none.import.unwrap().product, None);
    }

    fn shown(radius: &str, revision: &str) -> Value {
        json!({
            "id": "boss", "type": "cylinder", "revision": revision,
            "params": {"radius": {"value": radius, "m": 0.003}, "height": {"value": "8 mm", "m": 0.008}},
            "placement": {"origin": [{"value": "0 mm", "m": 0.0}, {"value": "0 mm", "m": 0.0}, {"value": "5 mm", "m": 0.005}],
                          "z": [0, 0, 1], "x": [1, 0, 0]},
        })
    }

    /// The fields a form's `feature.edit` gives besides parameters and
    /// placement.
    fn given(f: &Form) -> FeatureFields {
        let Request::FeatureEdit { fields, .. } = f.request().unwrap() else {
            panic!()
        };
        fields
    }

    #[test]
    fn what_a_feature_does_to_solids_is_edited() {
        // A cylinder that cuts the base, with the boss offered too.
        let mut v = shown("3 mm", "r1");
        v["operation"] = json!("cut");
        v["targets"] = json!(["base"]);
        let mut f = Form::from_show("p", &v);
        f.offer(&["base".into(), "boss".into(), "slot".into()]);
        let t = f.targets.as_mut().unwrap();
        assert_eq!(t.options, ["base", "slot"]);
        assert_eq!(f.operation.as_ref().unwrap().read, "cut");
        assert!(!f.dirty());
        f.targets.as_mut().unwrap().toggle("slot");
        f.operation.as_mut().unwrap().chosen = "join".into();
        let fields = given(&f);
        assert_eq!(fields.operation.as_deref(), Some("join"));
        assert_eq!(
            fields.targets,
            Some(vec!["base".to_owned(), "slot".to_owned()])
        );
        // Back to new: no targets.
        f.operation.as_mut().unwrap().chosen = "new".into();
        assert_eq!(given(&f).targets, None);
        f.revert();
        assert!(!f.dirty());
        // A sketch has none of it.
        let sketch = Form::from_show("p", &json!({"id": "s", "type": "sketch", "plane": "xy"}));
        assert!(sketch.operation.is_none() && sketch.extent.is_none() && !sketch.placed());
    }

    #[test]
    fn an_extent_and_a_boolean_are_edited() {
        let e = json!({"id": "h", "type": "extrude", "revision": "r1", "sketch": "s",
                       "operation": "cut", "targets": ["base"],
                       "extent": {"type": "blind", "length": {"value": "5 mm", "m": 0.005}}});
        let mut f = Form::from_show("p", &e);
        let x = f.extent.as_mut().unwrap();
        assert_eq!(
            (x.ty.read.as_str(), x.length.read.as_str()),
            ("blind", "5 mm")
        );
        x.length.text = "6 mm".into();
        assert_eq!(
            given(&f).extent,
            Some(json!({"type": "blind", "length": "6 mm"}))
        );
        let x = f.extent.as_mut().unwrap();
        x.ty.chosen = "through_all".into();
        x.both.chosen = true;
        assert_eq!(
            given(&f).extent,
            Some(json!({"type": "through_all", "both": true}))
        );
        let b = json!({"id": "u", "type": "boolean", "revision": "r1", "op": "unite",
                       "target": "base", "tools": ["boss"]});
        let mut f = Form::from_show("p", &b);
        f.offer(&["base".into(), "boss".into(), "lug".into(), "u".into()]);
        assert_eq!(f.target.as_ref().unwrap().options, ["base", "boss", "lug"]);
        assert!(f.operation.is_none());
        f.op.as_mut().unwrap().chosen = "subtract".into();
        f.tools.as_mut().unwrap().toggle("lug");
        let fields = given(&f);
        assert_eq!(fields.op.as_deref(), Some("subtract"));
        assert_eq!(
            fields.tools,
            Some(vec!["boss".to_owned(), "lug".to_owned()])
        );
        assert_eq!(fields.target, None);
    }

    #[test]
    fn a_fillets_sizes_and_edges_are_edited() {
        let v = json!({"id": "f", "type": "fillet", "revision": "r1", "groups": [
            {"radius": {"value": "2 mm", "m": 0.002}, "edges": ["p/b/aa", "p/b/bb"]}
        ]});
        let mut f = Form::from_show("p", &v);
        assert!(f.operation.is_none() && !f.placed());
        let b = f.blend.as_mut().unwrap();
        assert!(b.propagate.read);
        let (k, r) = &mut b.groups[0].sizes[0];
        assert_eq!(
            (k.as_str(), r.name.as_str(), r.read.as_str()),
            ("radius", "radius", "2 mm")
        );
        assert!(!f.dirty());
        // A size typed: every group goes, as typed, edges as they are.
        f.blend.as_mut().unwrap().groups[0].sizes[0].1.text = "3 mm".into();
        let fields = given(&f);
        assert_eq!(
            fields.groups,
            Some(json!([{"radius": "3 mm", "edges": ["p/b/aa", "p/b/bb"]}]))
        );
        assert_eq!(fields.propagate, None);
        // An edge taken out; propagation off.
        let b = f.blend.as_mut().unwrap();
        b.groups[0].edges.toggle("p/b/aa");
        b.propagate.chosen = false;
        let fields = given(&f);
        assert_eq!(fields.groups.unwrap()[0]["edges"], json!(["p/b/bb"]));
        assert_eq!(fields.propagate, Some(false));
        f.revert();
        assert!(!f.dirty());
        // Changed underneath: the size is listed with what was typed.
        f.blend.as_mut().unwrap().groups[0].sizes[0].1.text = "3 mm".into();
        let mut w = v.clone();
        w["groups"][0]["radius"]["value"] = json!("2.5 mm");
        w["revision"] = json!("r2");
        let again = f.reloaded(&w);
        assert_eq!(
            again.changed,
            [Changed {
                name: "radius".into(),
                theirs: "2.5 mm".into(),
                yours: "3 mm".into()
            }]
        );
    }

    #[test]
    fn a_chamfers_groups_keep_their_form_and_face() {
        let v = json!({"id": "c", "type": "chamfer", "revision": "r1", "propagate": false,
                       "groups": [
            {"form": "equal", "d": {"value": "1 mm", "m": 0.001}, "edges": ["p/b/aa"]},
            {"form": "distance_angle", "d": {"value": "0.5 mm", "m": 0.0005},
             "angle": {"value": "30 deg", "rad": 0.5}, "face": "p/b/ff", "edges": ["p/b/bb"]}
        ]});
        let mut f = Form::from_show("p", &v);
        let b = f.blend.as_mut().unwrap();
        assert!(!b.propagate.read);
        let names: Vec<&str> = b.groups[1]
            .sizes
            .iter()
            .map(|(_, f)| f.name.as_str())
            .collect();
        assert_eq!(names, ["d (group 2)", "angle (group 2)"]);
        b.groups[1].sizes[1].1.text = "40 deg".into();
        assert_eq!(
            given(&f).groups,
            Some(json!([
                {"form": "equal", "d": "1 mm", "edges": ["p/b/aa"]},
                {"form": "distance_angle", "d": "0.5 mm", "angle": "40 deg", "face": "p/b/ff",
                 "edges": ["p/b/bb"]}
            ]))
        );
    }

    #[test]
    fn a_holes_sizes_depth_and_points_are_edited() {
        let q = |v: &str, m: f64| json!({"value": v, "m": m});
        let v = json!({"id": "h", "type": "hole", "revision": "r1", "sketch": "s",
                       "form": "counterbore", "diameter": q("3 mm", 0.003),
                       "counterbore": {"diameter": q("6 mm", 0.006), "depth": q("2 mm", 0.002)},
                       "depth": {"blind": q("8 mm", 0.008)},
                       "bottom": {"drill": {"value": "118 deg", "rad": 2.0}},
                       "points": ["p1", "p2"], "targets": ["b"]});
        let mut f = Form::from_show("p", &v);
        assert!(f.operation.is_none() && f.targets.is_some());
        let h = f.hole.as_ref().unwrap();
        let names: Vec<&str> = h.sizes.iter().map(|(_, f)| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "diameter",
                "counterbore diameter",
                "counterbore depth",
                "drill point"
            ]
        );
        assert_eq!(
            (h.depth.read.as_str(), h.length.read.as_str()),
            ("blind", "8 mm")
        );
        assert!(!f.dirty());
        // A counterbore's depth, a drill angle and a point taken out: each
        // goes alone, the counterbore in part.
        let h = f.hole.as_mut().unwrap();
        h.sizes[2].1.text = "2.5 mm".into();
        h.sizes[3].1.text = "120 deg".into();
        h.points.toggle("p2");
        let fields = given(&f);
        assert_eq!(fields.counterbore, Some(json!({"depth": "2.5 mm"})));
        assert_eq!(fields.bottom, Some(json!({"drill": "120 deg"})));
        assert_eq!(fields.points, Some(vec!["p1".to_owned()]));
        assert_eq!((fields.diameter, fields.depth), (None, None));
        f.revert();
        // Through all, then blind again at a new length.
        let h = f.hole.as_mut().unwrap();
        h.depth.chosen = "through_all".into();
        assert_eq!(given(&f).depth, Some(json!("through_all")));
        let h = f.hole.as_mut().unwrap();
        h.depth.chosen = "blind".into();
        h.length.text = "9 mm".into();
        assert_eq!(given(&f).depth, Some(json!({"blind": "9 mm"})));
        // Up to a target is kept as it is.
        let mut w = v.clone();
        w["depth"] = json!({"up_to": {"target": "p/b/ff"}});
        let f = Form::from_show("p", &w);
        let h = f.hole.as_ref().unwrap();
        assert_eq!(
            (h.depth.read.as_str(), h.target.as_deref()),
            ("up_to", Some("p/b/ff"))
        );
        assert_eq!(given(&f).depth, None);
    }

    #[test]
    fn a_patterns_counts_skips_and_seeds_are_edited() {
        let v = json!({"id": "g", "type": "pattern", "revision": "r1", "form": "linear",
                       "features": ["h", "k"], "direction": "x",
                       "spacing": {"value": "8 mm", "m": 0.008}, "count": {"value": "3", "n": 3},
                       "direction2": "p/b/ee", "spacing2": {"value": "5 mm", "m": 0.005},
                       "count2": {"value": "n", "n": 2, "expr": true}, "reverse2": true,
                       "skip": [[2, 1]], "targets": ["b"]});
        let mut f = Form::from_show("p", &v);
        let p = f.pattern.as_ref().unwrap();
        let names: Vec<&str> = p.sizes.iter().map(|(_, f)| f.name.as_str()).collect();
        assert_eq!(names, ["spacing", "count", "spacing2", "count2"]);
        assert_eq!(p.skip.read, "[[2,1]]");
        assert_eq!(
            p.refs,
            [
                ("direction".to_owned(), "x".to_owned()),
                ("direction2".to_owned(), "p/b/ee".to_owned())
            ]
        );
        assert_eq!(
            p.reverse
                .iter()
                .map(|(k, f)| (k.as_str(), f.read))
                .collect::<Vec<_>>(),
            [("reverse", false), ("reverse2", true)]
        );
        // A count, the skips, a reversal and a seed taken out.
        let p = f.pattern.as_mut().unwrap();
        p.sizes[1].1.text = "4".into();
        p.skip.text = "[]".into();
        p.reverse[0].1.chosen = true;
        p.seeds.toggle("k");
        let fields = given(&f);
        assert_eq!(fields.count, Some(json!("4")));
        assert_eq!(fields.skip, Some(json!([])));
        assert_eq!((fields.reverse, fields.reverse2), (Some(true), None));
        assert_eq!(fields.features, Some(vec!["h".to_owned()]));
        assert_eq!(fields.spacing, None);
        // Skips that aren't a list are refused before anything is sent.
        f.pattern.as_mut().unwrap().skip.text = "two".into();
        assert!(f.request().is_err());
        // A circular pattern has an angle and no reversals.
        let w = json!({"id": "c", "type": "pattern", "revision": "r1", "form": "circular",
                       "features": ["h"], "axis": "z", "count": {"value": "6", "n": 6},
                       "angle": {"value": "360 deg", "rad": 6.25}, "targets": ["b"]});
        let f = Form::from_show("p", &w);
        let p = f.pattern.as_ref().unwrap();
        assert!(p.reverse.is_empty());
        assert_eq!(p.sizes[1].1.read, "360 deg");
        assert_eq!(p.skip.read, "[]");
    }

    #[test]
    fn a_save_sends_only_what_was_typed() {
        let mut f = Form::from_show("p", &shown("3 mm", "r1"));
        assert_eq!(f.params[1].read, "3 mm");
        assert_eq!(f.origin[2].read, "5 mm");
        assert_eq!(f.z[2].read, "1");
        assert!(!f.dirty());
        f.params[1].text = " 4 mm ".into();
        f.origin[0].text = "1 in".into();
        assert!(f.dirty());
        let Request::FeatureEdit {
            params,
            placement,
            expected_revision,
            ..
        } = f.request().unwrap()
        else {
            panic!()
        };
        assert_eq!(
            params,
            json!({"radius": "4 mm"}).as_object().unwrap().clone()
        );
        let p = placement.unwrap();
        assert_eq!(
            p.origin,
            Some([json!("1 in"), json!("0 mm"), json!("5 mm")])
        );
        assert_eq!((p.z, p.x), (None, None));
        assert_eq!(expected_revision.as_deref(), Some("r1"));
        f.z[0].text = "up".into();
        assert!(f.request().is_err());
    }

    #[test]
    fn a_conflict_lists_what_changed_underneath() {
        let mut f = Form::from_show("p", &shown("3 mm", "r1"));
        f.params[1].text = "4 mm".into();
        f.params[0].text = "9 mm".into();
        // Meanwhile an agent set the radius to 5 mm; the height is as it was.
        let again = f.reloaded(&shown("5 mm", "r2"));
        assert_eq!(again.revision, "r2");
        assert_eq!(again.params[1].text, "5 mm");
        assert_eq!(
            again.changed,
            [Changed {
                name: "radius".into(),
                theirs: "5 mm".into(),
                yours: "4 mm".into()
            }]
        );
    }

    #[test]
    fn fields_check_their_syntax_and_show_the_daemons_value() {
        let mut f = Field::new("radius", "r * 2".into()).valued(shown_value(
            &json!({"value": "r * 2", "m": 0.004, "expr": true}),
            Unit::Mm,
        ));
        assert_eq!(f.value.as_deref(), Some("4 mm"));
        // As read, and empty, nothing is judged.
        assert_eq!(f.syntax(), None);
        f.text = String::new();
        assert_eq!(f.syntax(), None);
        f.text = "2 * (wall".into();
        let fault = f.syntax().unwrap();
        assert!(fault.ends_with("at byte 9"), "{fault}");
        f.text = "2 * (3 mm".into();
        assert!(
            f.syntax().is_some_and(|e| e.contains("at byte")),
            "{:?}",
            f.syntax()
        );
        f.text = "2 * wall + 3 mm".into();
        assert_eq!(f.syntax(), None);
        // A literal shows no value beside it; an angle shows degrees.
        assert_eq!(
            shown_value(&json!({"value": "3 mm", "m": 0.003}), Unit::Mm),
            None
        );
        assert_eq!(
            shown_value(
                &json!({"value": "a / 2", "rad": core::f64::consts::FRAC_PI_4, "expr": true}),
                Unit::Mm
            )
            .as_deref(),
            Some("45 deg")
        );
    }

    #[test]
    fn refusals_land_on_their_field_and_mine_is_kept_on_request() {
        use ryucad::error::Code;
        let mut f = Form::from_show("p", &shown("3 mm", "r1"));
        f.params[1].text = "3 kg".into();
        f.refuse(
            Error::new(Code::UnitMismatch, "radius is a length").with("pointer", "/params/radius"),
        );
        assert_eq!(f.params[1].error.as_deref(), Some("radius is a length"));
        assert_eq!(f.error, None);
        // A refusal no field holds is the form's.
        f.refuse(Error::new(Code::Conflict, "something else"));
        assert!(f.params.iter().all(|x| x.error.is_none()));
        assert_eq!(f.error.as_ref().map(|e| e.code), Some(Code::Conflict));
        assert_eq!(
            field_of(&Error::new(Code::NotFound, "x").with("param", "height")).as_deref(),
            Some("height")
        );
        // After a conflict, Keep mine puts what was typed back over the
        // values read again, ready to apply over them.
        f.params[1].text = "4 mm".into();
        let mut again = f.reloaded(&shown("5 mm", "r2"));
        assert_eq!(again.params[1].text, "5 mm");
        again.keep_mine();
        assert_eq!(again.params[1].text, "4 mm");
        assert!(again.changed.is_empty() && again.dirty());
        assert_eq!(again.revision, "r2");
    }

    #[test]
    fn refusals_in_groups_and_offsets_land_by_their_whole_pointer() {
        use ryucad::error::Code;
        let refused = |p: &str| Error::new(Code::InvalidParam, "no").with("pointer", p);
        let v = json!({"id": "f", "type": "fillet", "revision": "r1", "groups": [
            {"radius": {"value": "2 mm", "m": 0.002}, "edges": ["p/b/aa"]},
            {"radius": {"value": "3 mm", "m": 0.003}, "edges": ["p/b/bb"]}
        ]});
        let mut f = Form::from_show("p", &v);
        f.refuse(refused("/groups/1/radius"));
        let b = f.blend.as_ref().unwrap();
        assert!(b.groups[0].sizes[0].1.error.is_none());
        assert!(b.groups[1].sizes[0].1.error.is_some());
        assert!(f.error.is_none());
        let n = "0123456789abcdef0123456789abcdef";
        let shown = json!({"id": "m1", "type": "mate", "revision": "r1", "form": "revolute",
                           "a": {"instance": ["base"], "at": {"feature": "slab", "name": n},
                                 "offset": {"origin": ["1 mm", "2 mm", "3 mm"]}},
                           "b": {"instance": ["pin"], "at": "origin",
                                 "offset": {"origin": ["1 mm", "2 mm", "3 mm"]}}});
        let mut f = Form::from_show("frame", &shown);
        f.refuse(refused("/b/offset/origin/1"));
        let m = f.mate.as_ref().unwrap();
        assert!(m.a.origin.iter().all(|x| x.error.is_none()));
        assert!(m.b.origin[1].error.is_some() && m.b.origin[0].error.is_none());
        f.refuse(refused("/a/offset/rotate"));
        let m = f.mate.as_ref().unwrap();
        assert!(m.a.rotate.error.is_some() && m.b.rotate.error.is_none());
        assert!(m.b.origin[1].error.is_none() && f.error.is_none());
    }

    /// Revert puts back the value a refusal was about, so the refusal goes
    /// with it, on a plain field and on a group's size alike (#532).
    #[test]
    fn revert_clears_the_refusals_on_its_fields() {
        use ryucad::error::Code;
        let refused = |p: &str| Error::new(Code::InvalidParam, "no").with("pointer", p);
        let v = json!({"id": "f", "type": "fillet", "revision": "r1", "groups": [
            {"radius": {"value": "2 mm", "m": 0.002}, "edges": ["p/b/aa"]}
        ]});
        let mut f = Form::from_show("p", &v);
        f.blend.as_mut().unwrap().groups[0].sizes[0].1.text = "-1 mm".into();
        f.refuse(refused("/groups/0/radius"));
        assert!(
            f.blend.as_ref().unwrap().groups[0].sizes[0]
                .1
                .error
                .is_some()
        );
        f.revert();
        assert!(!f.dirty());
        assert!(f.fields().all(|x| x.error.is_none()));
        assert!(f.error.is_none());
    }

    #[test]
    fn a_datums_sizes_and_flip_are_edited_and_its_references_shown() {
        let top = "p/b/0123456789abcdef0123456789abcdef";
        let shown = json!({"id": "d1", "type": "plane", "revision": "r1", "by": "angle",
                           "from": top, "about": "p/b/fedcba9876543210fedcba9876543210",
                           "angle": {"value": "15 deg", "rad": 0.26}, "flip": true});
        let mut f = Form::from_show("p", &shown);
        let d = f.datum.as_ref().unwrap();
        assert_eq!(d.by, "angle");
        assert_eq!(
            d.refs,
            [
                ("from".to_owned(), top.to_owned()),
                (
                    "about".to_owned(),
                    "p/b/fedcba9876543210fedcba9876543210".to_owned()
                )
            ]
        );
        assert_eq!(d.sizes.len(), 1);
        assert!(d.flag.as_ref().unwrap().read && d.hand.is_none());
        assert!(!f.dirty());
        let d = f.datum.as_mut().unwrap();
        d.sizes[0].text = "20 deg".into();
        d.flag.as_mut().unwrap().chosen = false;
        let g = given(&f);
        assert_eq!(g.angle, Some(json!("20 deg")));
        assert_eq!(g.flip, Some(false));
        assert_eq!((g.distance.clone(), g.from.clone()), (None, None));
        f.revert();
        assert!(!f.dirty());
        // A point's coordinates go together.
        let shown = json!({"id": "q1", "type": "point", "revision": "r1", "by": "coordinates",
                           "at": [{"value": "1 mm", "m": 0.001}, {"value": "2 mm", "m": 0.002},
                                  {"value": "0 mm", "m": 0.0}]});
        let mut f = Form::from_show("p", &shown);
        let d = f.datum.as_mut().unwrap();
        assert!(d.refs.is_empty() && d.flag.is_none());
        d.coordinates.as_mut().unwrap()[2].text = "5 mm".into();
        assert_eq!(
            given(&f).coordinates,
            Some(vec![json!("1 mm"), json!("2 mm"), json!("5 mm")])
        );
        // A plane normal to a curve: where along it is shown.
        let shown = json!({"id": "n", "type": "plane", "revision": "r1", "by": "normal_to",
                           "curve": "p/w", "at": "start"});
        let f = Form::from_show("p", &shown);
        let d = f.datum.as_ref().unwrap();
        assert_eq!(
            d.refs,
            [
                ("curve".to_owned(), "p/w".to_owned()),
                ("at".to_owned(), "start".to_owned())
            ]
        );
        assert!(d.coordinates.is_none());
    }

    #[test]
    fn a_sweeps_law_twist_scale_and_operation_are_edited_and_its_path_shown() {
        let shown = json!({"id": "w", "type": "sweep", "revision": "r1",
                           "profile": {"sketch": "s1", "loops": ["p7", "l3"]},
                           "path": "p/rim", "law": "binormal", "direction": "z",
                           "twist": {"value": "90 deg", "rad": 1.57},
                           "operation": "cut", "targets": ["base"]});
        let mut f = Form::from_show("p", &shown);
        let w = f.sweep.as_ref().unwrap();
        assert_eq!(w.profile, "sketch s1 (loops p7, l3)");
        assert_eq!(
            (w.path.as_str(), w.direction.as_deref()),
            ("p/rim", Some("z"))
        );
        assert_eq!(
            (w.law.read.as_str(), w.twist.read.as_str()),
            ("binormal", "90 deg")
        );
        // No scale written: an empty field, 1 by the kernel.
        assert_eq!(w.scale.read, "");
        assert_eq!(f.operation.as_ref().unwrap().read, "cut");
        assert_eq!(f.targets.as_ref().unwrap().read, ["base"]);
        assert!(!f.dirty());
        let w = f.sweep.as_mut().unwrap();
        w.twist.text = "180 deg".into();
        w.scale.text = "0.5".into();
        w.law.chosen = "minimal".into();
        f.operation.as_mut().unwrap().chosen = "join".into();
        let g = given(&f);
        assert_eq!(
            (g.twist.clone(), g.scale.clone(), g.law.clone()),
            (
                Some(json!("180 deg")),
                Some(json!("0.5")),
                Some("minimal".to_owned())
            )
        );
        assert_eq!(g.operation.as_deref(), Some("join"));
        assert_eq!((g.path.clone(), g.direction.clone()), (None, None));
        f.revert();
        assert!(!f.dirty());
        // Emptying the twist writes none.
        f.sweep.as_mut().unwrap().twist.text = String::new();
        assert_eq!(given(&f).twist, Some(json!("")));
        // A face profile.
        let shown = json!({"id": "w", "type": "sweep", "revision": "r1",
                           "profile": {"face": "p/b/0123456789abcdef0123456789abcdef"},
                           "path": "p/s/a1", "law": "minimal"});
        let f = Form::from_show("p", &shown);
        let w = f.sweep.as_ref().unwrap();
        assert_eq!(w.profile, "face p/b/0123456789abcdef0123456789abcdef");
        assert!(w.direction.is_none());
    }

    #[test]
    fn a_lofts_closing_and_weights_are_edited_and_its_sections_shown() {
        let shown = json!({"id": "l", "type": "loft", "revision": "r1",
                           "sections": [{"sketch": "s1"}, {"sketch": "s2", "loops": ["p7"]},
                                        {"face": "p/b/0123456789abcdef0123456789abcdef"}],
                           "start_tangent": {"direction": "normal",
                                             "weight": {"value": "1.5", "n": 1.5}},
                           "end_tangent": {"direction": "z"}});
        let mut f = Form::from_show("p", &shown);
        let l = f.loft.as_ref().unwrap();
        assert_eq!(
            l.sections,
            [
                "sketch s1",
                "sketch s2 (loops p7)",
                "face p/b/0123456789abcdef0123456789abcdef"
            ]
        );
        assert!(!l.closed.read);
        let (start, end) = (l.start.as_ref().unwrap(), l.end.as_ref().unwrap());
        assert_eq!(
            (start.direction.as_str(), start.weight.read.as_str()),
            ("normal", "1.5")
        );
        // A weight left out weighs 1.
        assert_eq!(
            (end.direction.as_str(), end.weight.read.as_str()),
            ("z", "1")
        );
        assert_eq!(f.operation.as_ref().unwrap().read, "new");
        let l = f.loft.as_mut().unwrap();
        l.start.as_mut().unwrap().weight.text = "2".into();
        let g = given(&f);
        assert_eq!(
            (g.start_weight.clone(), g.end_weight.clone()),
            (Some(json!("2")), None)
        );
        assert_eq!((g.closed, g.sections.clone()), (None, None));
        f.revert();
        // A closed loft has no tangents; opening it is a flag.
        let shown = json!({"id": "l", "type": "loft", "revision": "r1", "closed": true,
                           "sections": [{"sketch": "s1"}, {"sketch": "s2"}, {"sketch": "s3"}]});
        let mut f = Form::from_show("p", &shown);
        let l = f.loft.as_mut().unwrap();
        assert!(l.start.is_none() && l.end.is_none() && l.closed.read);
        l.closed.chosen = false;
        assert_eq!(given(&f).closed, Some(false));
    }

    #[test]
    fn a_path_reverses_and_a_helix_edits_its_sizes_and_hand() {
        let shown = json!({"id": "w", "type": "path", "revision": "r1",
                           "segments": ["p/s/l1", {"curve": "p/b/0123456789abcdef0123456789abcdef",
                                                   "propagate": true}]});
        let mut f = Form::from_show("p", &shown);
        let d = f.datum.as_mut().unwrap();
        assert_eq!(
            d.refs,
            [(
                "segments".to_owned(),
                "p/s/l1, p/b/0123456789abcdef0123456789abcdef (propagated)".to_owned()
            )]
        );
        d.flag.as_mut().unwrap().chosen = true;
        let g = given(&f);
        assert_eq!((g.reverse, g.flip), (Some(true), None));
        let shown = json!({"id": "h", "type": "helix", "revision": "r1", "axis": "z",
                           "radius": {"value": "8 mm", "m": 0.008},
                           "pitch": {"value": "2 mm", "m": 0.002},
                           "turns": {"value": "10", "n": 10.0}});
        let mut f = Form::from_show("p", &shown);
        let d = f.datum.as_mut().unwrap();
        let names: Vec<&str> = d.sizes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["radius", "pitch", "turns"]);
        assert_eq!(d.refs, [("axis".to_owned(), "z".to_owned())]);
        assert_eq!(d.hand.as_ref().unwrap().read, "right");
        d.sizes[2].text = "12".into();
        d.hand.as_mut().unwrap().chosen = "left".into();
        let g = given(&f);
        assert_eq!(g.turns, Some(json!("12")));
        assert_eq!(g.hand.as_deref(), Some("left"));
        assert_eq!((g.pitch.clone(), g.height.clone()), (None, None));
    }

    #[test]
    fn a_mates_drive_and_offsets_are_edited() {
        let n = "0123456789abcdef0123456789abcdef";
        let shown = json!({"id": "m1", "type": "mate", "revision": "r1", "form": "cylindrical",
                           "a": {"instance": ["base"], "at": {"feature": "slab", "name": n},
                                 "offset": {"origin": ["20 mm", "0 mm", "0 mm"]},
                                 "text": format!("frame/base/slab/{n}")},
                           "b": {"instance": ["pin"], "at": "origin", "text": "frame/pin"},
                           "distance": {"value": "3 mm", "m": 0.003}});
        let mut f = Form::from_show("frame", &shown);
        let m = f.mate.as_ref().unwrap();
        assert_eq!(m.takes(), (true, true));
        assert_eq!(
            (m.angle.read.as_str(), m.distance.read.as_str()),
            ("", "3 mm")
        );
        assert_eq!(m.a.origin[0].read, "20 mm");
        assert_eq!(m.connectors[1], ("b".to_owned(), "frame/pin".to_owned()));
        // Driven to an angle, the distance let go, b turned over.
        let m = f.mate.as_mut().unwrap();
        m.angle.text = "30 deg".into();
        m.distance.text = String::new();
        m.b.flip.chosen = true;
        let g = given(&f);
        assert_eq!(g.angle, Some(json!("30 deg")));
        assert_eq!(g.distance, Some(json!("")));
        assert_eq!(g.b_offset, Some(json!({"flip": true})));
        assert_eq!((g.a_offset.clone(), g.form.clone()), (None, None));
        // A's offset goes whole: a part left empty is zero; all emptied,
        // it is none.
        let m = f.mate.as_mut().unwrap();
        m.a.origin[1].text = String::new();
        m.a.origin[2].text = "2 mm".into();
        assert_eq!(
            given(&f).a_offset,
            Some(json!({"origin": ["20 mm", "0 mm", "2 mm"]}))
        );
        let m = f.mate.as_mut().unwrap();
        for o in &mut m.a.origin {
            o.text = String::new();
        }
        assert_eq!(given(&f).a_offset, Some(json!({})));
        f.revert();
        assert!(!f.dirty());
        let m = f.mate.as_mut().unwrap();
        m.form.chosen = "ball".into();
        assert_eq!(m.takes(), (false, false));
        assert_eq!(given(&f).form.as_deref(), Some("ball"));
        // An instance pattern's seeds are its instances.
        let shown = json!({"id": "ring", "type": "instance_pattern", "revision": "r1",
                           "form": "circular", "instances": ["pin", "nut"], "count": "3",
                           "angle": "360 deg", "axis": "z"});
        let mut f = Form::from_show("frame", &shown);
        let p = f.pattern.as_mut().unwrap();
        assert!(p.instances);
        assert_eq!(p.refs, [("axis".to_owned(), "z".to_owned())]);
        p.seeds.toggle("nut");
        let g = given(&f);
        assert_eq!(g.instances, Some(vec!["pin".to_owned()]));
        assert_eq!(g.features, None);
    }

    #[test]
    fn an_instance_edits_its_revision_pose_and_fixing() {
        let v = json!({"id": "b1", "type": "instance", "revision": "r1", "part": "bolt",
                       "rev": "working",
                       "pose": {"origin_m": [0.01, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}});
        let mut f = Form::from_show("frame", &v);
        assert!(f.placed());
        assert_eq!(f.origin[0].read, "0.01 m");
        assert_eq!(f.rev.as_ref().unwrap().options, ["working"]);
        f.offer_revs(&["A".into(), "B".into()]);
        assert_eq!(f.rev.as_ref().unwrap().options, ["working", "A", "B"]);
        assert!(!f.dirty());
        f.rev.as_mut().unwrap().chosen = "B".into();
        f.fixed.as_mut().unwrap().chosen = true;
        f.origin[1].text = "5 mm".into();
        assert!(f.dirty());
        let Request::FeatureEdit {
            fields, placement, ..
        } = f.request().unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (fields.rev.as_deref(), fields.fixed),
            (Some("B"), Some(true))
        );
        assert_eq!(placement.unwrap().origin.unwrap()[1], json!("5 mm"));
        f.revert();
        assert!(!f.dirty());
        // A label read keeps its option.
        let mut v = v;
        v["rev"] = json!({"label": "A"});
        v["fixed"] = json!(true);
        let f = Form::from_show("frame", &v);
        assert_eq!(f.rev.as_ref().unwrap().read, "A");
        assert!(f.fixed.as_ref().unwrap().read);
    }

    #[test]
    fn a_pose_reads_without_its_rounding() {
        let v = json!({"id": "b1", "type": "instance", "revision": "r1", "part": "bolt",
                       "rev": "working",
                       "pose": {"origin_m": [0.026000000000000002, -0.0125, 0],
                                "z": [0, 0, 1], "x": [6.123233995736766e-17, 1, -1e-13]}});
        let f = Form::from_show("frame", &v);
        let read = |fs: &[Field; 3]| fs.clone().map(|f| f.read);
        assert_eq!(read(&f.origin), ["0.026 m", "-0.0125 m", "0 m"]);
        assert_eq!(read(&f.z), ["0", "0", "1"]);
        assert_eq!(read(&f.x), ["0", "1", "0"]);
    }

    #[test]
    fn a_pose_reads_in_the_projects_unit() {
        use ryucad::units::Unit;
        // #108: a millimetre project shows its origins in millimetres.
        let v = json!({"id": "b1", "type": "instance", "revision": "r1", "part": "bolt",
                       "rev": "working",
                       "pose": {"origin_m": [0.05, 0.008000000000003199, -0.0125],
                                "z": [0, 0, 1], "x": [1, 0, 0]}});
        let f = Form::from_show_in("frame", &v, Unit::Mm);
        let read = |fs: &[Field; 3]| fs.clone().map(|f| f.read);
        assert_eq!(read(&f.origin), ["50 mm", "8 mm", "-12.5 mm"]);
        assert!(!f.dirty());
        // Read again after a conflict, it stays in millimetres.
        assert_eq!(read(&f.reloaded(&v).origin), ["50 mm", "8 mm", "-12.5 mm"]);
    }
}
