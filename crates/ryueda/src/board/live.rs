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
    /// Nothing the board derives from changed: its shapes, copper and
    /// findings held (the whole-board rules ran again to say so).
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
    /// What the last update changed; `None` after a build.
    pub dirty: Option<drc::Dirty>,
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
            dirty: None,
        })
    }

    /// The board's check as it stands.
    pub fn check(&self) -> &Arc<BoardCheck> {
        self.check.as_ref().expect("a live board has its check")
    }

    /// Carries the state to `b` as `p` has it now.
    pub fn update(&mut self, p: &Project, b: &BoardFiles) -> Result<Update> {
        let stage = crate::profile::stage("live.rules_key");
        let key = rules_key(p, b);
        drop(stage);
        if key != self.rules_key {
            *self = Live::build(p, b)?;
            return Ok(Update::Rebuilt);
        }
        let stage = crate::profile::stage("live.pieces");
        let pieces = shapes::pieces(p, b, Some(&self.pieces))?;
        drop(stage);
        if pieces.base != self.pieces.base {
            *self = Live::build(p, b)?;
            return Ok(Update::Rebuilt);
        }
        let stage = crate::profile::stage("live.assemble");
        let s = Arc::new(shapes::assemble(p, b, &pieces, &self.check().shapes.pours)?);
        drop(stage);
        let same = pieces.order == self.pieces.order
            && pieces
                .by
                .iter()
                .all(|(k, (h, _))| self.pieces.by.get(k).is_some_and(|(g, _)| g == h))
            && s.pours == self.check().shapes.pours;
        if same {
            // The copper and the shapes held, but the whole-board rules
            // read the design too (unplaced parts, constraints): they run
            // again, and the scoped rules keep every finding, judging an
            // empty dirty set (a review of 7.E found a component added
            // to the schematic left `board_unplaced` stale).
            let _stage = crate::profile::stage("live.recheck");
            let old = self.check().clone();
            let dirty = drc::Dirty::default();
            let c = drc::check_with(
                p,
                b,
                old.shapes.clone(),
                old.conn.clone(),
                Some((old.raw.as_slice(), &dirty)),
            )?;
            let moved = c.findings != old.findings;
            self.check = Some(Arc::new(c));
            self.pieces = pieces;
            self.dirty = Some(dirty);
            return Ok(if moved { Update::Edited } else { Update::Same });
        }
        let board = b.id.to_string();
        let old_shapes = self.check().shapes.clone();
        // The copper is edited in place only when nothing else holds it.
        let old = self.check.take().expect("a live board has its check");
        let taken = Arc::try_unwrap(old).ok().and_then(|c| {
            let raw = c.raw;
            Arc::try_unwrap(c.conn).ok().map(|conn| (conn, raw))
        });
        let stage = crate::profile::stage("live.apply");
        let delta = super::connect::Delta {
            was: &self.pieces,
            now: &pieces,
            old_pours: &old_shapes.pours,
        };
        let applied = taken.and_then(|(mut conn, raw)| {
            conn.apply_delta(&board, &s, Some(&delta))
                .map(|ch| (conn, raw, ch))
        });
        match applied {
            Some((conn, raw, changed)) => {
                drop(stage);
                let stage = crate::profile::stage("live.dirty");
                let dirty = dirty_of(
                    &board,
                    &conn.built,
                    &self.pieces,
                    &pieces,
                    &old_shapes,
                    &s,
                    changed,
                );
                // An edit that dirties most of the board is judged whole:
                // no slower, and the edit path stays for small edits.
                let most = 2 * dirty.objs.len() > pieces.order.len().max(1);
                let prev = (!most).then_some((raw.as_slice(), &dirty));
                drop(stage);
                let _stage = crate::profile::stage("live.check");
                self.check = Some(Arc::new(drc::check_with(p, b, s, Arc::new(conn), prev)?));
                self.pieces = pieces;
                self.dirty = Some(dirty);
                Ok(Update::Edited)
            }
            None => {
                *self = Live::build(p, b)?;
                Ok(Update::Rebuilt)
            }
        }
    }
}

