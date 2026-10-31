//! Body files: one representation's B-rep items split out of a STEP file
//! with every instance they reach, as the smallest file that heals as the
//! original body does.
//!
//! The root is a representation of the original's entity (a complex one
//! becomes `SHAPE_REPRESENTATION`) named `''`, holding only the B-rep items,
//! in the original's context. Instances are numbered from 1 in the order a
//! breadth-first walk from the root first meets them, and the header is
//! fixed, so the same body gives the same bytes whatever its instance
//! numbers were and whatever file held it.

use std::collections::HashMap;

use ryu_part21::{File, Header, Instance, Param, Part, Record};

/// The fixed header of a body file, naming the source's schemas.
fn header(schemas: &[&str]) -> Header {
    let s = |x: &str| Param::Str(x.into());
    Header {
        entities: vec![
            Part {
                name: "FILE_DESCRIPTION".into(),
                params: vec![
                    Param::List(vec![s("a body split out by step-hunt")]),
                    s("2;1"),
                ],
            },
            Part {
                name: "FILE_NAME".into(),
                params: vec![
                    s("body.step"),
                    s("2000-01-01T00:00:00"),
                    Param::List(vec![s("")]),
                    Param::List(vec![s("")]),
                    s("step-hunt"),
                    s("step-hunt"),
                    s(""),
                ],
            },
            Part {
                name: "FILE_SCHEMA".into(),
                params: vec![Param::List(schemas.iter().map(|x| s(x)).collect())],
            },
        ],
    }
}

fn refs(p: &Param, out: &mut Vec<u64>) {
    match p {
        Param::Ref(r) => out.push(*r),
        Param::List(l) => l.iter().for_each(|x| refs(x, out)),
        Param::Typed(_, v) => refs(v, out),
        _ => {}
    }
}

fn remap(p: &Param, ids: &HashMap<u64, u64>) -> Param {
    match p {
        Param::Ref(r) => Param::Ref(ids[r]),
        Param::List(l) => Param::List(l.iter().map(|x| remap(x, ids)).collect()),
        Param::Typed(n, v) => Param::Typed(n.clone(), Box::new(remap(v, ids))),
        other => other.clone(),
    }
}

fn remap_part(p: &Part, ids: &HashMap<u64, u64>) -> Part {
    Part {
        name: p.name.clone(),
        params: p.params.iter().map(|x| remap(x, ids)).collect(),
    }
}

/// The body file of representation `representation` (in `context`)
/// holding `items`, from the parsed `file`. An error names an instance
/// that names one the file lacks.
pub fn body_file(
    file: &File,
    representation: u64,
    context: u64,
    items: &[u64],
) -> Result<Vec<u8>, String> {
    let name: Box<str> = match file.get(representation).map(|i| &i.record) {
        Some(Record::Simple(p)) => p.name.clone(),
        Some(Record::Complex(_)) => "SHAPE_REPRESENTATION".into(),
        None => return Err(format!("no instance #{representation}")),
    };
    let root = Part {
        name,
        params: vec![
            Param::Str("".into()),
            Param::List(items.iter().map(|&i| Param::Ref(i)).collect()),
            Param::Ref(context),
        ],
    };
    // Number what the root reaches, breadth first.
    let mut ids: HashMap<u64, u64> = HashMap::new();
    let mut order: Vec<u64> = Vec::new();
    let mut found = Vec::new();
    root.params.iter().for_each(|p| refs(p, &mut found));
    let mut next = 0;
    loop {
        for r in found.drain(..) {
            if let std::collections::hash_map::Entry::Vacant(e) = ids.entry(r) {
                e.insert(order.len() as u64 + 2);
                order.push(r);
            }
        }
        let Some(&old) = order.get(next) else { break };
        next += 1;
        let inst = file
            .get(old)
            .ok_or_else(|| format!("#{old} is named but the file lacks it"))?;
        for part in inst.record.parts() {
            part.params.iter().for_each(|p| refs(p, &mut found));
        }
    }
    let mut instances = vec![Instance {
        id: 1,
        record: Record::Simple(remap_part(&root, &ids)),
    }];
    for (k, old) in order.iter().enumerate() {
        let record = match &file.get(*old).expect("found above").record {
            Record::Simple(p) => Record::Simple(remap_part(p, &ids)),
            Record::Complex(parts) => {
                Record::Complex(parts.iter().map(|p| remap_part(p, &ids)).collect())
            }
        };
        instances.push(Instance {
            id: k as u64 + 2,
            record,
        });
    }
    let out = File {
        header: header(&file.header.schemas()),
        instances,
    };
    ryu_part21::write(&out).map_err(|e| format!("{e}"))
}

/// A case's identifier: the first 16 hex digits of the BLAKE3 of its bytes.
pub fn case_id(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..16].to_owned()
}

