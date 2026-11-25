//! A board's state kept between requests (design 924, phase 9): its
//! pieces, shapes, connectivity and check, carried to each new state of
//! the files by comparing what they derive. After any update it says what
//! a check built from the files says (`tests/board_props.rs` holds this).

use super::drc::{self, BoardCheck};
use super::load::BoardFiles;
use super::shapes::{self, Pieces};
use crate::error::Result;
use crate::project::Project;
use std::sync::Arc;

/// How an update went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    /// Nothing the board derives from changed.
    Same,
    /// Edited in place.
    Edited,
    /// Built anew: the board file, its preset, its rules or its layers
    /// moved, or the old check was still shared.
    Rebuilt,
}

pub struct Live {
    pieces: Pieces,
    /// Always set but during an update, which takes it.
    check: Option<Arc<BoardCheck>>,
    /// The hash of what, when it changes, rebuilds from nothing.
    rules_key: [u8; 32],
}

impl Live {
    pub fn build(p: &Project, b: &BoardFiles) -> Result<Live> {
        let pieces = shapes::pieces(p, b, None)?;
        let s = Arc::new(shapes::assemble(p, b, &pieces, &[])?);
        let check = Arc::new(drc::check_shapes(p, b, s)?);
        Ok(Live {
            pieces,
            check: Some(check),
            rules_key: rules_key(p, b),
        })
    }

    /// The board's check as it stands.
    pub fn check(&self) -> &Arc<BoardCheck> {
        self.check.as_ref().expect("a live board has its check")
    }

    /// Carries the state to `b` as `p` has it now.
    pub fn update(&mut self, p: &Project, b: &BoardFiles) -> Result<Update> {
        if rules_key(p, b) != self.rules_key {
            *self = Live::build(p, b)?;
            return Ok(Update::Rebuilt);
        }
        let pieces = shapes::pieces(p, b, Some(&self.pieces))?;
        if pieces.base != self.pieces.base {
            *self = Live::build(p, b)?;
            return Ok(Update::Rebuilt);
        }
        let s = Arc::new(shapes::assemble(p, b, &pieces, &self.check().shapes.pours)?);
        let same = pieces.order == self.pieces.order
            && pieces
                .by
                .iter()
                .all(|(k, (h, _))| self.pieces.by.get(k).is_some_and(|(g, _)| g == h))
            && s.pours == self.check().shapes.pours;
        if same {
            self.pieces = pieces;
            return Ok(Update::Same);
        }
        let board = b.id.to_string();
        // The copper is edited in place only when nothing else holds it.
        let old = self.check.take().expect("a live board has its check");
        let conn = Arc::try_unwrap(old)
            .ok()
            .and_then(|c| Arc::try_unwrap(c.conn).ok())
            .and_then(|mut conn| conn.apply(&board, &s).map(|_| conn));
        match conn {
            Some(conn) => {
                self.check = Some(Arc::new(drc::check_with(p, b, s, Arc::new(conn))?));
                self.pieces = pieces;
                Ok(Update::Edited)
            }
            None => {
                *self = Live::build(p, b)?;
                Ok(Update::Rebuilt)
            }
        }
    }
}

/// What, when it changes, makes the board's state build anew: its rules
/// file, the board file, its preset and the design's nets' classes.
fn rules_key(p: &Project, b: &BoardFiles) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"ryueda.board-rules-key.1");
    if let Some(r) = &b.rules {
        h.update(format!("{:?}", r.item.as_ref().ok()).as_bytes());
    }
    if let Ok(f) = &b.board.item {
        h.update(serde_json::to_string(f).unwrap_or_default().as_bytes());
        if let Some(pr) = super::preset::resolve(p, &f.preset) {
            h.update(serde_json::to_string(&*pr).unwrap_or_default().as_bytes());
        }
        if let Ok(n) = crate::netlist::flatten(p, &f.design) {
            for x in &n.nets {
                h.update(format!("{}={:?};", x.id, x.class).as_bytes());
            }
        }
    }
    *h.finalize().as_bytes()
}

/// What a check says, as one string to compare: its findings, islands
/// and uncertain contacts.
pub fn said(c: &BoardCheck) -> String {
    format!(
        "{}\n{:?}\n{:?}",
        serde_json::to_string(&c.findings).unwrap_or_default(),
        c.conn.copper.islands(),
        c.conn.copper.uncertain()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::shapes::tests::{placed, placed_with};

    #[test]
    fn an_updated_live_is_a_rebuild() {
        let (_d, p, id) = placed();
        let mut live = Live::build(&p, &p.boards[&id]).unwrap();
        assert_eq!(live.update(&p, &p.boards[&id]).unwrap(), Update::Same);
        let net = crate::board::shapes::tests::a_net(&p);
        let track = format!(
            r#"{{"format":1,"kind":"ryueda.track","layer":"top","net":"{net}","points":[[1000000,1000000],[4000000,1000000]],"width_nm":250000}}"#
        );
        let (_d2, p2, _) = placed_with(&[("boards/b/tracks/t1.json".into(), track)]);
        assert_eq!(live.update(&p2, &p2.boards[&id]).unwrap(), Update::Edited);
        let fresh = crate::board::drc::check(&p2, &p2.boards[&id]).unwrap();
        assert_eq!(said(live.check()), said(&fresh));
        // And back.
        assert_eq!(live.update(&p, &p.boards[&id]).unwrap(), Update::Edited);
        assert_eq!(
            said(live.check()),
            said(&crate::board::drc::check(&p, &p.boards[&id]).unwrap())
        );
    }
}
