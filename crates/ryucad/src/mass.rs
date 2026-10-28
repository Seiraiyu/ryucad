//! `mass`: a solid's, a part's or an assembly's mass properties, from
//! ryukei's [`ryukei::measure::mass`], each solid at its part's material's
//! density.
//!
//! A part's density is its PDM `material` property's entry in the
//! project's material library (`materials.json`), else the library's
//! default. A part with neither still gives its volume, area and centre;
//! mass and inertia are left out and the answer warns `material_missing`.
//! An assembly adds up every occurrence at its pose, each part at its own
//! density: masses and volumes add, centres average by mass (by volume
//! where a density is missing), and inertias move to the common centre by
//! the parallel axis theorem.

use crate::doc::files::PartKind;
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::measure::{Kind, Located, failed, locate, number, point, value};
use crate::session::Session;
use ryukei::geom::{Point3, Vector3};
use ryukei::measure::{self as kernel, Entity, MassOptions, MassProperties, Value as Measured};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Mass properties already worked out, by body hash, density and
/// tolerance.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    found: BTreeMap<String, Arc<MassProperties>>,
}

impl Cache {
    pub fn len(&self) -> usize {
        self.found.len()
    }

    pub fn is_empty(&self) -> bool {
        self.found.is_empty()
    }
}

fn key(hash: &str, density: f64, tolerance: f64) -> String {
    format!(
        "{hash} {:016x} {:016x}",
        density.to_bits(),
        tolerance.to_bits()
    )
}

/// The tolerance `mass` uses when none is given.
pub const DEFAULT_TOLERANCE: f64 = 1e-9;

/// One solid's share: its part, its properties at the density (1 where
/// it has none), and where it is.
struct Share {
    part: Id,
    density: Option<f64>,
    props: Arc<MassProperties>,
    pose: ryukei::geom::Rigid,
}

/// Each part's material by name, and its density where the library has
/// it.
type Densities = BTreeMap<Id, (Option<String>, Option<f64>)>;

/// Each part's density: its material's in the library, if any.
fn densities(session: &Session, parts: impl IntoIterator<Item = Id>) -> Result<Densities> {
    let lib = crate::materials::library(session)?;
    Ok(parts
        .into_iter()
        .map(|p| {
            let named = crate::materials::material_of(session, &p);
            let found = lib.of(named.as_deref());
            let material = found.map(|(n, _)| n.to_owned()).or(named);
            let density = found.map(|(_, m)| m.density.si());
            (p, (material, density))
        })
        .collect())
}

fn mat3(r: &ryukei::geom::Rigid) -> [[f64; 3]; 3] {
    r.rotation_matrix()
}

/// `R I Rᵀ`.
fn rotated(i: &[[f64; 3]; 3], r: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (a, row) in out.iter_mut().enumerate() {
        for (b, x) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for k in 0..3 {
                for l in 0..3 {
                    s += r[a][k] * i[k][l] * r[b][l];
                }
            }
            *x = s;
        }
    }
    out
}

