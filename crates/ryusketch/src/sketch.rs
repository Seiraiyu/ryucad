//! What a sketch holds, and the checks that make it solvable at all.

use ryukei::tol::SIZE_BOX_HALF;
use std::collections::BTreeMap;
use std::fmt;

/// The longest dimension a sketch may hold (meters): the width of ryukei's
/// size box. Positions lie within `±SIZE_BOX_HALF` and radii are at most
/// that, so [`SOLVE_TOL`](crate::SOLVE_TOL) stays well above f64's
/// resolution everywhere in a sketch.
pub const MAX_LENGTH: f64 = 2.0 * SIZE_BOX_HALF;

/// An entity's or a constraint's ID, chosen by the caller. Entities and
/// constraints have separate ID spaces.
pub type Id = u32;

/// A piece of geometry. Lines, arcs and circles are built on point
/// entities, which constraints may share; their curves are what profiles
/// are made of unless they are `construction`.
#[derive(Clone, Debug, PartialEq)]
pub enum Entity {
    /// A point at `at`. A `fixed` point is a constant (projected geometry
    /// from elsewhere): the solver never moves it.
    Point { at: [f64; 2], fixed: bool },
    /// The segment from point `a` to point `b`.
    Line { a: Id, b: Id, construction: bool },
    /// Counterclockwise from `start` to `end` about `center`, all three
    /// points. Its radius is `|start - center|`; the solver keeps `end` on
    /// the same circle.
    Arc {
        center: Id,
        start: Id,
        end: Id,
        construction: bool,
    },
    /// A full circle about point `center`. A `fixed` circle keeps its
    /// radius.
    Circle {
        center: Id,
        r: f64,
        fixed: bool,
        construction: bool,
    },
}

/// A requirement on entities. Lengths are meters, angles radians.
#[derive(Clone, Debug, PartialEq)]
pub enum Constraint {
    /// Two points are one.
    Coincident(Id, Id),
    /// A line runs along x.
    Horizontal(Id),
    /// A line runs along y.
    Vertical(Id),
    Parallel(Id, Id),
    Perpendicular(Id, Id),
    /// A line and a circle or arc, or two circles or arcs, touch without
    /// crossing. Two curves that share an end touch there.
    Tangent(Id, Id),
    /// Two lines are as long, or two circles or arcs have one radius.
    Equal(Id, Id),
    /// Two points mirror each other about a line.
    Symmetric(Id, Id, Id),
    /// A point is the middle of a line.
    Midpoint(Id, Id),
    /// A point lies on a line (extended), circle or arc (its circle).
    On(Id, Id),
    /// Two circles or arcs share a center.
    Concentric(Id, Id),
    /// An entity stays where it is when the solve starts.
    Fix(Id),
    /// Between two points, a point and a line, or two lines (which makes
    /// them parallel).
    Distance(Id, Id, f64),
    /// The signed distance along x from the first point to the second.
    DistanceX(Id, Id, f64),
    /// The signed distance along y from the first point to the second.
    DistanceY(Id, Id, f64),
    Length(Id, f64),
    /// The angle from the first line's direction to the second's,
    /// counterclockwise; lines are directed from `a` to `b`, and an angle
    /// and the same angle plus a half turn are the same constraint.
    Angle(Id, Id, f64),
    Radius(Id, f64),
    Diameter(Id, f64),
}

/// A sketch: entities and constraints by ID.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sketch {
    pub entities: BTreeMap<Id, Entity>,
    pub constraints: BTreeMap<Id, Constraint>,
}

/// Why a sketch can't be solved at all: it refers to what isn't there, or
/// to the wrong kind of thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SketchError {
    /// An entity refers to an entity that isn't a point, or doesn't exist.
    BadEntity { entity: Id, why: &'static str },
    /// A constraint refers to entities of the wrong kind, or missing ones.
    BadConstraint { constraint: Id, why: &'static str },
}

impl fmt::Display for SketchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SketchError::BadEntity { entity, why } => write!(f, "entity {entity}: {why}"),
            SketchError::BadConstraint { constraint, why } => {
                write!(f, "constraint {constraint}: {why}")
            }
        }
    }
}

impl std::error::Error for SketchError {}

/// What kind an entity is, for checking references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Point,
    Line,
    Arc,
    Circle,
}

