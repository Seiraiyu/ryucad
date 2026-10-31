//! Stability, the spike's hard bar: on seeded blocks, a scripted edit (a
//! part added, removed or revalued; a net renamed, joined or split; a pin
//! moved to another net) laid out over the previous layout changes no
//! placement, label, text or route but those of the parts it names and the
//! nets their pins are on, and every hard rule holds after it. Release
//! only: `cargo test -p ryusch --release --test layout_props`.

use ryusch::engine::lay_out;
use ryusch::input::{Block, NetKind};
use ryusch::layout::Layout;
use ryusch::testing::{resistor, seeded};
use std::collections::BTreeSet;

/// What an edit may change: parts and nets by key.
struct Allowed {
    parts: BTreeSet<String>,
    nets: BTreeSet<String>,
}

/// The nets a part's pins are on, by key.
fn nets_of(b: &Block, part: &str) -> BTreeSet<String> {
    let p = b.part(part).unwrap();
    b.parts[p]
        .pins
        .iter()
        .filter_map(|x| x.net)
        .map(|n| b.nets[n].key.clone())
        .collect()
}

fn is_rail(b: &Block, key: &str) -> bool {
    b.net(key)
        .is_some_and(|n| b.nets[n].kind != NetKind::Signal)
}

/// Compares two layouts outside what an edit may change; rails' labels
/// one by one (a rail is on every sheet, so only the edited pins' may
/// move), every other net's whole.
fn unchanged(b: &Block, before: &Layout, after: &Layout, a: &Allowed, what: &str) {
    for (k, x) in &before.placed {
        if !a.parts.contains(k)
            && let Some(y) = after.placed.get(k)
        {
            assert_eq!(x, y, "{what}: {k} moved");
        }
    }
    for (k, x) in &before.text {
        if !a.parts.contains(k)
            && let Some(y) = after.text.get(k)
        {
            assert_eq!(x, y, "{what}: {k}'s text moved");
        }
    }
    let nets: BTreeSet<&String> = before.routes.keys().chain(after.routes.keys()).collect();
    for k in nets {
        if !a.nets.contains(k) && !is_rail(b, k) {
            assert_eq!(
                before.routes.get(k),
                after.routes.get(k),
                "{what}: {k}'s wires changed"
            );
        }
    }
    let labelled: BTreeSet<&String> = before.labels.keys().chain(after.labels.keys()).collect();
    for k in labelled {
        if a.nets.contains(k) && !is_rail(b, k) {
            continue;
        }
        if is_rail(b, k) {
            let keep = |l: &&ryusch::layout::Label| {
                let part = l.on.rsplit_once('/').map_or("", |(p, _)| p);
                !a.parts.contains(part)
            };
            let x: Vec<_> = before
                .labels
                .get(k)
                .into_iter()
                .flatten()
                .filter(keep)
                .collect();
            let y: Vec<_> = after
                .labels
                .get(k)
                .into_iter()
                .flatten()
                .filter(keep)
                .collect();
            assert_eq!(x, y, "{what}: {k}'s ports moved");
        } else {
            assert_eq!(
                before.labels.get(k),
                after.labels.get(k),
                "{what}: {k}'s labels changed"
            );
        }
    }
}

/// Lays `edited` out over `base`'s layout and holds it to the bar.
fn check(base: &Block, edited: &mut Block, a: Allowed, what: &str) {
    let before = lay_out(base).unwrap().layout;
    edited.previous = Some(before.clone());
    let after = lay_out(edited).unwrap().layout;
    unchanged(edited, &before, &after, &a, what);
    let broken = ryusch::rules::check(edited, &after);
    assert!(broken.is_empty(), "{what}: {broken:#?}");
}

/// A signal net with at least `pins` pins, and its pins as `(part, pin)`.
fn signal(b: &Block, pins: usize) -> Option<(usize, Vec<(usize, usize)>)> {
    (0..b.nets.len()).find_map(|n| {
        if b.nets[n].kind != NetKind::Signal {
            return None;
        }
        let on: Vec<(usize, usize)> = b
            .parts
            .iter()
            .enumerate()
            .flat_map(|(p, part)| {
                part.pins
                    .iter()
                    .enumerate()
                    .filter(move |(_, x)| x.net == Some(n))
                    .map(move |(k, _)| (p, k))
            })
            .collect();
        (on.len() >= pins).then_some((n, on))
    })
}

