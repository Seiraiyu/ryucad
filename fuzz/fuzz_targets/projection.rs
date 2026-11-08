//! Views and sections from bytes (design 326, plan 10.C): the first byte
//! a view direction on a coarse grid
//! ([`ryukei_testkit::projection::grid_direction`]), the second a section
//! plane's, the rest a body as the blend target makes them
//! ([`ryukei_testkit::blend_bytes::body`]). Every view is a typed
//! refusal or a projection that is the same twice, whose pieces the
//! mesh oracle doesn't contradict and whose silhouettes are square to
//! the view; every section is a typed refusal or a body that passes the
//! checker with caps on the plane. Calls share one
//! [`ryukei_testkit::FUZZ_BUDGET`] and are cancelled at
//! [`ryukei_testkit::FUZZ_DEADLINE`]. Any panic, and any other error, is
//! a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::Error;
use ryukei::check::check;
use ryukei::geom::{Frame, Plane, Point3, Rigid};
use ryukei::name::OpKey;
use ryukei::project::{Placed, ViewOptions, section_with, view_with};
use ryukei_testkit::projection::{
    Oracle, grid_direction, judge, look, placed_bodies, silhouette_excess, size,
};

fuzz_target!(|data: &[u8]| {
    let [d, c, rest @ ..] = data else {
        return;
    };
    let Some((body, _)) = ryukei_testkit::blend_bytes::body(rest) else {
        return;
    };
    let v = look(grid_direction(*d));
    ryukei_testkit::fuzz_case(|work| {
        let placed = [Placed {
            body: &body,
            place: Rigid::IDENTITY,
        }];
        let first = work.run(|w| view_with(&placed, &v, &ViewOptions::default(), w));
        let p = match first {
            Ok(p) => p,
            Err(Error::InvalidView(_) | Error::BudgetExceeded { .. } | Error::Cancelled) => return,
            Err(e) => panic!("unexpected {e:?}"),
        };
        match work.run(|w| view_with(&placed, &v, &ViewOptions::default(), w)) {
            Ok(again) => assert_eq!(format!("{:?}", again.pieces), format!("{:?}", p.pieces)),
            Err(Error::BudgetExceeded { .. } | Error::Cancelled) => return,
            Err(e) => panic!("unexpected second time {e:?}"),
        }
        let bodies = placed_bodies(&placed);
        assert!(silhouette_excess(&bodies, &p, &v) <= 0.0);
        let s = size(&bodies);
        if let Some(o) = Oracle::new(&bodies, &v, s * 2e-3) {
            let j = judge(&p, &bodies, &o);
            assert!(j.wrong.is_empty(), "{:?}", j.wrong);
        }
        // A section plane through the body's middle, along a grid
        // direction.
        let mid = {
            let b = bodies[0]
                .faces()
                .map(|f| ryukei::intersect::face_box(&bodies[0], f))
                .reduce(|a, b| a.union(&b))
                .unwrap();
            let (lo, hi) = (b.min(), b.max());
            Point3::new(
                (lo.x + hi.x) / 2.0,
                (lo.y + hi.y) / 2.0,
                (lo.z + hi.z) / 2.0,
            )
        };
        let n = grid_direction(*c);
        let hint = if n.x.abs() < 0.9 * n.length() {
            ryukei::geom::Vector3::new(1.0, 0.0, 0.0)
        } else {
            ryukei::geom::Vector3::new(0.0, 1.0, 0.0)
        };
        let plane = Plane::new(Frame::new(mid, n, hint).unwrap());
        match work.run(|w| section_with(OpKey(0x5ec), &body, &plane, w)) {
            Ok(cut) => {
                assert_eq!(check(&cut.body), []);
                for f in cut.faces {
                    for l in cut.body.face_loops(f) {
                        for fin in cut.body.loop_fins(l) {
                            let e = cut.body.fin(fin).unwrap().edge;
                            let c = cut.body.curve(cut.body.edge(e).unwrap().curve).unwrap();
                            let t = cut.body.edge(e).unwrap().interval;
                            let x = c.point((t[0] + t[1]) / 2.0);
                            assert!(
                                ((x - mid).dot(n.normalize().unwrap().get())).abs()
                                    < 1e-6 * s.max(1e-3)
                            );
                        }
                    }
                }
            }
            Err(Error::InvalidBoolean(_) | Error::BudgetExceeded { .. } | Error::Cancelled) => {}
            Err(e) => panic!("unexpected section {e:?}"),
        }
    });
});
