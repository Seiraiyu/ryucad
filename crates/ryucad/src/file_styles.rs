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
        colour: s.colour.map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}")),
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

/// An imported assembly's file layer: each occurrence style its files
/// give in a usage path that starts at the assembly's own product, under
/// the instance path its imported instances make of those usages —
/// `<path>` for a style on the product at its end, `<path>/<founder>` for
/// one on a body, the founder of the import that reads that body. A style
/// whose path no instance stands for, or on a face (an assembly colours
/// occurrences, not faces), is left out: the import's transfer report
/// counts it `retained`. `parts` finds a placed part's files; `files` an
/// import file by path.
pub fn of_assembly<'a>(
    part: &'a Part,
    parts: &dyn Fn(&Id) -> Option<&'a Part>,
    files: &dyn Fn(&str) -> Option<&'a ImportFile>,
) -> AppearanceFile {
    let mut out = AppearanceFile::default();
    let mut sources = std::collections::BTreeSet::new();
    for fid in part.order() {
        if let Some(Ok(Feature::Instance(i))) = part.features.get(fid)
            && let Some(u) = &i.import
        {
            sources.insert((u.file.clone(), u.product.clone()));
        }
    }
    for (path, product) in sources {
        let Some(file) = files(&path) else {
            continue;
        };
        let Ok(opened) = crate::import::opened(file) else {
            continue;
        };
        let Some(root) = definition_of(&opened, &product) else {
            continue;
        };
        for o in opened.occurrence_styles() {
            if o.path.first().map(|x| x.0) != Some(root) {
                continue;
            }
            let (Some(key), Some(l)) = (key_of(&opened, &path, part, &o, parts), look(&o.style))
            else {
                continue;
            };
            out.occurrences.entry(key).or_insert(l);
        }
    }
    out
}

/// The definition a product reference names: its id and name, the
/// `ordinal`th of those.
pub fn definition_of(opened: &Opened, p: &crate::doc::import::ProductRef) -> Option<usize> {
    opened
        .products
        .definitions
        .iter()
        .enumerate()
        .filter(|(_, d)| *d.product_id == p.id && *d.name == p.name)
        .nth(p.ordinal as usize)
        .map(|(k, _)| k)
}

/// The index among definition `k`'s usages of the one an imported
/// instance stands for: its id, and its ordinal among the usages with
/// that id and child.
pub fn usage_index(
    opened: &Opened,
    k: usize,
    u: &crate::doc::assembly::ImportUsage,
) -> Option<usize> {
    let usage = u.usage.as_deref()?;
    let children = &opened.products.definitions.get(k)?.children;
    (0..children.len()).find(|&n| {
        &*children[n].id == usage && crate::import::usage_ordinal(opened, k, n) == u.ordinal
    })
}

/// The key occurrence style `o` of file `file` takes in assembly `asm`'s
/// layer, walking its usages down the instances imported from them.
fn key_of<'a>(
    opened: &Opened,
    file: &str,
    asm: &'a Part,
    o: &ryustep::styles::OccurrenceStyle,
    parts: &dyn Fn(&Id) -> Option<&'a Part>,
) -> Option<String> {
    let mut at = asm;
    let mut steps: Vec<Id> = Vec::new();
    let mut leaf = None;
    for (step, &(k, n)) in o.path.iter().enumerate() {
        // Usages are told apart by id and ordinal per child, so the
        // instance's part must be the usage's child's product too.
        let child = crate::import::product_of(
            opened,
            opened.products.definitions[k].children[n].definition,
        );
        let fid = at.order().iter().find(|fid| {
            matches!(at.features.get(*fid), Some(Ok(Feature::Instance(i)))
                if i.import.as_ref().is_some_and(|u| u.file == file
                    && usage_index(opened, k, u) == Some(n))
                    && parts(&i.part)
                        .and_then(crate::export::imported)
                        .is_none_or(|p| p == child))
        })?;
        let Some(Ok(Feature::Instance(i))) = at.features.get(fid) else {
            return None;
        };
        steps.push(fid.clone());
        let placed = parts(&i.part)?;
        if step + 1 < o.path.len() {
            at = placed;
        } else {
            leaf = Some(placed);
        }
    }
    let path = crate::assembly::path_text(&steps);
    match o.target {
        Target::Product => Some(path),
        Target::Body(h) => {
            let leaf = leaf?;
            let founder = leaf.order().iter().find(|fid| {
                matches!(leaf.features.get(*fid), Some(Ok(Feature::Import(i)))
                    if i.file.path == file
                        && crate::import::select(fid, i, opened).ok() == Some(h))
            })?;
            Some(format!("{path}/{founder}"))
        }
        Target::Face { .. } | Target::Unplaced => None,
    }
}

