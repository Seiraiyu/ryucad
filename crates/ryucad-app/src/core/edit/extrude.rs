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

impl FeatureForm for ExtentForm {
    fn read(ty: &str, v: &Value) -> Option<ExtentForm> {
        (ty == "extrude").then(|| {
            let e = &v["extent"];
            ExtentForm {
                ty: Choice::new(
                    "extent",
                    &["blind", "symmetric", "up_to", "through_all"],
                    text(&e["type"]),
                ),
                length: Field::new("length", text(&e["length"])),
                both: Flag {
                    name: "both ways".to_owned(),
                    read: e["both"] == true,
                    chosen: e["both"] == true,
                },
            }
        })
    }
    // `Form::fields` never listed the length; `fields_mut` did.
    fn kind_fields_mut(&mut self) -> Vec<&mut Field> {
        vec![&mut self.length]
    }
    fn kind_dirty(&self) -> bool {
        self.dirty()
    }
    fn kind_revert(&mut self) {
        self.ty.chosen.clone_from(&self.ty.read);
        self.length.text.clone_from(&self.length.read);
        self.both.chosen = self.both.read;
    }
    fn kind_given(&self, out: &mut FeatureFields) -> Result<(), Error> {
        out.extent = self.dirty().then(|| {
            let mut v = json!({"type": self.ty.chosen});
            match self.ty.chosen.as_str() {
                "blind" | "symmetric" => v["length"] = json!(self.length.text.trim()),
                "through_all" => v["both"] = json!(self.both.chosen),
                _ => {}
            }
            v
        });
        Ok(())
    }
}
