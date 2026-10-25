//! What the solver solves: rigid bodies, the connectors on them, the mates
//! between connectors, and patterns that place copies of bodies.

use crate::pose::{Pose, norm};
use std::collections::BTreeMap;

/// A caller's ID for a body, a mate or a pattern; opaque to the solver.
pub type Id = u32;

/// The longest distance a mate may drive, meters: f64 can't resolve
/// `SOLVE_TOL` past it.
pub const MAX_LENGTH: f64 = 1000.0;

/// How far from orthonormal a rotation may be, and from unit a direction.
pub const AXIS_TOL: f64 = 1e-9;

/// A rigid body at its stored pose; a fixed one never moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub pose: Pose,
    pub fixed: bool,
}

/// What a connector is on: a body, or copy `index` (`[i, j]`, never
/// `[0, 0]`, which is the seed itself) of a pattern's seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum On {
    Body(Id),
    Copy {
        pattern: Id,
        seed: Id,
        index: [u32; 2],
    },
}

/// A frame on a body: `frame` in the body's own coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Connector {
    pub on: On,
    pub frame: Pose,
}

/// What a mate holds of connector `b`'s frame relative to connector
/// `a`'s, with the values that drive what it leaves free.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// The frames equal.
    Fastened,
    /// Origins and z axes equal; `angle` drives the turn about z, from
    /// `a`'s x to `b`'s.
    Revolute { angle: Option<f64> },
    /// z and x axes equal, `b`'s origin on `a`'s z axis; `distance` drives
    /// it along z.
    Slider { distance: Option<f64> },
    /// z axes equal, `b`'s origin on `a`'s z axis.
    Cylindrical {
        angle: Option<f64>,
        distance: Option<f64>,
    },
    /// z axes equal, `b`'s origin `distance` along `a`'s z from `a`'s xy
    /// plane.
    Planar { angle: Option<f64>, distance: f64 },
    /// Origins equal.
    Ball,
}

/// A mate between two connectors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mate {
    pub kind: Kind,
    pub a: Connector,
    pub b: Connector,
}

/// A pattern's direction or axis: a fixed unit direction in assembly
/// coordinates (through the origin, for an axis), or a connector's z axis
/// (through its origin), which moves with its body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    World([f64; 3]),
    Connector(Connector),
}

/// How a pattern moves copy `[i, j]` of a seed from where the seed is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// `i × spacing` along `direction`, and `j × spacing2` along the
    /// second direction if there is one.
    Linear {
        direction: Direction,
        spacing: f64,
        second: Option<(Direction, f64)>,
    },
    /// `i × step` radians about `axis` (right-handed).
    Circular { axis: Direction, step: f64 },
}

/// Copies of bodies: each copy of a seed is the seed moved by the
/// pattern's motion, and moves with it. Copies are never solved on their
/// own; mates may name them.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub seeds: Vec<Id>,
    pub motion: Motion,
}

/// An assembly to solve: bodies, patterns and mates, each by the caller's
/// ID.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assembly {
    pub bodies: BTreeMap<Id, Body>,
    pub patterns: BTreeMap<Id, Pattern>,
    pub mates: BTreeMap<Id, Mate>,
}

/// Why an assembly can't be solved as given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MateError {
    /// A mate or a pattern names a body that isn't there.
    UnknownBody { mate: Option<Id>, body: Id },
    /// A connector names a pattern that isn't there, a seed it doesn't
    /// have, or a second row it doesn't place.
    UnknownCopy {
        mate: Option<Id>,
        pattern: Id,
        seed: Id,
    },
    /// Copy `[0, 0]` is the seed itself.
    SeedAsCopy { mate: Option<Id>, pattern: Id },
    /// A pattern's direction is on a copy: directions move only with
    /// bodies.
    DirectionOnCopy { pattern: Id },
    /// A pose, a frame, a direction or a value that isn't finite, a
    /// rotation that isn't orthonormal or a direction that isn't unit.
    BadGeometry { what: String },
    /// A position outside ryukei's size box, or a distance past
    /// [`MAX_LENGTH`].
    OutsideSizeBox { what: String },
}

impl std::fmt::Display for MateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MateError::UnknownBody { mate, body } => match mate {
                Some(m) => write!(f, "mate {m} names body {body}, which isn't there"),
                None => write!(f, "a pattern names body {body}, which isn't there"),
            },
            MateError::UnknownCopy {
                mate,
                pattern,
                seed,
            } => write!(
                f,
                "{} names a copy of seed {seed} of pattern {pattern}, which isn't there",
                mate.map_or("a pattern".to_owned(), |m| format!("mate {m}"))
            ),
            MateError::SeedAsCopy { mate, pattern } => write!(
                f,
                "{} names copy [0, 0] of pattern {pattern}: that is the seed",
                mate.map_or("a pattern".to_owned(), |m| format!("mate {m}"))
            ),
            MateError::DirectionOnCopy { pattern } => {
                write!(f, "pattern {pattern}'s direction is on a copy")
            }
            MateError::BadGeometry { what } => write!(f, "bad geometry: {what}"),
            MateError::OutsideSizeBox { what } => write!(f, "outside the size box: {what}"),
        }
    }
}

