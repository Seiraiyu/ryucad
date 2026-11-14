//! A sweep's law, twist and scale in the property panel (ryucad's `along` kind, the type `sweep`).

use super::*;

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
pub(super) fn section_text(v: &Value) -> String {
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
    pub(super) fn from_show(v: &Value) -> SweepForm {
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

    pub(super) fn fields(&self) -> impl Iterator<Item = &Field> {
        [&self.twist, &self.scale].into_iter()
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty) || self.law.dirty()
    }

    pub(super) fn revert(&mut self) {
        for f in [&mut self.twist, &mut self.scale] {
            f.text.clone_from(&f.read);
        }
        self.law.chosen.clone_from(&self.law.read);
    }

    /// What changed, as `feature.edit` takes it: a twist or scale typed
    /// (emptied, `""`: none), and a law chosen.
    pub(super) fn given(&self, out: &mut FeatureFields) {
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