/// `mass` itself: `of` is a part or assembly ID, a solid
/// (`<part>/<solid>`) or an occurrence (`<asm>/<path>`).
pub fn run(
    session: &mut Session,
    of: &str,
    tolerance: Option<f64>,
    rev: Option<&str>,
) -> Result<Value> {
    let tolerance = tolerance.unwrap_or(DEFAULT_TOLERANCE);
    if !(tolerance > 0.0 && tolerance < 1e-2) {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "tolerance {tolerance} is out of range: give a relative tolerance in (0, 0.01)"
            ),
        )
        .with("tolerance", tolerance));
    }
    let (commit, s) = session.at(rev)?;
    let unit = s.project().manifest.default_unit;
    let (kind, located): (&str, Vec<Located>) = if of.contains('/') {
        let l = locate(s, of)?;
        if l.iter().any(|l| l.entity != Entity::Body) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{of} is a face, edge or vertex: mass properties are a solid's, a part's or an assembly's"),
            )
            .with("of", of));
        }
        (l[0].kind, l)
    } else {
        let id = crate::server::part_id(of)?;
        let p = s.project().part(&id)?;
        let kind = match crate::server::part_kind(p) {
            PartKind::Assembly => "assembly",
            _ => "part",
        };
        let model = s.model(&id, &crate::doc::assembly::Rev::Working)?;
        let l: Vec<Located> = model
            .occurrences
            .iter()
            .flat_map(|o| {
                o.solids.iter().map(|sd| Located {
                    part: o.part.clone(),
                    solid: sd.founder.clone(),
                    built: sd.built.clone(),
                    entity: Entity::Body,
                    pose: o.pose,
                    kind: "solid",
                })
            })
            .collect();
        if l.is_empty() {
            return Err(Error::new(Code::NotFound, format!("{of} shows no solid")).with("of", of));
        }
        (kind, l)
    };
    let dens = densities(s, located.iter().map(|l| l.part.clone()))?;
    // Work out what the cache lacks in parallel, each body once.
    let wanted: Vec<(String, &Located, f64)> = located
        .iter()
        .map(|l| {
            let d = dens[&l.part].1.unwrap_or(1.0);
            (key(&l.built.hash, d, tolerance), l, d)
        })
        .collect();
    let mut missing: Vec<&(String, &Located, f64)> = wanted
        .iter()
        .filter(|(k, _, _)| !s.mass.found.contains_key(k))
        .collect();
    missing.sort_by(|a, b| a.0.cmp(&b.0));
    missing.dedup_by(|a, b| a.0 == b.0);
    let opts = MassOptions {
        tolerance,
        ..MassOptions::default()
    };
    let refs = [of];
    let made: Vec<(String, std::result::Result<MassProperties, ryukei::Error>)> = {
        use rayon::prelude::*;
        missing
            .par_iter()
            .map(|(k, l, d)| (k.clone(), kernel::mass(&l.built.body, *d, &opts)))
            .collect()
    };
    for (k, r) in made {
        let m = r.map_err(|e| failed(e, &refs))?;
        s.mass.found.insert(k, Arc::new(m));
    }
    let shares: Vec<Share> = wanted
        .iter()
        .map(|(k, l, _)| Share {
            part: l.part.clone(),
            density: dens[&l.part].1,
            props: s.mass.found[k].clone(),
            pose: l.pose,
        })
        .collect();
    let mut v = combine(&shares, unit);
    v["of"] = json!(of);
    v["kind"] = json!(kind);
    v["solids"] = json!(shares.len());
    // Per part, in ID order.
    let mut rows: BTreeMap<&Id, (usize, Measured, Measured)> = BTreeMap::new();
    for sh in &shares {
        let e = rows
            .entry(&sh.part)
            .or_insert((0, Measured::Exact(0.0), Measured::Exact(0.0)));
        e.0 += 1;
        e.1 = add(e.1, sh.props.volume);
        e.2 = add(e.2, sh.props.mass);
    }
    let mut warnings = Vec::new();
    v["parts"] = rows
        .into_iter()
        .map(|(p, (n, vol, m))| {
            let (material, density) = &dens[p];
            let mut row = json!({"part": p, "solids": n, "volume": value(vol, Kind::Volume, unit)});
            if let Some(name) = material {
                row["material"] = json!(name);
            }
            match density {
                Some(d) => {
                    row["density"] = json!({"kg_m3": d});
                    row["mass"] = value(m, Kind::Mass, unit);
                }
                None => warnings.push(json!({
                    "code": "material_missing",
                    "part": p,
                    "message": match material {
                        Some(name) => format!("part {p}'s material {name:?} isn't in materials.json: no mass or inertia for it"),
                        None => format!("part {p} has no material and materials.json no default: no mass or inertia for it"),
                    },
                })),
            }
            row
        })
        .collect();
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    if let Some(c) = commit {
        v["commit"] = json!(c);
    }
    v["unit"] = json!(unit.as_str());
    Ok(v)
}