/// The entity name of an instance: a simple record's name, or a complex
/// instance's part names joined by `&`.
fn entity(inst: &Instance) -> String {
    inst.record
        .parts()
        .iter()
        .map(|p| &*p.name)
        .collect::<Vec<_>>()
        .join("&")
}

fn is_face(inst: &Instance) -> bool {
    inst.record
        .parts()
        .iter()
        .any(|p| &*p.name == "ADVANCED_FACE" || &*p.name == "FACE_SURFACE")
}

/// The geometry an instance stands for: a face's surface, an edge's curve,
/// else the instance itself.
fn geometry(file: &File, inst: &Instance) -> String {
    let at = |name: &str, k: usize| {
        inst.record
            .parts()
            .iter()
            .find(|p| &*p.name == name)
            .and_then(|p| match p.params.get(k) {
                Some(Param::Ref(r)) => file.get(*r),
                _ => None,
            })
    };
    let target = at("ADVANCED_FACE", 2)
        .or_else(|| at("FACE_SURFACE", 2))
        .or_else(|| at("EDGE_CURVE", 3));
    entity(target.unwrap_or(inst))
}

/// How many faces a STEP file holds.
pub fn faces(file: &File) -> usize {
    file.instances.iter().filter(|i| is_face(i)).count()
}

/// The surface entities a STEP file's faces lie on, sorted and distinct.
pub fn surfaces(file: &File) -> Vec<String> {
    let mut out: Vec<String> = file
        .instances
        .iter()
        .filter(|i| is_face(i))
        .map(|i| geometry(file, i))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// What the instances named `#n` in `step` lie on (faces' surfaces, edges'
/// curves, other instances' own entities), sorted and distinct; names
/// that aren't `#n` or the file lacks are left out.
pub fn named_kinds(file: &File, step: &[String]) -> Vec<String> {
    let mut out: Vec<String> = step
        .iter()
        .filter_map(|s| s.strip_prefix('#')?.parse::<u64>().ok())
        .filter_map(|n| file.get(n))
        .map(|i| geometry(file, i))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A file holding one plane face in a brep, numbered from `base`, with
    /// unrelated instances and a styled item beside it.
    pub(crate) fn sample(base: u64) -> String {
        let n = |k: u64| base + k;
        format!(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('x','',(''),(''),'','','');\n\
             FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n\
             #{a}=CARTESIAN_POINT('',(0.,0.,0.));\n\
             #{b}=AXIS2_PLACEMENT_3D('',#{a},$,$);\n\
             #{c}=PLANE('',#{b});\n\
             #{d}=ADVANCED_FACE('',(),#{c},.T.);\n\
             #{e}=CLOSED_SHELL('',(#{d}));\n\
             #{f}=MANIFOLD_SOLID_BREP('s',#{e});\n\
             #{g}=(GEOMETRIC_REPRESENTATION_CONTEXT(3)REPRESENTATION_CONTEXT('',''));\n\
             #{h}=ADVANCED_BREP_SHAPE_REPRESENTATION('named',(#{b},#{f}),#{g});\n\
             #{i}=STYLED_ITEM('',(),#{d});\n\
             #{j}=CARTESIAN_POINT('unused',(1.,0.,0.));\n\
             ENDSEC;\nEND-ISO-10303-21;\n",
            a = n(0),
            b = n(1),
            c = n(2),
            d = n(3),
            e = n(4),
            f = n(5),
            g = n(6),
            h = n(7),
            i = n(8),
            j = n(9),
        )
    }

    fn split(base: u64) -> Vec<u8> {
        let file = ryu_part21::parse(sample(base).as_bytes()).unwrap();
        body_file(&file, base + 7, base + 6, &[base + 5]).unwrap()
    }

    #[test]
    fn a_body_takes_only_what_its_items_reach() {
        let bytes = split(10);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with("ISO-10303-21;"), "{text}");
        assert!(
            text.contains("#1=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#2),#3);"),
            "{text}"
        );
        assert!(!text.contains("STYLED_ITEM"), "{text}");
        assert!(!text.contains("unused"), "{text}");
        let back = ryu_part21::parse(&bytes).unwrap();
        assert_eq!(back.instances.len(), 8);
        assert_eq!(faces(&back), 1);
        assert_eq!(surfaces(&back), ["PLANE"]);
        assert_eq!(
            named_kinds(&back, &["#5".into(), "#2".into(), "x".into()]),
            ["MANIFOLD_SOLID_BREP", "PLANE"]
        );
    }

    #[test]
    fn copies_numbered_differently_are_one_case() {
        assert_eq!(split(10), split(500));
        assert_eq!(case_id(&split(10)), case_id(&split(500)));
        assert_eq!(case_id(&split(10)).len(), 16);
    }

    #[test]
    fn a_dangling_reference_is_an_error() {
        let file =
            ryu_part21::parse(sample(10).replace("('s',#14)", "('s',#99)").as_bytes()).unwrap();
        assert!(body_file(&file, 17, 16, &[15]).unwrap_err().contains("#99"));
    }
}
