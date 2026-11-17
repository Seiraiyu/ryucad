//! A board's design as the board sees it: each occurrence with its
//! designator, value, footprint and each pad's net, by references with
//! the design's prefix dropped (`s1/u1`, `sda`, `rail:gnd`).

use crate::error::{Code, Error, Result};
use crate::footprint::Footprint;
use crate::id::Id;
use crate::netlist::{Netlist, Occurrence};
use crate::project::{Project, SELF};
use std::collections::BTreeMap;

/// One occurrence of the board's design.
#[derive(Clone, Debug)]
pub struct Occ<'a> {
    /// `s1/u1`.
    pub reference: String,
    pub refdes: Option<String>,
    pub value: Option<String>,
    /// `lib/<alias>/footprint/<id>`, when it has one.
    pub footprint_ref: Option<String>,
    pub footprint: Option<&'a Footprint>,
    /// Why its footprint can't be read, when it names one.
    pub footprint_error: Option<Error>,
    /// Each pad number's net, as the board names nets.
    pub nets: BTreeMap<String, String>,
    pub dnp: bool,
}

impl Occ<'_> {
    /// The name a pad reference starts with: the designator, else the
    /// occurrence.
    pub fn label(&self) -> &str {
        self.refdes.as_deref().unwrap_or(&self.reference)
    }
}

/// A flattened reference with the design's prefix (`board:`) dropped.
pub fn local(design: &Id, reference: &str) -> String {
    reference
        .strip_prefix(&format!("{design}:"))
        .unwrap_or(reference)
        .to_owned()
}

/// The footprint an occurrence places, chosen as `netlist` chose it: the
/// component's named one among its library component's, else the first.
fn footprint_of<'a>(p: &'a Project, o: &Occurrence) -> Option<Result<&'a Footprint>> {
    o.footprint.as_ref()?;
    let c = p
        .conn
        .blocks
        .get(&o.block)?
        .components
        .get(&o.component)?
        .item
        .as_ref()
        .ok()?;
    let lc = p.libcomp(SELF, &c.of).ok()?;
    let chosen = match &c.footprint {
        Some(f) => lc.footprints.iter().find(|u| &u.footprint == f),
        None => lc.footprints.first(),
    }?;
    Some(p.placed_footprint(&c.of, &chosen.footprint))
}

/// The design flattened and each occurrence as the board sees it, sorted
/// by reference. A design that doesn't flatten refuses with its faults,
/// as `netlist` does.
pub fn occurrences<'a>(p: &'a Project, design: &Id) -> Result<(Netlist, Vec<Occ<'a>>)> {
    let n = crate::netlist::flatten(p, design)?;
    let mut out = Vec::with_capacity(n.occurrences.len());
    for o in &n.occurrences {
        let mut nets = BTreeMap::new();
        for pin in &o.pins {
            if let Some(i) = pin.net {
                for pad in &pin.pads {
                    nets.insert(pad.clone(), local(design, &n.nets[i].id));
                }
            }
        }
        let (footprint, footprint_error) = match footprint_of(p, o) {
            Some(Ok(f)) => (Some(f), None),
            Some(Err(e)) => (None, Some(e)),
            None => (None, None),
        };
        out.push(Occ {
            reference: local(design, &o.reference),
            refdes: o.refdes.clone(),
            value: o.value.clone(),
            footprint_ref: o.footprint.clone(),
            footprint,
            footprint_error,
            nets,
            dnp: crate::erc::occurrence_dnp(p, o),
        });
    }
    out.sort_by(|a, b| a.reference.cmp(&b.reference));
    Ok((n, out))
}

/// The occurrence `text` names: an occurrence reference (`s1/u1`, or with
/// the design's prefix), else a designator two occurrences don't share.
pub fn resolve<'o, 'a>(occs: &'o [Occ<'a>], design: &Id, text: &str) -> Result<&'o Occ<'a>> {
    let text_local = local(design, text);
    if let Some(o) = occs.iter().find(|o| o.reference == text_local) {
        return Ok(o);
    }
    let by_refdes: Vec<&Occ> = occs
        .iter()
        .filter(|o| o.refdes.as_deref() == Some(text))
        .collect();
    match by_refdes.as_slice() {
        [one] => Ok(one),
        [] => Err(Error::new(
            Code::NotFound,
            format!(
                "the design has no occurrence or designator {text}; `ryueda board show` lists them"
            ),
        )
        .with("ref", text)),
        many => Err(Error::new(
            Code::BoardRefAmbiguous,
            format!(
                "{} occurrences share designator {text}; name one by its occurrence reference",
                many.len()
            ),
        )
        .with("ref", text)
        .with(
            "occurrences",
            many.iter().map(|o| o.reference.clone()).collect::<Vec<_>>(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurrences_carry_footprints_and_pad_nets() {
        let (_dir, p) = crate::netlist::tests::project(&[]);
        let design = crate::netlist::one_design(&p, None).unwrap();
        let (_, occs) = occurrences(&p, &design).unwrap();
        let refs: Vec<&str> = occs.iter().map(|o| o.reference.as_str()).collect();
        assert_eq!(refs, ["ch_a/u1", "ch_b/u1", "r1"]);
        let r1 = &occs[2];
        assert!(r1.footprint.is_some(), "{r1:?}");
        assert!(!r1.nets.is_empty());
        assert!(
            occs.iter()
                .flat_map(|o| o.nets.values())
                .all(|n| !n.contains(':') || n.starts_with("rail:")),
            "{occs:?}"
        );
    }

    #[test]
    fn references_and_designators_resolve() {
        let (_dir, p) = crate::netlist::tests::project(&[]);
        let design = crate::netlist::one_design(&p, None).unwrap();
        let (_, occs) = occurrences(&p, &design).unwrap();
        let o = &occs[0];
        assert_eq!(resolve(&occs, &design, &o.reference).unwrap().reference, o.reference);
        let prefixed = format!("{design}:{}", o.reference);
        assert_eq!(resolve(&occs, &design, &prefixed).unwrap().reference, o.reference);
        assert_eq!(resolve(&occs, &design, "U1_B").unwrap().reference, "ch_b/u1");
        assert_eq!(resolve(&occs, &design, "NOPE9").unwrap_err().code, Code::NotFound);
        let mut twins = occs.clone();
        twins[1].refdes = twins[0].refdes.clone();
        let label = twins[0].refdes.clone().unwrap();
        assert_eq!(
            resolve(&twins, &design, &label).unwrap_err().code,
            Code::BoardRefAmbiguous
        );
    }
}
