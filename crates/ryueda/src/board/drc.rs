//! DRC (design 732): a board's findings, ERC's shape, from its shapes,
//! its copper's connectivity against the design's netlist, and its
//! preset's rules. Every topology decision is `ryu-copper`'s exact
//! predicate; a rule a bound can't decide is reported, never dropped.

use super::load::BoardFiles;
use super::occ::Occ;
use super::shapes::BoardShapes;
use crate::error::Code;
use crate::fault::Severity;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// One finding on a board.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub id: String,
    pub code: Code,
    pub severity: Severity,
    pub board: String,
    pub refs: Vec<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

impl Finding {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// A finding's stable ID: 16 hex digits of BLAKE3 over its code, board
/// and sorted references.
pub fn finding_id(code: Code, board: &str, refs: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ryueda.board-finding.1\0");
    h.update(json!(code).as_str().unwrap_or_default().as_bytes());
    h.update(b"\0");
    h.update(board.as_bytes());
    let mut sorted: Vec<&String> = refs.iter().collect();
    sorted.sort();
    for r in sorted {
        h.update(b"\0");
        h.update(r.as_bytes());
    }
    h.finalize().to_hex()[..16].to_owned()
}

/// A finding; its references sorted.
pub fn finding(
    code: Code,
    severity: Severity,
    board: &str,
    mut refs: Vec<String>,
    message: String,
    details: Value,
) -> Finding {
    refs.sort();
    refs.dedup();
    Finding {
        id: finding_id(code, board, &refs),
        code,
        severity,
        board: board.to_owned(),
        refs,
        message,
        details,
    }
}

/// By code, then references.
pub fn sort(f: &mut [Finding]) {
    f.sort_by(|a, b| {
        (json!(a.code).as_str().unwrap_or_default(), &a.refs)
            .cmp(&(json!(b.code).as_str().unwrap_or_default(), &b.refs))
    });
}

/// The board's notices (5.A) as findings, and the placements: every
/// occurrence with a footprint and no placement, every placement whose
/// occurrence is gone.
pub fn placements(board: &str, b: &BoardFiles, occs: &[Occ], s: &BoardShapes) -> Vec<Finding> {
    let mut out = Vec::new();
    for n in &s.notices {
        let occurrence = n.error.details["occurrence"].as_str().unwrap_or_default();
        out.push(finding(
            n.error.code,
            n.severity,
            board,
            vec![format!("{board}:{occurrence}")],
            n.error.message.clone(),
            n.error.details.clone(),
        ));
    }
    for o in occs {
        if o.footprint_ref.is_some() && !b.placements.contains_key(&o.reference) {
            out.push(finding(
                Code::BoardUnplaced,
                Severity::Error,
                board,
                vec![format!("{board}:{}", o.label())],
                format!(
                    "{} isn't placed; `ryueda board place {board} {} --at X,Y`",
                    o.label(),
                    o.reference
                ),
                json!({"occurrence": o.reference, "footprint": o.footprint_ref}),
            ));
        }
    }
    let known: BTreeSet<&str> = occs.iter().map(|o| o.reference.as_str()).collect();
    for occurrence in b.placements.keys() {
        if !known.contains(occurrence.as_str()) {
            out.push(finding(
                Code::BoardPlacementOrphan,
                Severity::Error,
                board,
                vec![format!("{board}:{occurrence}")],
                format!("the design has no occurrence {occurrence} any more; `ryueda board unplace {board} {occurrence}`"),
                json!({"occurrence": occurrence}),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_ignore_reference_order_but_not_code_or_board() {
        let a = finding_id(Code::DrcClearance, "b", &["x".into(), "y".into()]);
        assert_eq!(
            a,
            finding_id(Code::DrcClearance, "b", &["y".into(), "x".into()])
        );
        assert_ne!(
            a,
            finding_id(Code::DrcHoleSpacing, "b", &["x".into(), "y".into()])
        );
        assert_ne!(
            a,
            finding_id(Code::DrcClearance, "c", &["x".into(), "y".into()])
        );
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn unplaced_orphaned_and_unsupported_placements_are_found() {
        let orphan =
            r#"{"at":[1000000,1000000],"format":1,"kind":"ryueda.placement","rotation_mdeg":0}"#;
        let (_d, mut p, id) = crate::board::shapes::tests::placed_with(&[(
            "boards/b/placements/zz9.json".into(),
            orphan.into(),
        )]);
        let first = p.boards[&id]
            .placements
            .keys()
            .find(|k| *k != "zz9")
            .cloned()
            .unwrap();
        p.boards.get_mut(&id).unwrap().placements.remove(&first);
        let design = p.boards[&id].board.item.as_ref().unwrap().design.clone();
        let (_, occs) = crate::board::occ::occurrences(&p, &design).unwrap();
        let s = crate::board::shapes::build(&p, &p.boards[&id]).unwrap();
        let f = placements("b", &p.boards[&id], &occs, &s);
        let codes: Vec<Code> = f.iter().map(|x| x.code).collect();
        assert_eq!(
            codes,
            [Code::BoardUnplaced, Code::BoardPlacementOrphan],
            "{f:?}"
        );
        assert_eq!(f[1].refs, ["b:zz9"]);
        assert!(f.iter().all(|x| x.severity == Severity::Error));
    }
}