impl Entity {
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Entity::Point { .. } => Kind::Point,
            Entity::Line { .. } => Kind::Line,
            Entity::Arc { .. } => Kind::Arc,
            Entity::Circle { .. } => Kind::Circle,
        }
    }

    /// The points it is built on.
    pub fn points(&self) -> Vec<Id> {
        match *self {
            Entity::Point { .. } => vec![],
            Entity::Line { a, b, .. } => vec![a, b],
            Entity::Arc {
                center, start, end, ..
            } => vec![center, start, end],
            Entity::Circle { center, .. } => vec![center],
        }
    }

    /// Whether it is a curve that profiles leave out.
    pub fn construction(&self) -> bool {
        match *self {
            Entity::Point { .. } => true,
            Entity::Line { construction, .. }
            | Entity::Arc { construction, .. }
            | Entity::Circle { construction, .. } => construction,
        }
    }
}

impl Sketch {
    pub(crate) fn kind(&self, id: Id) -> Option<Kind> {
        self.entities.get(&id).map(Entity::kind)
    }

    /// Checks references and numbers: every entity built on distinct,
    /// existing points; every constraint on entities of the kinds it takes;
    /// every position, radius and value finite; lengths and radii not
    /// negative; positions within ryukei's size box, radii at most half
    /// its width and lengths at most [`MAX_LENGTH`]. Past the box, f64
    /// can't tell whether a length holds within [`SOLVE_TOL`](crate::SOLVE_TOL).
    pub fn validate(&self) -> Result<(), SketchError> {
        for (&id, e) in &self.entities {
            let bad = |why| SketchError::BadEntity { entity: id, why };
            let pts = e.points();
            if pts.iter().any(|p| self.kind(*p) != Some(Kind::Point)) {
                return Err(bad("refers to an entity that isn't a point"));
            }
            for (i, p) in pts.iter().enumerate() {
                if pts[..i].contains(p) {
                    return Err(bad("uses one point twice"));
                }
            }
            match *e {
                Entity::Point { at, .. } if !at.iter().all(|x| x.is_finite()) => {
                    return Err(bad("position is not finite"));
                }
                Entity::Point { at, .. } if at.iter().any(|x| x.abs() > SIZE_BOX_HALF) => {
                    return Err(bad("position is outside the size box (±500 m)"));
                }
                Entity::Circle { r, .. } if !r.is_finite() || r < 0.0 => {
                    return Err(bad("radius must be finite and not negative"));
                }
                Entity::Circle { r, .. } if r > SIZE_BOX_HALF => {
                    return Err(bad("radius is larger than the size box allows (500 m)"));
                }
                _ => {}
            }
        }
        for (&id, c) in &self.constraints {
            self.check(c).map_err(|why| SketchError::BadConstraint {
                constraint: id,
                why,
            })?;
        }
        Ok(())
    }