impl std::error::Error for MateError {}

fn pose_ok(p: &Pose, what: &str) -> Result<(), MateError> {
    if !p.finite() || p.skew() > AXIS_TOL {
        return Err(MateError::BadGeometry {
            what: format!("{what}: not a finite orthonormal frame"),
        });
    }
    if p.origin
        .iter()
        .any(|x| x.abs() > ryukei::tol::SIZE_BOX_HALF)
    {
        return Err(MateError::OutsideSizeBox {
            what: format!("{what}'s origin"),
        });
    }
    Ok(())
}

fn length_ok(v: f64, what: &str) -> Result<(), MateError> {
    if !v.is_finite() {
        return Err(MateError::BadGeometry {
            what: format!("{what} isn't finite"),
        });
    }
    if v.abs() > MAX_LENGTH {
        return Err(MateError::OutsideSizeBox {
            what: format!("{what} is longer than {MAX_LENGTH} m"),
        });
    }
    Ok(())
}

fn unit_ok(d: [f64; 3], what: &str) -> Result<(), MateError> {
    if !d.iter().all(|x| x.is_finite()) || (norm(d) - 1.0).abs() > AXIS_TOL {
        return Err(MateError::BadGeometry {
            what: format!("{what} isn't a unit direction"),
        });
    }
    Ok(())
}

impl Assembly {
    /// Checks that everything named is there and every value is usable.
    pub fn validate(&self) -> Result<(), MateError> {
        for (id, b) in &self.bodies {
            pose_ok(&b.pose, &format!("body {id}'s pose"))?;
        }
        for (pid, p) in &self.patterns {
            for s in &p.seeds {
                if !self.bodies.contains_key(s) {
                    return Err(MateError::UnknownBody {
                        mate: None,
                        body: *s,
                    });
                }
            }
            let direction = |d: &Direction, what: &str| -> Result<(), MateError> {
                match d {
                    Direction::World(v) => unit_ok(*v, what),
                    Direction::Connector(c) => match c.on {
                        On::Body(b) if self.bodies.contains_key(&b) => pose_ok(&c.frame, what),
                        On::Body(b) => Err(MateError::UnknownBody {
                            mate: None,
                            body: b,
                        }),
                        On::Copy { .. } => Err(MateError::DirectionOnCopy { pattern: *pid }),
                    },
                }
            };
            match &p.motion {
                Motion::Linear {
                    direction: d,
                    spacing,
                    second,
                } => {
                    direction(d, &format!("pattern {pid}'s direction"))?;
                    length_ok(*spacing, &format!("pattern {pid}'s spacing"))?;
                    if let Some((d2, s2)) = second {
                        direction(d2, &format!("pattern {pid}'s second direction"))?;
                        length_ok(*s2, &format!("pattern {pid}'s second spacing"))?;
                    }
                }
                Motion::Circular { axis, step } => {
                    direction(axis, &format!("pattern {pid}'s axis"))?;
                    if !step.is_finite() {
                        return Err(MateError::BadGeometry {
                            what: format!("pattern {pid}'s angle isn't finite"),
                        });
                    }
                }
            }
        }
        for (mid, m) in &self.mates {
            for (side, c) in [("a", &m.a), ("b", &m.b)] {
                self.on_ok(Some(*mid), &c.on)?;
                pose_ok(&c.frame, &format!("mate {mid}'s connector {side}"))?;
            }
            let angle = |a: &Option<f64>| -> Result<(), MateError> {
                match a {
                    Some(a) if !a.is_finite() => Err(MateError::BadGeometry {
                        what: format!("mate {mid}'s angle isn't finite"),
                    }),
                    _ => Ok(()),
                }
            };
            let distance = |d: &Option<f64>| match d {
                Some(d) => length_ok(*d, &format!("mate {mid}'s distance")),
                None => Ok(()),
            };
            match &m.kind {
                Kind::Fastened | Kind::Ball => {}
                Kind::Revolute { angle: a } => angle(a)?,
                Kind::Slider { distance: d } => distance(d)?,
                Kind::Cylindrical {
                    angle: a,
                    distance: d,
                } => {
                    angle(a)?;
                    distance(d)?;
                }
                Kind::Planar {
                    angle: a,
                    distance: d,
                } => {
                    angle(a)?;
                    distance(&Some(*d))?;
                }
            }
        }
        Ok(())
    }

