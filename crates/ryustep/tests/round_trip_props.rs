//! Seeded sweeps, booleans and blends (corners among them), as the fuzz
//! targets make them from bytes, written to STEP and read back: the same
//! faces, edges and vertices on the same geometry (a revolution's frame
//! turned to its curve, a pipe as a NURBS), and from the first file
//! written from a body read back, the same bytes and the same body
//! however often they go round. A
//! body the operation made that doesn't mesh is its fault, not
//! writing's, and is counted apart. Release only: minutes in a debug
//! build.

use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::check::check;
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::sweep_bytes::sweep;
use ryustep::{LengthUnit, Product, ReadOptions, WriteModel, WriteOptions, read, write};

fn write_one(b: &Body) -> Result<Vec<u8>, String> {
    write(
        &WriteModel {
            products: vec![Product {
                id: "part".into(),
                name: "part".into(),
                bodies: vec![b.clone()],
                children: vec![],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Meter,
            ..WriteOptions::default()
        },
    )
    .map_err(|e| format!("write: {e}"))
}

fn read_one(bytes: &[u8]) -> Result<Body, String> {
    let m = read(
        bytes,
        &ReadOptions {
            key: OpKey(5),
            heal: Default::default(),
        },
    )
    .map_err(|e| format!("read: {e}"))?;
    if m.bodies.len() != 1 {
        return Err(format!("{} bodies", m.bodies.len()));
    }
    let b = m.bodies[0]
        .result
        .clone()
        .map_err(|e| format!("heal: {e}"))?
        .0;
    let faults = check(&b);
    if !faults.is_empty() {
        return Err(format!("check: {faults:?}"));
    }
    Ok(b)
}

/// Each face's surface (with its sense), each edge's curve and each
/// vertex's point, as the dump gives their bits, sorted; revolutions and
/// pipes by kind only (their frames and forms change).
fn geometry(b: &Body) -> Vec<String> {
    let dump = b.dump();
    let lines: Vec<&str> = dump.lines().collect();
    let entry = |kind: &str, id: &str| {
        let head = format!("{kind} {id} ");
        let x = lines
            .iter()
            .find_map(|l| l.strip_prefix(&head))
            .unwrap_or_default();
        if x.starts_with("revolution ") || x.starts_with("pipe ") || x.starts_with("nurbs ") {
            x.split(' ').next().unwrap_or_default().to_string()
        } else {
            x.to_string()
        }
    };
    let field = |l: &str, key: &str| {
        l.split(' ')
            .find_map(|w| w.strip_prefix(key))
            .unwrap_or_default()
            .to_string()
    };
    let mut out: Vec<String> = lines
        .iter()
        .filter_map(|l| {
            if l.starts_with("face ") {
                let s = entry("surface", &field(l, "surface="));
                // A pipe comes back a NURBS.
                let s = if s == "pipe" { "nurbs".to_string() } else { s };
                Some(format!("face {s} {}", field(l, "sense=")))
            } else if l.starts_with("edge ") {
                Some(format!("edge {}", entry("curve", &field(l, "curve="))))
            } else if l.starts_with("vertex ") {
                Some(format!("vertex {}", field(l, "point=")))
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

fn round_trip(b: &Body) -> Result<(), String> {
    let w1 = write_one(b)?;
    let b1 = read_one(&w1)?;
    if b1.counts() != b.counts() {
        return Err(format!("counts {:?} != {:?}", b1.counts(), b.counts()));
    }
    if geometry(&b1) != geometry(b) {
        return Err("geometry differs".into());
    }
    // From the first file written from a body read back, files and
    // bodies go round unchanged.
    let w2 = write_one(&b1)?;
    let b2 = read_one(&w2)?;
    let w3 = write_one(&b2)?;
    if w3 != w2 {
        return Err("bytes not a fixed point".into());
    }
    if read_one(&w3)?.dump() != b2.dump() {
        return Err("not a fixed point".into());
    }
    Ok(())
}

fn meshes(b: &Body) -> bool {
    use ryukei::mesh::{MeshParams, check_mesh, tessellate};
    tessellate(b, &MeshParams::new(1e-2, 0.5))
        .is_ok_and(|m| m.faults.is_empty() && check_mesh(b, &m).is_ok())
}

fn seeded(seed: u64, n: usize) -> Vec<u8> {
    let mut g = Lcg::new(seed);
    (0..n).map(|_| (g.next_u64() >> 56) as u8).collect()
}

/// Seeds that mesh but don't come back through STEP, each with the issue
/// that says why. One that comes back fails, so its line goes. Blend seed
/// 267 (#371) came off when the mesh estimate stopped refusing its ruled
/// chamfer for shell facing.
const KNOWN: &[(&str, u64, &str)] = &[];

fn corpus(name: &str, seeds: u64, make: impl Fn(&[u8]) -> Option<Body>) {
    let mut tried = 0;
    let mut unsound = 0;
    let mut failed = Vec::new();
    for seed in 0..seeds {
        // An empty body (a boolean that left nothing) writes no solid.
        let Some(b) = make(&seeded(seed, 96)).filter(|b| b.faces().next().is_some()) else {
            continue;
        };
        tried += 1;
        let known = KNOWN.iter().find(|k| k.0 == name && k.1 == seed);
        let r = round_trip(&b);
        if let (Some((_, _, issue)), Ok(())) = (known, &r) {
            failed.push(format!(
                "{name} seed {seed}: comes back now; take it off KNOWN ({issue})"
            ));
        }
        if let Err(why) = r {
            if known.is_some() {
                continue;
            }
            if meshes(&b) {
                failed.push(format!("{name} seed {seed}: {why}"));
            } else {
                unsound += 1;
            }
        }
    }
    assert!(tried > 0, "{name}: no bodies");
    assert!(
        failed.is_empty(),
        "{tried} tried, {unsound} unsound\n{}",
        failed.join("\n")
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn sweeps_round_trip() {
    corpus("sweep", 400, |d| sweep(d).map(|x| x.0));
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn booleans_round_trip() {
    corpus("boolean", 300, |d| {
        let case = boolean_case(d)?;
        boolean(OpKey(9), case.op, &case.target, &case.tools())
            .ok()
            .map(|x| x.0)
    });
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn blends_round_trip() {
    corpus("blend", 300, |d| {
        let case = blend_case(d)?;
        blend(OpKey(9), &case.body, &case.blends).ok().map(|x| x.0)
    });
}

/// Seeded sweeps with seeded paints (a colour each, an opacity on a
/// thousandth grid, every third face its own): written and read back,
/// the same styles as a reader composes them (a face's opacity its own or
/// its body's); the geometry instances the same as unpainted.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn styles_round_trip() {
    for seed in 0..64u64 {
        let Some(body) = sweep(&seeded(seed, 96))
            .map(|x| x.0)
            .filter(|b| b.faces().next().is_some())
        else {
            continue;
        };
        if ryustep::write(
            &ryustep::WriteModel {
                products: vec![ryustep::Product {
                    id: "p".into(),
                    bodies: vec![body.clone()],
                    ..Default::default()
                }],
            },
            &Default::default(),
        )
        .is_err()
        {
            continue;
        }
        let paint = |k: u64| ryustep::Paint {
            colour: [
                (k * 37 % 256) as u8,
                (k * 91 % 256) as u8,
                (k * 13 % 256) as u8,
            ],
            opacity: ((k % 1000) + 1) as f64 / 1000.0,
        };
        let faces: Vec<_> = body
            .faces()
            .map(|f| body.face(f).unwrap().name)
            .enumerate()
            .filter(|(i, _)| i.is_multiple_of(3))
            .map(|(i, n)| (n, paint(seed * 100 + i as u64)))
            .collect();
        let model = ryustep::WriteModel {
            products: vec![ryustep::Product {
                id: "p".into(),
                bodies: vec![body],
                ..Default::default()
            }],
        };
        let paints = ryustep::Paints {
            bodies: vec![ryustep::BodyPaint {
                product: 0,
                body: 0,
                base: Some(paint(seed)),
                faces: faces.clone(),
            }],
            occurrences: Vec::new(),
        };
        let opts = ryustep::WriteOptions::default();
        let w = ryustep::write_styled(&model, &[], &paints, &opts).unwrap();
        let plain = ryustep::part21::parse(&ryustep::write(&model, &opts).unwrap()).unwrap();
        let styled = ryustep::part21::parse(&w.bytes).unwrap();
        assert_eq!(
            format!("{:?}", &styled.instances[..plain.instances.len()]),
            format!("{:?}", plain.instances),
            "seed {seed}"
        );
        let o = ryustep::open(&w.bytes).unwrap();
        let s = o.body_styles(0);
        let base = paint(seed);
        assert_eq!(
            s.body,
            Some(ryustep::styles::Style {
                colour: Some(base.colour),
                opacity: Some(base.opacity).filter(|o| *o < 1.0),
            }),
            "seed {seed}"
        );
        // A face's style composes over its body's, field by field: its
        // opacity is its own or the body's.
        let want: Vec<_> = faces.iter().map(|(_, p)| (p.colour, p.opacity)).collect();
        let body_opacity = s.body.and_then(|b| b.opacity);
        let mut got: Vec<_> = s
            .faces
            .values()
            .map(|f| (f.colour.unwrap(), f.opacity.or(body_opacity).unwrap_or(1.0)))
            .collect();
        let mut want = want;
        got.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        want.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        assert_eq!(got, want, "seed {seed}");
    }
}

/// Seeded materials and attributes (design 448, plan 24.C), written over
/// a seeded body with styles, read back as written; the file with them
/// starts with every instance the file with the styles alone has.
#[test]
fn properties_round_trip() {
    use ryustep::properties::Value;
    use ryustep::{AttributeData, DataValue, MaterialData, PartData, ProductData};
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for seed in 0..64u64 {
        let body = ryukei::primitive::block(
            ryukei::name::OpKey(seed as u128 + 1),
            0.01 + (next() % 50) as f64 * 1e-3,
            0.02,
            0.03,
            &ryukei::geom::Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        let model = ryustep::WriteModel {
            products: vec![ryustep::Product {
                id: format!("p{seed}"),
                name: format!("p{seed}"),
                bodies: vec![body],
                ..Default::default()
            }],
        };
        let n = 1 + (next() % 6) as usize;
        let attributes: Vec<AttributeData> = (0..n)
            .map(|i| {
                let value = match next() % 5 {
                    0 => DataValue::Text(format!("t{}", next() % 1000)),
                    1 => DataValue::Integer((next() % 100_000) as i64 - 50_000),
                    2 => DataValue::Number((next() % 1_000_000) as f64 / 997.0),
                    3 => DataValue::Boolean(next() % 2 == 0),
                    _ => DataValue::Length((next() % 10_000) as f64 * 1e-4),
                };
                AttributeData {
                    name: format!("a{i}"),
                    value,
                }
            })
            .collect();
        let density = (next() % 2 == 0).then(|| 1000.0 + (next() % 20_000) as f64 / 2.0);
        let data = PartData {
            products: vec![ProductData {
                product: 0,
                material: Some(MaterialData {
                    name: format!("m{}", next() % 9),
                    density,
                }),
                attributes: attributes.clone(),
            }],
        };
        let paints = ryustep::Paints {
            bodies: vec![ryustep::BodyPaint {
                product: 0,
                body: 0,
                base: Some(ryustep::Paint {
                    colour: [200, 10, 10],
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
        let styled = ryustep::write_styled(&model, &[], &paints, &opts)
            .unwrap()
            .bytes;
        let both = ryustep::write_with_data(&model, &[], &paints, &data, &opts)
            .unwrap()
            .bytes;
        let (a, b) = (
            ryustep::part21::parse(&styled).unwrap(),
            ryustep::part21::parse(&both).unwrap(),
        );
        assert_eq!(
            format!("{:?}", &b.instances[..a.instances.len()]),
            format!("{:?}", a.instances),
            "seed {seed}: the styled file's instances first, unmoved"
        );
        let o = ryustep::open(&both).unwrap();
        let p = &o.properties().products[&0];
        let m = p.material.as_ref().unwrap();
        assert_eq!(m.density, density, "seed {seed}");
        assert_eq!(p.attributes.len(), attributes.len(), "seed {seed}");
        for (got, want) in p.attributes.iter().zip(&attributes) {
            let ok = match (&got.value, &want.value) {
                (Value::Text(a), DataValue::Text(b)) => a == b,
                (Value::Integer(a), DataValue::Integer(b)) => a == b,
                (Value::Number(a), DataValue::Number(b)) => a == b,
                (Value::Boolean(a), DataValue::Boolean(b)) => a == b,
                (Value::Measure { value, unit }, DataValue::Length(m)) => {
                    unit.name == "mm" && (value * 1e-3 - m).abs() <= 1e-15
                }
                _ => false,
            };
            assert!(ok, "seed {seed}: {got:?} against {want:?}");
        }
    }
}