    fn check(&self, c: &Constraint) -> Result<(), &'static str> {
        use Constraint as C;
        use Kind::*;
        let is = |id: Id, kinds: &[Kind]| self.kind(id).is_some_and(|k| kinds.contains(&k));
        let round = [Arc, Circle];
        let curve = [Line, Arc, Circle];
        let ok = match *c {
            C::Coincident(a, b) => is(a, &[Point]) && is(b, &[Point]) && a != b,
            C::Horizontal(l) | C::Vertical(l) => is(l, &[Line]),
            C::Parallel(a, b) | C::Perpendicular(a, b) | C::Angle(a, b, _) => {
                is(a, &[Line]) && is(b, &[Line]) && a != b
            }
            C::Tangent(a, b) => {
                a != b
                    && ((is(a, &[Line]) && is(b, &round))
                        || (is(a, &round) && is(b, &[Line]))
                        || (is(a, &round) && is(b, &round)))
            }
            C::Equal(a, b) => {
                a != b && ((is(a, &[Line]) && is(b, &[Line])) || (is(a, &round) && is(b, &round)))
            }
            C::Symmetric(p, q, l) => is(p, &[Point]) && is(q, &[Point]) && p != q && is(l, &[Line]),
            C::Midpoint(p, l) => is(p, &[Point]) && is(l, &[Line]),
            C::On(p, k) => is(p, &[Point]) && is(k, &curve),
            C::Concentric(a, b) => a != b && is(a, &round) && is(b, &round),
            C::Fix(e) => self.entities.contains_key(&e),
            C::Distance(a, b, _) => {
                a != b
                    && ((is(a, &[Point]) && is(b, &[Point, Line]))
                        || (is(a, &[Line]) && is(b, &[Point, Line])))
            }
            C::DistanceX(a, b, _) | C::DistanceY(a, b, _) => {
                is(a, &[Point]) && is(b, &[Point]) && a != b
            }
            C::Length(l, _) => is(l, &[Line]),
            C::Radius(k, _) | C::Diameter(k, _) => is(k, &round),
        };
        if !ok {
            return Err("refers to entities of the wrong kind, or missing ones");
        }
        let value = match *c {
            C::Distance(_, _, v)
            | C::Length(_, v)
            | C::Radius(_, v)
            | C::Diameter(_, v)
            | C::Angle(_, _, v)
            | C::DistanceX(_, _, v)
            | C::DistanceY(_, _, v) => Some(v),
            _ => None,
        };
        match (c, value) {
            (_, Some(v)) if !v.is_finite() => Err("value is not finite"),
            (C::Distance(..) | C::Length(..) | C::Radius(..) | C::Diameter(..), Some(v))
                if v < 0.0 =>
            {
                Err("a distance, length or radius can't be negative")
            }
            (C::Angle(..), _) => Ok(()),
            (_, Some(v)) if v.abs() > MAX_LENGTH => {
                Err("value is longer than the size box allows (1000 m)")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn point(x: f64, y: f64) -> Entity {
        Entity::Point {
            at: [x, y],
            fixed: false,
        }
    }

    pub(crate) fn line(a: Id, b: Id) -> Entity {
        Entity::Line {
            a,
            b,
            construction: false,
        }
    }

    /// Points 1..=4 at the corners of a 2×1 box, lines 11..=14 round it.
    pub(crate) fn rectangle() -> Sketch {
        let mut s = Sketch::default();
        for (id, x, y) in [(1, 0.0, 0.0), (2, 2.0, 0.0), (3, 2.0, 1.0), (4, 0.0, 1.0)] {
            s.entities.insert(id, point(x, y));
        }
        for (id, a, b) in [(11, 1, 2), (12, 2, 3), (13, 3, 4), (14, 4, 1)] {
            s.entities.insert(id, line(a, b));
        }
        s
    }

    #[test]
    fn a_sound_sketch_validates() {
        let mut s = rectangle();
        s.constraints.insert(1, Constraint::Horizontal(11));
        s.constraints.insert(2, Constraint::Distance(1, 13, 1.0));
        s.constraints.insert(3, Constraint::DistanceX(1, 2, -2.0));
        // The size box's limits are inclusive, and angles have none.
        s.constraints.insert(4, Constraint::Length(12, MAX_LENGTH));
        s.constraints.insert(5, Constraint::Angle(11, 12, 1e6));
        s.entities.insert(5, point(500.0, -500.0));
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn bad_entities() {
        let cases: [(Entity, &str); 8] = [
            (line(1, 99), "refers to an entity that isn't a point"),
            (line(1, 11), "refers to an entity that isn't a point"),
            (line(1, 1), "uses one point twice"),
            (point(f64::NAN, 0.0), "position is not finite"),
            (
                Entity::Circle {
                    center: 1,
                    r: -1.0,
                    fixed: false,
                    construction: false,
                },
                "radius must be finite and not negative",
            ),
            (
                point(0.0, 500.5),
                "position is outside the size box (±500 m)",
            ),
            (
                point(-7.8e20, 0.0),
                "position is outside the size box (±500 m)",
            ),
            (
                Entity::Circle {
                    center: 1,
                    r: 501.0,
                    fixed: false,
                    construction: false,
                },
                "radius is larger than the size box allows (500 m)",
            ),
        ];
        for (e, why) in cases {
            let mut s = rectangle();
            s.entities.insert(50, e);
            assert_eq!(
                s.validate(),
                Err(SketchError::BadEntity { entity: 50, why }),
                "{why}"
            );
        }
    }

    #[test]
    fn bad_constraints() {
        use Constraint as C;
        let wrong = "refers to entities of the wrong kind, or missing ones";
        for (c, why) in [
            (C::Horizontal(1), wrong),
            (C::Coincident(1, 1), wrong),
            (C::Parallel(11, 1), wrong),
            (C::Tangent(11, 12), wrong),
            (C::Equal(11, 1), wrong),
            (C::Radius(11, 1.0), wrong),
            (C::Fix(99), wrong),
            (C::Distance(1, 1, 1.0), wrong),
            (C::Length(11, f64::INFINITY), "value is not finite"),
            (
                C::Length(11, -1.0),
                "a distance, length or radius can't be negative",
            ),
            (
                C::DistanceX(1, 2, -1000.5),
                "value is longer than the size box allows (1000 m)",
            ),
        ] {
            let mut s = rectangle();
            s.constraints.insert(7, c.clone());
            assert_eq!(
                s.validate(),
                Err(SketchError::BadConstraint { constraint: 7, why }),
                "{c:?}"
            );
        }
    }
}
