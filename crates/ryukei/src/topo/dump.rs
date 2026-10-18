use super::{Body, Id, RegionKind, Sense};
use crate::geom::{Curve, Frame, KnotVector, Point3, Surface};
use core::fmt::Write;

/// The exact bits of a float, so a dump is bit-exact.
fn bits(x: f64) -> String {
    format!("{:016x}", x.to_bits())
}

fn point(p: Point3) -> String {
    format!("{},{},{}", bits(p.x), bits(p.y), bits(p.z))
}

fn frame(f: &Frame) -> String {
    let (x, z) = (f.x().get(), f.z().get());
    format!(
        "origin={} x={},{},{} z={},{},{}",
        point(f.origin()),
        bits(x.x),
        bits(x.y),
        bits(x.z),
        bits(z.x),
        bits(z.y),
        bits(z.z)
    )
}

/// Distinct knots and multiplicities as `knot:mult,...`.
fn knots(k: &KnotVector) -> String {
    let pairs: Vec<String> = k
        .knots()
        .iter()
        .zip(k.mults())
        .map(|(&x, m)| format!("{}:{m}", bits(x)))
        .collect();
    pairs.join(",")
}

fn points(ps: impl Iterator<Item = Point3>) -> String {
    ps.map(point).collect::<Vec<_>>().join(";")
}

fn floats(xs: &[f64]) -> String {
    xs.iter().map(|&x| bits(x)).collect::<Vec<_>>().join(",")
}

/// A curve's kind and the exact bits of its data.
fn curve(c: &Curve) -> String {
    match c {
        Curve::Line(l) => {
            let d = l.dir().get();
            format!(
                "line origin={} dir={},{},{}",
                point(l.origin()),
                bits(d.x),
                bits(d.y),
                bits(d.z)
            )
        }
        Curve::Circle(c) => format!("circle {} r={}", frame(c.frame()), bits(c.radius())),
        Curve::Ellipse(e) => format!(
            "ellipse {} major={} minor={}",
            frame(e.frame()),
            bits(e.major()),
            bits(e.minor())
        ),
        Curve::Nurbs(n) => format!(
            "nurbs degree={} periodic={} knots={} points={} weights={}",
            n.degree(),
            u8::from(n.knots().is_periodic()),
            knots(n.knots()),
            points(n.control_points()),
            floats(n.weights())
        ),
    }
}

fn opt<I: Id>(id: Option<I>) -> String {
    id.map_or_else(|| "-".to_string(), |i| i.index().to_string())
}

fn sense(s: Sense) -> char {
    match s {
        Sense::Forward => '+',
        Sense::Reversed => '-',
    }
}

impl Body {
    /// A stable text listing of every live entity, its references, its
    /// name and the exact bits of its geometry, in array order: one line
    /// each. Equal dumps mean equal bodies, bit for bit.
    pub fn dump(&self) -> String {
        self.dump_with(true)
    }

    /// [`Body::dump`] without the names, to compare topology and geometry
    /// alone.
    pub fn dump_unnamed(&self) -> String {
        self.dump_with(false)
    }

