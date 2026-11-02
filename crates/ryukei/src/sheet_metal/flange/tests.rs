use super::super::{EdgeFlange, FlangePosition, Measure, Relief, SheetOptions, recognize};
use super::plan::{Planned, SHORTEST, plan};
use crate::geom::{Point3, Rigid};
use crate::name::{Name, OpKey};
use crate::topo::Body;
use crate::{FlangeSize, SheetMetalError};
use core::f64::consts::{FRAC_PI_2, PI};

pub(super) const T: f64 = 0.001;
pub(super) const R: f64 = 0.0015;
/// The plate's extent along `x` and `y`.
pub(super) const LX: f64 = 0.04;
pub(super) const LY: f64 = 0.03;

/// A plate `LX × LY × T` from the origin.
pub(super) fn plate() -> Body {
    crate::primitive::block(OpKey(1), LX, LY, T, &Rigid::IDENTITY)
        .unwrap()
        .0
}

/// The name of the edge whose middle is nearest `p`.
pub(super) fn edge_at(b: &Body, p: Point3) -> Name {
    let e = b
        .edges()
        .min_by(|&x, &y| {
            let d = |e| super::super::shape::middle(b, e).distance(p);
            d(x).total_cmp(&d(y))
        })
        .unwrap();
    b.edges[e].name
}

/// The plate's top edge along its `+x` wall.
pub(super) fn east(b: &Body) -> Name {
    edge_at(b, Point3::new(LX, 0.5 * LY, T))
}

/// A square flange 10 mm long, material inside, on `edge`.
pub(super) fn square(edge: Name) -> EdgeFlange {
    EdgeFlange {
        edge,
        side: None,
        angle: FRAC_PI_2,
        length: 0.01,
        measured: Measure::Tangent,
        radius: R,
        position: FlangePosition::MaterialInside,
        setbacks: [0.0, 0.0],
        reliefs: [Relief::None; 2],
    }
}

fn planned(b: &Body, specs: &[EdgeFlange]) -> Result<Vec<Planned>, SheetMetalError> {
    let m = recognize(b, &SheetOptions::default()).unwrap();
    plan(b, &m, specs, 1e-7)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12
}

#[test]
fn a_flange_is_planned_in_its_edges_frame() {
    let b = plate();
    let e = east(&b);
    let p = planned(&b, &[square(e)]).unwrap();
    assert_eq!(p.len(), 1);
    let p = &p[0];
    assert_eq!(p.edge, e);
    assert!(close(p.length, LY));
    assert!(close(p.thickness, T));
    // Folding up, out of the +x wall.
    assert!(close(p.normal.get().z, 1.0));
    assert!(close(p.out.get().x, 1.0));
    assert!(close(p.along.get().y.abs(), 1.0));
    assert!(close(p.trim, R + T));
    assert!(close(p.straight, 0.01));
    assert_eq!(p.span, [0.0, p.length]);
    // The wall's top edge at `x` = 0 and `h` = 0.
    let q = p.at(0.5 * LY, 0.0, 0.0);
    assert!(q.distance(Point3::new(LX, 0.5 * LY, T)) < 1e-12);
}

#[test]
fn positions_trim_and_measures_shorten() {
    let b = plate();
    let e = east(&b);
    let theta = PI / 3.0;
    let tan = crate::math::tan(0.5 * theta);
    for (position, trim) in [
        (FlangePosition::MaterialInside, R + T),
        (FlangePosition::MaterialOutside, R),
        (FlangePosition::BendOutside, 0.0),
    ] {
        for (measured, straight) in [
            (Measure::Tangent, 0.01),
            (Measure::OuterMould, 0.01 - (R + T) * tan),
            (Measure::InnerMould, 0.01 - R * tan),
        ] {
            let spec = EdgeFlange {
                angle: theta,
                measured,
                position,
                ..square(e)
            };
            let p = &planned(&b, &[spec]).unwrap()[0];
            assert!(close(p.trim, trim), "{position:?}");
            assert!(close(p.straight, straight), "{measured:?}");
        }
    }
}

