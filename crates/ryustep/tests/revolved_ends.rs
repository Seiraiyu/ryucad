//! Pins of revolution whose ends KiCad's models draw on surfaces ryukei
//! can't hold whole, or with their loops written against their
//! neighbours': a horn torus and a ring torus (issue #364), a horn torus
//! written as a `SURFACE_OF_REVOLUTION` of a circle touching its axis
//! (#316), and the revolution of a circle crossing its axis, of which a
//! face uses one arc (#316). Each is a cylinder about z from z = 0 to
//! `top`, a disk on top, a band from the cylinder's rim at z = 0 to a
//! circle of radius `r1` at `z1`, and a disk there; the band's bound is
//! written both ways, as KiCad writes it (against its neighbours) and as
//! it should be: both heal to the solid's volume.

use core::f64::consts::PI;
use ryukei::check::check;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryustep::{ReadOptions, read};

/// The band's surface and the circle its seam runs along, from the rim
/// (its angle 0) to the inner circle, as STEP entities given a numbering
/// function.
struct Band {
    /// The surface, given the seam circle's instance.
    surface: fn(&mut Step, usize) -> usize,
    /// The seam circle: centre, radius, axis and reference direction.
    seam: ([f64; 3], f64, [f64; 3], [f64; 3]),
}

struct Step(Vec<String>);

impl Step {
    fn add(&mut self, s: String) -> usize {
        self.0.push(s);
        self.0.len()
    }
    fn point(&mut self, p: [f64; 3]) -> usize {
        self.add(format!(
            "CARTESIAN_POINT('',({:?},{:?},{:?}))",
            p[0], p[1], p[2]
        ))
    }
    fn dir(&mut self, d: [f64; 3]) -> usize {
        self.add(format!("DIRECTION('',({:?},{:?},{:?}))", d[0], d[1], d[2]))
    }
    fn place(&mut self, o: [f64; 3], z: [f64; 3], x: [f64; 3]) -> usize {
        let (o, z, x) = (self.point(o), self.dir(z), self.dir(x));
        self.add(format!("AXIS2_PLACEMENT_3D('',#{o},#{z},#{x})"))
    }
    fn circle(&mut self, o: [f64; 3], r: f64, z: [f64; 3], x: [f64; 3]) -> usize {
        let p = self.place(o, z, x);
        self.add(format!("CIRCLE('',#{p},{r:?})"))
    }
    fn vertex(&mut self, p: [f64; 3]) -> usize {
        let p = self.point(p);
        self.add(format!("VERTEX_POINT('',#{p})"))
    }
    fn edge(&mut self, a: usize, b: usize, c: usize) -> usize {
        self.add(format!("EDGE_CURVE('',#{a},#{b},#{c},.T.)"))
    }
    fn face(&mut self, uses: &[(usize, bool)], bound: bool, surface: usize, sense: bool) -> usize {
        let list: Vec<String> = uses
            .iter()
            .map(|&(e, s)| {
                let k = self.add(format!("ORIENTED_EDGE('',*,*,#{e},{})", flag(s)));
                format!("#{k}")
            })
            .collect();
        let l = self.add(format!("EDGE_LOOP('',({}))", list.join(",")));
        let b = self.add(format!("FACE_BOUND('',#{l},{})", flag(bound)));
        self.add(format!(
            "ADVANCED_FACE('',(#{b}),#{surface},{})",
            flag(sense)
        ))
    }
}

fn flag(b: bool) -> &'static str {
    if b { ".T." } else { ".F." }
}

const Z: [f64; 3] = [0.0, 0.0, 1.0];
const X: [f64; 3] = [1.0, 0.0, 0.0];

