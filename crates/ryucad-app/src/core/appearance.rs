//! Appearance in the window: how each drawn body's faces look, read from
//! the daemon (`appearance.get`), with what the Appearance window is
//! dragging laid over it until it is written; and that window's state.
//! A body nothing colours has no entry: it is drawn in the theme's
//! surface colour, opaque, from the one upload its mesh has.

use crate::core::model::BodyKey;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// How one face is drawn: its colour (sRGB; `None` for the theme's) and
/// opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub colour: Option<[f32; 3]>,
    pub opacity: f32,
}

impl Default for Face {
    fn default() -> Self {
        Face {
            colour: None,
            opacity: 1.0,
        }
    }
}

impl Face {
    fn of(v: &Value) -> Face {
        Face {
            colour: v["colour"].as_str().and_then(ryucad::appearance::rgb),
            opacity: v["opacity"].as_f64().unwrap_or(1.0) as f32,
        }
    }

    /// Whether it is drawn translucent.
    pub fn translucent(&self) -> bool {
        self.opacity < 1.0
    }
}

/// How a body's faces are drawn: all `base` but those in `faces`, by the
/// hex of their names. `id` tells looks apart, so bodies drawn alike
/// share an upload.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub id: u64,
    pub base: Face,
    pub faces: BTreeMap<String, Face>,
}

impl Look {
    fn new(base: Face, faces: BTreeMap<String, Face>) -> Look {
        let mut l = Look { id: 0, base, faces };
        l.id = l.digest();
        l
    }

    /// A digest of what it says, never 0 (which is the plain look's).
    fn digest(&self) -> u64 {
        let mut text = format!("{:?}", self.base);
        for (n, f) in &self.faces {
            text.push_str(&format!("{n}{f:?}"));
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        text.hash(&mut h);
        h.finish().max(1)
    }

    /// How the face named `name` (hex) is drawn.
    pub fn face(&self, name: &str) -> Face {
        self.faces.get(name).copied().unwrap_or(self.base)
    }

    /// Whether any face is translucent.
    pub fn translucent(&self) -> bool {
        self.base.translucent() || self.faces.values().any(Face::translucent)
    }
}

/// Every drawn body's look, by key, from `appearance.get`'s answer: those
/// drawn plain left out.
pub fn read(answer: &Value) -> BTreeMap<BodyKey, Arc<Look>> {
    let mut out = BTreeMap::new();
    for s in answer["solids"].as_array().into_iter().flatten() {
        let (Some(part), Some(solid)) = (s["part"].as_str(), s["solid"].as_str()) else {
            continue;
        };
        let faces: BTreeMap<String, Face> = s["faces"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(n, f)| (n.clone(), Face::of(f)))
            .collect();
        let base = Face::of(s);
        if base == Face::default() && faces.is_empty() {
            continue;
        }
        out.insert(
            (part.to_owned(), solid.to_owned()),
            Arc::new(Look::new(base, faces)),
        );
    }
    out
}

/// What a dragged change sets, before it is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preview {
    pub colour: Option<[f32; 3]>,
    pub opacity: Option<f32>,
}

/// The bodies (and, for a face, the face) `target` covers among `keys`:
/// a part's every solid, a solid, a face, an occurrence's every solid
/// under it, or an occurrence's solid.
fn covers<'a>(
    target: &str,
    keys: impl Iterator<Item = &'a BodyKey>,
) -> Vec<(BodyKey, Option<String>)> {
    let (part, rest) = match target.split_once('/') {
        Some((p, r)) => (p, Some(r)),
        None => (target, None),
    };
    let is_name = |s: &str| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit());
    let (prefix, face) = match rest {
        None => (None, None),
        Some(r) => match r.rsplit_once('/') {
            Some((solid, name)) if is_name(name) => (Some(solid.to_owned()), Some(name.to_owned())),
            _ => (Some(r.to_owned()), None),
        },
    };
    keys.filter(|(p, k)| {
        p == part
            && prefix
                .as_deref()
                .is_none_or(|x| k == x || k.starts_with(&format!("{x}/")))
    })
    .map(|k| (k.clone(), face.clone()))
    .collect()
}

