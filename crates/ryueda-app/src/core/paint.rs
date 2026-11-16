//! What a window paints over the scene (design 519, Pointing and
//! highlighting): each owner's mark colour from this window's selection,
//! its hover, others' selections and highlights, and changed flashes, in
//! that order; and the pins to ring. The colours are `ryu_session`'s, so
//! a highlight looks the same in every window and every render.

use crate::core::flash::Flash;
use crate::core::people::People;
use crate::core::scene::Scene;
use eframe::egui::Color32;
use ryu_app::style::{HOVER, SELECTED, color32, highlight, participant};
use ryu_session::live::Style;
use std::time::Instant;

/// An owner's mark: the colour its items are drawn in, and an edge round
/// its box (a highlight's participant).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub fill: Color32,
    pub edge: Option<Color32>,
}

/// Marks by owner index, and pins to ring as `(pin index, colour)`.
#[derive(Debug, Default, PartialEq)]
pub struct Marks {
    pub owners: Vec<Option<Mark>>,
    pub pins: Vec<(usize, Color32)>,
    /// Owners with a changed halo, and its colour (its alpha the fade).
    pub halos: Vec<(usize, Color32)>,
}

/// The owners a selected reference paints: its own, and for a net every
/// label of it.
fn owners_of<'a>(scene: &'a Scene, r: &'a str) -> impl Iterator<Item = usize> + 'a {
    scene
        .owners
        .iter()
        .enumerate()
        .filter(move |(_, o)| o.reference == r || o.net.as_deref() == Some(r))
        .map(|(i, _)| i)
}

