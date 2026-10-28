//! STL: every solid of every occurrence, placed in the root's frame, in
//! one file, binary or ASCII. STL has no unit field: the numbers are in
//! the export's unit, as the binary header says.
//!
//! Binary: an 80-byte header (the caller's text, padded with spaces, cut
//! at 80 bytes), the triangle count, then each triangle as its normal and
//! three vertices in little-endian `f32` and a zero attribute. ASCII: a
//! `solid <path>` block per placed solid, numbers as the shortest text
//! that reads back to the same `f32`. Positions are placed and scaled in
//! f64, then rounded to `f32` once; a facet's normal is its triangle's,
//! from the f64 positions, `0 0 0` where the triangle has no area.

use crate::scene::{Placed, Scene, Unit, placed_point};
use crate::text::push_f32;
use std::io::{self, Write};

/// How to write an STL file: its unit, ASCII or binary, and the binary
/// header's text.
#[derive(Clone, Debug, Default)]
pub struct Stl {
    pub unit: Unit,
    pub ascii: bool,
    pub header: String,
}

/// One placed solid's triangles: each its normal and three vertices.
fn facets<'a>(p: &'a Placed<'_>, unit: Unit) -> impl Iterator<Item = [[f32; 3]; 4]> + 'a {
    let at: Vec<[f64; 3]> = p
        .mesh
        .positions
        .iter()
        .map(|&q| placed_point(&p.place, unit, q))
        .collect();
    p.mesh.welded_triangles().map(move |t| {
        let [a, b, c] = t.map(|i| at[i as usize]);
        let n = normal(a, b, c);
        let f = |v: [f64; 3]| v.map(|x| x as f32);
        [n, f(a), f(b), f(c)]
    })
}

/// The unit normal of the triangle `a b c`, counterclockwise, or zero.
fn normal(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len == 0.0 || !len.is_finite() {
        return [0.0; 3];
    }
    n.map(|x| (x / len) as f32)
}

/// Writes `scene` as STL.
pub fn write(scene: &Scene, opts: &Stl, out: &mut dyn Write) -> io::Result<()> {
    let placed = scene.placed();
    if opts.ascii {
        ascii(scene, &placed, opts.unit, out)
    } else {
        binary(&placed, opts, out)
    }
}

fn binary(placed: &[Placed<'_>], opts: &Stl, out: &mut dyn Write) -> io::Result<()> {
    let mut header = [b' '; 80];
    let text = opts.header.as_bytes();
    let n = text.len().min(80);
    header[..n].copy_from_slice(&text[..n]);
    out.write_all(&header)?;
    let count: u64 = placed.iter().map(|p| p.mesh.triangles.len() as u64).sum();
    let count = u32::try_from(count).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{count} triangles: binary STL holds at most {}", u32::MAX),
        )
    })?;
    out.write_all(&count.to_le_bytes())?;
    let mut buf = Vec::with_capacity(50 * 1024);
    for p in placed {
        for facet in facets(p, opts.unit) {
            for v in facet {
                for x in v {
                    buf.extend_from_slice(&x.to_le_bytes());
                }
            }
            buf.extend_from_slice(&[0, 0]);
            if buf.len() >= 50 * 1024 {
                out.write_all(&buf)?;
                buf.clear();
            }
        }
    }
    out.write_all(&buf)
}

