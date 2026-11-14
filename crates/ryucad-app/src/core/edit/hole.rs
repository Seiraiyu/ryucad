//! A hole's sizes, depth and points in the property panel (ryucad's `hole` kind).

use super::*;

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
    /// The hole's canonical spec, when the rebuild reported one.
    pub spec: Option<String>,
    /// Every size the hole is built from, read-only: label, value and
    /// source (`rule`, `formula`, `override`, a standard, …).
    pub derived: Vec<(String, String, String)>,
}

/// A hole's sizes as the rebuild worked them out (`feature.show`'s
/// `sizes`), in the order the panel lists them.
fn derived_sizes(s: &Value) -> Vec<(String, String, String)> {
    let one = |x: &Value| (text(&x["value"]), text(&x["source"]));
    let mut out = Vec::new();
    let mut push = |label: &str, x: &Value| {
        if x.is_object() {
            let (v, src) = one(x);
            out.push((label.to_owned(), v, src));
        } else if let Some([lo, hi]) = x.as_array().map(Vec::as_slice) {
            // Limits: least and greatest, the greatest open when null.
            let (a, src) = one(lo);
            let b = if hi.is_null() {
                String::new()
            } else {
                text(&hi["value"])
            };
            out.push((
                label.to_owned(),
                format!("{a} – {b}").trim_end().to_owned(),
                src,
            ));
        }
    };
    push("drill", &s["drill"]);
    for (group, keys) in [
        ("counterbore", ["diameter", "depth"]),
        ("countersink", ["diameter", "angle"]),
    ] {
        for k in keys {
            push(&format!("{group} {k}"), &s[group][k]);
        }
    }
    push("major", &s["thread"]["major"]);
    push("pitch", &s["thread"]["pitch"]);
    push("minor", &s["thread"]["limits"]["minor"]);
    push("fit", &s["fit"]["limits"]);
    out
}

impl HoleForm {
    /// From a `feature.show` of a hole.
    pub(super) fn from_show(v: &Value) -> HoleForm {
        // A thread or fastener derives the drill: its diameter is an edit
        // only when written (an override).
        let intent = !v["thread"].is_null() || !v["fastener"].is_null();
        let mut sizes = Vec::new();
        if !intent || !v["diameter"].is_null() {
            sizes.push((
                "diameter".to_owned(),
                Field::new("diameter", text(&v["diameter"])),
            ));
        }
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
            spec: v["spec"].as_str().map(str::to_owned),
            derived: derived_sizes(&v["sizes"]),
        }
    }

    pub(super) fn fields(&self) -> impl Iterator<Item = &Field> {
        self.sizes.iter().map(|(_, f)| f).chain([&self.length])
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty) || self.depth.dirty() || self.points.dirty()
    }

    pub(super) fn revert(&mut self) {
        for (_, f) in &mut self.sizes {
            f.text.clone_from(&f.read);
        }
        self.length.text.clone_from(&self.length.read);
        self.depth.chosen.clone_from(&self.depth.read);
        self.points.chosen.clone_from(&self.points.read);
    }

    /// What changed, as `feature.edit` takes it.
    pub(super) fn given(&self, out: &mut FeatureFields) {
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
