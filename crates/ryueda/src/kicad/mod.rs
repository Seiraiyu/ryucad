//! KiCad 6 to 9 libraries, read into ryueda's library: `.kicad_sym`
//! symbol libraries, `.pretty` folders of `.kicad_mod` footprints and the
//! STEP models they name. Numbers convert from their text exactly (no
//! floating point); what ryueda has no place for is reported once per
//! file, never guessed at.

pub mod sexpr;
pub mod symbol;

use crate::coord;
use crate::error::{Code, Result};
use crate::fault::Fault;
use sexpr::{Sexpr, unreadable};
use std::collections::BTreeSet;

/// The newest symbol library format read as KiCad 9 writes it.
pub const SYMBOL_VERSION: u64 = 20241209;
/// The newest footprint format read as KiCad 9 writes it.
pub const FOOTPRINT_VERSION: u64 = 20241229;

/// What reading one file found besides its items.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notes {
    /// Fields ryueda has no place for, by where they are
    /// (`pad.pinfunction`), each once.
    pub ignored: BTreeSet<String>,
    /// Numbers rounded to a whole nanometre or millidegree, as written.
    pub rounded: BTreeSet<String>,
    /// KiCad layers with no ryueda equivalent, and what was on them.
    pub unmapped: BTreeSet<String>,
    /// The file's format version, past what KiCad 9 writes.
    pub newer: Option<u64>,
}

impl Notes {
    pub fn ignore(&mut self, what: impl Into<String>) {
        self.ignored.insert(what.into());
    }

    /// `text` times `factor`, exactly, rounded half to even once.
    pub fn scaled(&mut self, node: &Sexpr, factor: i64) -> Result<i64> {
        let text = node
            .text()
            .ok_or_else(|| unreadable(node.pos(), "a number, not a list"))?;
        let (v, rounded) = coord::exact(text, factor)
            .ok_or_else(|| unreadable(node.pos(), format!("{text:?} isn't a number")))?;
        if rounded {
            self.rounded.insert(text.to_owned());
        }
        Ok(v)
    }

    /// Millimetres as nanometres.
    pub fn mm(&mut self, node: &Sexpr) -> Result<i64> {
        self.scaled(node, 1_000_000)
    }

    /// Degrees as millidegrees.
    pub fn deg(&mut self, node: &Sexpr) -> Result<i64> {
        self.scaled(node, 1_000)
    }

    /// Item `i` of `list` in millimetres, or a fault naming `what`.
    pub fn mm_at(&mut self, list: &Sexpr, i: usize, what: &str) -> Result<i64> {
        match list.items().get(i) {
            Some(n) => self.mm(n),
            None => Err(unreadable(list.pos(), format!("{what} is missing"))),
        }
    }

    /// The faults these notes make for the file at `path`: warnings.
    pub fn faults(&self, path: &str) -> Vec<Fault> {
        let mut out = Vec::new();
        if let Some(v) = self.newer {
            out.push(
                Fault::warning(
                    Code::ImportVersionNewer,
                    format!(
                        "{path} is format {v}, newer than KiCad 9's; read as far as ryueda knows it"
                    ),
                )
                .with("version", v),
            );
        }
        if !self.ignored.is_empty() {
            out.push(
                Fault::warning(
                    Code::ImportIgnored,
                    format!(
                        "{path}: ryueda has no place for {}",
                        self.ignored.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                )
                .with("fields", &self.ignored),
            );
        }
        if !self.rounded.is_empty() {
            out.push(
                Fault::warning(
                    Code::ImportRounded,
                    format!(
                        "{path}: {} rounded to a whole nanometre or millidegree",
                        self.rounded.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                )
                .with("values", &self.rounded),
            );
        }
        if !self.unmapped.is_empty() {
            out.push(
                Fault::warning(
                    Code::ImportLayerUnmapped,
                    format!(
                        "{path}: layer {} has no ryueda equivalent; what is on it is left out",
                        self.unmapped.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                )
                .with("layers", &self.unmapped),
            );
        }
        out.into_iter().map(|f| f.with("path", path)).collect()
    }
}

/// The format version a file's `(version N)` gives, noting one newer
/// than `newest`.
pub fn version(top: &Sexpr, newest: u64, notes: &mut Notes) -> Result<Option<u64>> {
    let Some(v) = top.child("version") else {
        return Ok(None);
    };
    let text = v.arg(0, "the version")?;
    let n: u64 = text
        .parse()
        .map_err(|_| unreadable(v.pos(), format!("version {text:?} isn't a number")))?;
    if n > newest {
        notes.newer = Some(n);
    }
    Ok(Some(n))
}

/// KiCad's `yes`, `no` (and a bare flag, read as yes).
pub fn yes(node: &Sexpr) -> bool {
    node.items().first().and_then(Sexpr::text) != Some("no")
}

/// Whether a KiCad item is hidden: `(hide yes)` (KiCad 8 on), a bare
/// `hide` (KiCad 6 and 7), either inside its `effects` too.
pub fn hidden(node: &Sexpr) -> bool {
    let at = |n: &Sexpr| n.has_atom("hide") || n.child("hide").is_some_and(yes);
    at(node) || node.child("effects").is_some_and(at)
}

/// ryueda's layers for a KiCad layer name: none when it has no
/// equivalent. Inner copper layers are `copper.inner` (every inner layer)
/// until a board's stackup names them (stage 7).
pub fn layers(name: &str) -> Option<Vec<&'static str>> {
    let sided = |side: &str, top: &'static str, bottom: &'static str| match side {
        "F" => Some(vec![top]),
        "B" => Some(vec![bottom]),
        "*" => Some(vec![top, bottom]),
        _ => None,
    };
    if let Some(n) = name.strip_prefix("In").and_then(|r| r.strip_suffix(".Cu")) {
        return n
            .parse::<u32>()
            .is_ok_and(|n| (1..=30).contains(&n))
            .then(|| vec!["copper.inner"]);
    }
    if let Some(n) = name.strip_prefix("User.") {
        return n
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=45).contains(n) && !n_padded(name))
            .map(|n| vec![user_layer(n)]);
    }
    match name {
        "*.Cu" => return Some(vec!["copper.all"]),
        "F&B.Cu" => return Some(vec!["copper.top", "copper.bottom"]),
        "Edge.Cuts" => return Some(vec!["edge"]),
        "Margin" => return Some(vec!["margin"]),
        "Dwgs.User" => return Some(vec!["drawings"]),
        "Cmts.User" => return Some(vec!["comments"]),
        "Eco1.User" => return Some(vec!["eco.1"]),
        "Eco2.User" => return Some(vec!["eco.2"]),
        _ => {}
    }
    let (side, kind) = name.split_once('.')?;
    match kind {
        "Cu" => sided(side, "copper.top", "copper.bottom"),
        "Mask" => sided(side, "mask.top", "mask.bottom"),
        "Paste" => sided(side, "paste.top", "paste.bottom"),
        "SilkS" => sided(side, "silk.top", "silk.bottom"),
        "Fab" => sided(side, "fab.top", "fab.bottom"),
        "CrtYd" => sided(side, "courtyard.top", "courtyard.bottom"),
        "Adhes" => sided(side, "adhesive.top", "adhesive.bottom"),
        _ => None,
    }
}

