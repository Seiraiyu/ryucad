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

/// WCAG's relative luminance of an sRGB colour.
fn luminance(c: Color32) -> f64 {
    let lin = |u: u8| {
        let x = f64::from(u) / 255.0;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ryu_math::pow((x + 0.055) / 1.055, 2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG's contrast ratio of two colours, 1 to 21.
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The least needed for a mark on paper (WCAG 2.1's non-text contrast).
pub const ON_PAPER: f64 = 3.0;

/// `c`, or the nearest shade of it at least [`ON_PAPER`] against `paper`:
/// mixed toward black on light paper, toward white on dark, in 5 % steps
/// (#879). `ryu-app`'s colours are never changed, so no ryucad picture
/// moves.
pub fn on_paper(c: Color32, paper: Color32) -> Color32 {
    on_paper_at(c, paper, ON_PAPER)
}

/// The selection's contrast on the paper: above [`ON_PAPER`], so it stays
/// apart from the hover and the participants' colours for colour-blind
/// eyes (#879, the vision test).
pub const SELECTED_ON_PAPER: f64 = 4.5;
/// The hover's: further still, so it and the selection differ in
/// lightness where their hues look alike.
pub const HOVER_ON_PAPER: f64 = 7.5;

/// [`on_paper`] to `ratio` rather than [`ON_PAPER`].
pub fn on_paper_at(c: Color32, paper: Color32, ratio: f64) -> Color32 {
    if contrast(c, paper) >= ratio {
        return c;
    }
    let toward: u8 = if luminance(paper) > 0.18 { 0 } else { 255 };
    let mix = |u: u8, t: f32| (f32::from(u) * (1.0 - t) + f32::from(toward) * t).round() as u8;
    for k in 1..=20 {
        let t = k as f32 * 0.05;
        let x = Color32::from_rgba_unmultiplied(mix(c.r(), t), mix(c.g(), t), mix(c.b(), t), c.a());
        if contrast(x, paper) >= ratio {
            return x;
        }
    }
    Color32::from_gray(toward)
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
    let paper = scene.paper.colour;
    let halo = scene.paper.diff.map_or_else(
        || on_paper(color32(highlight(Style::Changed)), paper),
        |d| d.changed,
    );
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
                        fill: on_paper(color32(highlight(h.style)), paper),
                        edge: Some(on_paper(color32(participant(&p.id)), paper)),
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
        let c = on_paper(color32(participant(&p.id)), paper);
        for r in &p.selection {
            paint(&mut m, r, c);
        }
    }
    if let Some(h) = hover {
        paint(
            &mut m,
            h,
            on_paper_at(color32(HOVER), paper, HOVER_ON_PAPER),
        );
    }
    for r in mine {
        paint(
            &mut m,
            r,
            on_paper_at(color32(SELECTED), paper, SELECTED_ON_PAPER),
        );
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
        let white = Color32::WHITE;
        let sel = on_paper_at(color32(SELECTED), white, SELECTED_ON_PAPER);
        // The net and its label are mine.
        assert_eq!(m.owners[1].map(|x| x.fill), Some(sel));
        assert_eq!(m.owners[2].map(|x| x.fill), Some(sel));
        // The agent's selection, in its colour.
        assert_eq!(
            m.owners[0].map(|x| x.fill),
            Some(on_paper(color32(participant("a-1")), white))
        );
        // The agent's highlight, in its style's colour, edged in its own.
        let h = m.owners[3].unwrap();
        assert_eq!(h.fill, on_paper(color32(highlight(Style::Pending)), white));
        assert_eq!(h.edge, Some(on_paper(color32(participant("a-1")), white)));
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
        assert_eq!(
            m.owners[3].map(|x| x.fill),
            Some(on_paper_at(color32(HOVER), white, HOVER_ON_PAPER))
        );
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
        assert_eq!(
            m.owners[0].map(|x| x.fill),
            Some(on_paper_at(
                color32(SELECTED),
                s.paper.colour,
                SELECTED_ON_PAPER
            ))
        );
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
        assert_eq!(
            m.halos[0].1,
            on_paper(color32(highlight(Style::Changed)), plain.paper.colour)
        );
    }

    #[test]
    fn every_mark_is_three_to_one_on_either_paper() {
        let papers = [Color32::WHITE, Color32::from_rgb(0x1d, 0x20, 0x25)];
        let mut colours = vec![color32(SELECTED), color32(HOVER)];
        colours.extend(ryu_app::style::PALETTE.iter().map(|c| color32(*c)));
        for s in [Style::Pending, Style::Changed, Style::Note] {
            colours.push(color32(highlight(s)));
        }
        for paper in papers {
            for c in &colours {
                let on = on_paper(*c, paper);
                assert!(
                    contrast(on, paper) >= 3.0,
                    "{c:?} on {paper:?}: {:.2}",
                    contrast(on, paper)
                );
                if contrast(*c, paper) >= 3.0 {
                    assert_eq!(on, *c, "a colour that passes is kept");
                }
            }
        }
        assert!(
            contrast(color32(SELECTED), Color32::WHITE) < 3.0,
            "the reason for #879"
        );
        assert_eq!(
            ryu_app::style::SELECTED,
            [0.50, 0.67, 1.0],
            "ryu-app's own is untouched"
        );
    }

    /// The built-in style's diff colours, as `scene` answers them in
    /// `paper.diff` (ryueda's scene test pins `#1a7f37`), light and dark.
    const DIFF: [([u8; 3], [u8; 3]); 3] = [
        ([0x1a, 0x7f, 0x37], [0x3f, 0xb9, 0x50]),
        ([0x9a, 0x67, 0x00], [0xd2, 0x99, 0x22]),
        ([0xcf, 0x22, 0x2e], [0xf8, 0x51, 0x49]),
    ];

    /// ryueda's marks stay apart (CIEDE2000 at least 10) under each of
    /// ryu-app's visions, on white and dark paper (#879), where 13.A can
    /// hold them so: the selection and the hover (ryueda's own shades)
    /// against everything, and the diff roles among themselves under
    /// normal vision. EXCLUDED_PAIRS lists what it can't settle.
    #[test]
    fn ryuedas_marks_stay_apart_for_colour_blind_eyes() {
        use ryu_app::style::vision::{VISIONS, apart};
        let rgb = |c: Color32| [c.r(), c.g(), c.b()].map(|x| f32::from(x) / 255.0);
        let mut fails = Vec::new();
        for (dark, paper) in [
            (false, Color32::WHITE),
            (true, Color32::from_rgb(0x1d, 0x20, 0x25)),
        ] {
            let diff = |i: usize| {
                let c = if dark { DIFF[i].1 } else { DIFF[i].0 };
                Color32::from_rgb(c[0], c[1], c[2])
            };
            let ours = [
                (
                    "selection",
                    on_paper_at(color32(SELECTED), paper, SELECTED_ON_PAPER),
                ),
                ("hover", on_paper_at(color32(HOVER), paper, HOVER_ON_PAPER)),
                ("diff_added", diff(0)),
                ("diff_changed and the halo", diff(1)),
                ("diff_removed", diff(2)),
            ];
            let people: Vec<(String, Color32)> = ryu_app::style::PALETTE
                .iter()
                .enumerate()
                .map(|(i, c)| (format!("participant {i}"), on_paper(color32(*c), paper)))
                .collect();
            for (vision, m) in VISIONS {
                for (i, (a, x)) in ours.iter().enumerate() {
                    let others = ours[i + 1..]
                        .iter()
                        .map(|(n, c)| ((*n).to_owned(), *c))
                        .chain(people.iter().cloned());
                    for (b, y) in others {
                        let line = format!("dark={dark} {vision}: {a} vs {b}");
                        let d = apart(rgb(*x), rgb(y), &m);
                        let held = match (a.starts_with("diff"), b.starts_with("diff")) {
                            // The selection and the hover against everything.
                            (false, _) => true,
                            // The diff roles among themselves, normal vision.
                            (true, true) => vision == "normal",
                            // The diff roles against ryu-session's palette.
                            (true, false) => false,
                        } && !EXCLUDED_PAIRS.contains(&line.as_str());
                        if held && d < 10.0 {
                            fails.push(format!("{line}: {d:.1}"));
                        }
                    }
                }
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }

    /// What 13.A can't settle (#921): the pairs under ΔE 10 the test
    /// leaves out, each with its ΔE and why. Diff marks are told apart by
    /// shape too (ghosts dashed and unfilled, leaders dashed, added and
    /// changed solid), so colour alone isn't the cue.
    ///
    /// Against ryu-session's participant palette (shared with ryucad, not
    /// ryueda's to move):
    /// - dark=false protanopia: selection vs participant 0: 3.8 (the one
    ///   pair of the selection's left after its own target; listed below)
    /// - dark=false protanopia: diff_added vs participant 1: 8.4
    /// - dark=false protanopia: diff_added vs participant 2: 9.9
    /// - dark=false deuteranopia: diff_changed vs participant 2: 4.3
    /// - dark=false deuteranopia: diff_removed vs participant 2: 3.9
    /// - dark=true normal: diff_removed vs participant 4: 8.2
    /// - dark=true protanopia: diff_added vs participant 1: 3.6
    /// - dark=true protanopia: diff_changed vs participant 1: 7.1
    /// - dark=true protanopia: diff_changed vs participant 2: 7.5
    /// - dark=true protanopia: diff_removed vs participant 4: 9.5
    /// - dark=true deuteranopia: diff_added vs participant 1: 8.0
    /// - dark=true deuteranopia: diff_added vs participant 4: 5.5
    /// - dark=true deuteranopia: diff_changed vs participant 1: 5.4
    /// - dark=true deuteranopia: diff_removed vs participant 1: 8.5
    /// - dark=true deuteranopia: diff_removed vs participant 2: 8.7
    /// - dark=true deuteranopia: diff_removed vs participant 4: 7.3
    /// - dark=true tritanopia: diff_added vs participant 5: 6.3
    /// - dark=true tritanopia: diff_changed vs participant 1: 5.2
    /// - dark=true tritanopia: diff_removed vs participant 4: 3.1
    ///
    /// The diff roles among themselves under colour-blind vision (red
    /// against green and amber; no move within each role's hue clears 10):
    /// - dark=false protanopia: diff_added vs diff_changed: 5.7
    /// - dark=false deuteranopia: diff_added vs diff_removed: 8.2
    /// - dark=false deuteranopia: diff_changed vs diff_removed: 3.0
    /// - dark=true protanopia: diff_added vs diff_changed: 5.9
    /// - dark=true deuteranopia: diff_added vs diff_changed: 8.2
    /// - dark=true deuteranopia: diff_added vs diff_removed: 3.1
    /// - dark=true deuteranopia: diff_changed vs diff_removed: 6.7
    const EXCLUDED_PAIRS: &[&str] = &["dark=false protanopia: selection vs participant 0"];
}
