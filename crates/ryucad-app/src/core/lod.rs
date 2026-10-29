//! Levels of detail and culling. Every occurrence is drawn at one of the
//! three display qualities, chosen by how many pixels its body's diagonal
//! spans: coarse under 64, fine over 512, normal between, each shifted
//! one way or the other when the user's display quality is coarse or
//! fine. An occurrence whose box lies outside the view isn't drawn. A
//! level not fetched yet is asked for, nearest occurrences first, and
//! the body is drawn at the level it has meanwhile, so nothing stalls.
//! A view drawing more than [`COARSE_FIRST`] occurrences fetches every
//! body coarse first ([`Model::fetch_level`]), so its first full frame
//! comes from a tenth of the bytes, and finer levels follow where they
//! show; a mesh's level is read from it ([`level_of`]).
//! What the GPU holds stays within `display.gpu_memory` ([`within`]):
//! past it, the farthest bodies drop their finer levels first, then the
//! farthest go undrawn.

use crate::core::model::{BodyKey, Model, Pose};
use crate::core::nav::Projector;
use crate::core::pick::Picker;
use ryucad::mesh::{Quality, WireBody};
use std::sync::Arc;

/// Under this many pixels across, a body is drawn coarse.
pub const COARSE_BELOW: f64 = 64.0;
/// Over this many, fine.
pub const FINE_ABOVE: f64 = 512.0;

/// Over this many occurrences of solids drawn, bodies are fetched coarse
/// first. Design 72's 1,001 occurrences stay under it.
pub const COARSE_FIRST: usize = 2_000;

/// The level a mesh was made at, read from its angle (each level has its
/// own, [`Quality::angle`]); `base` for one with no mesh, or an angle no
/// level makes.
pub fn level_of(m: &WireBody, base: Quality) -> Quality {
    let Ok(w) = &m.mesh else {
        return base;
    };
    [Quality::Coarse, Quality::Normal, Quality::Fine]
        .into_iter()
        .find(|q| q.angle() == w.angle)
        .unwrap_or(base)
}

/// One occurrence to draw this frame: its key, the mesh at the level it
/// is drawn at, that level, the mesh picking and painting index faces in
/// (at the user's display quality once the window has it, else the
/// first fetched), and where it is.
#[derive(Clone, Debug)]
pub struct Drawn {
    pub key: BodyKey,
    pub mesh: Arc<WireBody>,
    pub level: Quality,
    pub base: Arc<WireBody>,
    pub pose: Pose,
}

fn rank(q: Quality) -> i32 {
    match q {
        Quality::Coarse => 0,
        Quality::Normal => 1,
        Quality::Fine => 2,
    }
}

fn of_rank(r: i32) -> Quality {
    match r.clamp(0, 2) {
        0 => Quality::Coarse,
        1 => Quality::Normal,
        _ => Quality::Fine,
    }
}

/// The level a body `pixels` across is drawn at, with the user's display
/// quality `base`.
pub fn level(pixels: f64, base: Quality) -> Quality {
    let by_size = if pixels < COARSE_BELOW {
        0
    } else if pixels > FINE_ABOVE {
        2
    } else {
        1
    };
    of_rank(by_size + rank(base) - 1)
}

/// What to draw: every occurrence the model draws whose box may be in
/// view, at its level (or the one it has, until that comes), and the
/// levels to fetch, nearest occurrence first. `height` is the viewport's
/// in pixels.
pub fn choose(
    model: &Model,
    picker: &Picker,
    pr: &Projector,
    height: f64,
) -> (Vec<Drawn>, Vec<(BodyKey, Quality)>) {
    let mut drawn = Vec::new();
    let mut wanted: Vec<(f64, BodyKey, Quality)> = Vec::new();
    let (_, _, forward) = pr.camera.basis();
    for (key, mesh, pose) in model.drawn() {
        let Some((lo, hi)) = picker.body_bounds(key) else {
            continue;
        };
        if !pr.sees_box(lo, hi) {
            continue;
        }
        let level = level(pr.box_pixels(lo, hi, height), model.base);
        let (shown, at) = match model.mesh_at(key, level) {
            Some(m) => (m, level),
            None => {
                let middle = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
                let rel = [0, 1, 2].map(|k| middle[k] - pr.camera.eye[k]);
                let depth = rel[0] * forward[0] + rel[1] * forward[1] + rel[2] * forward[2];
                wanted.push((depth, key.clone(), level));
                (Arc::clone(mesh), level_of(mesh, model.base))
            }
        };
        drawn.push(Drawn {
            key: key.clone(),
            mesh: shown,
            level: at,
            base: Arc::clone(mesh),
            pose,
        });
    }
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    (drawn, wanted.into_iter().map(|(_, k, l)| (k, l)).collect())
}

