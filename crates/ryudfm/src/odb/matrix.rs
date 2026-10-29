//! `matrix/matrix`: the job's steps and layers, in row order, with each
//! layer's side and each drill or rout layer's span.
//!
//! Names are lower case in the model: ODB++ entity names are lower case,
//! and writers that put capitals in the matrix (Altium) name the folders
//! in lower case.

use ryu_board::model::{Attributes, Context, Layer, LayerKind, Notice, Polarity, Side, Span};

use super::error::{Code, Error, fault, location, notice};
use super::text::{Array, structured};

pub const PATH: &str = "matrix/matrix";

/// A `STEP` of the matrix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatrixStep {
    pub col: u32,
    pub name: String,
    pub id: Option<u64>,
}

/// A `LAYER` of the matrix, as written but for its name, made lower case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatrixLayer {
    pub row: u32,
    pub name: String,
    pub context: Context,
    pub kind: LayerKind,
    pub add_type: Option<String>,
    pub polarity: Polarity,
    pub start: Option<String>,
    pub end: Option<String>,
    pub id: Option<u64>,
    /// `FORM=FLEX`.
    pub flex: bool,
    pub dielectric_type: Option<String>,
    pub dielectric_name: Option<String>,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matrix {
    /// In column order.
    pub steps: Vec<MatrixStep>,
    /// In row order.
    pub layers: Vec<MatrixLayer>,
}

fn kind(s: &str) -> LayerKind {
    match s.to_ascii_uppercase().as_str() {
        "SIGNAL" => LayerKind::Signal,
        "POWER_GROUND" => LayerKind::PowerGround,
        "MIXED" => LayerKind::Mixed,
        "SOLDER_MASK" => LayerKind::SolderMask,
        "SOLDER_PASTE" => LayerKind::SolderPaste,
        "SILK_SCREEN" => LayerKind::SilkScreen,
        "DRILL" => LayerKind::Drill,
        "ROUT" => LayerKind::Rout,
        "COMPONENT" => LayerKind::Component,
        "DIELECTRIC" => LayerKind::Dielectric,
        "DOCUMENT" => LayerKind::Document,
        "CONDUCTIVE_PASTE" => LayerKind::ConductivePaste,
        "MASK" => LayerKind::Mask,
        _ => LayerKind::Other(s.to_string()),
    }
}

fn bad(line: u32, message: impl Into<String>) -> Error {
    Error::new(Code::Unreadable, message).at(location(PATH, Some(line), None))
}

fn required<'a>(a: &'a Array, key: &str) -> Result<&'a str, Error> {
    a.get(key)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| bad(a.line, format!("{} without {key}", a.name)))
}

fn number(a: &Array, key: &str) -> Result<u32, Error> {
    let v = required(a, key)?;
    v.parse().map_err(|_| {
        bad(
            a.field(key).map_or(a.line, |f| f.line),
            format!("{key}={v}: not a number"),
        )
    })
}

fn optional(a: &Array, key: &str) -> Option<String> {
    a.get(key).filter(|v| !v.is_empty()).map(str::to_string)
}

/// Reads the matrix. Any fault in it fails the job.
pub fn matrix(text: &str) -> Result<Matrix, Error> {
    let s = structured(text).map_err(|b| {
        Error::new(Code::Unreadable, b.message).at(location(PATH, Some(b.line), Some(b.column)))
    })?;
    let mut steps = Vec::new();
    for a in s.arrays("STEP") {
        steps.push(MatrixStep {
            col: number(a, "COL")?,
            name: required(a, "NAME")?.to_ascii_lowercase(),
            id: a.get("ID").and_then(|v| v.parse().ok()),
        });
    }
    let mut layers = Vec::new();
    for a in s.arrays("LAYER") {
        let context = match required(a, "CONTEXT")?.to_ascii_uppercase().as_str() {
            "BOARD" => Context::Board,
            "MISC" => Context::Misc,
            other => return Err(bad(a.line, format!("CONTEXT={other}: not BOARD or MISC"))),
        };
        let polarity = match a
            .get("POLARITY")
            .unwrap_or("POSITIVE")
            .to_ascii_uppercase()
            .as_str()
        {
            "POSITIVE" | "" => Polarity::Positive,
            "NEGATIVE" => Polarity::Negative,
            other => {
                return Err(bad(
                    a.line,
                    format!("POLARITY={other}: not POSITIVE or NEGATIVE"),
                ));
            }
        };
        layers.push(MatrixLayer {
            row: number(a, "ROW")?,
            name: required(a, "NAME")?.to_ascii_lowercase(),
            context,
            kind: kind(required(a, "TYPE")?),
            add_type: optional(a, "ADD_TYPE"),
            polarity,
            start: optional(a, "START_NAME").map(|s| s.to_ascii_lowercase()),
            end: optional(a, "END_NAME").map(|s| s.to_ascii_lowercase()),
            id: a.get("ID").and_then(|v| v.parse().ok()),
            flex: a
                .get("FORM")
                .is_some_and(|f| f.eq_ignore_ascii_case("FLEX")),
            dielectric_type: optional(a, "DIELECTRIC_TYPE"),
            dielectric_name: optional(a, "DIELECTRIC_NAME"),
            line: a.line,
        });
    }
    for (i, a) in s.arrays.iter().enumerate() {
        if !a.name.eq_ignore_ascii_case("STEP") && !a.name.eq_ignore_ascii_case("LAYER") {
            return Err(bad(
                a.line,
                format!("{}: neither STEP nor LAYER", s.arrays[i].name),
            ));
        }
    }
    steps.sort_by_key(|s| s.col);
    layers.sort_by_key(|l| l.row);
    for w in steps.windows(2) {
        if w[0].col == w[1].col {
            return Err(bad(0, format!("two steps in column {}", w[0].col)));
        }
    }
    for w in layers.windows(2) {
        if w[0].row == w[1].row {
            return Err(bad(w[1].line, format!("two layers in row {}", w[0].row)));
        }
    }
    let mut names: Vec<&str> = steps.iter().map(|s| s.name.as_str()).collect();
    names.sort();
    if let Some(w) = names.windows(2).find(|w| w[0] == w[1]) {
        return Err(bad(0, format!("two steps named {}", w[0])));
    }
    let mut names: Vec<&str> = layers.iter().map(|l| l.name.as_str()).collect();
    names.sort();
    if let Some(w) = names.windows(2).find(|w| w[0] == w[1]) {
        return Err(bad(0, format!("two layers named {}", w[0])));
    }
    if steps.is_empty() {
        return Err(bad(1, "no STEP"));
    }
    Ok(Matrix { steps, layers })
}

