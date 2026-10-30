use ryukei::geom::Rigid;
use ryukei::name::OpKey;
use ryukei_bench::occt;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};

fn to_occt(name: &str, b: &ryukei::topo::Body) -> occt::Solid {
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: name.into(),
                name: name.into(),
                bodies: vec![b.clone()],
                children: vec![],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Meter,
            time_stamp: "2026-10-30T00:00:00".into(),
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let path = std::path::Path::new("/work/target").join(format!("ryukei-8f-{name}.stp"));
    let mut text = String::from_utf8(bytes).unwrap();
    if std::env::var_os("OPEN_FLAG").is_some() {
        text = text
            .replace(
                ".UNSPECIFIED.,.T.,.F.,.F.)B_SPLINE_SURFACE_WITH_KNOTS",
                ".UNSPECIFIED.,.F.,.F.,.F.)B_SPLINE_SURFACE_WITH_KNOTS",
            )
            .replace(
                ".UNSPECIFIED.,.F.,.T.,.F.)B_SPLINE_SURFACE_WITH_KNOTS",
                ".UNSPECIFIED.,.F.,.F.,.F.)B_SPLINE_SURFACE_WITH_KNOTS",
            )
            .replace(
                ".UNSPECIFIED.,.T.,.F.)B_SPLINE_CURVE_WITH_KNOTS",
                ".UNSPECIFIED.,.F.,.F.)B_SPLINE_CURVE_WITH_KNOTS",
            );
    }
    std::fs::write(&path, text).unwrap();
    occt::Solid::read_step(&path, false).unwrap()
}