#[test]
fn edges_and_sizes_out_of_range_are_refused() {
    let b = plate();
    let e = east(&b);
    let bad = |what| SheetMetalError::BadSize { what, edge: e };
    let cases = [
        (
            EdgeFlange {
                radius: 0.0,
                ..square(e)
            },
            bad(FlangeSize::Radius),
        ),
        (
            EdgeFlange {
                radius: f64::NAN,
                ..square(e)
            },
            bad(FlangeSize::Radius),
        ),
        (
            EdgeFlange {
                angle: 0.0,
                ..square(e)
            },
            bad(FlangeSize::Angle),
        ),
        (
            EdgeFlange {
                angle: 3.2,
                ..square(e)
            },
            bad(FlangeSize::Angle),
        ),
        (
            EdgeFlange {
                length: 0.0,
                ..square(e)
            },
            bad(FlangeSize::Length),
        ),
        (
            EdgeFlange {
                angle: PI,
                measured: Measure::OuterMould,
                ..square(e)
            },
            bad(FlangeSize::Length),
        ),
        (
            EdgeFlange {
                length: 0.002,
                measured: Measure::OuterMould,
                ..square(e)
            },
            bad(FlangeSize::Length),
        ),
        (
            EdgeFlange {
                setbacks: [-0.001, 0.0],
                ..square(e)
            },
            bad(FlangeSize::Setback),
        ),
        (
            EdgeFlange {
                setbacks: [0.02, 0.01],
                ..square(e)
            },
            bad(FlangeSize::Setback),
        ),
        (
            EdgeFlange {
                setbacks: [0.005, 0.0],
                reliefs: [
                    Relief::Rectangular {
                        width: 0.0,
                        depth: 0.001,
                    },
                    Relief::None,
                ],
                ..square(e)
            },
            bad(FlangeSize::Relief),
        ),
        (
            EdgeFlange {
                setbacks: [0.005, 0.0],
                reliefs: [
                    Relief::Obround {
                        width: 0.002,
                        depth: 0.0005,
                    },
                    Relief::None,
                ],
                ..square(e)
            },
            bad(FlangeSize::Relief),
        ),
    ];
    for (spec, want) in cases {
        assert_eq!(planned(&b, &[spec]), Err(want), "{spec:?}");
    }
    let n = Name::from_u128(5);
    assert_eq!(
        planned(&b, &[square(n)]),
        Err(SheetMetalError::NoSuchEdge { name: n })
    );
    assert_eq!(
        planned(&b, &[square(e), square(e)]),
        Err(SheetMetalError::DuplicateFlange { edge: e })
    );
    // A vertical edge of the plate is between two walls.
    let up = edge_at(&b, Point3::new(LX, LY, 0.5 * T));
    assert_eq!(
        planned(&b, &[square(up)]),
        Err(SheetMetalError::NotAFlangeEdge { edge: up })
    );
    // `side` naming the wall.
    let wall = b
        .faces()
        .map(|f| b.faces[f].name)
        .find(|&n| {
            let f = b.faces().find(|&f| b.faces[f].name == n).unwrap();
            super::super::shape::flat(&b, f).is_some_and(|p| p.normal.get().x > 0.5)
        })
        .unwrap();
    assert_eq!(
        planned(
            &b,
            &[EdgeFlange {
                side: Some(wall),
                ..square(e)
            }]
        ),
        Err(SheetMetalError::NotAFlangeEdge { edge: e })
    );
}

#[test]
fn a_setback_cutting_into_the_base_needs_a_relief() {
    let b = plate();
    let e = east(&b);
    let spec = EdgeFlange {
        setbacks: [0.005, 0.0],
        ..square(e)
    };
    let p = &planned(&b, &[spec]).unwrap_err();
    let SheetMetalError::ReliefNeeded { edge, vertex } = p else {
        panic!("{p:?}");
    };
    assert_eq!(*edge, e);
    // The start vertex.
    let v = b
        .vertices()
        .find(|&v| b.vertices[v].name == *vertex)
        .unwrap();
    let ed = b.edges().find(|&x| b.edges[x].name == e).unwrap();
    assert_eq!(v, b.edges[ed].start);
    // A bend outside cuts nothing: no relief needed, and one given isn't cut.
    let out = EdgeFlange {
        position: FlangePosition::BendOutside,
        reliefs: [Relief::Tear { width: 0.001 }; 2],
        ..spec
    };
    let p = &planned(&b, &[out]).unwrap()[0];
    assert_eq!(p.reliefs, [Relief::None; 2]);
    // With one, the start's relief is kept; the end has no setback.
    let ok = EdgeFlange {
        reliefs: [Relief::Rectangular {
            width: 0.001,
            depth: 0.001,
        }; 2],
        ..spec
    };
    let p = &planned(&b, &[ok]).unwrap()[0];
    assert_eq!(
        p.reliefs,
        [
            Relief::Rectangular {
                width: 0.001,
                depth: 0.001
            },
            Relief::None
        ]
    );
    assert!(close(p.span[0], 0.005));
}

#[test]
fn flanges_meeting_at_a_corner_are_refused_and_setbacks_part_them() {
    let b = plate();
    let e = east(&b);
    let n = edge_at(&b, Point3::new(0.5 * LX, LY, T));
    let corner = b
        .vertices()
        .find(|&v| b.vertices[v].point.distance(Point3::new(LX, LY, T)) < 1e-12)
        .map(|v| b.vertices[v].name)
        .unwrap();
    assert_eq!(
        planned(&b, &[square(e), square(n)]),
        Err(SheetMetalError::FlangesMeet { vertex: corner })
    );
    // Each set back from the corner past the other's trim and relief.
    let relief = Relief::Rectangular {
        width: 0.001,
        depth: 0.0005,
    };
    let back = |edge: Name, at_corner: usize| {
        let mut setbacks = [0.0; 2];
        let mut reliefs = [Relief::None; 2];
        setbacks[at_corner] = R + T + 0.0015 + SHORTEST;
        reliefs[at_corner] = relief;
        EdgeFlange {
            setbacks,
            reliefs,
            ..square(edge)
        }
    };
    let end = |edge: Name| {
        let ed = b.edges().find(|&x| b.edges[x].name == edge).unwrap();
        usize::from(
            b.edges[ed].end
                == b.vertices()
                    .find(|&v| b.vertices[v].name == corner)
                    .unwrap(),
        )
    };
    let specs = [back(e, end(e)), back(n, end(n))];
    assert_eq!(planned(&b, &specs).unwrap().len(), 2);
    // Two flanges on opposite edges never meet.
    let west = edge_at(&b, Point3::new(0.0, 0.5 * LY, T));
    assert_eq!(planned(&b, &[square(e), square(west)]).unwrap().len(), 2);
}