fn ascii(scene: &Scene, placed: &[Placed<'_>], unit: Unit, out: &mut dyn Write) -> io::Result<()> {
    if placed.is_empty() {
        let name = scene.products.first().map_or("", |p| p.part.as_str());
        return write!(out, "solid {name}\nendsolid {name}\n");
    }
    let mut s = String::new();
    let vector = |s: &mut String, v: [f32; 3]| {
        for (i, x) in v.into_iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            push_f32(s, x);
        }
        s.push('\n');
    };
    for p in placed {
        s.push_str("solid ");
        s.push_str(&p.path);
        s.push('\n');
        for [n, a, b, c] in facets(p, unit) {
            s.push_str("  facet normal ");
            vector(&mut s, n);
            s.push_str("    outer loop\n");
            for v in [a, b, c] {
                s.push_str("      vertex ");
                vector(&mut s, v);
            }
            s.push_str("    endloop\n  endfacet\n");
            if s.len() >= 64 * 1024 {
                out.write_all(s.as_bytes())?;
                s.clear();
            }
        }
        s.push_str("endsolid ");
        s.push_str(&p.path);
        s.push('\n');
    }
    out.write_all(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::tests::{block, frame};
    use crate::scene::{Product, Solid};

    fn written(scene: &Scene, opts: &Stl) -> Vec<u8> {
        let mut out = Vec::new();
        write(scene, opts, &mut out).unwrap();
        out
    }

    fn one_box() -> Scene {
        Scene {
            products: vec![Product {
                id: "p".into(),
                name: "p".into(),
                part: "p".into(),
                solids: vec![Solid {
                    name: "b".into(),
                    mesh: block(0.01, 0.02, 0.005),
                }],
                children: vec![],
            }],
        }
    }

    #[test]
    fn a_box_in_binary() {
        let opts = Stl {
            header: "ryucad 0.0.0 p mm".into(),
            ..Stl::default()
        };
        let bytes = written(&one_box(), &opts);
        assert_eq!(bytes.len(), 84 + 50 * 12);
        assert_eq!(&bytes[..17], b"ryucad 0.0.0 p mm");
        assert!(bytes[17..80].iter().all(|&b| b == b' '));
        assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()), 12);
        let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let mut most = [0.0f32; 3];
        for t in 0..12 {
            let at = 84 + 50 * t;
            // Each normal is an axis, each corner on the box, in millimetres.
            let n = [f(at), f(at + 4), f(at + 8)];
            assert_eq!(n.iter().map(|x| x.abs()).sum::<f32>(), 1.0, "{n:?}");
            for k in 0..9 {
                most[k % 3] = most[k % 3].max(f(at + 12 + 4 * k));
            }
            assert_eq!(&bytes[at + 48..at + 50], [0, 0]);
        }
        assert_eq!(most, [10.0, 20.0, 5.0]);
        // A header longer than 80 bytes is cut.
        let long = Stl {
            header: "x".repeat(100),
            ..Stl::default()
        };
        assert_eq!(
            &written(&one_box(), &long)[..81],
            [vec![b'x'; 80], vec![12]].concat()
        );
    }

    #[test]
    fn a_box_in_ascii() {
        let opts = Stl {
            ascii: true,
            unit: Unit::Meter,
            ..Stl::default()
        };
        let text = String::from_utf8(written(&one_box(), &opts)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2 + 12 * 7);
        assert_eq!(lines[0], "solid p/b");
        assert!(lines[1].starts_with("  facet normal "));
        assert_eq!(lines[2], "    outer loop");
        assert!(lines[3].starts_with("      vertex "));
        assert_eq!(lines[6], "    endloop");
        assert_eq!(lines[7], "  endfacet");
        assert_eq!(*lines.last().unwrap(), "endsolid p/b");
        assert!(text.contains("vertex 0.01 0.02 0.005\n"), "{text}");
    }

    #[test]
    fn an_assembly_is_one_file_of_every_occurrence() {
        let s = frame();
        let bytes = written(&s, &Stl::default());
        assert_eq!(bytes.len(), 84 + 50 * 36);
        let text = String::from_utf8(written(
            &s,
            &Stl {
                ascii: true,
                ..Stl::default()
            },
        ))
        .unwrap();
        let solids: Vec<&str> = text.lines().filter(|l| l.starts_with("solid ")).collect();
        assert_eq!(
            solids,
            [
                "solid frame/plate",
                "solid frame/a/shank",
                "solid frame/b/shank"
            ]
        );
        // The second pin is 50 mm along x and 10 mm up.
        assert!(text.contains("vertex 60 10 30\n"), "{text}");
    }

    #[test]
    fn an_empty_scene_is_an_empty_file() {
        let empty = Scene {
            products: vec![Product {
                part: "p".into(),
                ..Product::default()
            }],
        };
        let bytes = written(&empty, &Stl::default());
        assert_eq!(bytes.len(), 84);
        assert_eq!(&bytes[80..], [0, 0, 0, 0]);
        let ascii = Stl {
            ascii: true,
            ..Stl::default()
        };
        assert_eq!(written(&empty, &ascii), b"solid p\nendsolid p\n");
    }

    #[test]
    fn degenerate_triangles_have_no_normal() {
        assert_eq!(normal([0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]), [0.0; 3]);
        assert_eq!(
            normal([0.0; 3], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
            [0.0, 0.0, 1.0]
        );
    }
}