#[test]
fn scratch() {
    use ryukei::measure::{Entity, Properties, properties};
    let mut c = ryukei::primitive::cylinder(OpKey(4), 0.5, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let plain = to_occt("plain", &c);
    ryukei_testkit::nurbs::convert_body(&mut c).unwrap();
    let n = to_occt("nurbs", &c);
    for deg in [0.0f64, 30.0, 45.0, 60.0, 90.0, 135.0, 200.0, 315.0] {
        let a = deg.to_radians();
        let p = ryukei::geom::Point3::new(500.0 * a.cos(), 500.0 * a.sin(), 1000.0);
        eprintln!("SCRATCH at {deg} deg: distance {:e} mm", n.distance(p));
    }
    let Properties::Body { volume, .. } = properties(&c, Entity::Body).unwrap() else {
        unreachable!()
    };
    eprintln!(
        "SCRATCH cylinder ryukei {:?} occt analytic {} occt nurbs {} exact {}",
        volume,
        plain.volume() * 1e-9,
        n.volume() * 1e-9,
        core::f64::consts::PI * 0.5
    );
    let mut s = ryukei::primitive::sphere(OpKey(4), 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    ryukei_testkit::nurbs::convert_body(&mut s).unwrap();
    eprintln!(
        "SCRATCH sphere occt nurbs {} exact {}",
        to_occt("sphere", &s).volume() * 1e-9,
        4.0 / 3.0 * core::f64::consts::PI * 0.125
    );
    {
        use ryukei::geom::{Axis, Curve2, Frame, Line2, Point2, Point3, Vector3};
        use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, revolve};
        let pts = [
            Point2::new(0.0, 0.0),
            Point2::new(0.5, 0.0),
            Point2::new(0.5, 2.0),
            Point2::new(0.0, 2.0),
        ];
        let segments = (0..4)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % 4]);
                Segment {
                    key: SegmentKey(i as u128 + 1),
                    curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                    t0: 0.0,
                    t1: p.distance(q),
                }
            })
            .collect();
        let profile = ValidProfile::new(Profile {
            frame: Frame::new(
                Point3::ORIGIN,
                Vector3::new(0.0, -1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
            loops: vec![Loop {
                key: LoopKey(1),
                segments,
            }],
        })
        .unwrap();
        let mut half = revolve(
            OpKey(30),
            &profile,
            Axis::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap(),
            0.0,
            core::f64::consts::PI,
        )
        .unwrap()
        .0;
        ryukei_testkit::nurbs::convert_body(&mut half).unwrap();
        let h = to_occt("half", &half);
        h.write_step(std::path::Path::new("/work/target/half-back.stp"), true)
            .unwrap();
        for deg in [10.0f64, 45.0, 90.0, 135.0, 170.0] {
            let a = deg.to_radians();
            let p = Point3::new(500.0 * a.cos(), 500.0 * a.sin(), 1000.0);
            eprintln!("SCRATCH half at {deg} deg: distance {:e} mm", h.distance(p));
        }
    }
    {
        let block = ryukei_testkit::nurbs_race::block();
        let ob = to_occt("block", &block);
        let top = block
            .faces()
            .max_by(|&x, &y| {
                ryukei::intersect::face_box(&block, x)
                    .max()
                    .z
                    .total_cmp(&ryukei::intersect::face_box(&block, y).max().z)
            })
            .unwrap();
        let s = block.surface(block.face(top).unwrap().surface).unwrap();
        let [du, dv] = s.domain();
        let mut worst: f64 = 0.0;
        for i in 1..20 {
            for j in 1..20 {
                let p = s.point(
                    du[0] + (du[1] - du[0]) * i as f64 / 20.0,
                    dv[0] + (dv[1] - dv[0]) * j as f64 / 20.0,
                );
                worst = worst.max(ob.distance(ryukei::geom::Point3::new(
                    p.x * 1e3,
                    p.y * 1e3,
                    p.z * 1e3,
                )));
            }
        }
        let rational =
            matches!(s, ryukei::geom::Surface::Nurbs(n) if n.weights().iter().any(|&w| w != 1.0));
        eprintln!("SCRATCH block top rational {rational}: worst distance {worst:e} mm");
        let pin = ryukei_testkit::nurbs_race::clear_pin();
        let op = to_occt("pin", &pin);
        let side = pin
            .faces()
            .find(|&f| {
                matches!(
                    pin.surface(pin.face(f).unwrap().surface),
                    Some(ryukei::geom::Surface::Nurbs(_))
                )
            })
            .unwrap();
        let s = pin.surface(pin.face(side).unwrap().surface).unwrap();
        let [du, dv] = s.domain();
        let mut worst: f64 = 0.0;
        for i in 0..20 {
            for j in 1..20 {
                let p = s.point(
                    du[0] + (du[1] - du[0]) * i as f64 / 20.0,
                    dv[0] + (dv[1] - dv[0]) * j as f64 / 20.0,
                );
                worst = worst.max(op.distance(ryukei::geom::Point3::new(
                    p.x * 1e3,
                    p.y * 1e3,
                    p.z * 1e3,
                )));
            }
        }
        let rational =
            matches!(s, ryukei::geom::Surface::Nurbs(n) if n.weights().iter().any(|&w| w != 1.0));
        let periodic = matches!(s, ryukei::geom::Surface::Nurbs(n) if n.periods() != (None, None));
        eprintln!(
            "SCRATCH pin side rational {rational} periodic {periodic}: worst distance {worst:e} mm"
        );
    }
    {
        use ryukei::geom::{Curve2, Frame, KnotVector, NurbsCurve2, Point2, Point3, Vector3};
        use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
        let n = 12;
        let pts = (0..n)
            .map(|i| {
                let t = core::f64::consts::TAU * i as f64 / n as f64;
                let r = 0.3 * (1.0 + 0.06 * (3.0 * t).cos());
                Point2::new(r * t.cos(), r * t.sin())
            })
            .collect();
        let knots: Vec<f64> = (0..=n).map(|i| i as f64).collect();
        let k = KnotVector::periodic(3, &knots, &vec![1; knots.len()]).unwrap();
        let c = Curve2::Nurbs2(std::sync::Arc::new(NurbsCurve2::new(k, pts, None).unwrap()));
        let profile = ValidProfile::new(Profile {
            frame: Frame::WORLD,
            loops: vec![Loop {
                key: LoopKey(1),
                segments: vec![Segment {
                    key: SegmentKey(1),
                    curve: c,
                    t0: 0.0,
                    t1: 12.0,
                }],
            }],
        })
        .unwrap();
        let mut b = extrude(OpKey(40), &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
            .unwrap()
            .0;
        for label in ["extrusion", "nurbs"] {
            if label == "nurbs" {
                ryukei_testkit::nurbs::convert_body(&mut b).unwrap();
            }
            let o = to_occt(label, &b);
            let side = b
                .faces()
                .find(|&f| {
                    !matches!(
                        b.surface(b.face(f).unwrap().surface),
                        Some(ryukei::geom::Surface::Plane(_))
                    )
                })
                .unwrap();
            let s = b.surface(b.face(side).unwrap().surface).unwrap();
            let [du, dv] = s.domain();
            let mut worst: f64 = 0.0;
            for i in 0..24 {
                for j in 1..10 {
                    let p = s.point(
                        du[0] + (du[1] - du[0]) * i as f64 / 24.0,
                        (dv[0].max(0.0) + (dv[1].min(1.0) - dv[0].max(0.0)) * j as f64 / 10.0),
                    );
                    worst = worst.max(o.distance(Point3::new(p.x * 1e3, p.y * 1e3, p.z * 1e3)));
                }
            }
            eprintln!("SCRATCH wavy {label}: worst distance {worst:e} mm");
        }
    }
    let o = occt::Solid::new(
        ryukei_bench::Primitive::Cylinder { r: 0.5, h: 2.0 },
        &Rigid::IDENTITY,
    )
    .to_nurbs();
    o.write_step(std::path::Path::new("/work/target/occt-cyl.stp"), true)
        .unwrap();
    let back =
        occt::Solid::read_step(std::path::Path::new("/work/target/occt-cyl.stp"), false).unwrap();
    eprintln!(
        "SCRATCH occt's own nurbs cylinder {} back {}",
        o.volume(),
        back.volume()
    );
}
