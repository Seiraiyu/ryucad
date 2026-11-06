//! Hidden lines (design 326, phase 3): every edge and silhouette of
//! several placed bodies, cut wherever its visibility may change (its
//! image's crossings with every other image curve, its folds, its ends),
//! each piece judged by one certified ray towards the viewer: how many
//! faces it crosses in front of the piece.

use super::events::events;
use super::items::{Item, items};
use super::pieces::{Span, cut, link};
use super::rays::{Ray, Scene, judge};
use super::{ImageCurve, Silhouettes, View, silhouettes_in};
use crate::Error;
use crate::geom::{Curve, Rigid};
use crate::interval::Interval;
use crate::name::Name;
use crate::tol::touch_reach;
use crate::topo::{Body, EdgeId, FaceId};
use crate::work::{Meter, Work};
use std::borrow::Cow;

/// A body placed in the view's model space, as a boolean's tool is.
#[derive(Clone, Copy, Debug)]
pub struct Placed<'a> {
    pub body: &'a Body,
    pub place: Rigid,
}

/// What a view computes besides edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewOptions {
    /// Trace every face's silhouette (true by default). Off, only edges
    /// are drawn: a view of bodies whose outlines are all edges (prisms)
    /// traces nothing.
    pub silhouettes: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self { silhouettes: true }
    }
}

/// What a piece is part of. `body` is the placed body's index in the
/// slice the view was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Edge {
        body: u32,
        edge: EdgeId,
        name: Name,
    },
    /// The `ordinal`th silhouette stretch of `face` (10.A's numbering).
    Silhouette {
        body: u32,
        face: FaceId,
        face_name: Name,
        ordinal: u32,
    },
}

/// How a drawing draws a piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// An edge whose faces meet at an angle (convex or concave).
    Sharp,
    /// An edge whose faces are tangent (a fillet's rails).
    Smooth,
    /// A sheet's laminar edge.
    Boundary,
    /// A silhouette.
    Silhouette,
    /// An edge whose bend rounding couldn't settle.
    Uncertain,
}

/// Whether a piece is seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Visible,
    /// Behind this many faces (its quantitative invisibility).
    Hidden(u32),
    /// No point of the piece could be judged with a certificate.
    Uncertain,
}

impl Visibility {
    /// The visibility of a piece behind `n` faces.
    pub fn of(n: u32) -> Self {
        if n == 0 {
            Self::Visible
        } else {
            Self::Hidden(n)
        }
    }
}

/// One stretch of an edge or a silhouette along which visibility is
/// constant.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub source: Source,
    /// The exact image in the view's frame, and the stretch of the 3D
    /// curve's parameter it is the image of (one to one).
    pub image: ImageCurve,
    /// Encloses the piece's depth along `towards` (nearer is larger).
    pub depth: [f64; 2],
    pub visibility: Visibility,
    /// The pieces whose images this one's runs along (over more than 100
    /// tolerances), as indices into [`Projection::pieces`], nearest first
    /// by depth: a drawing draws the nearest.
    pub overlaps: Vec<u32>,
    pub class: Class,
    /// An edge a silhouette runs along (a half cylinder's boundary
    /// ruling): it draws the outline there.
    pub on_silhouette: bool,
}

/// What a view draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Projection {
    /// By placed body; in each, edges by ID, then silhouettes by face and
    /// ordinal; along each, by parameter.
    pub pieces: Vec<Piece>,
    /// Each placed body's silhouettes in the placed frame (their 3D
    /// curves, and the faces seen edge-on), in the order given.
    pub silhouettes: Vec<Silhouettes>,
}

/// [`view_with`] with no cancel or budget.
pub fn view(placed: &[Placed], view: &View, options: &ViewOptions) -> Result<Projection, Error> {
    view_with(placed, view, options, &Work::unlimited())
}

/// What `placed` draw in `view`: every edge and, with
/// [`ViewOptions::silhouettes`], every silhouette, cut wherever its
/// visibility may change, each piece judged by certified rays towards
/// the viewer. Refused with `ViewError::Uncertain` naming a face whose
/// silhouette couldn't be certified, `ViewError::Image` naming an edge
/// whose image couldn't be, what `Body::transformed` refuses, and
/// `Cancelled` and `BudgetExceeded` as every `_with` call; a piece no
/// ray settles is `Visibility::Uncertain` in the result.
pub fn view_with(
    placed: &[Placed],
    view: &View,
    options: &ViewOptions,
    work: &Work,
) -> Result<Projection, Error> {
    crate::work::run(work, |m| project(placed, view, options, m))
}