/// `looks` with `preview` laid over what `target` covers among `keys`.
pub fn previewed<'a>(
    looks: &BTreeMap<BodyKey, Arc<Look>>,
    keys: impl Iterator<Item = &'a BodyKey>,
    target: &str,
    preview: Preview,
) -> BTreeMap<BodyKey, Arc<Look>> {
    let mut out = looks.clone();
    let over = |f: Face| Face {
        colour: preview.colour.or(f.colour),
        opacity: preview.opacity.unwrap_or(f.opacity),
    };
    for (key, face) in covers(target, keys) {
        let now = out.get(&key).map(|l| (**l).clone()).unwrap_or_default();
        let look = match face {
            Some(name) => {
                let mut faces = now.faces.clone();
                faces.insert(name.clone(), over(now.face(&name)));
                Look::new(now.base, faces)
            }
            None => Look::new(
                over(now.base),
                now.faces
                    .iter()
                    .map(|(n, f)| (n.clone(), over(*f)))
                    .collect(),
            ),
        };
        out.insert(key, Arc::new(look));
    }
    out
}

/// What the Appearance window may colour for a selected `entity` (or,
/// with none, the one part the window shows): a part's face, its solid
/// and the part; an occurrence's solid and the occurrences above it.
/// `assembly` says whether its first part is an assembly.
pub fn targets(entity: &str, assembly: bool) -> Vec<String> {
    let parts: Vec<&str> = entity.split('/').collect();
    let named = parts
        .last()
        .is_some_and(|n| n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit()));
    let mut out = Vec::new();
    if assembly {
        // `<asm>/<path>/<solid>/<name>`: the solid, then each path up.
        let end = if named { parts.len() - 1 } else { parts.len() };
        for k in (2..=end).rev() {
            if parts[k - 1].contains(':') {
                continue;
            }
            out.push(parts[..k].join("/"));
        }
        return out;
    }
    for k in (1..=parts.len().min(3)).rev() {
        out.push(parts[..k].join("/"));
    }
    out
}

/// The Appearance window: what it colours, the colour and opacity it
/// shows, what is being dragged, and the last refusal.
#[derive(Clone, Debug, Default)]
pub struct Editor {
    pub open: bool,
    /// What can be coloured, most specific first, and which is chosen.
    pub targets: Vec<String>,
    pub chosen: usize,
    /// sRGB bytes.
    pub colour: [u8; 3],
    pub opacity: f32,
    /// A change dragged and not yet written.
    pub dragging: Option<Preview>,
    /// The colour as typed, `#rrggbb`.
    pub typed: String,
    pub error: Option<ryucad::error::Error>,
}

impl Editor {
    /// Opens on `targets`, showing how `look`'s first face (or the base)
    /// is drawn now, with the theme's colour `theme` where nothing
    /// colours it.
    pub fn open_on(&mut self, targets: Vec<String>, now: Face, theme: [f32; 3]) {
        self.open = true;
        self.targets = targets;
        self.chosen = 0;
        let c = now.colour.unwrap_or(theme);
        self.colour = c.map(|x| (x * 255.0).round() as u8);
        self.opacity = now.opacity;
        self.dragging = None;
        self.typed = self.hex();
        self.error = None;
    }

    /// Takes the colour typed, if it is `#rrggbb`.
    pub fn take_typed(&mut self) -> bool {
        let t = self.typed.trim().trim_start_matches('#');
        let byte = |k: usize| u8::from_str_radix(t.get(k..k + 2)?, 16).ok();
        match (t.len(), byte(0), byte(2), byte(4)) {
            (6, Some(r), Some(g), Some(b)) => {
                self.colour = [r, g, b];
                self.typed = self.hex();
                true
            }
            _ => false,
        }
    }

    /// What it colours, if anything.
    pub fn target(&self) -> Option<&str> {
        self.targets.get(self.chosen).map(String::as_str)
    }

