//! Pieces: the shape's faces joined by the edges they share, once sewn,
//! and what kind of body each makes (design 295).
//!
//! With [`Open::Refuse`] every piece is a solid, as healing has always
//! taken them. With [`Open::Sheet`] a piece is what the file declares
//! its shells: a solid's ([`Shape::solids`]) must close, else
//! [`HealError::OpenShell`] whatever the option (it says it is a solid);
//! a surface model's ([`Shape::sheets`]) is a sheet, open or closed. A
//! shape whose pieces are of both kinds, or a piece joining shells of
//! both, is [`HealError::MixedShape`]: it would be a general body.

use super::names::Names;
use super::sew::find;
use super::{Loop, Open, Shape};
use crate::name::Name;
use crate::{Error, HealError};
use std::collections::BTreeMap;

/// What a healed shape becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Solid,
    Sheet,
}

/// The faces of each edge's uses.
fn edge_faces(s: &Shape) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); s.edges.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    out[u.edge].push(k);
                }
            }
        }
    }
    out
}

/// The shape's pieces: faces joined by edges, each in face order, in the
/// order of their first face.
pub(super) fn pieces(s: &Shape) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..s.faces.len()).collect();
    for list in edge_faces(s) {
        for w in list.windows(2) {
            let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for k in 0..s.faces.len() {
        let r = find(&mut parent, k);
        by_root.entry(r).or_default().push(k);
    }
    by_root.into_values().collect()
}

/// The lesser of two names either of which may be missing.
fn lesser(a: Option<Name>, b: Option<Name>) -> Option<Name> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// What the sewn shape `s` becomes under `open`.
pub(super) fn kind(s: &Shape, names: &Names, open: Open) -> Result<Kind, Error> {
    if open == Open::Refuse {
        return Ok(Kind::Solid);
    }
    let mut declared = vec![false; s.faces.len()];
    for &sh in &s.sheets {
        for &f in &s.shells[sh].faces {
            declared[f] = true;
        }
    }
    let uses = edge_faces(s);
    let least = |faces: &mut dyn Iterator<Item = usize>| -> Option<Name> {
        faces.map(|k| names.faces[k]).min()
    };
    let (mut solid, mut sheet): (Option<Name>, Option<Name>) = (None, None);
    let mut free: Vec<Name> = Vec::new();
    for piece in pieces(s) {
        let of_sheets = least(&mut piece.iter().copied().filter(|&k| declared[k]));
        let of_solids = least(&mut piece.iter().copied().filter(|&k| !declared[k]));
        if let (Some(solid), Some(sheet)) = (of_solids, of_sheets) {
            return Err(HealError::MixedShape { solid, sheet }.into());
        }
        if of_solids.is_some() {
            for &k in &piece {
                for l in &s.faces[k].loops {
                    if let Loop::Edges(list) = l {
                        free.extend(
                            list.iter()
                                .filter(|u| uses[u.edge].len() == 1)
                                .map(|u| names.edges[u.edge]),
                        );
                    }
                }
            }
        }
        solid = lesser(solid, of_solids);
        sheet = lesser(sheet, of_sheets);
    }
    if !free.is_empty() {
        free.sort();
        free.dedup();
        return Err(HealError::OpenShell { free_edges: free }.into());
    }
    match (solid, sheet) {
        (Some(solid), Some(sheet)) => Err(HealError::MixedShape { solid, sheet }.into()),
        (_, Some(_)) => Ok(Kind::Sheet),
        _ => Ok(Kind::Solid),
    }
}