fn every_edit(seed: u64, ics: usize) {
    let b = seeded(seed, ics);
    let what = |e: &str| format!("seed {seed}, {e}");
    let gnd = b.net("rail:gnd").unwrap();
    // A part added: a resistor from a signal to ground.
    if let Some((n, _)) = signal(&b, 2) {
        let mut e = b.clone();
        e.parts
            .push(resistor("rx", "R999", "10k", [Some(n), Some(gnd)]));
        let a = Allowed {
            parts: ["rx".to_owned()].into(),
            nets: [b.nets[n].key.clone()].into(),
        };
        check(&b, &mut e, a, &what("a part added"));
    }
    // A part removed: the first pull-up.
    if let Some(p) = b.parts.iter().position(|p| p.key.starts_with("rp")) {
        let mut e = b.clone();
        let key = b.parts[p].key.clone();
        let nets = nets_of(&b, &key);
        e.parts.remove(p);
        check(
            &b,
            &mut e,
            Allowed {
                parts: [key].into(),
                nets,
            },
            &what("a part removed"),
        );
    }
    // A part revalued.
    {
        let mut e = b.clone();
        let p = e.part("u0").unwrap();
        e.parts[p].value = "LOGIC-HC".into();
        check(
            &b,
            &mut e,
            Allowed {
                parts: ["u0".to_owned()].into(),
                nets: BTreeSet::new(),
            },
            &what("a value"),
        );
    }
    // A net renamed.
    if let Some((n, _)) = signal(&b, 2) {
        let mut e = b.clone();
        e.nets[n].name = format!("{}_RENAMED", b.nets[n].name);
        let a = Allowed {
            parts: BTreeSet::new(),
            nets: [b.nets[n].key.clone()].into(),
        };
        check(&b, &mut e, a, &what("a net renamed"));
    }
    // Two nets joined: every pin of one onto the other.
    let signals: Vec<usize> = (0..b.nets.len())
        .filter(|n| b.nets[*n].kind == NetKind::Signal)
        .collect();
    if signals.len() >= 2 {
        let (from, into) = (signals[0], signals[1]);
        let mut e = b.clone();
        for part in &mut e.parts {
            for pin in &mut part.pins {
                if pin.net == Some(from) {
                    pin.net = Some(into);
                }
            }
        }
        let a = Allowed {
            parts: BTreeSet::new(),
            nets: [b.nets[from].key.clone(), b.nets[into].key.clone()].into(),
        };
        check(&b, &mut e, a, &what("two nets joined"));
    }
    // A net split: its last pin onto a net of its own.
    if let Some((n, on)) = signal(&b, 3) {
        let mut e = b.clone();
        e.nets
            .push(ryusch::testing::net("split", "SPLIT", NetKind::Signal));
        let (p, k) = *on.last().unwrap();
        e.parts[p].pins[k].net = Some(e.nets.len() - 1);
        let a = Allowed {
            parts: BTreeSet::new(),
            nets: [b.nets[n].key.clone(), "split".to_owned()].into(),
        };
        check(&b, &mut e, a, &what("a net split"));
    }
    // A pin moved to another net.
    if signals.len() >= 2
        && let Some((n, on)) = signal(&b, 2)
    {
        let to = *signals.iter().find(|m| **m != n).unwrap();
        let mut e = b.clone();
        let (p, k) = on[0];
        e.parts[p].pins[k].net = Some(to);
        let a = Allowed {
            parts: BTreeSet::new(),
            nets: [b.nets[n].key.clone(), b.nets[to].key.clone()].into(),
        };
        check(&b, &mut e, a, &what("a pin moved"));
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn edits_change_only_what_they_name() {
    for seed in 1..=64 {
        every_edit(seed, 2 + (seed as usize % 9));
    }
}

#[test]
fn one_seed_quickly() {
    every_edit(7, 3);
}
