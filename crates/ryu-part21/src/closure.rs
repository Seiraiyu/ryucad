//! A representation's closure cut out of a file: its B-rep items and
//! every instance they reach, numbered afresh, so the same body gives the
//! same instances whatever file held it. step-hunt writes them as a body
//! file; ryustep writes them into another file (a reference import's
//! own representation, design 364).

use std::collections::HashMap;

use crate::{File, Instance, Param, Part, Record};

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

/// `record` with every instance it names renumbered by `ids`, which must
/// hold each.
pub fn remap_record(record: &Record, ids: &HashMap<u64, u64>) -> Record {
    match record {
        Record::Simple(p) => Record::Simple(remap_part(p, ids)),
        Record::Complex(parts) => {
            Record::Complex(parts.iter().map(|p| remap_part(p, ids)).collect())
        }
    }
}

/// The instances representation `representation` (in `context`) holding
/// `items` reaches in `file`, renumbered from 1 in the order a breadth-
/// first walk from the root first meets them: the root first, named
/// `name`, holding only `items`, in `context` (a complex representation
/// becomes `SHAPE_REPRESENTATION`). An error names an instance the file
/// lacks.
pub fn cut(
    file: &File,
    representation: u64,
    context: u64,
    items: &[u64],
    name: &str,
) -> Result<Vec<Instance>, String> {
    let entity: Box<str> = match file.get(representation).map(|i| &i.record) {
        Some(Record::Simple(p)) => p.name.clone(),
        Some(Record::Complex(_)) => "SHAPE_REPRESENTATION".into(),
        None => return Err(format!("no instance #{representation}")),
    };
    let root = Part {
        name: entity,
        params: vec![
            Param::Str(name.into()),
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
        let record = remap_record(&file.get(*old).expect("found above").record, &ids);
        instances.push(Instance {
            id: k as u64 + 2,
            record,
        });
    }
    Ok(instances)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_holds_what_its_root_reaches_numbered_breadth_first() {
        let text = b"ISO-10303-21;HEADER;FILE_DESCRIPTION((''),'2;1');FILE_NAME('','',(''),(''),'','','');FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;DATA;
#5=CARTESIAN_POINT('',(0.,0.,0.));#9=VERTEX_POINT('',#5);#3=OTHER('x');
#7=GEOMETRIC_REPRESENTATION_CONTEXT(3);#2=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#9),#7);ENDSEC;END-ISO-10303-21;";
        let file = crate::parse(text).unwrap();
        let cut = cut(&file, 2, 7, &[9], "body").unwrap();
        let ids: Vec<u64> = cut.iter().map(|i| i.id).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
        // The root first, named and holding only the items, in its context.
        let Record::Simple(root) = &cut[0].record else {
            panic!()
        };
        assert_eq!(&*root.name, "ADVANCED_BREP_SHAPE_REPRESENTATION");
        assert_eq!(root.params[0], Param::Str("body".into()));
        assert_eq!(root.params[1], Param::List(vec![Param::Ref(2)]));
        assert_eq!(root.params[2], Param::Ref(3));
        // #3 isn't reached; #9 is 2, #7 is 3, #5 is 4.
        assert!(
            cut.iter()
                .all(|i| !matches!(&i.record, Record::Simple(p) if &*p.name == "OTHER"))
        );
        let Record::Simple(v) = &cut[1].record else {
            panic!()
        };
        assert_eq!(v.params[1], Param::Ref(4));
        // A name the file lacks is an error naming it.
        assert!(
            super::cut(&file, 2, 7, &[99], "")
                .unwrap_err()
                .contains("#99")
        );
    }
}
