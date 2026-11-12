//! Splits against exact volumes, through the protocol: a box split by a
//! datum plane into two solids whose volumes sum to the box's; a trim; a
//! U-shaped part into a solid of two lumps; each side filleted after;
//! names kept across a move of the plane; a plane missing the target.

mod common;

use common::{must, project};
use ryucad::doc::id::Id;
use ryucad::error::Code;
use ryucad::session::Session;
use serde_json::{Value, json};

fn add(s: &mut Session, f: Value) -> Value {
    let mut f = f;
    f["part"] = json!("p");
    must(s, "feature.add", f)
}

/// The volume in mm³ of solid `solid` as feature `f` left it, after the
/// kernel's checks, meshed finely.
fn volume(s: &mut Session, f: &str, solid: &str) -> f64 {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed");
    let out = out.as_ref().unwrap_or_else(|e| panic!("{f}: {e}"));
    let b = &out
        .solid(&Id::parse(solid, "feature").unwrap())
        .expect("the solid")
        .body;
    assert!(ryukei::check::check(b).is_empty(), "{f}");
    assert_eq!(ryukei::check::check_intersections(b), [], "{f}");
    let m = ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(2e-7, 0.05)).unwrap();
    assert!(m.faults.is_empty(), "{f}: {:?}", m.faults);
    ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9
}

/// `got` within a part in `rel` of `want`.
fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want,
        "{got} mm³ against {want} mm³"
    );
}

/// Why feature `f` failed in the part's rebuild.
fn failure(s: &mut Session, f: &str) -> ryucad::error::Error {
    let part = Id::parse("p", "part").unwrap();
    let r = s.rebuilt(&part).unwrap();
    let (_, out) = r
        .features
        .iter()
        .find(|(i, _)| i.as_str() == f)
        .expect("listed");
    out.as_ref()
        .err()
        .unwrap_or_else(|| panic!("{f} built"))
        .clone()
}

/// The model's solids, by founder.
fn solids(s: &mut Session) -> Vec<String> {
    let r = must(s, "rebuild", json!({"part": "p"}));
    r["parts"][0]["solids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["solid"].as_str().unwrap().to_owned())
        .collect()
}

/// [`project`] with a box `h` 40 × 30 × 20 mm and a datum plane `parting`
/// `z` mm up from xy.
fn housing(z: &str) -> (tempfile::TempDir, Session) {
    let (dir, mut s) = project();
    add(
        &mut s,
        json!({"type": "box", "id": "h", "params": {"x": 40, "y": 30, "z": 20}}),
    );
    add(
        &mut s,
        json!({"type": "plane", "id": "parting", "by": "offset", "from": "xy",
                       "distance": z}),
    );
    (dir, s)
}

#[test]
fn a_box_splits_into_two_solids_whose_volumes_sum_to_it() {
    let (_dir, mut s) = housing("8 mm");
    add(
        &mut s,
        json!({"type": "split", "id": "s", "target": "h", "plane": "p/parting"}),
    );
    assert_eq!(solids(&mut s), ["h", "s"]);
    let (behind, front) = (volume(&mut s, "s", "h"), volume(&mut s, "s", "s"));
    near(behind, 40.0 * 30.0 * 8.0, 1e-9);
    near(front, 40.0 * 30.0 * 12.0, 1e-9);
    near(behind + front, 24000.0, 1e-9);
    // Kept behind only, a trim: one solid.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "s", "keep": "behind"}),
    );
    assert_eq!(solids(&mut s), ["h"]);
    near(volume(&mut s, "s", "h"), 9600.0, 1e-9);
}

