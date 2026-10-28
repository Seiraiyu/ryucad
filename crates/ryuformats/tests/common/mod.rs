//! Test-only readers and checks for what the writers write: never part
//! of the crate, which writes these formats and never reads them.

#![allow(dead_code)]

use ryuformats::scene::{Placed, Scene, Unit, placed_point};
use std::collections::BTreeMap;

/// A triangle as three vertices.
pub type Triangle = [[f32; 3]; 3];

/// Reads a binary STL file: its header and triangles. Panics on anything
/// malformed.
pub fn read_binary_stl(bytes: &[u8]) -> (Vec<u8>, Vec<Triangle>) {
    assert!(bytes.len() >= 84, "shorter than a header");
    let n = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    assert_eq!(
        bytes.len(),
        84 + 50 * n,
        "the count and the length disagree"
    );
    let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let triangles = (0..n)
        .map(|t| {
            let at = 84 + 50 * t + 12;
            [0, 1, 2].map(|v| [0, 1, 2].map(|k| f(at + 12 * v + 4 * k)))
        })
        .collect();
    (bytes[..80].to_vec(), triangles)
}

/// Reads an ASCII STL file: each solid's name and triangles. Panics on
/// anything malformed.
pub fn read_ascii_stl(text: &str) -> Vec<(String, Vec<Triangle>)> {
    let mut words = text.split_whitespace().peekable();
    let number = |w: Option<&str>| {
        let w = w.expect("the file ends early");
        w.parse::<f32>()
            .unwrap_or_else(|_| panic!("not a number: {w}"))
    };
    let mut solids = Vec::new();
    while words.peek().is_some() {
        assert_eq!(words.next(), Some("solid"));
        let name = words.next().expect("a solid's name").to_owned();
        let mut triangles = Vec::new();
        loop {
            match words.next() {
                Some("endsolid") => {
                    assert_eq!(words.next(), Some(name.as_str()));
                    break;
                }
                Some("facet") => assert_eq!(words.next(), Some("normal")),
                w => panic!("expected a facet, got {w:?}"),
            }
            let _normal = [0; 3].map(|_| number(words.next()));
            assert_eq!((words.next(), words.next()), (Some("outer"), Some("loop")));
            let t = [0; 3].map(|_| {
                assert_eq!(words.next(), Some("vertex"));
                [0; 3].map(|_| number(words.next()))
            });
            assert_eq!(
                (words.next(), words.next()),
                (Some("endloop"), Some("endfacet"))
            );
            triangles.push(t);
        }
        solids.push((name, triangles));
    }
    solids
}

/// Whether `triangles` close up: welded by their vertices' bits, every
/// edge used as often one way as the other, and never by one triangle
/// twice.
pub fn watertight(triangles: &[Triangle]) -> bool {
    let key = |v: [f32; 3]| v.map(f32::to_bits);
    let mut uses: BTreeMap<([u32; 3], [u32; 3]), i64> = BTreeMap::new();
    for t in triangles {
        let k = t.map(key);
        if k[0] == k[1] || k[1] == k[2] || k[2] == k[0] {
            return false;
        }
        for (a, b) in [(k[0], k[1]), (k[1], k[2]), (k[2], k[0])] {
            *uses.entry((a, b)).or_default() += 1;
        }
    }
    uses.iter()
        .all(|(&(a, b), &n)| uses.get(&(b, a)).copied().unwrap_or(0) == n)
}

/// The volume `triangles` enclose, in f64.
pub fn volume(triangles: &[Triangle]) -> f64 {
    triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| v.map(f64::from));
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum()
}

/// The volume a scene's placed meshes enclose, in `unit`'s cube, from
/// their f64 positions.
pub fn scene_volume(scene: &Scene, unit: Unit) -> f64 {
    scene.placed().iter().map(|p| placed_volume(p, unit)).sum()
}

fn placed_volume(p: &Placed<'_>, unit: Unit) -> f64 {
    let at: Vec<[f64; 3]> = p
        .mesh
        .positions
        .iter()
        .map(|&q| placed_point(&p.place, unit, q))
        .collect();
    p.mesh
        .welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| at[i as usize]);
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum()
}
