//! What marks a row (design 938, Pointing): this window's selection, its
//! hover, others' selections in their participants' colours, highlights
//! in their style's colour, and a changed flash, in that order. Every
//! colour is held to the tile's background through `ryu_app::style::held`
//! (WCAG 3:1; the selection 4.5:1 and the hover 7.5:1, so they stay apart
//! from the participants' colours for colour-blind eyes, as ryueda-app's).

use crate::core::flash::Flash;
use crate::core::people::People;
use eframe::egui::Color32;
use ryu_app::style::{HOVER, ON_BACKGROUND, SELECTED, color32, held, highlight, participant};
use std::time::Instant;

pub const SELECTED_ON_BACKGROUND: f64 = 4.5;
pub const HOVER_ON_BACKGROUND: f64 = 7.5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    Me,
    Hover,
    Other(String),
    Highlight(String),
    Changed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub colour: Color32,
    pub who: Who,
}

/// What marking reads.
pub struct Marking<'a> {
    pub me: &'a str,
    pub mine: &'a [String],
    pub hover: Option<&'a str>,
    pub people: &'a People,
    pub flash: &'a Flash,
    pub now: Instant,
    pub background: Color32,
}

/// The mark of the row whose live reference is `r`, if any.
pub fn mark(r: &str, k: &Marking) -> Option<Mark> {
    marked(|x| x == r, r, k)
}

/// The mark of item `id`'s row: the item, or any of its lines.
pub fn mark_item(id: &str, k: &Marking) -> Option<Mark> {
    let me = format!("item/{id}");
    let line = format!("{me}/line/");
    marked(|x| x == me || x.starts_with(&line), &me, k)
}

fn marked(hit: impl Fn(&str) -> bool, flash_ref: &str, k: &Marking) -> Option<Mark> {
    let bg = k.background;
    if k.mine.iter().any(|s| hit(s)) {
        return Some(Mark {
            colour: held(color32(SELECTED), bg, SELECTED_ON_BACKGROUND),
            who: Who::Me,
        });
    }
    if k.hover.is_some_and(&hit) {
        return Some(Mark {
            colour: held(color32(HOVER), bg, HOVER_ON_BACKGROUND),
            who: Who::Hover,
        });
    }
    for p in k.people.others(k.me) {
        if p.selection.iter().any(|s| hit(s)) {
            return Some(Mark {
                colour: held(color32(participant(&p.id)), bg, ON_BACKGROUND),
                who: Who::Other(p.id.clone()),
            });
        }
    }
    for p in k.people.all.values() {
        for h in &p.highlights {
            if h.entities.iter().any(|s| hit(s)) {
                return Some(Mark {
                    colour: held(color32(highlight(h.style)), bg, ON_BACKGROUND),
                    who: Who::Highlight(p.id.clone()),
                });
            }
        }
    }
    let s = k.flash.strength(flash_ref, k.now);
    (s > 0.0).then(|| {
        let c = held(
            color32(highlight(ryu_session::live::Style::Changed)),
            bg,
            ON_BACKGROUND,
        );
        Mark {
            colour: c.gamma_multiply(s),
            who: Who::Changed,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::people::People;
    use eframe::egui::Color32;
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[test]
    fn a_row_takes_the_strongest_mark_held_to_the_background() {
        let mut people = People::default();
        people.reset(&json!([
            {"id": "p-1", "kind": "human", "name": "me", "selection": [], "highlights": []},
            {"id": "p-2", "kind": "human", "name": "Dana", "selection": ["item/b"], "highlights": []},
            {"id": "p-3", "kind": "agent", "name": "agent", "selection": [],
             "highlights": [{"style": "pending", "label": null, "entities": ["item/c"]}]},
        ]));
        let flash = Flash::default();
        let now = Instant::now();
        let bg = Color32::WHITE;
        let m = |r: &str, mine: &[&str], hover: Option<&str>| {
            let mine: Vec<String> = mine.iter().map(|s| s.to_string()).collect();
            mark(
                r,
                &Marking {
                    me: "p-1",
                    mine: &mine,
                    hover,
                    people: &people,
                    flash: &flash,
                    now,
                    background: bg,
                },
            )
        };
        assert_eq!(m("item/z", &[], None), None);
        let sel = m("item/b", &["item/b"], None).unwrap();
        assert_eq!(sel.who, Who::Me);
        assert!(ryu_app::style::contrast(sel.colour, bg) >= SELECTED_ON_BACKGROUND);
        assert_eq!(m("item/b", &[], Some("item/b")).unwrap().who, Who::Hover);
        let theirs = m("item/b", &[], None).unwrap();
        assert_eq!(theirs.who, Who::Other("p-2".into()));
        assert!(ryu_app::style::contrast(theirs.colour, bg) >= ryu_app::style::ON_BACKGROUND);
        assert_eq!(
            m("item/c", &[], None).unwrap().who,
            Who::Highlight("p-3".into())
        );
        // A line's mark marks its item's row too.
        assert_eq!(
            mark_item(
                "b",
                &Marking {
                    me: "p-1",
                    mine: &["item/b/line/l1".into()],
                    hover: None,
                    people: &people,
                    flash: &flash,
                    now,
                    background: bg
                }
            )
            .unwrap()
            .who,
            Who::Me
        );
        // A changed row flashes for two seconds, fading.
        let mut f = Flash::default();
        f.changed(&json!(["item/z", "catalogue"]), now);
        let k = Marking {
            me: "p-1",
            mine: &[],
            hover: None,
            people: &people,
            flash: &f,
            now,
            background: bg,
        };
        assert_eq!(mark("item/z", &k).unwrap().who, Who::Changed);
        assert!((f.strength("item/z", now + Duration::from_secs(1)) - 0.5).abs() < 1e-3);
        f.expire(now + crate::core::flash::GLOW);
        assert!(!f.due());
    }
}