pub fn marks(
    scene: &Scene,
    me: &str,
    mine: &[String],
    hover: Option<&str>,
    people: &People,
    flash: &Flash,
    now: Instant,
) -> Marks {
    let mut m = Marks {
        owners: vec![None; scene.owners.len()],
        pins: Vec::new(),
        halos: Vec::new(),
    };
    // Changed flashes (#879): a halo under the owner in the theme's
    // diff_changed, fading; never a recolour, so never a selection.
    let halo = scene
        .paper
        .diff
        .map_or_else(|| color32(highlight(Style::Changed)), |d| d.changed);
    for (i, o) in scene.owners.iter().enumerate() {
        let s = flash.strength(&o.reference, now);
        if s > 0.0 {
            m.halos.push((i, halo.gamma_multiply(s)));
        }
    }
    // Lowest precedence first; later writes win.
    // Compare colours (plan 13.A): over flashes, under everything else.
    if let Some(d) = scene.paper.diff {
        for (i, o) in scene.owners.iter().enumerate() {
            let fill = match o.mark {
                Some(crate::core::scene::Diff::Added) => d.added,
                Some(_) => d.changed,
                None => continue,
            };
            m.owners[i] = Some(Mark { fill, edge: None });
        }
    }
    let others: Vec<_> = people.others(me).collect();
    for p in &others {
        for h in &p.highlights {
            for r in &h.entities {
                for i in owners_of(scene, r) {
                    m.owners[i] = Some(Mark {
                        fill: color32(highlight(h.style)),
                        edge: Some(color32(participant(&p.id))),
                    });
                }
            }
        }
    }
    let paint = |m: &mut Marks, r: &str, c: Color32| {
        for i in owners_of(scene, r) {
            m.owners[i] = Some(Mark {
                fill: c,
                edge: None,
            });
        }
        for (k, pin) in scene.pins.iter().enumerate() {
            if pin.reference == r {
                m.pins.retain(|(j, _)| *j != k);
                m.pins.push((k, c));
            }
        }
    };
    for p in &others {
        let c = color32(participant(&p.id));
        for r in &p.selection {
            paint(&mut m, r, c);
        }
    }
    if let Some(h) = hover {
        paint(&mut m, h, color32(HOVER));
    }
    for r in mine {
        paint(&mut m, r, color32(SELECTED));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::people::People;
    use crate::core::scene::{Scene, tests::small};
    use serde_json::json;

    #[test]
    fn mine_then_hover_then_others_then_highlights_then_flashes() {
        let s = Scene::from_json(&small()).unwrap();
        let mut people = People::default();
        people.reset(&json!([
            {"id": "me", "kind": "human", "name": "Dana", "selection": [], "highlights": []},
            {"id": "a-1", "kind": "agent", "name": "agent", "selection": ["top/r1"],
             "highlights": [{"style": "pending", "entities": ["top/region:power"]}]},
        ]));
        let flash = crate::core::flash::Flash::default();
        let now = std::time::Instant::now();
        let mine = vec!["top/sda".to_owned()];
        let m = marks(&s, "me", &mine, None, &people, &flash, now);
        let sel = color32(SELECTED);
        // The net and its label are mine.
        assert_eq!(m.owners[1].map(|x| x.fill), Some(sel));
        assert_eq!(m.owners[2].map(|x| x.fill), Some(sel));
        // The agent's selection, in its colour.
        assert_eq!(
            m.owners[0].map(|x| x.fill),
            Some(color32(participant("a-1")))
        );
        // The agent's highlight, in its style's colour, edged in its own.
        let h = m.owners[3].unwrap();
        assert_eq!(h.fill, color32(highlight(Style::Pending)));
        assert_eq!(h.edge, Some(color32(participant("a-1"))));
        // Hovering the region: hover is over another's highlight.
        let m = marks(
            &s,
            "me",
            &[],
            Some("top/region:power"),
            &people,
            &flash,
            now,
        );
        assert_eq!(m.owners[3].map(|x| x.fill), Some(color32(HOVER)));
        // A selected pin rings its pin.
        let m = marks(
            &s,
            "me",
            &["top/r1/2".to_owned()],
            None,
            &people,
            &flash,
            now,
        );
        assert_eq!(m.pins, [(1, sel)]);
    }

    #[test]
    fn compare_colours_sit_under_highlights_and_selections() {
        use crate::core::scene::tests::compared;
        let s = Scene::from_json(&compared()).unwrap();
        let d = s.paper.diff.unwrap();
        let mut people = People::default();
        people.reset(&json!([
            {"id": "me", "kind": "human", "name": "Dana", "selection": [], "highlights": []},
        ]));
        let flash = crate::core::flash::Flash::default();
        let now = std::time::Instant::now();
        let m = marks(&s, "me", &[], None, &people, &flash, now);
        assert_eq!(
            m.owners[0].map(|x| x.fill),
            Some(d.changed),
            "moved: diff_changed"
        );
        assert_eq!(m.owners[1].map(|x| x.fill), Some(d.added));
        assert_eq!(m.owners[2], None, "unmarked");
        // Mine over the compare colour.
        let m = marks(&s, "me", &["top/r1".to_owned()], None, &people, &flash, now);
        assert_eq!(m.owners[0].map(|x| x.fill), Some(color32(SELECTED)));
    }

    #[test]
    fn a_flash_is_a_fading_halo_never_a_recolour() {
        use crate::core::scene::tests::compared;
        let s = Scene::from_json(&compared()).unwrap();
        let people = People::default();
        let mut flash = crate::core::flash::Flash::default();
        let now = std::time::Instant::now();
        flash.start("top/label:sda@1", now);
        let m = marks(&s, "me", &[], None, &people, &flash, now);
        assert_eq!(m.owners[2], None, "a flash recolours nothing");
        let changed = s.paper.diff.unwrap().changed;
        assert_eq!(m.halos, [(2, changed)]);
        let later = now + std::time::Duration::from_secs(1);
        let faded = marks(&s, "me", &[], None, &people, &flash, later).halos[0].1;
        assert!(faded.a() < 255 && faded.a() > 0, "{faded:?}");
        // A plain scene from before paper.diff: ryu-session's changed colour.
        let mut v = crate::core::scene::tests::small();
        v["paper"].as_object_mut().unwrap().remove("diff");
        let plain = Scene::from_json(&v).unwrap();
        let m = marks(&plain, "me", &[], None, &people, &flash, now);
        assert_eq!(m.halos[0].1, color32(highlight(Style::Changed)));
    }
}