fn add(a: Measured, b: Measured) -> Measured {
    match (a, b) {
        (Measured::Exact(x), Measured::Exact(y)) => Measured::Exact(x + y),
        _ => Measured::Bounded {
            value: a.value() + b.value(),
            bound: a.bound() + b.bound(),
        },
    }
}

/// The shares added up in the frame of what was asked for.
fn combine(shares: &[Share], unit: crate::units::Unit) -> Value {
    let all_dense = shares.iter().all(|s| s.density.is_some());
    let weight = |s: &Share| {
        if all_dense {
            s.props.mass.value()
        } else {
            s.props.volume.value()
        }
    };
    let (mut volume, mut area, mut mass) = (
        Measured::Exact(0.0),
        Measured::Exact(0.0),
        Measured::Exact(0.0),
    );
    let mut total = 0.0;
    let mut moment = Vector3::ZERO;
    let centres: Vec<Point3> = shares
        .iter()
        .map(|s| s.pose.apply_point(s.props.centre))
        .collect();
    for (s, c) in shares.iter().zip(&centres) {
        volume = add(volume, s.props.volume);
        area = add(area, s.props.area);
        mass = add(mass, s.props.mass);
        total += weight(s);
        moment += c.to_vector() * weight(s);
    }
    let centre = Point3::ORIGIN + moment * (1.0 / total);
    let mut centre_bound = 0.0;
    let mut inertia = [[0.0; 3]; 3];
    let mut inertia_bound = 0.0;
    for (s, c) in shares.iter().zip(&centres) {
        let w = weight(s);
        let r = *c - centre;
        let wb = if all_dense {
            s.props.mass.bound()
        } else {
            s.props.volume.bound()
        };
        centre_bound += (w * s.props.centre_bound + wb * r.length()) / total;
        let i = rotated(&s.props.inertia, &mat3(&s.pose));
        let rr = [r.x, r.y, r.z];
        let m = s.props.mass.value();
        for a in 0..3 {
            for b in 0..3 {
                let delta = if a == b { r.length_squared() } else { 0.0 };
                inertia[a][b] += i[a][b] + m * (delta - rr[a] * rr[b]);
            }
        }
        inertia_bound += s.props.inertia_bound
            + s.props.mass.bound() * r.length_squared()
            + 2.0 * m * r.length() * s.props.centre_bound;
    }
    let mut v = json!({
        "volume": value(volume, Kind::Volume, unit),
        "area": value(area, Kind::Area, unit),
        "centre": point(centre, unit),
        "centre_bound_m": centre_bound,
    });
    if all_dense {
        v["mass"] = value(mass, Kind::Mass, unit);
        let (moments, axes) = kernel::principal(&inertia);
        let f = unit.factor() * unit.factor();
        v["inertia"] = json!({
            "kg_m2": inertia,
            "bound_kg_m2": inertia_bound,
            "value": inertia.map(|row| row.map(|x| number(x / f))),
            "unit": format!("kg {}^2", unit.as_str()),
            "about": "centre",
            "principal": moments
                .iter()
                .zip(axes)
                .map(|(m, a)| json!({
                    "moment": value(Measured::Bounded { value: *m, bound: inertia_bound }, Kind::Inertia, unit),
                    "axis": [a.get().x, a.get().y, a.get().z],
                }))
                .collect::<Vec<_>>(),
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use crate::assembly::tests::{place_in, project};
    use crate::error::Code;
    use crate::server::tests::call;
    use serde_json::json;

    fn steel(s: &mut crate::session::Session) {
        call(
            s,
            "materials.set",
            json!({"name": "steel", "density": "7850 kg/m^3", "default": true}),
        )
        .unwrap();
    }

    #[test]
    fn a_plate_weighs_its_volume_times_its_density() {
        let (_dir, mut s) = project();
        let v = call(&mut s, "mass", json!({"of": "plate"})).unwrap();
        // No library: geometry only, and a warning.
        assert!(v.get("mass").is_none() && v.get("inertia").is_none(), "{v}");
        assert_eq!(v["warnings"][0]["code"], "material_missing");
        assert_eq!(v["volume"]["value"], "25000 mm^3");
        steel(&mut s);
        let v = call(&mut s, "mass", json!({"of": "plate"})).unwrap();
        let kg = 7850.0 * 25e-6;
        assert!(
            (v["mass"]["kg"].as_f64().unwrap() - kg).abs() < 1e-12,
            "{v}"
        );
        assert!(v.get("warnings").is_none(), "{v}");
        let c = &v["centre"]["m"];
        assert!((c[0].as_f64().unwrap() - 0.05).abs() < 1e-12, "{v}");
        // About its centre: m (b² + c²) / 12 about x.
        let ixx = kg * (0.05f64 * 0.05 + 0.005 * 0.005) / 12.0;
        let got = v["inertia"]["kg_m2"][0][0].as_f64().unwrap();
        assert!(
            (got - ixx).abs() < 1e-12 * ixx.max(1.0),
            "{got} against {ixx}"
        );
        assert_eq!(v["parts"][0]["material"], "steel");
        // The same again comes from the cache.
        let n = s.mass.len();
        assert_eq!(
            call(&mut s, "mass", json!({"of": "plate/slab"})).unwrap()["mass"],
            v["mass"]
        );
        assert_eq!(s.mass.len(), n);
    }

    #[test]
    fn an_assembly_adds_its_occurrences_where_they_are() {
        let (dir, mut s) = project();
        steel(&mut s);
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0, 0.0, 0.0],
        );
        place_in(
            dir.path(),
            "frame",
            "p2",
            "plate",
            json!("working"),
            [0.0, 0.0, 0.1],
        );
        s.refresh().unwrap();
        let one = call(&mut s, "mass", json!({"of": "plate"})).unwrap();
        let both = call(&mut s, "mass", json!({"of": "frame"})).unwrap();
        let kg = one["mass"]["kg"].as_f64().unwrap();
        assert!((both["mass"]["kg"].as_f64().unwrap() - 2.0 * kg).abs() < 1e-12);
        let z = both["centre"]["m"][2].as_f64().unwrap();
        assert!((z - (0.0025 + 0.05)).abs() < 1e-12, "{both}");
        // Two plates 100 mm apart: each 50 mm from the centre.
        let ixx = 2.0 * one["inertia"]["kg_m2"][0][0].as_f64().unwrap() + 2.0 * kg * 0.05 * 0.05;
        let got = both["inertia"]["kg_m2"][0][0].as_f64().unwrap();
        assert!((got - ixx).abs() < 1e-12, "{got} against {ixx}");
        assert_eq!(both["parts"][0]["solids"], 2);
        assert_eq!(both["kind"], "assembly");
        let occ = call(&mut s, "mass", json!({"of": "frame/p2"})).unwrap();
        assert_eq!(occ["kind"], "occurrence");
    }

    #[test]
    fn refusals() {
        let (_dir, mut s) = project();
        let body = call(&mut s, "inspect", json!({"part": "plate"})).unwrap()["bodies"][0].clone();
        let face = format!("plate/slab/{}", body["faces"][0]["name"].as_str().unwrap());
        assert_eq!(
            call(&mut s, "mass", json!({"of": face})).unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(
            call(&mut s, "mass", json!({"of": "plate", "tolerance": 0.5}))
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
        assert_eq!(
            call(&mut s, "mass", json!({"of": "nope"}))
                .unwrap_err()
                .code,
            Code::NotFound
        );
        assert_eq!(
            call(&mut s, "mass", json!({"of": "frame"}))
                .unwrap_err()
                .code,
            Code::NotFound
        );
    }
}