#[test]
fn a_u_shaped_part_splits_into_a_solid_of_two_lumps() {
    let (_dir, mut s) = housing("10 mm");
    // A slot 20 wide and 15 deep down the middle of the top: a U.
    add(
        &mut s,
        json!({"type": "box", "id": "slot", "operation": "cut", "targets": ["h"],
                       "params": {"x": 20, "y": 40, "z": 20},
                       "placement": {"origin": [10, -5, 5]}}),
    );
    add(
        &mut s,
        json!({"type": "split", "id": "s", "target": "h", "plane": "p/parting"}),
    );
    let r = must(&mut s, "rebuild", json!({"part": "p"}));
    let sp = r["parts"][0]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "s")
        .unwrap()
        .clone();
    assert_eq!(
        sp["split"]["sides"],
        json!([{"side": "behind", "solid": "h", "lumps": 1, "kind": "solid"},
               {"side": "front", "solid": "s", "lumps": 2, "kind": "solid"}]),
        "{sp}"
    );
    // The two prongs, 10 × 30 × 10 each.
    near(volume(&mut s, "s", "s"), 6000.0, 1e-9);
}

#[test]
fn each_side_takes_a_fillet_and_the_cut_faces_keep_their_names_as_the_plane_moves() {
    let (_dir, mut s) = housing("8 mm");
    add(
        &mut s,
        json!({"type": "split", "id": "s", "target": "h", "plane": "p/parting"}),
    );
    // Each side's cut face, by its origin: `inspect --feature` gives each
    // solid the feature leaves, its faces with their origins.
    let cut = |s: &mut Session, solid: &str, side: &str| -> String {
        let bodies = must(s, "inspect", json!({"part": "p", "feature": "s"}))["bodies"].clone();
        let b = bodies
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["solid"] == solid)
            .cloned()
            .expect("the solid");
        let f = b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["origin"] == json!({"feature": "s", "side": side}))
            .unwrap_or_else(|| panic!("no {side} cut face"));
        f["name"].as_str().unwrap().to_owned()
    };
    let (back, front) = (cut(&mut s, "h", "behind"), cut(&mut s, "s", "front"));
    // Moved up 2 mm, the cut faces keep their names.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "p", "id": "parting", "distance": "10 mm"}),
    );
    assert_eq!(
        (cut(&mut s, "h", "behind"), cut(&mut s, "s", "front")),
        (back.clone(), front)
    );
    // Each side filleted after, along an edge of its cut face.
    for (solid, z) in [("h", 10.0), ("s", 10.0)] {
        let b = must(&mut s, "inspect", json!({"part": "p", "feature": "s"}))["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["solid"] == solid)
            .cloned()
            .unwrap();
        let e = b["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| {
                let p = |v: &str| {
                    b["vertices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|x| x["name"] == v)
                        .unwrap()["point"]
                        .clone()
                };
                let (a, c) = (
                    p(e["start"].as_str().unwrap()),
                    p(e["end"].as_str().unwrap()),
                );
                (a[2].as_f64().unwrap() - z * 1e-3).abs() < 1e-12
                    && (c[2].as_f64().unwrap() - z * 1e-3).abs() < 1e-12
                    && a[1].as_f64().unwrap().abs() < 1e-12
                    && c[1].as_f64().unwrap().abs() < 1e-12
            })
            .expect("the cut face's front edge");
        add(
            &mut s,
            json!({"type": "fillet", "id": format!("r{solid}"), "radius": "2 mm",
                           "edges": [format!("p/{solid}/{}", e["name"].as_str().unwrap())]}),
        );
    }
    let r = must(&mut s, "rebuild", json!({"part": "p"}));
    assert_eq!(r["parts"][0]["failed"], 0, "{r}");
    // Each loses a quarter-round strip, (4 - π) mm² along 40 mm.
    let strip = (4.0 - core::f64::consts::PI) * 40.0;
    // The plane now at 10 mm: each side 40 × 30 × 10.
    near(volume(&mut s, "rh", "h"), 12000.0 - strip, 1e-6);
    near(volume(&mut s, "rs", "s"), 12000.0 - strip, 1e-6);
}

#[test]
fn a_plane_that_misses_says_which_side_is_empty() {
    let (_dir, mut s) = housing("30 mm");
    add(
        &mut s,
        json!({"type": "split", "id": "s", "target": "h", "plane": "p/parting"}),
    );
    let e = failure(&mut s, "s");
    assert_eq!(
        (e.code, e.details["side"].as_str()),
        (Code::OperationNoEffect, Some("front")),
        "{e}"
    );
}
