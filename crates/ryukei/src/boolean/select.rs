//! Selection: which pieces each operation keeps, and which way round.
//!
//! The tools act as their union `T`. A target piece's class against `T`
//! is in if it is in any tool (or on two tools facing both ways), on if
//! it lies on a tool's face and is in no tool, out otherwise. Between
//! tools a piece follows the unite rule: one inside another tool, or on
//! one facing the other way, goes; of pieces on one surface facing one
//! way, the lowest tool's stays.
//!
//! | Op | Target pieces | Tool pieces | On, same way | On, opposite |
//! |---|---|---|---|---|
//! | Unite | out of `T` | out of the target | the target's | none |
//! | Subtract | out of `T` | in the target, reversed | none | the target's |
//! | Intersect | in `T` | in the target | the target's | none |

use super::BooleanOp;
use super::build::Kept;
use super::classify::Class;
use crate::split::Piece;

/// The kept pieces, in piece order.
pub(crate) fn select(op: BooleanOp, pieces: &[Piece], classes: &[Vec<Option<Class>>]) -> Vec<Kept> {
    let mut out = Vec::new();
    for (p, c) in pieces.iter().zip(classes) {
        let keep = if p.face.op == 0 {
            target(op, c)
        } else {
            tool(op, p.face.op, c)
        };
        if let Some(reversed) = keep {
            out.push(Kept {
                piece: p.clone(),
                reversed,
            });
        }
    }
    out
}

/// A target piece: kept, and whether reversed.
fn target(op: BooleanOp, c: &[Option<Class>]) -> Option<bool> {
    let tools: Vec<Class> = c[1..].iter().flatten().copied().collect();
    let on_same = tools.contains(&Class::On { same: true });
    let on_opposite = tools.contains(&Class::On { same: false });
    let class = if tools.contains(&Class::In) || (on_same && on_opposite) {
        Class::In
    } else if on_same {
        Class::On { same: true }
    } else if on_opposite {
        Class::On { same: false }
    } else {
        Class::Out
    };
    let keep = matches!(
        (op, class),
        (BooleanOp::Unite, Class::Out | Class::On { same: true })
            | (BooleanOp::Subtract, Class::Out | Class::On { same: false })
            | (BooleanOp::Intersect, Class::In | Class::On { same: true })
    );
    keep.then_some(false)
}

/// A piece of tool `j`: kept, and whether reversed.
fn tool(op: BooleanOp, j: usize, c: &[Option<Class>]) -> Option<bool> {
    for (k, class) in c.iter().enumerate().skip(1) {
        match class {
            Some(Class::In) | Some(Class::On { same: false }) => return None,
            Some(Class::On { same: true }) if k < j => return None,
            _ => {}
        }
    }
    match (op, c[0]?) {
        (BooleanOp::Unite, Class::Out) => Some(false),
        (BooleanOp::Subtract, Class::In) => Some(true),
        (BooleanOp::Intersect, Class::In) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table() {
        use BooleanOp::*;
        use Class::*;
        let t = |op, c| target(op, &[None, Some(c)]);
        assert_eq!(t(Unite, Out), Some(false));
        assert_eq!(t(Unite, In), None);
        assert_eq!(t(Unite, On { same: true }), Some(false));
        assert_eq!(t(Unite, On { same: false }), None);
        assert_eq!(t(Subtract, Out), Some(false));
        assert_eq!(t(Subtract, On { same: true }), None);
        assert_eq!(t(Subtract, On { same: false }), Some(false));
        assert_eq!(t(Intersect, In), Some(false));
        assert_eq!(t(Intersect, On { same: true }), Some(false));
        assert_eq!(t(Intersect, Out), None);
        let s = |op, c| tool(op, 1, &[Some(c), None]);
        assert_eq!(s(Unite, Out), Some(false));
        assert_eq!(s(Unite, On { same: true }), None);
        assert_eq!(s(Subtract, In), Some(true));
        assert_eq!(s(Subtract, Out), None);
        assert_eq!(s(Subtract, On { same: false }), None);
        assert_eq!(s(Intersect, In), Some(false));
        assert_eq!(s(Intersect, On { same: true }), None);
        // In a target piece's eyes the tools are one: in any is in.
        assert_eq!(target(Unite, &[None, Some(Out), Some(In)]), None);
        // Between tools: in another goes; on one surface the lowest stays.
        assert_eq!(tool(Unite, 2, &[Some(Out), Some(In), None]), None);
        assert_eq!(
            tool(Unite, 2, &[Some(Out), Some(On { same: true }), None]),
            None
        );
        assert_eq!(
            tool(Unite, 1, &[Some(Out), None, Some(On { same: true })]),
            Some(false)
        );
        assert_eq!(
            tool(Subtract, 1, &[Some(In), None, Some(On { same: false })]),
            None
        );
    }
}