/// The pin's STEP file (millimetres), the band's bound as `bound` says.
fn pin(r: f64, top: f64, r1: f64, z1: f64, band: &Band, bound: bool) -> Vec<u8> {
    let mut s = Step(Vec::new());
    let (v_top, v_rim, v_in) = (
        s.vertex([r, 0.0, top]),
        s.vertex([r, 0.0, 0.0]),
        s.vertex([r1, 0.0, z1]),
    );
    let c_top = s.circle([0.0, 0.0, top], r, Z, X);
    let c_rim = s.circle([0.0, 0.0, 0.0], r, Z, X);
    let c_in = s.circle([0.0, 0.0, z1], r1, Z, X);
    let (e_top, e_rim, e_in) = (
        s.edge(v_top, v_top, c_top),
        s.edge(v_rim, v_rim, c_rim),
        s.edge(v_in, v_in, c_in),
    );
    let (o, d) = (s.point([r, 0.0, 0.0]), s.dir(Z));
    let v = s.add(format!("VECTOR('',#{d},1.)"));
    let line = s.add(format!("LINE('',#{o},#{v})"));
    let s1 = s.edge(v_rim, v_top, line);
    let (c, rr, z, x) = band.seam;
    let seam = s.circle(c, rr, z, x);
    let s2 = s.edge(v_rim, v_in, seam);
    let p = s.place([0.0, 0.0, top], Z, X);
    let plane_top = s.add(format!("PLANE('',#{p})"));
    let p = s.place([0.0, 0.0, 0.0], Z, X);
    let cyl = s.add(format!("CYLINDRICAL_SURFACE('',#{p},{r:?})"));
    let p = s.place([0.0, 0.0, z1], Z, X);
    let plane_in = s.add(format!("PLANE('',#{p})"));
    let surface = (band.surface)(&mut s, seam);
    let f1 = s.face(&[(e_top, true)], true, plane_top, true);
    let f2 = s.face(
        &[(e_rim, true), (s1, true), (e_top, false), (s1, false)],
        true,
        cyl,
        true,
    );
    // The band's loop round its rim and back round its inner circle. With
    // the bound true each of those edges is used against its neighbour's
    // use; KiCad writes it false, so used as its neighbour uses it.
    let f3 = s.face(
        &[(e_rim, false), (s2, true), (e_in, true), (s2, false)],
        bound,
        surface,
        true,
    );
    let f4 = s.face(&[(e_in, false)], true, plane_in, false);
    let shell = s.add(format!("CLOSED_SHELL('',(#{f1},#{f2},#{f3},#{f4}))"));
    let brep = s.add(format!("MANIFOLD_SOLID_BREP('',#{shell})"));
    let n = s.0.len();
    let mut text = String::from(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\n\
         FILE_NAME('pin','2026-11-03T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n",
    );
    for (i, e) in s.0.iter().enumerate() {
        text += &format!("#{}={e};\n", i + 1);
    }
    let k = n;
    text += &format!(
        "#{a}=APPLICATION_CONTEXT('automotive design');\n\
         #{b}=PRODUCT_CONTEXT('',#{a},'mechanical');\n\
         #{c}=PRODUCT('pin','pin','',(#{b}));\n\
         #{d}=PRODUCT_DEFINITION_FORMATION('','',#{c});\n\
         #{e}=PRODUCT_DEFINITION_CONTEXT('part definition',#{a},'design');\n\
         #{f}=PRODUCT_DEFINITION('design','',#{d},#{e});\n\
         #{g}=PRODUCT_DEFINITION_SHAPE('','',#{f});\n\
         #{h}=( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );\n\
         #{i}=( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );\n\
         #{j}=( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() );\n\
         #{u}=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),#{h},'distance_accuracy_value','');\n\
         #{ctx}=( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{u})) \
         GLOBAL_UNIT_ASSIGNED_CONTEXT((#{h},#{i},#{j})) REPRESENTATION_CONTEXT('',''));\n\
         #{rep}=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{brep}),#{ctx});\n\
         #{sdr}=SHAPE_DEFINITION_REPRESENTATION(#{g},#{rep});\nENDSEC;\nEND-ISO-10303-21;\n",
        a = k + 1,
        b = k + 2,
        c = k + 3,
        d = k + 4,
        e = k + 5,
        f = k + 6,
        g = k + 7,
        h = k + 8,
        i = k + 9,
        j = k + 10,
        u = k + 11,
        ctx = k + 12,
        rep = k + 13,
        sdr = k + 14,
    );
    text.into_bytes()
}

/// The one body the file heals to, sound, and its volume in mm³ from a
/// fine mesh.
fn volume(bytes: &[u8]) -> f64 {
    let opts = ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    };
    let m = read(bytes, &opts).unwrap();
    assert_eq!(m.bodies.len(), 1);
    let (b, _, _): (Body, _, _) = m.bodies[0].result.clone().expect("the pin heals");
    assert_eq!(check(&b), []);
    let mesh = tessellate(&b, &MeshParams::new(1e-7, 0.2)).unwrap();
    assert_eq!(mesh.faults, []);
    check_mesh(&b, &mesh).unwrap();
    mesh.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum::<f64>()
        * 1e9
}

fn near(got: f64, want: f64) {
    assert!((got - want).abs() <= 1e-3 * want, "{got} mm³, want {want}");
}

/// A pin 0.32 mm across rounded at its foot by a horn torus of radius
/// 0.16 about its axis, down to a flat of radius 0.16: a cylinder and
/// the solid of revolution of the tube's outer quarter,
/// `π a³ (5/3 + π/2)` for `a` = 0.16.
fn horn_pin(band: &Band, bound: bool) {
    let a = 0.16;
    let want = PI * 0.32 * 0.32 * 1.0 + PI * a * a * a * (5.0 / 3.0 + PI / 2.0);
    near(volume(&pin(0.32, 1.0, a, -a, band, bound)), want);
}

