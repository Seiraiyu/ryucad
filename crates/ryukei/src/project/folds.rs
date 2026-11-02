//! Images of NURBS and their folds: Task 4.

use super::{ImageCurve, View};
use crate::geom::NurbsCurve;
use crate::intersect::Failure;
use std::sync::Arc;

pub(super) fn nurbs(
    _n: &Arc<NurbsCurve>,
    _range: [f64; 2],
    _view: &View,
) -> Result<Vec<ImageCurve>, Failure> {
    Err(Failure::Unsupported)
}
