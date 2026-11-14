//! Editing a feature's parameters and placement, what it does to solids,
//! an extrude's extent, a boolean's solids, a fillet's or chamfer's sizes
//! and edges, a hole's sizes, depth and points, a pattern's spacings,
//! counts, skips and seeds, and an instance's revision, pose and whether
//! it is fixed: the property panel's form.
//! Values stay as written (quantities with their units). A save is an
//! ordinary `feature.edit` with the revision the form was read at, so a
//! human's stale edit fails with `conflict` like an agent's; the form then
//! reloads and lists what changed instead of overwriting it.

mod blend;
mod body;
mod datum;
mod extrude;
mod hole;
mod loft;
mod mate;
mod pattern;
mod sheet_metal;
mod sweep;
mod tweak;
pub use blend::{BlendForm, GroupForm};
pub use body::{BodyOffsetForm, FaceGroup, MirrorForm, ShellForm, SplitForm};
pub use datum::DatumForm;
pub use extrude::ExtentForm;
pub use hole::HoleForm;
pub use loft::{LoftForm, TangentForm};
pub use mate::{MateForm, OffsetForm};
pub use pattern::PatternForm;
pub use sheet_metal::{
    AllowanceForm, BaseFlangeForm, EndForm, FlangeForm, OverrideForm, RefoldForm, ReliefForm,
    SheetMetalForm, UnfoldForm,
};
pub use sweep::SweepForm;
mod surface;
pub use surface::{ExtractForm, FillForm, FillSide, SewForm, SideGroup, SurfaceForm, ThickenForm};
pub use tweak::{
    DeleteBlendForm, DeleteFaceForm, DraftForm, MoveFaceForm, ReplaceFaceForm, ResizeBlendForm,
    ResizeGroup, RetrimForm, TweakForm, TweakGroup, WithGroup,
};

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

/// One feature kind's part of the property panel's form (design 541,
/// "Forms per kind"): read from its `feature.show`, its fields, and what
/// it gives `feature.edit`. A `Form` holds at most one (its `type`'s) and
/// asks it.
pub trait FeatureForm {
    /// This kind's form from a `feature.show` of type `ty`; `None` for a
    /// type it doesn't own.
    fn read(ty: &str, v: &Value) -> Option<Self>
    where
        Self: Sized;
    /// Its fields that `Form`'s checks list (what was typed, its syntax).
    fn kind_fields(&self) -> Vec<&Field> {
        Vec::new()
    }
    /// Its fields that a refusal lands on and "keep mine" restores.
    fn kind_fields_mut(&mut self) -> Vec<&mut Field> {
        Vec::new()
    }
    /// Whether anything of it was typed or chosen.
    fn kind_dirty(&self) -> bool {
        false
    }
    /// Everything of it back as read.
    fn kind_revert(&mut self) {}
    /// What of it changed, written into `feature.edit`'s fields.
    fn kind_given(&self, _out: &mut FeatureFields) -> Result<(), Error> {
        Ok(())
    }
    /// The field a refusal's pointer (as steps: `["groups", "1",
    /// "radius"]`) names in it, when the pointer is this kind's.
    fn kind_at_pointer(&mut self, _steps: &[&str]) -> Option<&mut Field> {
        None
    }
}

/// The pointers only a blend, shell, offset or mate answers: before 27.F
/// `Form::at_pointer` tried these first whatever the form, so no other
/// kind is asked for them.
fn claimed(steps: &[&str]) -> bool {
    matches!(
        steps,
        ["groups", _, _] | ["faces", _, "thickness" | "distance"] | ["a" | "b", "offset", ..]
    )
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

/// A value someone else changed while the form was being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub name: String,
    /// What it is now.
    pub theirs: String,
    /// What this window tried to save.
    pub yours: String,
}