    fn dump_with(&self, names: bool) -> String {
        let mut out = String::new();
        let name = |n: crate::name::Name| {
            if names {
                format!(" name={n}")
            } else {
                String::new()
            }
        };
        // Writing to a String cannot fail.
        let mut line = |args: core::fmt::Arguments<'_>| {
            out.write_fmt(args).unwrap();
            out.push('\n');
        };
        for (id, r) in self.regions.live() {
            let kind = match r.kind {
                RegionKind::Void => "void",
                RegionKind::Solid => "solid",
            };
            line(format_args!(
                "region {} {kind} shells={}",
                id.index(),
                opt(r.first_shell)
            ));
        }
        for (id, s) in self.shells.live() {
            line(format_args!(
                "shell {} region={} next={} prev={} faces={}",
                id.index(),
                s.region.index(),
                s.next.index(),
                s.prev.index(),
                opt(s.first_face)
            ));
        }
        for (id, f) in self.faces.live() {
            line(format_args!(
                "face {} shell={} next={} prev={} loops={} surface={} sense={}{}",
                id.index(),
                f.shell.index(),
                f.next.index(),
                f.prev.index(),
                opt(f.first_loop),
                f.surface.index(),
                sense(f.sense),
                name(f.name)
            ));
        }
        for (id, l) in self.loops.live() {
            line(format_args!(
                "loop {} face={} next={} prev={} fins={} vertex={}",
                id.index(),
                l.face.index(),
                l.next.index(),
                l.prev.index(),
                opt(l.first_fin),
                opt(l.vertex)
            ));
        }
        for (id, f) in self.fins.live() {
            line(format_args!(
                "fin {} loop={} edge={} sense={} next={} prev={} around={}",
                id.index(),
                f.loop_.index(),
                f.edge.index(),
                sense(f.sense),
                f.next.index(),
                f.prev.index(),
                f.around.index()
            ));
        }
        for (id, e) in self.edges.live() {
            line(format_args!(
                "edge {} fin={} start={} end={} curve={} interval={},{} tol={}{}",
                id.index(),
                e.fin.index(),
                e.start.index(),
                e.end.index(),
                e.curve.index(),
                bits(e.interval[0]),
                bits(e.interval[1]),
                bits(e.tolerance.get()),
                name(e.name)
            ));
        }
        for (id, v) in self.vertices.live() {
            line(format_args!(
                "vertex {} point={} tol={} edge={} lone={}{}",
                id.index(),
                point(v.point),
                bits(v.tolerance.get()),
                opt(v.edge),
                opt(v.lone),
                name(v.name)
            ));
        }
        for (id, c) in self.curves.live() {
            let geom = curve(c);
            line(format_args!("curve {} {geom}", id.index()));
        }
        for (id, s) in self.surfaces.live() {
            let f = s.frame().map(frame).unwrap_or_default();
            let geom = match s {
                Surface::Plane(_) => format!("plane {f}"),
                Surface::Cylinder(c) => format!("cylinder {f} r={}", bits(c.radius())),
                Surface::Cone(c) => format!(
                    "cone {f} r={} angle={}",
                    bits(c.radius()),
                    bits(c.half_angle())
                ),
                Surface::Sphere(c) => format!("sphere {f} r={}", bits(c.radius())),
                Surface::Torus(t) => format!(
                    "torus {f} major={} minor={}",
                    bits(t.major()),
                    bits(t.minor())
                ),
                Surface::Extrusion(e) => {
                    let d = e.dir().get();
                    format!(
                        "extrusion dir={},{},{} curve={}",
                        bits(d.x),
                        bits(d.y),
                        bits(d.z),
                        curve(e.curve())
                    )
                }
                Surface::Revolution(r) => {
                    format!("revolution {} curve={}", frame(r.frame()), curve(r.curve()))
                }
                Surface::Nurbs(n) => {
                    let ([p, q], [nu, nv]) = (n.degrees(), n.counts());
                    let [ku, kv] = n.knots();
                    format!(
                        "nurbs degrees={p},{q} periodic={},{} counts={nu},{nv} uknots={} vknots={} points={} weights={}",
                        u8::from(ku.is_periodic()),
                        u8::from(kv.is_periodic()),
                        knots(ku),
                        knots(kv),
                        points(n.control_points()),
                        floats(n.weights())
                    )
                }
            };
            line(format_args!("surface {} {geom}", id.index()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::{Circle, Cone, Curve, Ellipse, Frame, Sphere, Surface, Torus};
    use crate::topo::fixture::tetrahedron;

    #[test]
    fn dump_lists_every_entity_in_order() {
        let b = tetrahedron();
        let d = b.dump();
        let count = |prefix: &str| d.lines().filter(|l| l.starts_with(prefix)).count();
        assert_eq!(count("region "), 2);
        assert_eq!(count("shell "), 1);
        assert_eq!(count("face "), 4);
        assert_eq!(count("loop "), 4);
        assert_eq!(count("fin "), 12);
        assert_eq!(count("edge "), 6);
        assert_eq!(count("vertex "), 4);
        assert_eq!(count("curve "), 6);
        assert_eq!(count("surface "), 4);
        let lines: Vec<&str> = d.lines().collect();
        assert_eq!(lines[0], "region 0 void shells=-");
        assert_eq!(lines[1], "region 1 solid shells=0");
        assert_eq!(lines[2], "shell 0 region=1 next=0 prev=0 faces=0");
        let v1 = d.lines().find(|l| l.starts_with("vertex 1 ")).unwrap();
        assert!(
            v1.starts_with(
                "vertex 1 point=3ff0000000000000,0000000000000000,0000000000000000 \
                 tol=3e45798ee2308c3a edge=1 lone=- name="
            ),
            "{v1}"
        );
        assert!(d.contains("curve 0 line origin=0000000000000000,"));
        assert!(d.contains("surface 0 plane origin="));
    }

    #[test]
    fn names_are_optional_and_dumps_are_stable() {
        let b = tetrahedron();
        assert_eq!(b.dump(), b.clone().dump());
        assert!(b.dump().contains(" name="));
        assert!(!b.dump_unnamed().contains("name="));
        assert_eq!(b.dump_unnamed().lines().count(), b.dump().lines().count());
    }

    #[test]
    fn every_curve_kind_dumps() {
        let mut b = tetrahedron();
        let f = Frame::WORLD;
        let before = b.dump().lines().filter(|l| l.starts_with("curve ")).count();
        b.add_curve(Curve::Circle(Circle::new(f, 2.0).unwrap()));
        b.add_curve(Curve::Ellipse(Ellipse::new(f, 3.0, 2.0).unwrap()));
        let d = b.dump();
        assert!(d.contains(&format!(
            "curve {} circle origin=0000000000000000,0000000000000000,0000000000000000 \
             x=3ff0000000000000,0000000000000000,0000000000000000 \
             z=0000000000000000,0000000000000000,3ff0000000000000 r=4000000000000000",
            before
        )));
        assert!(d.contains(&format!(
            "curve {} ellipse origin=0000000000000000,0000000000000000,0000000000000000 \
             x=3ff0000000000000,0000000000000000,0000000000000000 \
             z=0000000000000000,0000000000000000,3ff0000000000000 \
             major=4008000000000000 minor=4000000000000000",
            before + 1
        )));
    }

    #[test]
    fn nurbs_dump_their_data() {
        use crate::geom::Plane;
        use std::sync::Arc;
        let (n, _) = Circle::new(Frame::WORLD, 2.0).unwrap().to_nurbs().unwrap();
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let (s, _) = plane.to_nurbs([[0.0, 1.0], [0.0, 2.0]]).unwrap();
        let mut b = tetrahedron();
        let shared = Arc::new(n.clone());
        b.add_curve(Curve::Nurbs(shared.clone()));
        b.add_curve(Curve::Nurbs(shared));
        b.add_surface(Surface::Nurbs(Arc::new(s)));
        let d = b.dump();
        let lines: Vec<&str> = d.lines().filter(|l| l.contains(" nurbs ")).collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains(" nurbs degree=2 periodic=1 knots=0000000000000000:2,"));
        assert!(lines[0].contains(" weights=3fe6a09e667f3bcd,3ff0000000000000,"));
        assert!(lines[2].contains(" nurbs degrees=1,1 periodic=0,0 counts=2,2 "));
        assert!(lines[2].contains(&format!("{:016x}", 2.0f64.to_bits())));
        // A shared curve and an equal private copy dump alike.
        let mut c = tetrahedron();
        c.add_curve(Curve::Nurbs(Arc::new(n.clone())));
        c.add_curve(Curve::Nurbs(Arc::new(n)));
        let mut d2 = c.dump();
        d2.push_str(lines[2]);
        d2.push('\n');
        assert_eq!(d, d2);
    }

    #[test]
    fn every_surface_kind_dumps() {
        let mut b = tetrahedron();
        let f = Frame::WORLD;
        b.add_surface(Surface::Cone(Cone::new(f, 1.0, 0.5).unwrap()));
        b.add_surface(Surface::Sphere(Sphere::new(f, 1.0).unwrap()));
        b.add_surface(Surface::Torus(Torus::new(f, 2.0, 1.0).unwrap()));
        let d = b.dump();
        assert!(d.contains(
            "surface 5 sphere origin=0000000000000000,0000000000000000,0000000000000000 \
             x=3ff0000000000000,0000000000000000,0000000000000000 \
             z=0000000000000000,0000000000000000,3ff0000000000000 r=3ff0000000000000"
        ));
        assert!(d.contains("surface 4 cone "));
        assert!(d.contains(" angle=3fe0000000000000"));
        assert!(d.contains("surface 6 torus "));
        assert!(d.contains(" major=4000000000000000 minor=3ff0000000000000"));
    }

    #[test]
    fn swept_surfaces_dump_their_curves() {
        use crate::geom::{Extrusion, Vector3};
        use std::sync::Arc;
        let mut b = tetrahedron();
        let circle = Curve::Circle(Circle::new(Frame::WORLD, 2.0).unwrap());
        let e = Extrusion::new(circle, Vector3::new(0.0, 0.0, 1.0)).unwrap();
        b.add_surface(Surface::Extrusion(Arc::new(e)));
        let d = b.dump();
        assert!(d.contains(
            "surface 4 extrusion dir=0000000000000000,0000000000000000,3ff0000000000000 \
             curve=circle origin=0000000000000000,0000000000000000,0000000000000000 \
             x=3ff0000000000000,0000000000000000,0000000000000000 \
             z=0000000000000000,0000000000000000,3ff0000000000000 r=4000000000000000\n"
        ));
        // A torus's tube about the world z axis.
        use crate::geom::{Point3, Revolution};
        let tube = Frame::new(
            Point3::new(3.0, 0.0, 0.0),
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let tube = Curve::Circle(Circle::new(tube, 1.0).unwrap());
        let r = Revolution::new(Frame::WORLD, tube).unwrap();
        b.add_surface(Surface::Revolution(Arc::new(r)));
        let d = b.dump();
        assert!(d.contains(
            "surface 5 revolution origin=0000000000000000,0000000000000000,0000000000000000 \
             x=3ff0000000000000,0000000000000000,0000000000000000 \
             z=0000000000000000,0000000000000000,3ff0000000000000 \
             curve=circle origin=4008000000000000,0000000000000000,0000000000000000 \
             x=3ff0000000000000,0000000000000000,0000000000000000 \
             z=0000000000000000,bff0000000000000,0000000000000000 r=3ff0000000000000\n"
        ));
    }
}
