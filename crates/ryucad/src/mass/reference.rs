//! A reference's mass properties from its mesh (design 364): a closed
//! reference is weighed as the polyhedron its display mesh at normal
//! quality is, bounded by the mesh's distance from the surface. The
//! kernel's `measure::mass` takes only checked solids.

use crate::error::{Code, Error, Result};
use crate::rebuild::Built;
use ryukei::geom::{Point3, UnitVector3};
use ryukei::measure::{MassProperties, Value as Measured};

/// The mass properties of the closed, outward triangles `t` over `v` at
/// `density`, each element within what a mesh `chord` from the surface
/// (and `tol` from it again, for the body's tolerances) allows. Only
/// `+ - * /` and `sqrt`, correctly rounded: the same bits everywhere.
pub fn of_triangles(
    v: &[[f64; 3]],
    t: &[[u32; 3]],
    density: f64,
    chord: f64,
    tol: f64,
) -> MassProperties {
    // About the box's middle, so the sums keep their digits.
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in v {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let o = [
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    ];
    let reach = {
        let d = [hi[0] - o[0], hi[1] - o[1], hi[2] - o[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    };
    let (mut vol, mut area) = (0.0, 0.0);
    let mut first = [0.0; 3];
    // ∫ x_i x_j over the solid, about `o`.
    let mut second = [[0.0; 3]; 3];
    for tri in t {
        let [a, b, c] = tri.map(|k| {
            let p = v[k as usize];
            [p[0] - o[0], p[1] - o[1], p[2] - o[2]]
        });
        let cross = [
            b[1] * c[2] - b[2] * c[1],
            b[2] * c[0] - b[0] * c[2],
            b[0] * c[1] - b[1] * c[0],
        ];
        let d = (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0;
        vol += d;
        for i in 0..3 {
            first[i] += d * (a[i] + b[i] + c[i]) / 4.0;
            for j in 0..3 {
                second[i][j] += d / 20.0
                    * (2.0 * (a[i] * a[j] + b[i] * b[j] + c[i] * c[j])
                        + a[i] * b[j]
                        + a[j] * b[i]
                        + a[i] * c[j]
                        + a[j] * c[i]
                        + b[i] * c[j]
                        + b[j] * c[i]);
            }
        }
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        area += (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0;
    }
    let vb = (chord + tol) * area;
    let rel = [first[0] / vol, first[1] / vol, first[2] / vol];
    let centre = Point3::new(rel[0] + o[0], rel[1] + o[1], rel[2] + o[2]);
    // Inertia about the centre: ∫(|x|² δ_ij - x_i x_j) less the shift.
    let mut inertia = [[0.0; 3]; 3];
    let tr = second[0][0] + second[1][1] + second[2][2];
    let c2 = rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2];
    for i in 0..3 {
        for j in 0..3 {
            let delta = if i == j { 1.0 } else { 0.0 };
            let about_o = tr * delta - second[i][j];
            let shift = vol * (c2 * delta - rel[i] * rel[j]);
            inertia[i][j] = density * (about_o - shift);
        }
    }
    let (principal_moments, principal_axes): ([f64; 3], [UnitVector3; 3]) =
        ryukei::measure::principal(&inertia);
    MassProperties {
        volume: Measured::Bounded {
            value: vol,
            bound: vb,
        },
        area: Measured::Bounded {
            value: area,
            bound: 2.0 * (chord + tol) * area / reach.max(chord),
        },
        mass: Measured::Bounded {
            value: density * vol,
            bound: density * vb,
        },
        centre,
        centre_bound: 2.0 * vb * reach / (vol - vb).max(f64::MIN_POSITIVE),
        inertia,
        inertia_bound: density * vb * reach * reach * 4.0,
        principal_moments,
        principal_axes,
    }
}

/// The largest tolerance of `b`'s edges and vertices.
fn largest_tolerance(b: &ryukei::topo::Body) -> f64 {
    let edges = b.edges().map(|e| b.edge(e).expect("live").tolerance.get());
    let vertices = b
        .vertices()
        .map(|v| b.vertex(v).expect("live").tolerance.get());
    edges.chain(vertices).fold(0.0, f64::max)
}

/// A reference's mass properties at `density`, from its mesh at normal
/// display quality, or `reference_not_solid` where its shells don't close
/// (`reason: open`) or its mesh doesn't (`reason: unmeshed`).
pub fn weigh(b: &Built, density: f64, solid: &str) -> Result<MassProperties> {
    let laminar = b.body.edges().filter(|&e| b.body.is_laminar(e)).count();
    if laminar > 0 {
        return Err(Error::new(
            Code::ReferenceNotSolid,
            format!(
                "{solid} is a reference whose shells don't close ({laminar} laminar edges): it holds no volume"
            ),
        )
        .with("solid", solid)
        .with("reason", "open")
        .with("laminar", laminar));
    }
    let diagonal = match crate::inspect::loose_extent(&b.body) {
        Some((lo, hi)) => {
            let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        }
        None => 1.0,
    };
    let params = crate::mesh::Quality::Normal.params(diagonal);
    let mesh = ryukei::mesh::tessellate(&b.body, &params).map_err(|e| {
        Error::new(
            Code::ReferenceNotSolid,
            format!("{solid} is a reference that doesn't mesh: {e}"),
        )
        .with("solid", solid)
        .with("reason", "unmeshed")
    })?;
    if !mesh.faults.is_empty() {
        let faces: Vec<String> = mesh
            .faults
            .iter()
            .filter_map(|f| Some(b.body.face(f.face)?.name.to_string()))
            .collect();
        return Err(Error::new(
            Code::ReferenceNotSolid,
            format!(
                "{solid} is a reference {} of whose faces don't mesh: its mesh doesn't close",
                faces.len()
            ),
        )
        .with("solid", solid)
        .with("reason", "unmeshed")
        .with("faces", faces));
    }
    let v: Vec<[f64; 3]> = mesh.positions.iter().map(|p| [p.x, p.y, p.z]).collect();
    let t: Vec<[u32; 3]> = mesh
        .triangles
        .iter()
        .map(|tri| tri.map(|c| mesh.corners[c as usize].position))
        .collect();
    Ok(of_triangles(
        &v,
        &t,
        density,
        params.chord,
        largest_tolerance(&b.body),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cube of side `a` as a closed triangle mesh, outward.
    fn cube(a: f64) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let p = |x: f64, y: f64, z: f64| [x * a, y * a, z * a];
        let v = vec![
            p(0., 0., 0.),
            p(1., 0., 0.),
            p(1., 1., 0.),
            p(0., 1., 0.),
            p(0., 0., 1.),
            p(1., 0., 1.),
            p(1., 1., 1.),
            p(0., 1., 1.),
        ];
        let t = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        (v, t)
    }

    #[test]
    fn a_cubes_mesh_weighs_as_a_cube() {
        let (v, t) = cube(0.02);
        let m = of_triangles(&v, &t, 1000.0, 1e-6, 0.0);
        let vol = m.volume.value();
        assert!((vol - 8e-6).abs() < 1e-18, "{vol}");
        assert!((m.mass.value() - 8e-3).abs() < 1e-15);
        assert!((m.centre.x - 0.01).abs() < 1e-15);
        // I = m a² / 6 about each axis through the centre, no products.
        let i = 8e-3 * 0.02 * 0.02 / 6.0;
        for k in 0..3 {
            assert!((m.inertia[k][k] - i).abs() < 1e-15, "{:?}", m.inertia);
            for j in 0..3 {
                if j != k {
                    assert!(m.inertia[k][j].abs() < 1e-18);
                }
            }
        }
        // The bound is the chord times the area, at least.
        let Measured::Bounded { bound, .. } = m.volume else {
            panic!()
        };
        assert!(bound >= 1e-6 * 6.0 * 4e-4);
    }
}