/// Strings of a list as `feature.show` gives them.
fn texts(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().map(text).collect()
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
    /// A delete_body's solids, each to take out (adding one is an agent's
    /// work).
    pub solids: Option<Picks>,
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
    /// Whether an extrude, revolve, sweep or loft makes a sheet (ryucad
    /// 18's `surface`); its operation is then `new`.
    pub surface: Option<Flag>,
    /// A fill's, extract's, sew's or thicken's sides, faces, sheets and
    /// sizes (ryucad 18).
    pub surface_form: Option<SurfaceForm>,
    /// A base flange's, flange's, unfold's or refold's settings, sizes
    /// and what it names.
    pub sheet: Option<SheetMetalForm>,
    /// A draft's, move face's or replace face's sizes, flags and faces; a delete's, blend delete's or resize's (ryucad 22).
    pub tweak: Option<TweakForm>,
    /// An instance's revision (`working` or a label) and whether it is
    /// fixed; its pose is the placement, its origin in [`Form::unit`].
    pub rev: Option<Choice>,
    pub fixed: Option<Flag>,
    /// For an instance of another project's part: that project's ID.
    /// Its revision offers only its official labels (never `working`).
    pub foreign: Option<String>,
    /// An insert's source part (design 521): shown, never edited here.
    pub of: Option<String>,
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
        let insert = ty == "insert";
        let foreign = v["project"]
            .as_str()
            .filter(|_| instance || insert)
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
        // An insert at the identity writes no placement: read it as one,
        // so it can be moved (design 521).
        let zero = format!("0 {}", unit.as_str());
        let identity = json!({
            "origin": [zero.clone(), zero.clone(), zero],
            "z": ["0", "0", "1"],
            "x": ["1", "0", "0"],
        });
        let p = match pose.as_ref() {
            Some(p) => p,
            None if insert && !v["placement"].is_object() => &identity,
            None => &v["placement"],
        };
        let strings =
            |v: &Value| -> Vec<String> { v.as_array().into_iter().flatten().map(text).collect() };
        let acts = matches!(
            ty.as_str(),
            "extrude" | "revolve" | "sweep" | "loft" | "mirror" | "thicken"
        ) || v["params"].is_object() && v["placement"].is_object();
        let operation = acts.then(|| {
            let read = v["operation"].as_str().unwrap_or("new").to_owned();
            Choice::new("operation", &["new", "join", "cut", "intersect"], read)
        });
        // A hole and a pattern act on solids too, always cutting or doing
        // what their seeds do.
        let repeats = matches!(ty.as_str(), "hole" | "pattern");
        let targets = (acts || repeats).then(|| Picks::new("targets", strings(&v["targets"])));
        let extent = ExtentForm::read(&ty, v);
        let boolean = ty == "boolean";
        let blend = BlendForm::read(&ty, v);
        let hole = HoleForm::read(&ty, v);
        let pattern = PatternForm::read(&ty, v);
        let mate = MateForm::read(&ty, v);
        let datum = DatumForm::read(&ty, v);
        let sweep = SweepForm::read(&ty, v);
        let loft = LoftForm::read(&ty, v);
        let shell = ShellForm::read(&ty, v);
        let offset = BodyOffsetForm::read(&ty, v);
        let mirror = MirrorForm::read(&ty, v);
        let split = SplitForm::read(&ty, v);
        let sheet = SheetMetalForm::read(&ty, v);
        let surface = matches!(ty.as_str(), "extrude" | "revolve" | "sweep" | "loft").then(|| {
            let on = v["surface"] == true;
            Flag {
                name: "surface".to_owned(),
                read: on,
                chosen: on,
            }
        });
        let surface_form = SurfaceForm::read(&ty, v);
        let tweak = TweakForm::read(&ty, v);
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
            solids: (ty == "delete_body").then(|| Picks::new("solids", strings(&v["solids"]))),
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
            sheet,
            surface,
            surface_form,
            tweak,
            rev: (instance || insert).then(|| {
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
            of: insert.then(|| text(&v["part"])),
            fixed: instance.then(|| Flag {
                name: "fixed".to_owned(),
                read: v["fixed"] == true,
                chosen: v["fixed"] == true,
            }),
            import: (ty == "import").then(|| {
                crate::core::model::ImportInfo::from_json(&json!({
                    "file": v["file"]["path"], "blake3": v["file"]["blake3"],
                    "product": v["product"], "body": v["body"], "mode": v["mode"]}))
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

    /// The one kind's form this feature has, if any (a `type` reads at
    /// most one: `every_type_reads_at_most_one_kinds_form`).
    fn kind(&self) -> Option<&dyn FeatureForm> {
        self.kinds().into_iter().next()
    }

    /// Every kind's form this feature has: never more than one.
    fn kinds(&self) -> Vec<&dyn FeatureForm> {
        fn k<T: FeatureForm>(f: &Option<T>) -> Option<&dyn FeatureForm> {
            f.as_ref().map(|f| f as &dyn FeatureForm)
        }
        [
            k(&self.extent),
            k(&self.blend),
            k(&self.hole),
            k(&self.pattern),
            k(&self.mate),
            k(&self.datum),
            k(&self.sweep),
            k(&self.loft),
            k(&self.shell),
            k(&self.offset),
            k(&self.mirror),
            k(&self.split),
            k(&self.sheet),
            k(&self.surface_form),
            k(&self.tweak),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// The parameters' and placement's fields, and the kind's form,
    /// borrowed apart so both can be written.
    fn parts_mut(&mut self) -> (Vec<&mut Field>, Option<&mut dyn FeatureForm>) {
        fn k<T: FeatureForm>(f: &mut Option<T>) -> Option<&mut dyn FeatureForm> {
            f.as_mut().map(|f| f as &mut dyn FeatureForm)
        }
        let Form {
            params,
            origin,
            z,
            x,
            extent,
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
            sheet,
            surface_form,
            tweak,
            ..
        } = self;
        let mut base: Vec<&mut Field> = Vec::new();
        base.extend(params.iter_mut());
        base.extend(origin.iter_mut());
        base.extend(z.iter_mut());
        base.extend(x.iter_mut());
        let kind = [
            k(extent),
            k(blend),
            k(hole),
            k(pattern),
            k(mate),
            k(datum),
            k(sweep),
            k(loft),
            k(shell),
            k(offset),
            k(mirror),
            k(split),
            k(sheet),
            k(surface_form),
            k(tweak),
        ]
        .into_iter()
        .flatten()
        .next();
        (base, kind)
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        self.params
            .iter()
            .chain(&self.origin)
            .chain(&self.z)
            .chain(&self.x)
            .chain(self.kind().into_iter().flat_map(|k| k.kind_fields()))
    }

    /// Every field, to place errors and keep what was typed.
    fn fields_mut(&mut self) -> Vec<&mut Field> {
        let (mut out, kind) = self.parts_mut();
        if let Some(k) = kind {
            out.extend(k.kind_fields_mut());
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

    /// The field at a pointer into the kind's form: a blend's groups, a
    /// shell's or offset's faces, a mate's offsets, a tweak's, sheet
    /// metal's or a surface's own.
    fn at_pointer(&mut self, pointer: &str) -> Option<&mut Field> {
        let steps: Vec<&str> = pointer.strip_prefix('/')?.split('/').collect();
        self.parts_mut().1?.kind_at_pointer(&steps)
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
            || [&self.targets, &self.tools, &self.solids]
                .into_iter()
                .flatten()
                .any(Picks::dirty)
            || self.kind().is_some_and(|k| k.kind_dirty())
            || self.surface.as_ref().is_some_and(Flag::dirty)
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
        for p in [&mut self.targets, &mut self.tools, &mut self.solids]
            .into_iter()
            .flatten()
        {
            p.chosen.clone_from(&p.read);
        }
        if let Some(k) = self.parts_mut().1 {
            k.kind_revert();
        }
        if let Some(f) = &mut self.surface {
            f.chosen = f.read;
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
        // Made a surface, a feature founds a sheet: its operation is new.
        let surfaced = self
            .surface
            .as_ref()
            .filter(|f| f.dirty())
            .map(|f| f.chosen);
        let mut operation = chosen(&self.operation);
        if surfaced == Some(true) && self.operation.as_ref().is_some_and(|o| o.chosen != "new") {
            operation = Some("new".to_owned());
        }
        // Back to `new`, a feature has no targets to give.
        let targets = match operation.as_deref() {
            Some("new") => None,
            _ => picked(&self.targets),
        };
        let mut out = FeatureFields {
            operation,
            targets,
            op: chosen(&self.op),
            target: chosen(&self.target),
            tools: picked(&self.tools),
            solids: picked(&self.solids),
            rev: chosen(&self.rev),
            fixed: self.fixed.as_ref().filter(|f| f.dirty()).map(|f| f.chosen),
            surface: surfaced,
            ..FeatureFields::default()
        };
        if let Some(k) = self.kind() {
            k.kind_given(&mut out)?;
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
            dry_run: false,
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

    /// Design 541's forms per kind: a `Form` asks its one kind's form, so no
    /// type may read two.
    #[test]
    fn every_type_reads_at_most_one_kinds_form() {
        let types = [
            "box",
            "cylinder",
            "cone",
            "sphere",
            "torus",
            "sketch",
            "extrude",
            "revolve",
            "boolean",
            "fillet",
            "chamfer",
            "hole",
            "pattern",
            "instance_pattern",
            "import",
            "instance",
            "mate",
            "plane",
            "axis",
            "point",
            "path",
            "helix",
            "sweep",
            "loft",
            "shell",
            "offset",
            "mirror",
            "split",
            "base_flange",
            "flange",
            "unfold",
            "refold",
            "planar",
            "fill",
            "extract",
            "sew",
            "thicken",
            "draft",
            "move_face",
            "replace_face",
            "delete_face",
            "delete_blend",
            "resize_blend",
            "insert",
            "delete_body",
        ];
        for ty in types {
            let f = Form::from_show("p", &json!({"id": "f", "type": ty, "revision": "1"}));
            assert!(
                f.kinds().len() <= 1,
                "{ty} reads {} kinds' forms",
                f.kinds().len()
            );
        }
    }

    #[test]
    fn a_deletes_solids_are_taken_out_one_by_one() {
        let shown = json!({"id": "d", "type": "delete_body", "revision": "r1",
                           "solids": ["brd/boss", "lid"]});
        let mut f = Form::from_show_in("e", &shown, Unit::Mm);
        let s = f.solids.as_mut().unwrap();
        assert_eq!(s.read, ["brd/boss", "lid"]);
        s.toggle("lid");
        assert!(f.dirty());
        let Request::FeatureEdit { fields, .. } = f.request().unwrap() else {
            panic!("an edit")
        };
        assert_eq!(fields.solids, Some(vec!["brd/boss".to_owned()]));
        f.revert();
        assert!(!f.dirty());
    }

    #[test]
    fn an_inserts_form_edits_its_pin_and_its_placement_from_the_identity() {
        let shown = json!({"id": "brd", "type": "insert", "revision": "r1",
                           "part": "pcb", "rev": {"label": "A"}});
        let mut f = Form::from_show_in("e", &shown, Unit::Mm);
        assert_eq!(f.of.as_deref(), Some("pcb"));
        let rev = f.rev.as_ref().unwrap();
        assert_eq!(
            (rev.read.as_str(), rev.options.clone()),
            ("A", vec!["working".to_owned(), "A".to_owned()])
        );
        f.offer_revs(&["B".to_owned(), "A".to_owned()]);
        assert_eq!(f.rev.as_ref().unwrap().options, ["working", "A", "B"]);
        // At the identity its placement still reads, so it can be moved.
        assert!(f.placed());
        assert_eq!(f.origin[2].read, "0 mm");
        f.origin[2].text = "20 mm".into();
        f.rev.as_mut().unwrap().chosen = "B".into();
        let Request::FeatureEdit {
            placement, fields, ..
        } = f.request().unwrap()
        else {
            panic!("an edit")
        };
        let p = placement.unwrap();
        assert_eq!(p.origin.unwrap()[2], json!("20 mm"));
        assert!(p.z.is_none() && p.x.is_none());
        assert_eq!(fields.rev.as_deref(), Some("B"));
        // Another project's part offers only its labels.
        let foreign = json!({"id": "brd", "type": "insert", "revision": "r1",
                             "part": "pcb", "project": "aa", "rev": {"label": "A"}});
        let f = Form::from_show_in("e", &foreign, Unit::Mm);
        assert_eq!(f.foreign.as_deref(), Some("aa"));
        assert_eq!(f.rev.unwrap().options, ["A"]);
    }

    #[test]
    fn a_holes_derived_sizes_show_read_only() {
        let show = json!({"id": "h", "type": "hole", "form": "simple",
            "spec": "M6x1.0-6H THRU", "thread": {"standard": "iso_metric", "size": "M6"},
            "sizes": {"drill": {"value": "5 mm", "m": 0.005, "source": "rule"},
                      "thread": {"designation": "M6x1.0-6H",
                                 "pitch": {"value": "1 mm", "m": 0.001, "source": "iso_261"},
                                 "limits": {"minor": [{"value": "4.917 mm", "source": "formula"},
                                                      {"value": "5.153 mm", "source": "formula"}]}}},
            "depth": "through_all", "points": ["p1"]});
        let f = HoleForm::from_show(&show);
        assert_eq!(f.spec.as_deref(), Some("M6x1.0-6H THRU"));
        assert_eq!(f.derived[0], ("drill".into(), "5 mm".into(), "rule".into()));
        assert!(
            f.derived
                .iter()
                .any(|(l, v, _)| l == "pitch" && v == "1 mm")
        );
        assert!(
            f.derived
                .iter()
                .any(|(l, v, s)| l == "minor" && v == "4.917 mm – 5.153 mm" && s == "formula")
        );
        assert!(
            f.sizes.iter().all(|(k, _)| k != "diameter"),
            "a derived drill isn't an edit"
        );
        // A hole without intent edits its diameter as before.
        let plain = json!({"id": "c", "type": "hole", "form": "simple", "diameter": "6 mm",
                           "spec": "DIA 6 THRU",
                           "sizes": {"drill": {"value": "6 mm", "m": 0.006, "source": "override"}},
                           "depth": "through_all", "points": ["p1"]});
        let f = HoleForm::from_show(&plain);
        assert_eq!(f.sizes[0].0, "diameter");
        assert_eq!(f.derived[0].2, "override");
    }

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
        assert_eq!(
            (g.flip, g.keep.as_deref()),
            (Some(true.into()), Some("behind"))
        );
        f.revert();
        let g = given(&f);
        assert_eq!((g.flip, g.keep), (None, None));
    }

    #[test]
    fn a_sweeps_surface_is_toggled_and_makes_its_operation_new() {
        let shown = json!({"id": "w", "type": "extrude", "revision": "r1", "sketch": "s",
                           "extent": {"type": "blind", "length": {"value": "20 mm"}},
                           "operation": "join", "targets": ["b"]});
        let mut f = Form::from_show("p", &shown);
        assert!(!f.surface.as_ref().unwrap().read);
        f.surface.as_mut().unwrap().chosen = true;
        let g = given(&f);
        // A surface founds a sheet: new, and no targets.
        assert_eq!(
            (g.surface, g.operation.as_deref(), &g.targets),
            (Some(true), Some("new"), &None)
        );
        f.revert();
        assert!(!f.dirty());
        // Read as a surface, it is shown so; a box has no toggle.
        let mut on = shown.clone();
        on["surface"] = json!(true);
        on["operation"] = json!("new");
        assert!(Form::from_show("p", &on).surface.unwrap().read);
        for ty in ["revolve", "sweep", "loft"] {
            let v = json!({"id": "x", "type": ty, "revision": "r1"});
            assert!(Form::from_show("p", &v).surface.is_some(), "{ty}");
        }
        let v = json!({"id": "b", "type": "box", "revision": "r1",
                       "params": {"x": "1 mm"}, "placement": {}});
        assert!(Form::from_show("p", &v).surface.is_none());
    }

    #[test]
    fn a_split_by_a_tool_shows_it_and_a_thicken_edits_through_the_form() {
        let shown = json!({"id": "sp", "type": "split", "revision": "r1", "target": "h",
                           "tool": {"body": "parting"}, "keep": "both"});
        let f = Form::from_show("p", &shown);
        let s = f.split.as_ref().unwrap();
        assert_eq!(
            (s.plane.as_str(), s.tool.as_deref()),
            ("", Some("body parting"))
        );
        let shown = json!({"id": "sp", "type": "split", "revision": "r1", "target": "h",
                           "tool": {"faces": ["p/k/a", "p/k/b"]}});
        let f = Form::from_show("p", &shown);
        assert_eq!(f.split.unwrap().tool.as_deref(), Some("faces p/k/a, p/k/b"));
        // A thicken: its operation and targets the form's own, its sizes
        // its surface form's; a refusal at a group's side lands there.
        let shown = json!({"id": "t", "type": "thicken", "revision": "r1",
                           "from": {"sheet": "skin"}, "front": {"value": "2 mm"},
                           "back": {"value": "0 mm"},
                           "faces": [{"front": {"value": "1 mm"}, "back": {"value": "0 mm"},
                                      "faces": ["p/skin/a"]}],
                           "operation": "new"});
        let mut f = Form::from_show("p", &shown);
        assert_eq!(f.operation.as_ref().unwrap().read, "new");
        let Some(SurfaceForm::Thicken(t)) = &mut f.surface_form else {
            panic!("a thicken's form");
        };
        t.front.text = "3 mm".into();
        assert!(f.dirty());
        let g = given(&f);
        assert_eq!(g.front, Some(json!("3 mm")));
        let e =
            Error::new(ryucad::error::Code::InvalidParam, "no").with("pointer", "/faces/0/back");
        f.refuse(e);
        let Some(SurfaceForm::Thicken(t)) = &f.surface_form else {
            unreachable!()
        };
        assert_eq!(t.groups[0].back.error.as_deref(), Some("no"));
        assert!(t.back.error.is_none() && f.error.is_none());
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
        let none = none.import.unwrap();
        assert_eq!(none.product, None);
        assert!(!none.reference, "no mode: a healed solid");
        let r = Form::from_show(
            "p",
            &json!({"id": "body", "type": "import", "revision": "r1", "body": 0,
                    "mode": "reference",
                    "file": {"path": "imports/loose.step", "blake3": "ab12"}}),
        );
        assert!(r.import.unwrap().reference);
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
    fn a_following_patterns_from_and_points_are_edited_and_its_driver_shown() {
        let v = json!({"type": "instance_pattern", "form": "points", "instances": ["s0"],
                       "on": {"instance": ["lid"], "feature": "layout", "text": "pack/lid/layout"},
                       "from": "p1", "points": ["p1", "p5"]});
        let mut f = PatternForm::from_show(&v);
        assert_eq!(f.refs, [("on".to_owned(), "pack/lid/layout".to_owned())]);
        assert!(!f.dirty());
        f.from.text = "p5".into();
        f.points.text = "p1, p5, p7".into();
        let mut out = FeatureFields::default();
        f.given(&mut out).unwrap();
        assert_eq!(out.from.as_deref(), Some("p5"));
        assert_eq!(
            out.points,
            Some(vec!["p1".to_owned(), "p5".to_owned(), "p7".to_owned()])
        );
        f.points.text = String::new();
        let mut out = FeatureFields::default();
        f.given(&mut out).unwrap();
        assert_eq!(out.points, Some(Vec::new()), "every free point again");
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

    /// A shell's or offset's group refused by its whole pointer lands on
    /// the group's quantity, not the top-level field sharing its last step.
    #[test]
    fn a_shell_or_offset_groups_refusal_lands_on_the_group() {
        use ryucad::error::Code;
        let refused = |p: &str| Error::new(Code::InvalidParam, "no").with("pointer", p);
        let face = "p/b/".to_owned() + &"b".repeat(32);
        let shell = json!({"id": "sh", "type": "shell", "revision": "r1", "target": "b",
                           "thickness": {"value": "2 mm", "m": 0.002},
                           "faces": [{"thickness": {"value": "4 mm", "m": 0.004}, "faces": [face]}]});
        let mut f = Form::from_show("p", &shell);
        f.refuse(refused("/faces/0/thickness"));
        let s = f.shell.as_ref().unwrap();
        assert!(s.groups[0].quantity.error.is_some());
        assert!(s.thickness.error.is_none() && f.error.is_none());
        // The top-level pointer still lands on the top-level field.
        f.refuse(refused("/thickness"));
        let s = f.shell.as_ref().unwrap();
        assert!(s.thickness.error.is_some() && s.groups[0].quantity.error.is_none());
        let offset = json!({"id": "o", "type": "offset", "revision": "r1", "target": "b",
                            "distance": {"value": "1 mm", "m": 0.001},
                            "faces": [{"distance": {"value": "3 mm", "m": 0.003}, "faces": [face]}]});
        let mut f = Form::from_show("p", &offset);
        f.refuse(refused("/faces/0/distance"));
        let o = f.offset.as_ref().unwrap();
        assert!(o.groups[0].quantity.error.is_some());
        assert!(o.distance.error.is_none() && f.error.is_none());
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
        assert_eq!(g.flip, Some(false.into()));
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
    fn a_mates_limits_are_edited_bound_by_bound() {
        let shown = json!({"id": "m1", "type": "mate", "revision": "r1", "form": "cylindrical",
                           "a": {"instance": ["base"], "at": "origin", "text": "frame/base"},
                           "b": {"instance": ["pin"], "at": "origin", "text": "frame/pin"},
                           "limits": {"angle": {"min": "-6 deg", "max": "6 deg"}}});
        let mut f = Form::from_show("frame", &shown);
        let m = f.mate.as_ref().unwrap();
        assert_eq!(m.takes_limits(), (true, true));
        let reads: Vec<&str> = m.limits.iter().map(|l| l.read.as_str()).collect();
        assert_eq!(reads, ["-6 deg", "6 deg", "", ""]);
        // A bound changed, one cleared, a distance stop added.
        let m = f.mate.as_mut().unwrap();
        m.limits[1].text = "8 deg".into();
        m.limits[0].text = String::new();
        m.limits[3].text = "10 mm".into();
        assert_eq!(
            given(&f).limits,
            Some(json!({"angle": {"min": "", "max": "8 deg"}, "distance": {"max": "10 mm"}}))
        );
        f.revert();
        assert!(!f.dirty());
        assert_eq!(given(&f).limits, None);
        // A planar mate's distance is always driven: no distance stops.
        let m = f.mate.as_mut().unwrap();
        m.form.chosen = "planar".into();
        assert_eq!(m.takes_limits(), (true, false));
        m.form.chosen = "slider".into();
        assert_eq!(m.takes_limits(), (false, true));
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