/// `User.01` isn't KiCad's way of writing a user layer.
fn n_padded(name: &str) -> bool {
    name.strip_prefix("User.")
        .is_some_and(|n| n.starts_with('0'))
}

/// `user.<n>`, as a static string.
fn user_layer(n: u32) -> &'static str {
    const USER: [&str; 45] = [
        "user.1", "user.2", "user.3", "user.4", "user.5", "user.6", "user.7", "user.8", "user.9",
        "user.10", "user.11", "user.12", "user.13", "user.14", "user.15", "user.16", "user.17",
        "user.18", "user.19", "user.20", "user.21", "user.22", "user.23", "user.24", "user.25",
        "user.26", "user.27", "user.28", "user.29", "user.30", "user.31", "user.32", "user.33",
        "user.34", "user.35", "user.36", "user.37", "user.38", "user.39", "user.40", "user.41",
        "user.42", "user.43", "user.44", "user.45",
    ];
    USER[n as usize - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_map_to_ryuedas_names() {
        for (kicad, ours) in [
            ("F.Cu", vec!["copper.top"]),
            ("*.Cu", vec!["copper.all"]),
            ("F&B.Cu", vec!["copper.top", "copper.bottom"]),
            ("In3.Cu", vec!["copper.inner"]),
            ("*.Mask", vec!["mask.top", "mask.bottom"]),
            ("B.SilkS", vec!["silk.bottom"]),
            ("F.CrtYd", vec!["courtyard.top"]),
            ("F.Adhes", vec!["adhesive.top"]),
            ("Edge.Cuts", vec!["edge"]),
            ("Dwgs.User", vec!["drawings"]),
            ("Eco2.User", vec!["eco.2"]),
            ("User.45", vec!["user.45"]),
        ] {
            assert_eq!(layers(kicad), Some(ours.clone()), "{kicad}");
            for l in ours {
                assert!(crate::footprint::layer_known(l), "{l}");
            }
        }
        for none in ["In31.Cu", "User.46", "User.01", "X.Cu", "Rescue", "F.Nope"] {
            assert_eq!(layers(none), None, "{none}");
        }
    }

    #[test]
    fn numbers_convert_exactly_and_say_when_they_round() {
        let mut n = Notes::default();
        let s = sexpr::parse("(at 0.153641 -1.27 0.0000005 0.0000015 1e-3 x)").unwrap();
        let v: Vec<i64> = (0..5).map(|i| n.mm(&s.items()[i]).unwrap()).collect();
        assert_eq!(v, [153_641, -1_270_000, 0, 2, 1_000]);
        assert_eq!(
            n.rounded.iter().collect::<Vec<_>>(),
            ["0.0000005", "0.0000015"]
        );
        assert_eq!(n.mm(&s.items()[5]).unwrap_err().code, Code::KicadUnreadable);
        assert_eq!(n.deg(&s.items()[0]).unwrap(), 154);
        let faults = n.faults("x.kicad_mod");
        assert_eq!(faults[0].error.code, Code::ImportRounded);
        assert_eq!(faults[0].error.details["path"], "x.kicad_mod");
    }

    #[test]
    fn hidden_reads_every_kicad_form() {
        for (text, h) in [
            ("(t hide (effects))", true),
            ("(t (hide yes))", true),
            ("(t (hide no))", false),
            ("(t (effects (font) hide))", true),
            ("(t (effects (font) (hide yes)))", true),
            ("(t (effects (font)))", false),
        ] {
            assert_eq!(hidden(&sexpr::parse(text).unwrap()), h, "{text}");
        }
    }
}
