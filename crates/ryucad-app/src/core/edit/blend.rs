//! A fillet's or chamfer's groups in the property panel (ryucad's `blend` kind).

use super::*;

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
    pub(super) fn from_show(v: &Value) -> BlendForm {
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

    pub(super) fn sizes(&self) -> impl Iterator<Item = &Field> {
        self.groups
            .iter()
            .flat_map(|g| g.sizes.iter().map(|(_, f)| f))
    }

    /// Whether a group's sizes or edges changed: then all of them go.
    pub(super) fn groups_dirty(&self) -> bool {
        self.sizes().any(Field::dirty) || self.groups.iter().any(|g| g.edges.dirty())
    }

    pub fn dirty(&self) -> bool {
        self.groups_dirty() || self.propagate.dirty()
    }

    pub(super) fn revert(&mut self) {
        for g in &mut self.groups {
            for (_, f) in &mut g.sizes {
                f.text.clone_from(&f.read);
            }
            g.edges.chosen.clone_from(&g.edges.read);
        }
        self.propagate.chosen = self.propagate.read;
    }

    /// The groups as `feature.edit` takes them: every one, as typed.
    pub(super) fn groups_json(&self) -> Value {
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

impl FeatureForm for BlendForm {
    fn read(ty: &str, v: &Value) -> Option<BlendForm> {
        matches!(ty, "fillet" | "chamfer").then(|| BlendForm::from_show(v))
    }
    fn kind_fields(&self) -> Vec<&Field> {
        self.sizes().collect()
    }
    fn kind_fields_mut(&mut self) -> Vec<&mut Field> {
        let mut out: Vec<&mut Field> = Vec::new();
        for g in &mut self.groups {
            out.extend(g.sizes.iter_mut().map(|(_, f)| f));
        }
        out
    }
    fn kind_dirty(&self) -> bool {
        self.dirty()
    }
    fn kind_revert(&mut self) {
        self.revert();
    }
    fn kind_given(&self, out: &mut FeatureFields) -> Result<(), Error> {
        out.groups = self.groups_dirty().then(|| self.groups_json());
        out.propagate = self.propagate.dirty().then_some(self.propagate.chosen);
        Ok(())
    }
    fn kind_at_pointer(&mut self, steps: &[&str]) -> Option<&mut Field> {
        let ["groups", i, k] = steps else {
            return None;
        };
        let i: usize = i.parse().ok()?;
        let g = self.groups.get_mut(i)?;
        g.sizes.iter_mut().find(|(key, _)| key == k).map(|(_, f)| f)
    }
}

/// The tree's icon of a type this kind owns.
pub(crate) fn icon(ty: &str, _op: Option<&str>, _form: Option<&str>) -> Option<&'static str> {
    Some(match ty {
        "fillet" => "fillet",
        "chamfer" => "chamfer",
        _ => return None,
    })
}