/// What the GPU holds of a mesh, in bytes: its triangles' corners and
/// its edges' ends, 40 bytes each (position, normal, colour).
pub fn gpu_bytes(m: &WireBody) -> u64 {
    let Ok(w) = &m.mesh else {
        return 0;
    };
    let segments: usize = w.edges.iter().map(|(_, p)| p.len().saturating_sub(1)).sum();
    ((w.triangles.len() * 3 + segments * 2) * 40) as u64
}

/// `drawn` and `wanted` ([`choose`]'s) kept within `budget` bytes of
/// meshes on the GPU, each body and level counted once: bodies drawn
/// finer than the user's display quality are drawn at it instead, the
/// farthest first, then the farthest bodies aren't drawn, until what is
/// drawn fits; and no finer level is asked for while it doesn't.
pub fn within(
    model: &Model,
    pr: &Projector,
    mut drawn: Vec<Drawn>,
    mut wanted: Vec<(BodyKey, Quality)>,
    budget: u64,
) -> (Vec<Drawn>, Vec<(BodyKey, Quality)>) {
    use std::collections::HashMap;
    // Each body and level: its bytes and how many occurrences draw it.
    let mut held: HashMap<(String, Quality), (u64, usize)> = HashMap::new();
    let mut total = 0;
    let add = |held: &mut HashMap<(String, Quality), (u64, usize)>, d: &Drawn| {
        let e = held
            .entry((d.mesh.hash.clone(), d.level))
            .or_insert_with(|| (gpu_bytes(&d.mesh), 0));
        e.1 += 1;
        if e.1 == 1 { e.0 } else { 0 }
    };
    for d in &drawn {
        total += add(&mut held, d);
    }
    if total <= budget {
        return (drawn, wanted);
    }
    let take = |held: &mut HashMap<(String, Quality), (u64, usize)>, d: &Drawn| {
        let e = held
            .get_mut(&(d.mesh.hash.clone(), d.level))
            .expect("counted");
        e.1 -= 1;
        if e.1 == 0 { e.0 } else { 0 }
    };
    let (_, _, forward) = pr.camera.basis();
    let depth = |d: &Drawn| {
        let o = d.pose.apply([0.0; 3]);
        (0..3)
            .map(|k| (o[k] - pr.camera.eye[k]) * forward[k])
            .sum::<f64>()
    };
    // Farthest first.
    drawn.sort_by(|a, b| depth(b).total_cmp(&depth(a)));
    wanted.retain(|(_, l)| rank(*l) <= rank(model.base));
    for d in drawn.iter_mut() {
        if total <= budget {
            break;
        }
        if rank(d.level) > rank(model.base) {
            total -= take(&mut held, d);
            d.mesh = Arc::clone(&d.base);
            d.level = level_of(&d.base, model.base);
            total += add(&mut held, d);
        }
    }
    let mut gone = 0;
    while total > budget && gone < drawn.len() {
        total -= take(&mut held, &drawn[gone]);
        gone += 1;
    }
    drawn.drain(..gone);
    // Nearest first again, as drawn.
    drawn.reverse();
    (drawn, wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_size_and_the_users_quality() {
        use Quality::*;
        assert_eq!(level(10.0, Normal), Coarse);
        assert_eq!(level(64.0, Normal), Normal);
        assert_eq!(level(512.0, Normal), Normal);
        assert_eq!(level(513.0, Normal), Fine);
        // A coarse preference draws everything one level coarser, a fine
        // one one finer, within the three.
        assert_eq!(level(10.0, Coarse), Coarse);
        assert_eq!(level(100.0, Coarse), Coarse);
        assert_eq!(level(1000.0, Coarse), Normal);
        assert_eq!(level(10.0, Fine), Normal);
        assert_eq!(level(100.0, Fine), Fine);
        assert_eq!(level(f64::INFINITY, Fine), Fine);
    }

    /// An assembly `row` of 1 cm cubes (one body) at these x, all 0.5 cm
    /// off the axis the camera looks along, their mesh fetched.
    fn row(xs: &[f64]) -> Model {
        use serde_json::json;
        let occurrences: Vec<serde_json::Value> = xs
            .iter()
            .enumerate()
            .map(|(i, x)| {
                json!({"path": format!("c{i}"), "part": "cube",
                       "solids": [{"solid": "s", "hash": "h"}],
                       "pose": {"origin_m": [x, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}})
            })
            .collect();
        let mut m = Model::default();
        let want = m.apply_part(&json!({"part": "row", "revision": "r", "kind": "assembly",
            "features": [], "solids": [], "occurrences": occurrences}));
        m.ask(want);
        assert!(m.take_mesh("row", cube("c0/s", 0.0)));
        m
    }

    /// A mesh of a 1 cm cube's corners at `level`'s chord (no faces: only
    /// its box matters here).
    fn cube(feature: &str, chord: f64) -> WireBody {
        let positions = (0..8)
            .map(|i| [0, 1, 2].map(|k| if i >> k & 1 == 1 { 0.01 } else { 0.0 }))
            .collect();
        WireBody {
            feature: feature.into(),
            hash: "h".into(),
            mesh: Ok(ryucad::mesh::WireMesh {
                origin: [0.0; 3],
                chord,
                angle: 0.0,
                positions,
                corners: Vec::new(),
                triangles: Vec::new(),
                faces: Vec::new(),
                edges: Vec::new(),
                vertices: Vec::new(),
                faults: Vec::new(),
            }),
        }
    }

    #[test]
    fn near_is_fine_far_is_coarse_and_behind_is_culled() {
        let mut m = row(&[0.0, 1.0, -1.0]);
        let camera = ryucad::view::Camera {
            eye: [-0.01, 0.005, 0.005],
            target: [1.0, 0.005, 0.005],
            up: [0.0, 0.0, 1.0],
            projection: ryucad::view::Projection::Perspective { fov_deg: 30.0 },
        };
        let pr = Projector::new(&camera, 1.0, [0.5, 0.0, 0.0], 1.0);
        let picker = Picker::new(&m);
        let (drawn, wanted) = choose(&m, &picker, &pr, 400.0);
        let keys: Vec<&str> = drawn.iter().map(|d| d.key.1.as_str()).collect();
        assert_eq!(keys, ["c0/s", "c1/s"], "the one behind the eye is culled");
        assert!(
            drawn.iter().all(|d| d.level == Quality::Normal),
            "until fetched"
        );
        let row = |s: &str| ("row".to_owned(), s.to_owned());
        assert_eq!(
            wanted,
            [(row("c0/s"), Quality::Fine), (row("c1/s"), Quality::Coarse)],
            "nearest first"
        );
        // Asked for once each; the fine one comes.
        assert_eq!(m.ask_levels(wanted.clone()).len(), 2);
        assert!(m.ask_levels(wanted).is_empty());
        assert!(m.take_level(cube("c0/s", 1e-5), Quality::Fine));
        let picker = Picker::new(&m);
        let (drawn, wanted) = choose(&m, &picker, &pr, 400.0);
        assert_eq!(drawn[0].level, Quality::Fine);
        assert!(!Arc::ptr_eq(&drawn[0].mesh, &drawn[0].base));
        assert_eq!(drawn[1].level, Quality::Normal);
        assert_eq!(wanted, [(row("c1/s"), Quality::Coarse)]);
        // The display quality changing drops every level.
        m.clear_meshes();
        assert!(m.levels.is_empty() && m.levels_asked.is_empty());
    }

    /// [`cube`] made at `level`, as its angle says.
    fn at(feature: &str, level: Quality) -> WireBody {
        let mut b = cube(feature, 0.0);
        if let Ok(w) = &mut b.mesh {
            w.angle = level.angle();
        }
        b
    }

    #[test]
    fn a_large_view_draws_coarse_first_meshes_until_finer_ones_come() {
        use Quality::*;
        assert_eq!(level_of(&at("s", Coarse), Normal), Coarse);
        assert_eq!(level_of(&at("s", Fine), Normal), Fine);
        assert_eq!(
            level_of(&cube("s", 0.0), Normal),
            Normal,
            "no level's angle"
        );
        // Fetched coarse first: over 2,000 occurrences drawn.
        assert_eq!(row(&[0.0; COARSE_FIRST]).fetch_level(), Normal);
        let mut m = row(&[0.0; COARSE_FIRST + 1]);
        assert_eq!(m.fetch_level(), Coarse);
        m = row(&[0.0, 1.0, -1.0]);
        let first = Arc::new(at("c0/s", Coarse));
        m.meshes.values_mut().for_each(|b| *b = Arc::clone(&first));
        let camera = ryucad::view::Camera {
            eye: [-0.01, 0.005, 0.005],
            target: [1.0, 0.005, 0.005],
            up: [0.0, 0.0, 1.0],
            projection: ryucad::view::Projection::Perspective { fov_deg: 30.0 },
        };
        let pr = Projector::new(&camera, 1.0, [0.5, 0.0, 0.0], 1.0);
        let (drawn, wanted) = choose(&m, &Picker::new(&m), &pr, 400.0);
        // Both drawn coarse meanwhile; the far one is where it should be,
        // and only the near one's fine level is wanted.
        assert!(
            drawn
                .iter()
                .all(|d| d.level == Coarse && Arc::ptr_eq(&d.mesh, &first))
        );
        let row = |s: &str| ("row".to_owned(), s.to_owned());
        assert_eq!(wanted, [(row("c0/s"), Fine)]);
        assert!(m.ask_levels(vec![(row("c1/s"), Coarse)]).is_empty());
        // The user's display quality comes: picking and painting read it,
        // and the far one is still drawn coarse.
        assert!(m.take_level(at("c0/s", Normal), Normal));
        assert!(m.drawn().all(|(_, b, _)| level_of(b, m.base) == Normal));
        let (drawn, _) = choose(&m, &Picker::new(&m), &pr, 400.0);
        assert_eq!(drawn[1].level, Coarse);
        assert!(Arc::ptr_eq(&drawn[1].mesh, &first));
        assert_eq!(level_of(&drawn[1].base, m.base), Normal);
        assert_eq!(drawn[0].level, Normal, "fine not here yet");
    }

    #[test]
    fn past_the_gpu_budget_far_bodies_go_coarser_then_go() {
        // Three distinct bodies in a row, each fetched fine and normal:
        // 30 triangles fine, 10 normal, 3600 and 1200 bytes.
        let mesh = |hash: &str, tris: usize| WireBody {
            feature: "s".into(),
            hash: hash.into(),
            mesh: Ok(ryucad::mesh::WireMesh {
                triangles: vec![[0, 1, 2]; tris],
                ..cube("s", 0.0).mesh.unwrap()
            }),
        };
        let mut m = Model::default();
        m.base = Quality::Normal;
        let camera = ryucad::view::Camera {
            eye: [-1.0, 0.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
            projection: ryucad::view::Projection::Perspective { fov_deg: 30.0 },
        };
        let pr = Projector::new(&camera, 1.0, [0.0; 3], 1.0);
        let drawn: Vec<Drawn> = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, h)| Drawn {
                key: ("row".into(), format!("c{i}/s")),
                mesh: Arc::new(mesh(h, 30)),
                level: Quality::Fine,
                base: Arc::new(mesh(h, 10)),
                pose: crate::core::model::Pose::of(
                    [i as f64, 0.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [1.0, 0.0, 0.0],
                ),
            })
            .collect();
        let wanted = vec![(("row".to_owned(), "c0/s".to_owned()), Quality::Fine)];
        // Everything fits.
        let (all, w) = within(&m, &pr, drawn.clone(), wanted.clone(), 10_800);
        assert_eq!(all.len(), 3);
        assert_eq!(w.len(), 1);
        // The two farthest go normal; nothing finer is asked for.
        let (some, w) = within(&m, &pr, drawn.clone(), wanted.clone(), 6_000);
        let levels: Vec<(&str, Quality)> =
            some.iter().map(|d| (d.key.1.as_str(), d.level)).collect();
        use Quality::*;
        assert_eq!(levels, [("c0/s", Fine), ("c1/s", Normal), ("c2/s", Normal)]);
        assert!(w.is_empty());
        // Smaller still: all normal, then the farthest isn't drawn.
        let (few, _) = within(&m, &pr, drawn, wanted, 2_400);
        let keys: Vec<&str> = few.iter().map(|d| d.key.1.as_str()).collect();
        assert_eq!(keys, ["c0/s", "c1/s"]);
    }

    #[test]
    fn boxes_beside_the_view_are_culled() {
        let camera = ryucad::view::frame(
            [0.0; 3],
            [1.0; 3],
            ryucad::view::StandardView::Top,
            ryucad::view::DEFAULT_PROJECTION,
        );
        let pr = Projector::new(&camera, 1.0, [0.5; 3], 1.0);
        assert!(pr.sees_box([0.0; 3], [1.0; 3]));
        assert!(
            pr.sees_box([0.9, 0.9, 0.0], [5.0, 5.0, 1.0]),
            "partly in view"
        );
        assert!(
            !pr.sees_box([5.0, 0.0, 0.0], [6.0, 1.0, 1.0]),
            "off to the right"
        );
        assert!(!pr.sees_box([0.0, -6.0, 0.0], [1.0, -5.0, 1.0]), "below");
        let near = pr.box_pixels([0.0; 3], [1.0; 3], 400.0);
        let small = pr.box_pixels([0.45; 3], [0.55; 3], 400.0);
        assert!((near / small - 10.0).abs() < 1e-9, "{near} {small}");
    }
}
