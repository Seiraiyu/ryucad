//! A datum's, path's or helix's sizes, flip or reverse and hand in the property panel (ryucad's `datum` and `path` kinds).

use super::*;

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
    pub(super) fn from_show(v: &Value) -> DatumForm {
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

    pub(super) fn fields(&self) -> impl Iterator<Item = &Field> {
        self.sizes.iter().chain(self.coordinates.iter().flatten())
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty)
            || self.flag.as_ref().is_some_and(Flag::dirty)
            || self.hand.as_ref().is_some_and(Choice::dirty)
    }

    pub(super) fn revert(&mut self) {
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
    pub(super) fn given(&self, out: &mut FeatureFields) {
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
                _ => out.flip = Some(f.chosen.into()),
            }
        }
        if let Some(h) = self.hand.as_ref().filter(|h| h.dirty()) {
            out.hand = Some(h.chosen.clone());
        }
    }
}

impl FeatureForm for DatumForm {
    fn read(ty: &str, v: &Value) -> Option<DatumForm> {
        crate::core::datums::kind_of(ty).map(|_| DatumForm::from_show(v))
    }
    fn kind_fields(&self) -> Vec<&Field> {
        self.fields().collect()
    }
    fn kind_fields_mut(&mut self) -> Vec<&mut Field> {
        let mut out: Vec<&mut Field> = self.sizes.iter_mut().collect();
        out.extend(self.coordinates.iter_mut().flatten());
        out
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
}

/// The tree's icon of a type this kind owns.
pub(crate) fn icon(ty: &str, _op: Option<&str>, _form: Option<&str>) -> Option<&'static str> {
    Some(match ty {
        "plane" => "datum-plane",
        "axis" => "datum-axis",
        "point" => "datum-point",
        "path" | "helix" => "path",
        _ => return None,
    })
}
