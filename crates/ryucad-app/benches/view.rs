//! The window's hot paths on a 100-feature part: decoding the meshes it is
//! sent, preparing them for picking, and picking under the mouse, which
//! runs on every mouse move.

use divan::Bencher;
use ryucad::proto::Request;
use ryucad::server::{handle, init, meshes};
use ryucad::session::Session;
use ryucad::view::{self, StandardView};
use ryucad_app::core::model::Model;
use ryucad_app::core::nav::Projector;
use ryucad_app::core::pick::Picker;
use serde_json::json;
use std::sync::Arc;

fn main() {
    divan::main();
}

/// The `ryucad.mesh.2` bytes of a part of 100 features, all five types,
/// at normal display quality: what the window reads.
fn encoded() -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    handle(
        &mut s,
        Request::PartCreate {
            part: "p".into(),
            kind: None,
        },
    )
    .unwrap();
    let kinds = [
        ("box", json!({"x": 10, "y": 20, "z": 30})),
        ("cylinder", json!({"radius": 5, "height": 10})),
        ("cone", json!({"radius": 5, "height": 10, "top_radius": 2})),
        ("sphere", json!({"radius": 5})),
        ("torus", json!({"major_radius": 10, "minor_radius": 2})),
    ];
    for i in 0..100 {
        let (ty, params) = &kinds[i % 5];
        let req = Request::from_rpc(
            "feature.add",
            Some(json!({"part": "p", "type": ty, "params": params,
                        "placement": {"origin": [i * 25, 0, 0]}})),
        )
        .unwrap();
        handle(&mut s, req).unwrap();
    }
    let part = ryucad::doc::id::Id::parse("p", "part").unwrap();
    let bodies = meshes(&mut s, &part, None, ryucad::mesh::Quality::Normal).unwrap();
    ryucad::mesh::encode_compact(&bodies)
}

fn model(bytes: &[u8]) -> Model {
    let mut m = Model::default();
    for b in ryucad::mesh::decode(bytes).unwrap() {
        m.meshes
            .insert(("p".into(), b.feature.clone()), Arc::new(b));
    }
    m
}

#[divan::bench]
fn decode_100_bodies(b: Bencher) {
    let bytes = encoded();
    b.bench(|| ryucad::mesh::decode(divan::black_box(&bytes)).unwrap());
}

#[divan::bench]
fn prepare_for_picking(b: Bencher) {
    let m = model(&encoded());
    b.bench(|| Picker::new(divan::black_box(&m)));
}

#[divan::bench]
fn pick_under_the_mouse(b: Bencher) {
    let m = model(&encoded());
    let picker = Picker::new(&m);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, 1.5, centre, 3.0);
    let size = [1200.0, 800.0];
    b.bench(|| picker.pick(&pr, size, divan::black_box([600.0, 400.0])));
}
