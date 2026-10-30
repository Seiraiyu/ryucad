use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}
fn sub(a: &ryukei::topo::Body, b: &ryukei::topo::Body, p: Rigid, k: u128) -> ryukei::topo::Body {
    boolean(
        OpKey(k),
        BooleanOp::Subtract,
        a,
        &[Tool {
            key: ToolKey(1),
            body: b,
            place: p,
        }],
    )
    .unwrap()
    .0
}

#[test]
fn scratch() {
    let (ro, ri, len) = (0.036205, 0.033775, 0.02);
    // A block with a bore of radius ro along z.
    let block = primitive::block(OpKey(1), 0.12, 0.12, len, &at(-0.06, -0.06, 0.0))
        .unwrap()
        .0;
    let bore = primitive::cylinder(OpKey(2), ro, 2.0 * len, &at(0.0, 0.0, -0.5 * len))
        .unwrap()
        .0;
    let a = sub(&block, &bore, Rigid::IDENTITY, 10);
    // A half shell: the ring between ri and ro, its lower half cut away.
    let outer = primitive::cylinder(OpKey(3), ro, len, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let inner = primitive::cylinder(OpKey(4), ri, 2.0 * len, &at(0.0, 0.0, -0.5 * len))
        .unwrap()
        .0;
    let ring = sub(&outer, &inner, Rigid::IDENTITY, 11);
    let half = primitive::block(OpKey(5), 0.2, 0.1, 2.0 * len, &at(-0.1, -0.1, -0.5 * len))
        .unwrap()
        .0;
    let shell = sub(&ring, &half, Rigid::IDENTITY, 12);
    let mut bad = 0;
    for i in 0..200 {
        let ax = Vector3::new(1.0, 0.3 + i as f64 * 0.01, 0.7)
            .normalize()
            .unwrap();
        let t = 0.1 + i as f64 * 0.37;
        let pa = Rigid::rotation(ax, t).then_after(&at(0.4, 0.1 * i as f64, -0.2));
        let pb = Rigid::rotation(ax, t).then_after(&at(0.4, 0.1 * i as f64, -0.2));
        // as interference: b placed in a's frame
        let rel = pa.inverse().then_after(&pb);
        let rel2 = pb.then_after(&pa.inverse());
        for place in [rel, rel2] {
            let r = boolean(
                OpKey(20),
                BooleanOp::Intersect,
                &a,
                &[Tool {
                    key: ToolKey(1),
                    body: &shell,
                    place,
                }],
            );
            if let Err(e) = &r {
                bad += 1;
                if bad < 4 {
                    eprintln!("SCRATCH bad {i}: {e:?} {place:?}");
                }
            }
        }
    }
    eprintln!("SCRATCH bore+shell bad {bad} of 400");
    // A cap on a seat: two blocks sharing a face.
    let b1 = primitive::block(OpKey(6), 0.1, 0.1, 0.1, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let b2 = primitive::block(OpKey(7), 0.05, 0.05, 0.05, &at(0.02, 0.02, 0.1))
        .unwrap()
        .0;
    let r = boolean(
        OpKey(21),
        BooleanOp::Intersect,
        &b1,
        &[Tool {
            key: ToolKey(1),
            body: &b2,
            place: Rigid::IDENTITY,
        }],
    );
    eprintln!(
        "SCRATCH seat intersect: {:?}",
        r.as_ref().map(|b| b.0.counts().faces)
    );
}

#[test]
fn scratch_revolved() {
    use ryukei::geom::{Axis, Curve2, Frame, Line2, Point2, Point3};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, revolve};
    let mm = 0.001;
    let ro_hole = 72.41 * mm / 2.0;
    let (ri, ro, len) = (33.775 * mm, 36.205 * mm, 20.0 * mm);
    eprintln!("SCRATCH radii {ro_hole:e} {ro:e} diff {:e}", ro - ro_hole);
    let block = primitive::block(OpKey(1), 0.12, 0.12, len, &at(-0.06, -0.06, 0.0))
        .unwrap()
        .0;
    let bore = primitive::cylinder(OpKey(2), ro_hole, 2.0 * len, &at(0.0, 0.0, -0.5 * len))
        .unwrap()
        .0;
    let a = sub(&block, &bore, Rigid::IDENTITY, 10);
    // The profile in the xz plane: x from ri to ro, z from 0 to len.
    let pts = [
        Point2::new(ri, 0.0),
        Point2::new(ro, 0.0),
        Point2::new(ro, len),
        Point2::new(ri, len),
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
    let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let shell = revolve(OpKey(30), &profile, axis, 0.0, core::f64::consts::PI)
        .unwrap()
        .0;
    let mut bad = 0;
    for i in 0..100 {
        let ax = Vector3::new(1.0, 0.3 + i as f64 * 0.01, 0.7)
            .normalize()
            .unwrap();
        let pa =
            Rigid::rotation(ax, 0.1 + i as f64 * 0.37).then_after(&at(0.4, 0.1 * i as f64, -0.2));
        let place = pa.inverse().then_after(&pa);
        let r = boolean(
            OpKey(20),
            BooleanOp::Intersect,
            &a,
            &[Tool {
                key: ToolKey(1),
                body: &shell,
                place,
            }],
        );
        match &r {
            Err(e) => {
                bad += 1;
                if bad < 4 {
                    eprintln!("SCRATCH bad {i}: {e:?}");
                }
            }
            Ok(b) if b.0.counts().faces > 0 => {
                eprintln!("SCRATCH {i}: faces {}", b.0.counts().faces)
            }
            _ => {}
        }
    }
    eprintln!("SCRATCH revolved bad {bad} of 100");
}
