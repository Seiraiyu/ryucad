//! A pattern's or an instance pattern's spacings, counts, skips and seeds in the property panel (ryucad's `pattern` and `instance_pattern` kinds).

use super::*;

/// A pattern's spacings, counts and angle, its directions' reversals, its
/// skips (as JSON: `[2]`, or `[[1, 1]]` with two directions), its seeds,
/// and its form, directions, axis or plane as read; an instance pattern's
/// the same, its seeds instances and its directions connectors. Seeds can
/// be taken out, not added.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternForm {
    /// Whether it is an instance pattern: its seeds are `instances`.
    pub instances: bool,
    /// `linear`, `circular` or `mirror`; an instance pattern's `feature`
    /// or `points` too.
    pub form: String,
    /// Each size by its key in the file, with its field.
    pub sizes: Vec<(String, Field)>,
    pub reverse: Vec<(String, Flag)>,
    pub skip: Field,
    /// Its direction, second direction, axis or plane, by key; a
    /// following pattern's places (`on`).
    pub refs: Vec<(String, String)>,
    pub seeds: Picks,
    /// A following pattern's seed point (empty for its first).
    pub from: Field,
    /// A points pattern's points, comma-separated (empty for every free
    /// point).
    pub points: Field,
}

impl PatternForm {
    /// From a `feature.show` of a pattern.
    pub(super) fn from_show(v: &Value) -> PatternForm {
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
        let refs = ["direction", "direction2", "axis", "plane", "on"]
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
        let ids = |x: &Value| {
            x.as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let from = Field::new("from", text(&v["from"]));
        let points = Field::new("points", ids(&v["points"]));
        PatternForm {
            instances,
            form: text(&v["form"]),
            sizes,
            reverse,
            skip: Field::new("skip", skip),
            refs,
            seeds: Picks::new("seeds", seeds),
            from,
            points,
        }
    }

    pub(super) fn fields(&self) -> impl Iterator<Item = &Field> {
        self.sizes
            .iter()
            .map(|(_, f)| f)
            .chain([&self.skip, &self.from, &self.points])
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty)
            || self.reverse.iter().any(|(_, f)| f.dirty())
            || self.seeds.dirty()
    }

    pub(super) fn revert(&mut self) {
        for (_, f) in &mut self.sizes {
            f.text.clone_from(&f.read);
        }
        for (_, f) in &mut self.reverse {
            f.chosen = f.read;
        }
        self.skip.text.clone_from(&self.skip.read);
        self.from.text.clone_from(&self.from.read);
        self.points.text.clone_from(&self.points.read);
        self.seeds.chosen.clone_from(&self.seeds.read);
    }

    /// What changed, as `feature.edit` takes it; skips that aren't JSON
    /// are refused before anything is sent.
    pub(super) fn given(&self, out: &mut FeatureFields) -> Result<(), Error> {
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
        if self.from.dirty() {
            out.from = Some(self.from.text.trim().to_owned());
        }
        if self.points.dirty() {
            out.points = Some(
                self.points
                    .text
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
                    .collect(),
            );
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