    fn on_ok(&self, mate: Option<Id>, on: &On) -> Result<(), MateError> {
        match *on {
            On::Body(b) => {
                if self.bodies.contains_key(&b) {
                    Ok(())
                } else {
                    Err(MateError::UnknownBody { mate, body: b })
                }
            }
            On::Copy {
                pattern,
                seed,
                index,
            } => {
                // A circular pattern, or a linear one in one direction, has
                // copies in one row only.
                let one_row = |p: &Pattern| match p.motion {
                    Motion::Linear { second, .. } => second.is_none(),
                    Motion::Circular { .. } => true,
                };
                let known = self
                    .patterns
                    .get(&pattern)
                    .is_some_and(|p| p.seeds.contains(&seed) && (index[1] == 0 || !one_row(p)));
                if !known {
                    return Err(MateError::UnknownCopy {
                        mate,
                        pattern,
                        seed,
                    });
                }
                if index == [0, 0] {
                    return Err(MateError::SeedAsCopy { mate, pattern });
                }
                Ok(())
            }
        }
    }

    /// The bodies `on` moves with: a body itself, or a copy's seed and
    /// the bodies its pattern's directions are on.
    pub(crate) fn bodies_of(&self, on: &On) -> Vec<Id> {
        match *on {
            On::Body(b) => vec![b],
            On::Copy { pattern, seed, .. } => {
                let mut out = vec![seed];
                out.extend(self.pattern_bodies(pattern));
                out
            }
        }
    }

    /// The bodies a pattern's directions are on.
    pub(crate) fn pattern_bodies(&self, pattern: Id) -> Vec<Id> {
        let on = |d: &Direction| match d {
            Direction::Connector(Connector {
                on: On::Body(b), ..
            }) => Some(*b),
            _ => None,
        };
        match &self.patterns[&pattern].motion {
            Motion::Linear {
                direction, second, ..
            } => on(direction)
                .into_iter()
                .chain(second.as_ref().and_then(|(d, _)| on(d)))
                .collect(),
            Motion::Circular { axis, .. } => on(axis).into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Body {
        Body {
            pose: Pose::IDENTITY,
            fixed: false,
        }
    }

    fn connector(on: On) -> Connector {
        Connector {
            on,
            frame: Pose::IDENTITY,
        }
    }

    #[test]
    fn what_a_mate_names_must_be_there() {
        let mut a = Assembly::default();
        a.bodies.insert(1, body());
        a.mates.insert(
            10,
            Mate {
                kind: Kind::Ball,
                a: connector(On::Body(1)),
                b: connector(On::Body(2)),
            },
        );
        assert_eq!(
            a.validate(),
            Err(MateError::UnknownBody {
                mate: Some(10),
                body: 2
            })
        );
        a.bodies.insert(2, body());
        assert_eq!(a.validate(), Ok(()));
        a.mates.get_mut(&10).unwrap().b = connector(On::Copy {
            pattern: 5,
            seed: 2,
            index: [1, 0],
        });
        assert!(matches!(a.validate(), Err(MateError::UnknownCopy { .. })));
        a.patterns.insert(
            5,
            Pattern {
                seeds: vec![2],
                motion: Motion::Linear {
                    direction: Direction::World([1.0, 0.0, 0.0]),
                    spacing: 0.01,
                    second: None,
                },
            },
        );
        assert_eq!(a.validate(), Ok(()));
        a.mates.get_mut(&10).unwrap().b = connector(On::Copy {
            pattern: 5,
            seed: 2,
            index: [0, 0],
        });
        assert!(matches!(a.validate(), Err(MateError::SeedAsCopy { .. })));
        a.mates.get_mut(&10).unwrap().b = connector(On::Copy {
            pattern: 5,
            seed: 2,
            index: [0, 1],
        });
        assert!(matches!(a.validate(), Err(MateError::UnknownCopy { .. })));
    }

    #[test]
    fn geometry_must_be_usable() {
        let mut a = Assembly::default();
        let mut b = body();
        b.pose.rotation[0][0] = 2.0;
        a.bodies.insert(1, b);
        assert!(matches!(a.validate(), Err(MateError::BadGeometry { .. })));
        let mut b = body();
        b.pose.origin[2] = 600.0;
        a.bodies.insert(1, b);
        assert!(matches!(
            a.validate(),
            Err(MateError::OutsideSizeBox { .. })
        ));
        a.bodies.insert(1, body());
        a.bodies.insert(2, body());
        a.mates.insert(
            3,
            Mate {
                kind: Kind::Slider {
                    distance: Some(2000.0),
                },
                a: connector(On::Body(1)),
                b: connector(On::Body(2)),
            },
        );
        assert!(matches!(
            a.validate(),
            Err(MateError::OutsideSizeBox { .. })
        ));
        a.mates.clear();
        a.patterns.insert(
            4,
            Pattern {
                seeds: vec![1],
                motion: Motion::Circular {
                    axis: Direction::World([0.0, 0.0, 2.0]),
                    step: 1.0,
                },
            },
        );
        assert!(matches!(a.validate(), Err(MateError::BadGeometry { .. })));
    }
}
