//! A mate's form, driven values and offsets in the property panel (ryucad's `mate` kind).

use super::*;

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
    /// Its stops as typed: angle min, angle max, distance min, distance
    /// max; empty for none.
    pub limits: [Field; 4],
    pub a: OffsetForm,
    pub b: OffsetForm,
    /// Each connector by side, as requests give it.
    pub connectors: Vec<(String, String)>,
}

impl MateForm {
    /// From a `feature.show` of a mate.
    pub(super) fn from_show(v: &Value) -> MateForm {
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
            limits: [
                ("angle min", &v["limits"]["angle"]["min"]),
                ("angle max", &v["limits"]["angle"]["max"]),
                ("distance min", &v["limits"]["distance"]["min"]),
                ("distance max", &v["limits"]["distance"]["max"]),
            ]
            .map(|(name, x)| Field::new(name, text(x))),
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

    /// Whether the chosen form may stop its angle, and its distance (a
    /// planar mate's distance is always driven).
    pub fn takes_limits(&self) -> (bool, bool) {
        let f = self.form.chosen.as_str();
        (
            matches!(f, "revolute" | "cylindrical" | "planar"),
            matches!(f, "slider" | "cylindrical"),
        )
    }

    pub(super) fn fields(&self) -> impl Iterator<Item = &Field> {
        [&self.angle, &self.distance]
            .into_iter()
            .chain(self.limits.iter())
            .chain(self.a.fields())
            .chain(self.b.fields())
    }

    pub fn dirty(&self) -> bool {
        self.form.dirty()
            || self.angle.dirty()
            || self.distance.dirty()
            || self.limits.iter().any(Field::dirty)
            || self.a.dirty()
            || self.b.dirty()
    }

    pub(super) fn revert(&mut self) {
        self.form.chosen.clone_from(&self.form.read);
        for f in [&mut self.angle, &mut self.distance]
            .into_iter()
            .chain(self.limits.iter_mut())
        {
            f.text.clone_from(&f.read);
        }
        self.a.revert();
        self.b.revert();
    }

    /// What changed, as `feature.edit` takes it: an angle or distance
    /// emptied lets it go, an offset goes whole.
    pub(super) fn given(&self, out: &mut FeatureFields) {
        if self.form.dirty() {
            out.form = Some(self.form.chosen.clone());
        }
        if self.angle.dirty() {
            out.angle = Some(json!(self.angle.text.trim()));
        }
        if self.distance.dirty() {
            out.distance = Some(json!(self.distance.text.trim()));
        }
        // Each changed bound, laid over what the mate has ("" clears it).
        let mut limits = json!({});
        for (k, f) in self.limits.iter().enumerate() {
            if f.dirty() {
                let value = if k < 2 { "angle" } else { "distance" };
                let bound = if k % 2 == 0 { "min" } else { "max" };
                limits[value][bound] = json!(f.text.trim());
            }
        }
        if limits.as_object().is_some_and(|o| !o.is_empty()) {
            out.limits = Some(limits);
        }
        if self.a.dirty() {
            out.a_offset = Some(self.a.json());
        }
        if self.b.dirty() {
            out.b_offset = Some(self.b.json());
        }
    }
}
