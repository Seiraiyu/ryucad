//! Imports: written by `import`, never by feature.add or feature.edit (ryucad 27.C).

use super::{BodyUse, FeatureKind, Given, Place, Write};
use crate::doc::files::Feature;
use crate::doc::project::Part;
use crate::error::Result;
use crate::error::{Code, Error};
use serde_json::Value;

/// `invalid_param` for `feature.add` or `feature.edit` of an import:
/// its file, product and body come from the file, through `import`.
pub(crate) fn import_not_written_here(how: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!(
            "an import can't be {how}ed with feature.{how}: import a STEP file with `import` (`--into` adds its bodies to a part)"
        ),
    )
    .with("type", "import")
}

/// Imports.
pub(crate) struct Imports;

impl FeatureKind for Imports {
    fn types(&self) -> &'static [&'static str] {
        &["import"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &[]
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::FOUNDS
    }

    fn refuse_others(&self, _ty: &str, _given: &Given) -> Result<()> {
        Ok(())
    }

    fn from_given(
        &self,
        _ty: &str,
        _current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        match cx.place {
            Place::New { .. } => Err(import_not_written_here("add")),
            Place::Edit { id } => Err(import_not_written_here("edit").with("feature", id)),
        }
    }

    fn report(&self, _part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        f.import().expect("an import").fields()
    }
}
