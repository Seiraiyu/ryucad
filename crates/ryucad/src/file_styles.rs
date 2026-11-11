//! The file layer of appearance (design 448, plan 24.B): the colours and
//! transparencies an imported part's committed STEP files give its
//! bodies, faces and occurrences, read from the bytes each time they are
//! asked for and never written anywhere, so a re-import's new bytes
//! bring their own colours and nothing can collide with a person's
//! entries. A layer has `appearance.json`'s shape: a part's `solids` by
//! the founder an import feature founds and `faces` by
//! `<founder>/<name>`; an imported assembly's `occurrences` by instance
//! path ([`crate::appearance`] interleaves it with the entries).

use crate::doc::appearance::{AppearanceFile, Look, face_key};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::import::Via;
use crate::doc::project::{ImportFile, Part};
use crate::rebuild::{Output, PartResult};
use ryustep::Opened;
use ryustep::styles::{Style, Target};
use serde_json::Value;

/// A file's style as an entry: `#rrggbb` and the opacity; `None` for
/// one that sets neither.
pub fn look(s: &Style) -> Option<Look> {
    let l = Look {
        colour: s
            .colour
            .map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}")),
        opacity: s.opacity,
        ..Default::default()
    };
    (!l.is_empty()).then_some(l)
}

/// The usages of `path` (definition, usage index) as a flattened import's
/// `place.via` names them.
pub fn via_of(opened: &Opened, path: &[(usize, usize)]) -> Vec<Via> {
    path.iter()
        .map(|&(k, n)| {
            let c = &opened.products.definitions[k].children[n];
            Via {
                usage: c.id.to_string(),
                product: crate::import::product_of(opened, c.definition),
                ordinal: crate::import::usage_ordinal(opened, k, n),
            }
        })
        .collect()
}

/// A part's file layer: for each import feature that built, its body's
/// style under its founder and its faces' under `<founder>/<name>`, each
/// face found through its origin's STEP instance. A flattened import's
/// body and faces take the styles the file gives the occurrence its
/// `place.via` names, over their own. A feature whose file doesn't open
/// or whose body isn't there gives nothing: the rebuild says why.
pub fn of_part(part: &Part, result: &PartResult) -> AppearanceFile {
    let mut out = AppearanceFile::default();
    for (fid, r) in &result.features {
        let Some(Ok(Feature::Import(i))) = part.features.get(fid) else {
            continue;
        };
        let Ok(Output::Solids(made)) = r else {
            continue;
        };
        let Some(file) = part.imports.get(&i.file.path) else {
            continue;
        };
        let Ok(opened) = crate::import::opened(file) else {
            continue;
        };
        let Ok(k) = crate::import::select(fid, i, &opened) else {
            continue;
        };
        let styles = opened.body_styles(k);
        let mut body = styles.body;
        let mut faces = styles.faces;
        if let Some(place) = &i.place {
            for o in opened.occurrence_styles() {
                if o.path.is_empty() || via_of(&opened, &o.path) != place.via {
                    continue;
                }
                match o.target {
                    Target::Body(h) if h == k => body = Some(o.style.over(body)),
                    Target::Face { held, face } if held == k => {
                        let s = o.style.over(faces.get(&face).copied());
                        faces.insert(face, s);
                    }
                    _ => {}
                }
            }
        }
        for (founder, built) in made.solids.iter().filter(|(f, _)| f == fid) {
            if let Some(l) = body.as_ref().and_then(look) {
                out.solids.insert(founder.to_string(), l);
            }
            if faces.is_empty() {
                continue;
            }
            for (name, origin) in built.origins.iter() {
                if origin["feature"] != fid.as_str() {
                    continue;
                }
                let Some(n) = origin["step"]
                    .as_str()
                    .and_then(|s| s.strip_prefix('#'))
                    .and_then(|s| s.parse::<u64>().ok())
                else {
                    continue;
                };
                if let Some(l) = faces.get(&n).and_then(look) {
                    out.faces.insert(face_key(founder.as_str(), name), l);
                }
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};
    use crate::session::Session;
    use serde_json::json;

    /// A file of ryustep's styled fixtures (`painted.stp`,
    /// `painted_assembly.stp`), by path.
    pub(crate) fn fixture(name: &str) -> String {
        format!(
            "{}/../ryustep/tests/data/styles/{name}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    /// `name` imported as solids into a fresh project, with `extra`
    /// laid over the request; the import's answer.
    pub(crate) fn imported(name: &str, extra: Value) -> (tempfile::TempDir, Session, Value) {
        let (dir, mut s) = fresh();
        let mut p = json!({"path": fixture(name), "solid": true});
        for (k, v) in extra.as_object().into_iter().flatten() {
            p[k] = v.clone();
        }
        let v = call(&mut s, "import", p).unwrap();
        (dir, s, v)
    }

    /// The answer's part of `kind` (`part` or `assembly`), `n`th.
    pub(crate) fn part_of(v: &Value, kind: &str, n: usize) -> Id {
        let p = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["kind"] == kind)
            .nth(n)
            .unwrap_or_else(|| panic!("no {kind} {n} in {v}"));
        Id::parse(p["part"].as_str().unwrap(), "part").unwrap()
    }

    fn layer(s: &mut Session, part: &Id) -> AppearanceFile {
        let r = s.rebuilt(part).unwrap().clone();
        of_part(s.project().part(part).unwrap(), &r)
    }

    #[test]
    fn a_parts_layer_takes_its_bodys_and_faces_styles() {
        let (_d, mut s, v) = imported("painted.stp", json!({}));
        let part = part_of(&v, "part", 0);
        let l = layer(&mut s, &part);
        assert_eq!(l.solids.len(), 1, "{l:?}");
        let (founder, look) = l.solids.iter().next().unwrap();
        assert_eq!(look.colour.as_deref(), Some("#ff0000"));
        assert_eq!(l.faces.len(), 1, "{l:?}");
        let (key, face) = l.faces.iter().next().unwrap();
        assert!(key.starts_with(&format!("{founder}/")), "{key}");
        assert_eq!(face.colour.as_deref(), Some("#0000ff"));
        assert!(l.part.is_none() && l.occurrences.is_empty());
    }

    #[test]
    fn a_flattened_body_takes_its_occurrences_style_over_its_own() {
        let (_d, mut s, v) = imported("painted_assembly.stp", json!({"flatten": true}));
        let part = part_of(&v, "part", 0);
        let l = layer(&mut s, &part);
        let mut looks: Vec<(Option<&str>, Option<f64>)> = l
            .solids
            .values()
            .map(|x| (x.colour.as_deref(), x.opacity))
            .collect();
        looks.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        assert_eq!(
            looks,
            [
                (Some("#0000ff"), Some(0.5)),
                (Some("#00ff00"), None),
                (Some("#ff0000"), None)
            ]
        );
    }

    #[test]
    fn a_part_with_no_import_has_no_layer() {
        let (_d, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap();
        let p = Id::parse("p", "part").unwrap();
        assert!(layer(&mut s, &p).is_empty());
    }
}