fn project(
    placed: &[Placed],
    view: &View,
    options: &ViewOptions,
    m: &Meter,
) -> Result<Projection, Error> {
    let owned: Vec<Cow<Body>> = placed
        .iter()
        .map(|p| {
            if p.place == Rigid::IDENTITY {
                Ok(Cow::Borrowed(p.body))
            } else {
                p.body.transformed(&p.place).map(Cow::Owned)
            }
        })
        .collect::<Result<_, _>>()?;
    let bodies: Vec<&Body> = owned.iter().map(|c| c.as_ref()).collect();
    let originals: Vec<&Body> = placed.iter().map(|p| p.body).collect();
    let mut silhouettes = Vec::with_capacity(bodies.len());
    for &b in &bodies {
        silhouettes.push(if options.silhouettes {
            silhouettes_in(b, view, m)?
        } else {
            Silhouettes::default()
        });
    }
    let items = items(&bodies, &originals, &silhouettes, view, m)?;
    // Every image piece, by item and image.
    let imgs: Vec<(usize, usize)> = items
        .iter()
        .enumerate()
        .flat_map(|(i, it)| (0..it.images.len()).map(move |k| (i, k)))
        .collect();
    let image = |g: usize| &items[imgs[g].0].images[imgs[g].1];
    let item = |g: usize| &items[imgs[g].0];
    let met: Vec<(&ImageCurve, f64)> = (0..imgs.len())
        .map(|g| (image(g), item(g).tolerance))
        .collect();
    let ev = events(&met, m);
    let mut spans: Vec<Span> = Vec::new();
    let mut first = Vec::with_capacity(imgs.len() + 1);
    for g in 0..imgs.len() {
        first.push(spans.len());
        let it = item(g);
        for stretch in cut(
            image(g),
            &it.curve,
            &ev.cuts[g],
            &ev.unsure[g],
            it.tolerance,
        ) {
            spans.push(Span::new(g, stretch, image(g), &it.curve, view));
        }
    }
    first.push(spans.len());
    let tols: Vec<f64> = (0..imgs.len()).map(|g| item(g).tolerance).collect();
    let overlaps = link(&spans, &first, &ev.overlaps, &tols);
    let scene = Scene::new(view, &bodies);
    let order: Vec<usize> = (0..spans.len()).collect();
    let judged = m.par_map(&order, |m, &s| {
        let span = &spans[s];
        let it: &Item = item(span.img);
        let piece = ImageCurve {
            image: image(span.img).image.clone(),
            range: span.range,
        };
        let ray = Ray {
            body: it.body,
            own: &it.own,
            skip_own: it.skip_own,
            near: touch_reach(it.tolerance),
            lenient: !overlaps[s].is_empty(),
        };
        let visibility = judge(&scene, &it.curve, &piece, &ray, span.flagged, m);
        (piece, depth(&it.curve, span.range, view), visibility)
    });
    let pieces = judged
        .into_iter()
        .zip(overlaps)
        .zip(&spans)
        .map(|(((image, depth, visibility), overlaps), span)| {
            let it = item(span.img);
            Piece {
                source: it.source,
                image,
                depth,
                visibility,
                overlaps,
                class: it.class,
                on_silhouette: it.on_silhouette,
            }
        })
        .collect();
    Ok(Projection {
        pieces,
        silhouettes,
    })
}

/// An enclosure of `curve`'s depth along the view over `range`.
fn depth(curve: &Curve, range: [f64; 2], view: &View) -> [f64; 2] {
    let p = curve.jet_box(range)[0];
    let t = view.towards().get();
    let d = p[0] * Interval::point(t.x) + p[1] * Interval::point(t.y) + p[2] * Interval::point(t.z);
    [d.lo(), d.hi()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_trace_silhouettes_by_default_and_a_count_of_none_is_visible() {
        assert!(ViewOptions::default().silhouettes);
        assert_eq!(Visibility::of(0), Visibility::Visible);
        assert_eq!(Visibility::of(3), Visibility::Hidden(3));
        assert_eq!(Projection::default().pieces.len(), 0);
    }

    use crate::geom::{Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::project::View;

    fn hidden_length(p: &Projection, body: &Body) -> f64 {
        p.pieces
            .iter()
            .filter(|q| matches!(q.visibility, Visibility::Hidden(_)))
            .map(|q| {
                let Source::Edge { edge, .. } = q.source else {
                    panic!("{q:?}")
                };
                let c = &body.curves[body.edges[edge].curve];
                c.point(q.image.range[0])
                    .distance(c.point(q.image.range[1]))
            })
            .sum()
    }

    #[test]
    fn a_block_seen_from_a_corner_hides_the_three_edges_at_its_far_corner() {
        let (b, _) = primitive::block(OpKey(7), 0.01, 0.02, 0.03, &Rigid::IDENTITY).unwrap();
        let iso = View::new(Vector3::new(1.0, -1.0, 1.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let p = view(
            &[Placed {
                body: &b,
                place: Rigid::IDENTITY,
            }],
            &iso,
            &ViewOptions::default(),
        )
        .unwrap();
        assert!(
            p.pieces
                .iter()
                .all(|q| q.visibility != Visibility::Uncertain)
        );
        assert!(
            p.pieces
                .iter()
                .all(|q| q.overlaps.is_empty() && q.class == Class::Sharp)
        );
        assert!((hidden_length(&p, &b) - 0.06).abs() < 1e-12);
        assert_eq!(p.silhouettes.len(), 1);
    }

    #[test]
    fn nothing_placed_draws_nothing() {
        let iso = View::new(Vector3::new(1.0, -1.0, 1.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        assert_eq!(
            view(&[], &iso, &ViewOptions::default()).unwrap(),
            Projection::default()
        );
    }
}