    /// The colour as `#rrggbb`.
    pub fn hex(&self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            self.colour[0], self.colour[1], self.colour[2]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NAME: &str = "0123456789abcdef0123456789abcdef";

    fn key(p: &str, s: &str) -> BodyKey {
        (p.to_owned(), s.to_owned())
    }

    #[test]
    fn answers_read_as_looks_and_plain_bodies_have_none() {
        let v = json!({"solids": [
            {"part": "p", "solid": "a", "opacity": 1},
            {"part": "p", "solid": "b", "colour": "#ff0000", "opacity": 0.5,
             "faces": {NAME: {"colour": "#0000ff", "opacity": 1}}},
            {"part": "asm", "solid": "i/b", "colour": "#ff0000", "opacity": 0.5,
             "faces": {NAME: {"colour": "#0000ff", "opacity": 1}}},
        ]});
        let looks = read(&v);
        assert_eq!(looks.len(), 2);
        let b = &looks[&key("p", "b")];
        assert_eq!(b.base.colour, Some([1.0, 0.0, 0.0]));
        assert!(b.translucent());
        assert_eq!(b.face(NAME).colour, Some([0.0, 0.0, 1.0]));
        assert_eq!(b.face("other").opacity, 0.5);
        // Alike looks are one upload.
        assert_eq!(b.id, looks[&key("asm", "i/b")].id);
        assert_ne!(b.id, 0);
    }

    #[test]
    fn a_preview_covers_what_its_target_does() {
        let keys = [
            key("p", "a"),
            key("p", "b"),
            key("asm", "i/a"),
            key("asm", "i/j/a"),
            key("asm", "k/a"),
        ];
        let red = Preview {
            colour: Some([1.0, 0.0, 0.0]),
            opacity: None,
        };
        let covered = |t: &str| -> Vec<BodyKey> {
            let out = previewed(&BTreeMap::new(), keys.iter(), t, red);
            out.keys().cloned().collect()
        };
        assert_eq!(covered("p"), [key("p", "a"), key("p", "b")]);
        assert_eq!(covered("p/b"), [key("p", "b")]);
        assert_eq!(covered("asm/i"), [key("asm", "i/a"), key("asm", "i/j/a")]);
        assert_eq!(covered("asm/i/a"), [key("asm", "i/a")]);
        let face = previewed(&BTreeMap::new(), keys.iter(), &format!("p/a/{NAME}"), red);
        let a = &face[&key("p", "a")];
        assert_eq!(a.base, Face::default());
        assert_eq!(a.face(NAME).colour, Some([1.0, 0.0, 0.0]));
        // An opacity keeps the colour under it.
        let half = Preview {
            colour: None,
            opacity: Some(0.5),
        };
        let both = previewed(&face, keys.iter(), "p/a", half);
        assert_eq!(
            both[&key("p", "a")].face(NAME),
            Face {
                colour: Some([1.0, 0.0, 0.0]),
                opacity: 0.5
            }
        );
    }

    #[test]
    fn targets_run_from_the_face_out() {
        assert_eq!(
            targets(&format!("p/b/{NAME}"), false),
            [format!("p/b/{NAME}"), "p/b".to_owned(), "p".to_owned()]
        );
        assert_eq!(targets("p", false), ["p"]);
        assert_eq!(
            targets(&format!("asm/i/j/b/{NAME}"), true),
            ["asm/i/j/b", "asm/i/j", "asm/i"]
        );
        assert_eq!(targets("asm/pat:2/seed", true), ["asm/pat:2/seed"]);
        let mut e = Editor::default();
        e.open_on(targets("p", false), Face::default(), [1.0, 0.5, 0.0]);
        assert_eq!((e.target(), e.hex().as_str()), (Some("p"), "#ff8000"));
        assert_eq!(e.typed, "#ff8000");
        e.typed = "2050C0".into();
        assert!(e.take_typed());
        assert_eq!(
            (e.colour, e.typed.as_str()),
            ([0x20, 0x50, 0xc0], "#2050c0")
        );
        e.typed = "#12345".into();
        assert!(!e.take_typed());
    }
}