impl Matrix {
    /// The model's layers, in row order: each board layer's side by where
    /// its row lies against the copper layers' (the first copper layer and
    /// those above it are the top, the last and those below the bottom,
    /// the rest inner), and each drill or rout layer's span. An empty
    /// `START_NAME` or `END_NAME` is the first or last copper layer. A span
    /// naming a layer the matrix lacks is a fault, and the layer has none.
    pub fn layers(&self) -> (Vec<Layer>, Vec<Notice>) {
        let copper: Vec<usize> = (0..self.layers.len())
            .filter(|&i| {
                self.layers[i].context == Context::Board && self.layers[i].kind.is_copper()
            })
            .collect();
        let mut faults = Vec::new();
        let index = |name: &str| self.layers.iter().position(|l| l.name == name);
        let out = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let drilled = matches!(l.kind, LayerKind::Drill | LayerKind::Rout);
                let side = match (copper.first(), copper.last()) {
                    (Some(&first), Some(&last)) if l.context == Context::Board && !drilled => {
                        Some(if i <= first {
                            Side::Top
                        } else if i >= last {
                            Side::Bottom
                        } else {
                            Side::Inner
                        })
                    }
                    _ => None,
                };
                let span = if drilled {
                    let end = |name: &Option<String>, dflt: Option<&usize>| match name {
                        Some(n) => index(n).ok_or_else(|| n.clone()),
                        None => dflt.copied().ok_or_else(|| "a copper layer".to_string()),
                    };
                    match (end(&l.start, copper.first()), end(&l.end, copper.last())) {
                        (Ok(start), Ok(end)) => Some(Span { start, end }),
                        (Err(n), _) | (_, Err(n)) => {
                            faults.push(notice(
                                fault::FILE_INVALID,
                                Some(l.name.clone()),
                                format!("{} spans to {n}, which the matrix lacks", l.name),
                                Some(location(PATH, Some(l.line), None)),
                            ));
                            None
                        }
                    }
                } else {
                    None
                };
                Layer {
                    name: l.name.clone(),
                    row: l.row,
                    context: l.context,
                    kind: l.kind.clone(),
                    polarity: l.polarity,
                    side,
                    span,
                    attributes: Attributes::new(),
                }
            })
            .collect();
        (out, faults)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALTIUM: &str = "STEP {\n    COL=1\n    NAME=PCB\n}\n\
LAYER {\n ROW=1\n CONTEXT=BOARD\n TYPE=COMPONENT\n ID=1\n NAME=COMP_+_TOP\n POLARITY=POSITIVE\n START_NAME=\n END_NAME=\n}\n\
LAYER {\n ROW=3\n CONTEXT=BOARD\n TYPE=SILK_SCREEN\n NAME=TOP_OVERLAY\n POLARITY=POSITIVE\n}\n\
LAYER {\n ROW=5\n CONTEXT=BOARD\n TYPE=SIGNAL\n ID=5\n NAME=TOP\n POLARITY=POSITIVE\n}\n\
LAYER {\n ROW=6\n CONTEXT=BOARD\n TYPE=DIELECTRIC\n NAME=DIELECTRIC1\n DIELECTRIC_TYPE=CORE\n DIELECTRIC_NAME=FR-4\n}\n\
LAYER {\n ROW=7\n CONTEXT=BOARD\n TYPE=POWER_GROUND\n NAME=GND\n POLARITY=NEGATIVE\n}\n\
LAYER {\n ROW=8\n CONTEXT=BOARD\n TYPE=SIGNAL\n NAME=BOTTOM\n}\n\
LAYER {\n ROW=9\n CONTEXT=BOARD\n TYPE=SOLDER_MASK\n NAME=BOTTOM_SOLDER\n}\n\
LAYER {\n ROW=11\n CONTEXT=BOARD\n TYPE=DRILL\n NAME=DRILL\n START_NAME=TOP\n END_NAME=BOTTOM\n}\n\
LAYER {\n ROW=12\n CONTEXT=BOARD\n TYPE=DRILL\n NAME=VIA_1_2\n START_NAME=TOP\n END_NAME=GND\n ADD_TYPE=BACKDRILL\n}\n\
LAYER {\n ROW=13\n CONTEXT=BOARD\n TYPE=ROUT\n NAME=ROUT\n}\n\
LAYER {\n ROW=14\n CONTEXT=BOARD\n TYPE=COMPONENT\n NAME=COMP_+_BOT\n}\n\
LAYER {\n ROW=15\n CONTEXT=MISC\n TYPE=DOCUMENT\n NAME=MECHANICAL_17\n FORM=FLEX\n}\n\
LAYER {\n ROW=16\n CONTEXT=BOARD\n TYPE=SIGNAL_FLEX\n NAME=ODD\n}\n";

    #[test]
    fn layers_in_row_order_with_sides_and_spans() {
        let m = matrix(ALTIUM).unwrap();
        assert_eq!(
            m.steps,
            [MatrixStep {
                col: 1,
                name: "pcb".into(),
                id: None
            }]
        );
        let (layers, faults) = m.layers();
        assert!(faults.is_empty());
        let got: Vec<_> = layers
            .iter()
            .map(|l| (l.name.as_str(), l.side, l.span.map(|s| (s.start, s.end))))
            .collect();
        use Side::*;
        assert_eq!(
            got,
            [
                ("comp_+_top", Some(Top), None),
                ("top_overlay", Some(Top), None),
                ("top", Some(Top), None),
                ("dielectric1", Some(Inner), None),
                ("gnd", Some(Inner), None),
                ("bottom", Some(Bottom), None),
                ("bottom_solder", Some(Bottom), None),
                ("drill", None, Some((2, 5))),
                ("via_1_2", None, Some((2, 4))),
                ("rout", None, Some((2, 5))),
                ("comp_+_bot", Some(Bottom), None),
                ("mechanical_17", None, None),
                ("odd", Some(Bottom), None),
            ]
        );
        assert_eq!(layers[4].kind, LayerKind::PowerGround);
        assert_eq!(layers[4].polarity, Polarity::Negative);
        assert_eq!(layers[11].context, Context::Misc);
        assert_eq!(layers[12].kind, LayerKind::Other("SIGNAL_FLEX".into()));
        assert_eq!(m.layers[8].add_type.as_deref(), Some("BACKDRILL"));
        assert!(m.layers[11].flex);
        assert_eq!(m.layers[3].dielectric_name.as_deref(), Some("FR-4"));
        assert_eq!(m.layers[0].id, Some(1));
    }

    #[test]
    fn spans_to_missing_layers_are_faults() {
        let m = matrix(&ALTIUM.replace("END_NAME=GND", "END_NAME=SIG9")).unwrap();
        let (layers, faults) = m.layers();
        assert_eq!(layers[8].span, None);
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].code, "odb_file_invalid");
        assert_eq!(faults[0].about.as_deref(), Some("via_1_2"));
    }

    #[test]
    fn faults_fail_the_matrix() {
        let code = |t: &str| matrix(t).unwrap_err();
        assert_eq!(code("").message, "no STEP");
        assert_eq!(code("STEP {\nNAME=a\n}\n").message, "STEP without COL");
        assert!(
            code("STEP {\nCOL=1\nNAME=a\n}\nSTEP {\nCOL=1\nNAME=b\n}\n")
                .message
                .contains("column 1")
        );
        assert!(
            code("STEP {\nCOL=1\nNAME=a\n}\nSTEP {\nCOL=2\nNAME=A\n}\n")
                .message
                .contains("two steps named a")
        );
        let l = |body: &str| format!("STEP {{\nCOL=1\nNAME=a\n}}\nLAYER {{\n{body}\n}}\n");
        assert!(
            code(&l("ROW=1\nCONTEXT=X\nTYPE=SIGNAL\nNAME=t"))
                .message
                .contains("CONTEXT=X")
        );
        assert!(
            code(&l("ROW=x\nCONTEXT=BOARD\nTYPE=SIGNAL\nNAME=t"))
                .message
                .contains("ROW=x")
        );
        assert!(
            code(&l("ROW=1\nCONTEXT=BOARD\nTYPE=SIGNAL\nNAME=t\nPOLARITY=UP"))
                .message
                .contains("POLARITY")
        );
        assert!(
            code("STEP {\nCOL=1\nNAME=a\n}\nWHAT {\n}\n")
                .message
                .contains("WHAT")
        );
        let e = code("STEP {\nCOL=1\nNAME=a\n");
        assert_eq!(e.location.unwrap().path, "matrix/matrix");
    }
}