/// The tube circle at the rim: centre (0.16, 0, 0), from the rim (angle
/// 0) down to the flat.
const TUBE: ([f64; 3], f64, [f64; 3], [f64; 3]) =
    ([0.16, 0.0, 0.0], 0.16, [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]);

/// Issue #364: KiCad's horn tori read as the tube's arc turned about the
/// axis, the band's loops running against its neighbours'. Healing turned
/// the face over, which kept the complement of the band: three quarters
/// of the tube, through the axis where the arc stops, which never meshed.
#[test]
fn a_horn_torus_band_written_against_its_neighbours_heals() {
    let band = Band {
        surface: |s, _| {
            let p = s.place([0.0, 0.0, 0.0], Z, X);
            s.add(format!("TOROIDAL_SURFACE('',#{p},0.16,0.16)"))
        },
        seam: TUBE,
    };
    horn_pin(&band, true);
    horn_pin(&band, false);
}

/// Issue #364 too: the same fault on a ring torus, a pin 2 mm in radius
/// filleted at its foot by a torus of major radius 1.5 and minor 0.5. Both
/// of a torus band's regions are faces, and turning it over kept three
/// quarters of the tube, which crossed the rest of the solid; its sense
/// is the file's, its loops reversed. The fillet's solid of revolution is
/// `π (R² a + π R a² / 2 + 2 a³ / 3)`.
#[test]
fn a_ring_torus_band_written_against_its_neighbours_keeps_its_quarter() {
    let band = Band {
        surface: |s, _| {
            let p = s.place([0.0, 0.0, 0.0], Z, X);
            s.add(format!("TOROIDAL_SURFACE('',#{p},1.5,0.5)"))
        },
        seam: ([1.5, 0.0, 0.0], 0.5, [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    };
    let (big, a) = (1.5, 0.5);
    let want =
        PI * 4.0 * 3.0 + PI * (big * big * a + PI * big * a * a / 2.0 + 2.0 * a * a * a / 3.0);
    for bound in [true, false] {
        near(volume(&pin(2.0, 3.0, big, -a, &band, bound)), want);
    }
}

/// Issue #316: a `SURFACE_OF_REVOLUTION` of a circle touching its axis is
/// a horn torus by another name.
#[test]
fn a_revolution_of_a_circle_touching_its_axis_is_a_horn_torus() {
    let band = Band {
        surface: |s, seam| {
            let (o, d) = (s.point([0.0, 0.0, 0.0]), s.dir(Z));
            let axis = s.add(format!("AXIS1_PLACEMENT('',#{o},#{d})"));
            s.add(format!("SURFACE_OF_REVOLUTION('',#{seam},#{axis})"))
        },
        seam: TUBE,
    };
    horn_pin(&band, true);
    horn_pin(&band, false);
}

/// Issue #316: KiCad's axial inductors draw their ends on revolutions of
/// circles hundreds of millimetres across that cross the axis; a face
/// uses an arc clear of it. Here a pin 2 mm in radius, its foot dimpled
/// up to a flat of radius 0.5 at z = 0.5 along the circle through both,
/// centred at (-1, 0, -6.5): it crosses the axis at z = -6.5 ± √50.25.
#[test]
fn a_revolution_of_a_circle_crossing_its_axis_reads_the_arc_its_faces_use() {
    let r = 51.25f64.sqrt();
    let band = Band {
        surface: |s, seam| {
            let (o, d) = (s.point([0.0, 0.0, 0.0]), s.dir(Z));
            let axis = s.add(format!("AXIS1_PLACEMENT('',#{o},#{d})"));
            s.add(format!("SURFACE_OF_REVOLUTION('',#{seam},#{axis})"))
        },
        seam: (
            [-1.0, 0.0, -6.5],
            r,
            [0.0, -1.0, 0.0],
            [3.0 / r, 0.0, 6.5 / r],
        ),
    };
    // The dimple: below the arc's radius `ρ(z) = √(51.25 - (z + 6.5)²) - 1`
    // from z = 0 to 0.5, by Simpson's rule.
    let n = 2000;
    let h = 0.5 / n as f64;
    let rho = |z: f64| (51.25 - (z + 6.5) * (z + 6.5)).sqrt() - 1.0;
    let dimple: f64 = (0..=n)
        .map(|i| {
            let w = if i == 0 || i == n {
                1.0
            } else if i % 2 == 1 {
                4.0
            } else {
                2.0
            };
            w * PI * rho(i as f64 * h).powi(2)
        })
        .sum::<f64>()
        * h
        / 3.0;
    let want = PI * 4.0 * 10.0 - dimple;
    for bound in [true, false] {
        near(volume(&pin(2.0, 10.0, 0.5, 0.5, &band, bound)), want);
    }
}
