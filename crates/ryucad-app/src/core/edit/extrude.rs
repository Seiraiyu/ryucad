//! An extrude's extent in the property panel (ryucad's `extrude` kind).

use super::*;

/// An extrude's extent: its type, its length (blind or symmetric) and
/// whether through all goes each way.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtentForm {
    pub ty: Choice,
    pub length: Field,
    pub both: Flag,
}

impl ExtentForm {
    pub fn dirty(&self) -> bool {
        self.ty.dirty() || self.length.dirty() || self.both.dirty()
    }
}