/// What an import interprets of a file's styles (design 448): every
/// instance of the styles of each body it read (`bodies`, indices into
/// `held`), and of each occurrence style whose usages an instance stands
/// for (`made`: (definition, usage index)) or a flattened import's
/// `place.via` names (`vias`); the rest are `unplaced`, each with its
/// styled item and the usages it names (`#n`).
pub fn interpreted(
    opened: &Opened,
    bodies: &std::collections::BTreeSet<usize>,
    made: &std::collections::BTreeSet<(usize, usize)>,
    vias: &std::collections::BTreeSet<Vec<Via>>,
) -> (Vec<u64>, Vec<Value>) {
    let mut instances = Vec::new();
    for &k in bodies {
        instances.extend(opened.body_styles(k).instances);
    }
    let mut unplaced = Vec::new();
    for o in opened.occurrence_styles() {
        let placed = !o.path.is_empty()
            && match o.target {
                Target::Body(h) | Target::Face { held: h, .. }
                    if vias.contains(&via_of(opened, &o.path)) =>
                {
                    bodies.contains(&h)
                }
                Target::Body(h) => bodies.contains(&h) && o.path.iter().all(|u| made.contains(u)),
                Target::Product => o.path.iter().all(|u| made.contains(u)),
                Target::Face { .. } | Target::Unplaced => false,
            };
        if placed {
            instances.extend(&o.instances);
        } else {
            unplaced.push(serde_json::json!({
                "step": format!("#{}", o.instance),
                "usages": o.usages.iter().map(|u| format!("#{u}")).collect::<Vec<_>>(),
            }));
        }
    }
    instances.sort_unstable();
    instances.dedup();
    (instances, unplaced)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::session::Session;
    use crate::testing::{call, fresh};
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
                // An occurrence's colour is taken whole, as OCCT writes
                // it: opaque, over the block's own.
                (Some("#00ff00"), Some(1.0)),
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

    #[test]
    fn an_assemblys_layer_names_the_instance_its_usage_made() {
        let (_d, s, v) = imported("painted_assembly.stp", json!({}));
        let rack = part_of(&v, "assembly", 0);
        let project = s.project();
        let p = project.part(&rack).unwrap();
        let l = of_assembly(p, &|id| project.parts.get(id), &|path| {
            project.imports.get(path)
        });
        assert_eq!(l.occurrences.len(), 1, "{l:?}");
        let (key, look) = l.occurrences.iter().next().unwrap();
        assert_eq!(look.colour.as_deref(), Some("#00ff00"));
        // `<instance>/<founder>`: the instance the file's usage `2` made.
        let (instance, founder) = key.split_once('/').unwrap();
        let Some(Ok(Feature::Instance(i))) =
            p.features.get(&Id::parse(instance, "feature").unwrap())
        else {
            panic!("{key}")
        };
        assert_eq!(i.import.as_ref().unwrap().usage.as_deref(), Some("2"));
        let block = project.part(&i.part).unwrap();
        assert!(matches!(
            block.features.get(&Id::parse(founder, "feature").unwrap()),
            Some(Ok(Feature::Import(_)))
        ));
    }

    fn appearance(v: &Value) -> Value {
        v["transfer"]["categories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["category"] == "appearance")
            .cloned()
            .unwrap_or_else(|| panic!("no appearance in {v}"))
    }

    #[test]
    fn an_imports_styles_are_interpreted_and_its_containers_retained() {
        // Two styled items of eight instances each; the presentation
        // representation holding them is retained.
        let (_d, mut s, v) = imported("painted.stp", json!({}));
        let a = appearance(&v);
        assert_eq!(a["interpreted"]["count"], 16, "{a}");
        assert_eq!(a["retained"]["count"], 1, "{a}");
        assert!(a.get("unplaced").is_none());
        // import.report says the same of the committed file.
        let path = v["file"]["path"].as_str().unwrap().to_owned();
        let r = call(&mut s, "import.report", json!({"file": path})).unwrap();
        assert_eq!(appearance(&r)["interpreted"], a["interpreted"]);
        // The assembly: the block's, the pin's (transparent: ten) and the
        // override's; three presentation representations.
        let (_d, _s, v) = imported("painted_assembly.stp", json!({}));
        let a = appearance(&v);
        assert_eq!(a["interpreted"]["count"], 26, "{a}");
        assert_eq!(a["retained"]["count"], 3, "{a}");
    }

    #[test]
    fn an_occurrence_style_no_instance_stands_for_is_unplaced() {
        let bytes = std::fs::read(fixture("painted_assembly.stp")).unwrap();
        let opened = ryustep::open(&bytes).unwrap();
        let none = std::collections::BTreeSet::new();
        let (instances, unplaced) =
            interpreted(&opened, &none, &Default::default(), &Default::default());
        assert!(instances.is_empty());
        assert_eq!(unplaced.len(), 1);
        let u = &unplaced[0];
        assert!(u["step"].as_str().unwrap().starts_with('#'));
        assert_eq!(u["usages"].as_array().unwrap().len(), 1);
    }

    /// `painted.stp`'s block, its body `colour` and no face styled: the
    /// same geometry, so an update keeps every name.
    fn repainted(colour: [u8; 3]) -> Vec<u8> {
        let b = ryukei::primitive::block(
            ryukei::name::OpKey(1),
            0.01,
            0.02,
            0.03,
            &ryukei::geom::Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        let model = ryustep::WriteModel {
            products: vec![ryustep::Product {
                id: "block".into(),
                name: "block".into(),
                bodies: vec![b],
                names: vec!["body".into()],
                ..Default::default()
            }],
        };
        let paints = ryustep::Paints {
            bodies: vec![ryustep::BodyPaint {
                product: 0,
                body: 0,
                base: Some(ryustep::Paint {
                    colour,
                    opacity: 1.0,
                }),
                faces: Vec::new(),
            }],
            occurrences: Vec::new(),
        };
        let opts = ryustep::WriteOptions {
            unit: ryustep::LengthUnit::Millimeter,
            ..Default::default()
        };
        ryustep::write_styled(&model, &[], &paints, &opts)
            .unwrap()
            .bytes
    }

    #[test]
    fn an_update_brings_the_new_files_colours_and_keeps_a_persons() {
        let (_d, mut s, v) = imported("painted.stp", json!({}));
        let part = part_of(&v, "part", 0);
        let stored = v["file"]["path"].as_str().unwrap().to_owned();
        let get = |s: &mut Session| {
            call(s, "appearance.get", json!({"part": part.as_str()})).unwrap()["solids"][0].clone()
        };
        let before = get(&mut s);
        let blue = before["faces"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        // A person's face entry on another face.
        let solid = before["solid"].as_str().unwrap().to_owned();
        let names = crate::appearance::tests::face_names(&mut s, part.as_str(), &solid);
        let other = names.iter().find(|n| **n != blue).unwrap().clone();
        call(
            &mut s,
            "appearance.set",
            json!({"entities": [format!("{part}/{solid}/{other}")], "colour": "#ffffff"}),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let next = dir.path().join("painted.stp");
        std::fs::write(&next, repainted([0, 255, 0])).unwrap();
        call(
            &mut s,
            "import",
            json!({"path": next, "solid": true, "update": stored}),
        )
        .unwrap();
        let after = get(&mut s);
        assert_eq!(after["colour"], "#00ff00", "{after}");
        assert_eq!(after["source"]["colour"], "file");
        // The old file's blue face is gone with it; the person's stays.
        assert!(after["faces"].get(&blue).is_none(), "{after}");
        assert_eq!(after["faces"][&other]["colour"], "#ffffff");
    }
}
