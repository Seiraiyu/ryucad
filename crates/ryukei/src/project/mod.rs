//! Projection for drawings (design 326): orthographic views, the exact
//! images of curves in a view's plane, and silhouettes.
//!
//! A [`View`] looks along `-towards`: `towards` points from the model to
//! the viewer, image `x` right and `y` up, depth along `towards`. Views are
//! orthographic only (design 326, question 11), so projection is affine
//! and every curve's image is exact ([`project_curve`]). A silhouette is
//! where a face's natural normal is square to the view
//! ([`silhouettes`]). Hidden lines are plan 10.B's; sections 10.C's.

mod view;

pub use view::View;