/// The dirty set of an update: the copper's changes, every object whose
/// piece changed (new, gone or hashed differently), and the references
/// of those objects in the old shapes and the new.
fn dirty_of(
    board: &str,
    built: &super::layout::Built,
    was: &Pieces,
    now: &Pieces,
    old: &super::shapes::BoardShapes,
    new: &super::shapes::BoardShapes,
    changed: super::connect::Changed,
) -> drc::Dirty {
    use super::shapes::Obj;
    let mut d = drc::Dirty {
        keys: changed.keys,
        refs: changed.refs,
        ..drc::Dirty::default()
    };
    for (k, (h, _)) in &now.by {
        if was.by.get(k).is_none_or(|(g, _)| g != h) {
            d.objs.insert(k.clone());
        }
    }
    for k in was.by.keys() {
        if !now.by.contains_key(k) {
            d.objs.insert(k.clone());
        }
    }
    for s in [old, new] {
        for o in &d.objs {
            match o {
                Obj::Placement(r) => {
                    d.refs.extend(
                        s.pads
                            .iter()
                            .filter(|x| &x.occurrence == r)
                            .map(|x| x.reference.clone()),
                    );
                    d.refs.extend(
                        s.courtyards
                            .iter()
                            .filter(|x| &x.occurrence == r)
                            .map(|x| x.owner.clone()),
                    );
                    d.refs.extend(
                        s.keepouts
                            .iter()
                            .filter(|x| x.part.as_deref() == Some(r.as_str()))
                            .map(|x| x.reference.clone()),
                    );
                    d.refs.extend(s.holes.iter().filter(|x| matches!(&x.of, super::shapes::HoleOf::Pad { occ, .. } if occ == r)).map(|x| x.owner.clone()));
                }
                Obj::Track(_) | Obj::Via(_) | Obj::Hole(_) | Obj::Drawing(_) => {}
                Obj::Keepout(id) => {
                    d.refs.extend(
                        s.keepouts
                            .iter()
                            .filter(|x| x.reference.ends_with(&format!(":keepout/{id}")))
                            .map(|x| x.reference.clone()),
                    );
                }
            }
        }
    }
    for o in &d.objs {
        match o {
            Obj::Track(id) => d.refs.insert(format!("{board}:track/{id}")),
            Obj::Via(id) => d.refs.insert(format!("{board}:via/{id}")),
            Obj::Hole(id) => d.refs.insert(format!("{board}:hole/{id}")),
            Obj::Drawing(id) => d.refs.insert(format!("{board}:drawing/{id}")),
            Obj::Keepout(id) => d.refs.insert(format!("{board}:keepout/{id}")),
            Obj::Placement(_) => false,
        };
    }
    // Every layout key a dirty reference names: a pair rule asks the
    // copper about all of a reference's keys (a pad on each layer, a via
    // on each of its span).
    let scope = drc::Scope::Dirty(&d);
    let widen: Vec<super::layout::Key> = built
        .layout
        .items
        .keys()
        .chain(built.layout.holes.keys())
        .filter(|k| !d.keys.contains(*k) && scope.reference(&built.reference(k)))
        .cloned()
        .collect();
    d.keys.extend(widen);
    d
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

/// What a check says, as one string to compare: its findings, islands,
/// uncertain contacts and fills (each pour's polygons, regions' places,
/// islands removed, uncertain regions and thermals; not the tiles and
/// grown obstacles it keeps for the next fill).
pub fn said(c: &BoardCheck) -> String {
    let fills: Vec<String> = c
        .shapes
        .pours
        .iter()
        .map(|f| {
            format!(
                "{} {} {} {:?} {:?} {:?} {} {:?} {:?}",
                f.id,
                f.net,
                f.layer,
                f.rules,
                f.polygons,
                f.places,
                f.islands_removed,
                f.uncertain,
                f.thermals
            )
        })
        .collect();
    format!(
        "{}\n{:?}\n{:?}\n{}",
        serde_json::to_string(&c.findings).unwrap_or_default(),
        c.conn.copper.islands(),
        c.conn.copper.uncertain(),
        fills.join("\n")
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

    #[test]
    fn an_edit_judges_only_what_it_dirtied_and_says_what_a_rebuild_says() {
        let (_d, p, id) = placed();
        let mut live = Live::build(&p, &p.boards[&id]).unwrap();
        let net = crate::board::shapes::tests::a_net(&p);
        let track = format!(
            r#"{{"format":1,"kind":"ryueda.track","layer":"top","net":"{net}","points":[[1000000,1000000],[1500000,1000000]],"width_nm":10}}"#
        );
        let (_d2, p2, _) = placed_with(&[("boards/b/tracks/t1.json".into(), track)]);
        assert_eq!(live.update(&p2, &p2.boards[&id]).unwrap(), Update::Edited);
        let judged = live
            .dirty
            .as_ref()
            .unwrap()
            .judged
            .load(std::sync::atomic::Ordering::Relaxed);
        let s = &live.check().shapes;
        let objects = s.pads.len() + s.tracks.len() + s.vias.len() + s.holes.len();
        assert!(
            judged > 0 && judged * 2 < objects,
            "{judged} judged of {objects} objects"
        );
        let fresh = crate::board::drc::check(&p2, &p2.boards[&id]).unwrap();
        assert!(
            fresh
                .findings
                .iter()
                .any(|f| f.code == crate::error::Code::DrcTrackWidth)
        );
        assert_eq!(said(live.check()), said(&fresh));
    }
}
