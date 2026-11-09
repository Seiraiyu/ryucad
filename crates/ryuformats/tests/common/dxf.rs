//! A reader of the DXF subset `ryuformats::dxf` writes: group code and
//! value pairs, sections, the layer and block record tables, and the
//! entities. Panics on anything malformed, and checks what an R2000
//! reader relies on: every handle unique hexadecimal, `$HANDSEED` past
//! them all, every entity owned by `*Model_Space`'s block record, and the
//! file ending in `EOF`.

use std::collections::{BTreeMap, BTreeSet};

/// One group: its code and its value as written.
pub type Group = (u16, String);

#[derive(Clone, Debug)]
pub struct Layer {
    pub name: String,
    pub colour: i32,
    pub linetype: String,
}

/// An entity: its type, layer, handle, owner and every group after its
/// `0`, in order.
#[derive(Clone, Debug)]
pub struct Entity {
    pub kind: String,
    pub layer: String,
    pub handle: String,
    pub owner: String,
    pub groups: Vec<Group>,
}

impl Entity {
    /// The first value of `code` as a number.
    pub fn f(&self, code: u16) -> f64 {
        self.all(code)[0]
    }

    /// Every value of `code` as numbers, in order.
    pub fn all(&self, code: u16) -> Vec<f64> {
        self.groups
            .iter()
            .filter(|(c, _)| *c == code)
            .map(|(_, v)| v.trim().parse().unwrap_or_else(|_| panic!("{code}: {v}")))
            .collect()
    }

    /// The first value of `code` as text.
    pub fn text(&self, code: u16) -> &str {
        &self.groups.iter().find(|(c, _)| *c == code).unwrap().1
    }
}

#[derive(Clone, Debug, Default)]
pub struct Dxf {
    /// Header variables by name, each its groups.
    pub header: BTreeMap<String, Vec<Group>>,
    pub layers: Vec<Layer>,
    pub linetypes: Vec<String>,
    pub entities: Vec<Entity>,
}

impl Dxf {
    /// A header variable's first value.
    pub fn var(&self, name: &str) -> &str {
        &self.header[name][0].1
    }

    pub fn on(&self, layer: &str) -> Vec<&Entity> {
        self.entities.iter().filter(|e| e.layer == layer).collect()
    }
}

/// Reads `bytes`, checking as the module comment says.
pub fn read(bytes: &[u8]) -> Dxf {
    let text = std::str::from_utf8(bytes).expect("ASCII");
    assert!(!text.contains('\r'), "lines end in LF alone");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len() % 2, 0, "groups are pairs of lines");
    let groups: Vec<Group> = lines
        .chunks(2)
        .map(|p| (p[0].trim().parse().expect("a group code"), p[1].to_owned()))
        .collect();
    assert_eq!(
        groups.last().unwrap(),
        &(0, "EOF".to_owned()),
        "ends in EOF"
    );
    let mut out = Dxf::default();
    let mut handles = BTreeSet::new();
    let mut model_space = None;
    // Split into records at each 0.
    let mut records: Vec<Vec<Group>> = Vec::new();
    for g in groups {
        if g.0 == 0 {
            records.push(vec![g]);
        } else {
            records.last_mut().expect("starts with a 0").push(g);
        }
    }
    let mut section = String::new();
    let mut table = String::new();
    for r in &records {
        let kind = r[0].1.as_str();
        let first = |code: u16| r.iter().find(|g| g.0 == code).map(|g| g.1.clone());
        // The header's `5` is `$HANDSEED`'s value, not a handle.
        let header = kind == "SECTION" && first(2).as_deref() == Some("HEADER");
        if let Some(h) = first(5).or_else(|| first(105)).filter(|_| !header) {
            assert!(u64::from_str_radix(&h, 16).is_ok(), "handle {h}");
            assert!(handles.insert(h.clone()), "handle {h} twice");
        }
        match kind {
            "SECTION" => section = first(2).unwrap(),
            "ENDSEC" => section.clear(),
            "TABLE" => table = first(2).unwrap(),
            "ENDTAB" => table.clear(),
            _ => {}
        }
        if section == "HEADER" && kind == "SECTION" {
            let mut name = String::new();
            for g in &r[2..] {
                if g.0 == 9 {
                    name = g.1.clone();
                    out.header.insert(name.clone(), Vec::new());
                } else {
                    out.header.get_mut(&name).unwrap().push(g.clone());
                }
            }
        }
        if section == "TABLES" && kind == "LAYER" {
            out.layers.push(Layer {
                name: first(2).unwrap(),
                colour: first(62).unwrap().trim().parse().unwrap(),
                linetype: first(6).unwrap(),
            });
        }
        if section == "TABLES" && kind == "LTYPE" {
            out.linetypes.push(first(2).unwrap());
        }
        if section == "TABLES"
            && kind == "BLOCK_RECORD"
            && first(2).as_deref() == Some("*Model_Space")
        {
            model_space = first(5);
        }
        if section == "ENTITIES" && kind != "SECTION" && kind != "ENDSEC" {
            out.entities.push(Entity {
                kind: kind.to_owned(),
                layer: first(8).expect("a layer"),
                handle: first(5).expect("a handle"),
                owner: first(330).expect("an owner"),
                groups: r[1..].to_vec(),
            });
        }
    }
    let model_space = model_space.expect("a *Model_Space block record");
    for e in &out.entities {
        assert_eq!(
            e.owner, model_space,
            "{} {} is owned by model space",
            e.kind, e.handle
        );
    }
    let seed = u64::from_str_radix(out.var("$HANDSEED"), 16).unwrap();
    let most = handles
        .iter()
        .map(|h| u64::from_str_radix(h, 16).unwrap())
        .max()
        .unwrap();
    assert!(
        seed > most,
        "$HANDSEED {seed:X} is past every handle ({most:X})"
    );
    out
}
