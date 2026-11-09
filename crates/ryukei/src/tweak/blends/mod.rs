//! Blends read back from a body and edited (design 399, plan 13.C):
//! fillets, chamfers and their corners recognised on any body
//! ([`recognise_blends`]), deleted by size ([`delete_blends`]), made again
//! at a new size ([`resize_blends`]) or after a tweak that would break
//! them ([`Blends::Rebuild`]), their faces, edges and vertices keeping
//! their names.

pub(crate) mod fit;
pub(crate) mod recognise;
pub(crate) mod remake;

#[cfg(test)]
mod tests;

use crate::blend::BlendKind;
use crate::name::{Journal, Name, OpKey};
use crate::topo::Body;
use crate::work::{Meter, Work};
use crate::Error;

/// How close a face must be to a blend to be read as one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecogniseOptions {
    /// How far a spline may lie from the cylinder or torus it is read as,
    /// how far a chamfer's setback may vary, and how far two sizes may
    /// differ and be one, in meters: from `LINEAR_RESOLUTION` to
    /// `TOLERANCE_CAP`.
    pub tolerance: f64,
}

/// [`RecogniseOptions::tolerance`] by default: 10 µm, the most a healed
/// import's fitted pcurve may be off.
pub const DEFAULT_TOLERANCE: f64 = 1e-5;

impl Default for RecogniseOptions {
    fn default() -> Self {
        Self {
            tolerance: DEFAULT_TOLERANCE,
        }
    }
}

/// One blend: its faces (sorted), its kind and size, the two faces it
/// rolls or stands on (by name order; a `Distances` chamfer's first is
/// `supports[0]`), and the chain of blends joined tangentially it is in.
#[derive(Clone, Debug, PartialEq)]
pub struct Recognised {
    pub faces: Vec<Name>,
    pub kind: BlendKind,
    pub supports: [Name; 2],
    pub chain: usize,
}

/// A ball corner: its face and the blends it joins (indices into
/// [`Recognition::blends`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Corner {
    pub face: Name,
    pub blends: Vec<usize>,
}

/// A body's blends, by least face name, and its ball corners, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recognition {
    pub blends: Vec<Recognised>,
    pub corners: Vec<Corner>,
}

impl Recognition {
    /// The blend or corner holding face `face`: `Some(Ok(i))` for blend
    /// `i`, `Some(Err(j))` for corner `j`.
    pub fn holding(&self, face: Name) -> Option<Result<usize, usize>> {
        if let Some(i) = self.blends.iter().position(|b| b.faces.contains(&face)) {
            return Some(Ok(i));
        }
        self.corners.iter().position(|c| c.face == face).map(Err)
    }
}

/// `body`'s fillets, chamfers and ball corners. Read only.
pub fn recognise_blends(body: &Body, opts: &RecogniseOptions) -> Result<Recognition, Error> {
    recognise_blends_with(body, opts, &Work::unlimited())
}

/// [`recognise_blends`] under `work`.
pub fn recognise_blends_with(
    body: &Body,
    opts: &RecogniseOptions,
    work: &Work,
) -> Result<Recognition, Error> {
    crate::work::run(work, |m| recognise::recognise(body, opts, m))
}

/// Whether a tweak keeps the blends beside the faces it changes as they
/// are, or makes them again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blends {
    /// Phase 9's: a blend the tweak would break refuses it
    /// (`TangencyBroken`).
    #[default]
    Keep,
    /// Where the tweak refuses, the recognised blends standing on the
    /// faces it changes are deleted, the tweak made on the sharp body,
    /// and each blend made again at its size, keeping its names.
    Rebuild,
}

/// Which blends [`delete_blends`] takes away: every one no larger than
/// `max_size`, and those holding `faces`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeleteBlends {
    pub max_size: Option<f64>,
    pub faces: Vec<Name>,
    /// How recognition reads the body.
    pub options: RecogniseOptions,
}

/// `body` with the blends `spec` picks deleted, their supports grown
/// until they meet, with the whole chain of each and every corner it
/// reaches: a new body and the journal (each blend face `Replaced` by the
/// sharp edges it leaves). Picking nothing changes nothing.
pub fn delete_blends(key: OpKey, body: &Body, spec: &DeleteBlends) -> Result<(Body, Journal), Error> {
    delete_blends_with(key, body, spec, &Work::unlimited())
}

/// [`delete_blends`] under `work`.
pub fn delete_blends_with(
    key: OpKey,
    body: &Body,
    spec: &DeleteBlends,
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |m| remake::delete_blends(key, body, spec, m))
}

/// `body` with each named face's blend made again at the kind given
/// (its chain and corners with it, at their own sizes): its faces, edges
/// and vertices keep their names where the new blend has one in the same
/// place.
pub fn resize_blends(
    key: OpKey,
    body: &Body,
    sizes: &[(Name, BlendKind)],
) -> Result<(Body, Journal), Error> {
    resize_blends_with(key, body, sizes, &RecogniseOptions::default(), &Work::unlimited())
}

/// [`resize_blends`] with its recognition's options, under `work`.
pub fn resize_blends_with(
    key: OpKey,
    body: &Body,
    sizes: &[(Name, BlendKind)],
    opts: &RecogniseOptions,
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |m| remake::resize(key, body, sizes, opts, m))
}

/// A tweak made with its `blends` option: its own result, or where it
/// refuses with `Blends::Rebuild` and blends stand on `faces`, those
/// blends deleted, the tweak made on the sharp body, and the blends made
/// again.
pub(crate) fn tweak_with<X>(
    key: OpKey,
    body: &Body,
    faces: &[Name],
    blends: Blends,
    m: &Meter,
    run: impl Fn(&Body, &Meter) -> Result<(Body, Journal, X), Error>,
) -> Result<(Body, Journal, X), Error> {
    let plain = run(body, m);
    match (&plain, blends) {
        (Ok(_), _) | (Err(_), Blends::Keep) => plain,
        (Err(e), Blends::Rebuild) if !remake::breaks(e) => plain,
        (Err(e), Blends::Rebuild) => remake::rebuild(key, body, faces, e, m, &run).unwrap_or(plain),
    }
}
