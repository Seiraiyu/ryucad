//! Stability, the spike's hard bar: on seeded blocks, a scripted edit (a
//! part added, removed or revalued; a net renamed, joined or split; a pin
//! moved to another net; a part moved to another sheet; a layer added to
//! a stack) laid out over the previous layout changes no
//! placement, label, text or route but those of the parts it names and the
//! nets their pins are on, and every hard rule holds after it. And the
//! layout merge: two sides' edits merged keep every key neither side
//! touched, find every area both changed as a conflict, and laid out again
//! there keep every rule and every untouched part's place. Release only:
//! `cargo test -p ryusch --release --test layout_props`.

use ryusch::engine::lay_out;
use ryusch::input::{Block, Flow, NetKind};
use ryusch::layout::{Instruction, Layout};
use ryusch::testing::{dense_block, resistor, seeded, with_stage};
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
        .flat_map(|x| x.nets())
        .map(|n| b.nets[n].key.clone())
        .collect()
}

/// A stack of `layers` channels, each layer's input on its own signal (the
/// first `layers` signals of `b`, each a new one past them), its output on
/// a net of its own, its supply the block's rail.
fn stack(b: &mut Block, layers: usize) -> ryusch::input::Part {
    let signals: Vec<usize> = (0..b.nets.len())
        .filter(|n| b.nets[*n].kind == NetKind::Signal)
        .collect();
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    for l in 0..layers {
        ins.push(Some(signals[l % signals.len()]));
        let key = format!("ch_out{l}");
        let n = b.net(&key).unwrap_or_else(|| {
            b.nets.push(ryusch::testing::net(
                &key,
                &format!("CH_OUT{l}"),
                NetKind::Signal,
            ));
            b.nets.len() - 1
        });
        outs.push(Some(n));
    }
    let vcc = b.net("rail:vcc");
    ryusch::shape::stack_symbol(
        "instance:ch_a",
        "Channel",
        &[
            ("in".into(), "IN".into(), Flow::In, ins),
            ("out".into(), "OUT".into(), Flow::Out, outs),
            ("vdd".into(), "VDD".into(), Flow::Power, vec![vcc; layers]),
        ],
    )
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

/// Lays `edited` out over `base`'s layout and holds it to the bar. A
/// region an edited part is in may move whole (plan 2.A): its parts and
/// their nets may then move, but each keeps its place in the region, and
/// every other region its origin.
fn check(base: &Block, edited: &mut Block, mut a: Allowed, what: &str) {
    let before = lay_out(base).unwrap().layout;
    edited.previous = Some(before.clone());
    let after = lay_out(edited).unwrap().layout;
    let (rb, ra) = (before.relative(), after.relative());
    for (id, r) in &after.regions {
        let Some(old) = before.regions.get(id) else {
            continue;
        };
        let touched = r.parts.iter().any(|k| a.parts.contains(k));
        if old.at != r.at && old.sheet == r.sheet {
            assert!(
                touched,
                "{what}: region {id} moved, though nothing edited is in it"
            );
            for k in &old.parts {
                if r.parts.contains(k)
                    && !a.parts.contains(k)
                    && let (Some(x), Some(y)) = (rb.placed.get(k), ra.placed.get(k))
                {
                    assert_eq!(x, y, "{what}: {k} moved inside region {id}");
                }
            }
            for k in r.parts.iter().chain(&old.parts) {
                if edited.part(k).is_some() {
                    a.nets.extend(nets_of(edited, k));
                }
                a.parts.insert(k.clone());
            }
        }
    }
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
    let mut b = seeded(seed, ics);
    // Half the seeds on the A4/A3 series, so paper fit and growth are
    // reached (plan 2.A).
    if seed.is_multiple_of(2) {
        papers(&mut b);
    }
    every_edit_on(b, &format!("seed {seed}"));
}

/// The A4/A3 series, A3 the project's paper.
fn papers(b: &mut Block) {
    use ryusch::input::Paper;
    use ryusch::layout::Orientation;
    let a4 = Paper {
        size: "a4".into(),
        orientation: Orientation::Landscape,
        area: [277_000_000, 190_000_000],
    };
    let a3 = Paper {
        size: "a3".into(),
        orientation: Orientation::Landscape,
        area: [400_000_000, 277_000_000],
    };
    b.sheet = a3.area;
    b.papers = vec![a4, a3];
}

fn every_edit_on(b: Block, name: &str) {
    let what = |e: &str| format!("{name}, {e}");
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
    // A part moved to another sheet, by instruction: it, the idioms
    // serving it and their nets may move.
    {
        let mut e = b.clone();
        e.instructions.insert(
            "i1".into(),
            Instruction::Sheet {
                parts: vec!["u0".into()],
                sheet: "Elsewhere".into(),
            },
        );
        let mut parts: BTreeSet<String> = ["u0", "c0", "rp0"].map(str::to_owned).into();
        parts.retain(|k| b.part(k).is_some());
        let nets = parts.iter().flat_map(|k| nets_of(&b, k)).collect();
        check(
            &b,
            &mut e,
            Allowed { parts, nets },
            &what("a part to another sheet"),
        );
        // And it went there.
        let out = lay_out(&e).unwrap().layout;
        assert!(
            ryusch::instr::achieved(&e, &out, &e.instructions["i1"]),
            "{}: the instruction doesn't hold",
            what("a part to another sheet")
        );
    }
    // A layer added to a stack: only the stack and its nets may move.
    {
        let mut base = b.clone();
        let two = stack(&mut base, 2);
        base.parts.push(two);
        let mut e = base.clone();
        let three = stack(&mut e, 3);
        let at = e.part("instance:ch_a").unwrap();
        e.parts[at] = three;
        let mut nets = nets_of(&e, "instance:ch_a");
        nets.extend(nets_of(&base, "instance:ch_a"));
        let a = Allowed {
            parts: ["instance:ch_a".to_owned()].into(),
            nets,
        };
        check(&base, &mut e, a, &what("a layer added to a stack"));
    }
    // A region moved whole by a move: only it and the nets entering it may
    // change, and every hard rule holds.
    {
        let first = lay_out(&b).unwrap().layout;
        let pick = first.regions.iter().find_map(|(id, r)| {
            let other = first.placed.iter().find(|(k, x)| {
                x.sheet == r.sheet && !r.parts.contains(k) && !k.starts_with("port:")
            })?;
            Some((id.clone(), r.clone(), other.0.clone()))
        });
        if let Some((id, r, other)) = pick {
            let mut e = b.clone();
            e.moves = vec![Instruction::Beside {
                parts: vec![format!("region:{id}")],
                of: other,
                side: ryusch::geom::Side::Right,
            }];
            let parts: BTreeSet<String> = r.parts.iter().cloned().collect();
            let nets = parts.iter().flat_map(|k| nets_of(&b, k)).collect();
            e.previous = Some(first.clone());
            if let Ok(out) = lay_out(&e) {
                let after = out.layout;
                // Its parts may be placed again inside it where a label
                // finds no room (the crowded pass): only what lies outside
                // it is held still.
                unchanged(
                    &e,
                    &first,
                    &after,
                    &Allowed { parts, nets },
                    &what("a region moved"),
                );
                let broken = ryusch::rules::check(&e, &after);
                assert!(broken.is_empty(), "{}: {broken:#?}", what("a region moved"));
            }
        }
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

/// A dense part's bank (#429, plan 2.B) laid out over its own layout:
/// each pull revalued, a pull added on an unpulled row and one deleted
/// change only that part, its row's net, and the rails (whose bars and
/// ports the comb draws), every hard rule holding.
fn dense_edits(ports: bool) {
    let b = dense_block(ports);
    let name = if ports { "dense, ports" } else { "dense" };
    for i in 1..=10 {
        let key = format!("r{i}");
        let mut e = b.clone();
        let p = e.part(&key).unwrap();
        e.parts[p].value = "4.7k".into();
        let a = Allowed {
            parts: [key.clone()].into(),
            nets: nets_of(&b, &key),
        };
        check(&b, &mut e, a, &format!("{name}, {key} revalued"));
    }
    // A pull added on S10, which only the connector reaches.
    {
        let mut e = b.clone();
        let s10 = e.net("s10").unwrap();
        let v = e.net("rail:v").unwrap();
        e.parts
            .push(resistor("r11", "R11", "10k", [Some(s10), Some(v)]));
        let a = Allowed {
            parts: ["r11".to_owned()].into(),
            nets: nets_of(&e, "r11"),
        };
        check(&b, &mut e, a, &format!("{name}, a pull added"));
    }
    // A pull deleted: its rail's bar may part, so the members beside it
    // on that rail may take ports of their own, but stay where they are.
    {
        let mut e = b.clone();
        let nets = nets_of(&b, "r5");
        e.parts.retain(|p| p.key != "r5");
        let beside = ["r4", "r6"];
        let a = Allowed {
            parts: ["r5"]
                .into_iter()
                .chain(beside)
                .map(str::to_owned)
                .collect(),
            nets,
        };
        check(&b, &mut e, a, &format!("{name}, a pull deleted"));
        let before = lay_out(&b).unwrap().layout;
        e.previous = Some(before.clone());
        let after = lay_out(&e).unwrap().layout;
        for k in beside {
            assert_eq!(before.placed[k], after.placed[k], "{name}: {k} moved");
        }
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_banks_edits_change_only_what_they_name() {
    dense_edits(false);
    dense_edits(true);
}

/// A seeded block with an op-amp stage whose output is an out port: laid
/// out over its own layout it doesn't move, and every edit changes only
/// what it names.
fn stage(seed: u64, ics: usize) {
    let b = with_stage(seeded(seed, ics));
    let out = lay_out(&b).unwrap();
    assert!(
        out.idioms
            .iter()
            .any(|i| i.kind == ryusch::layout::IdiomKind::OpampStage),
        "seed {seed}: no stage matched"
    );
    let first = out.layout;
    let mut again = b.clone();
    again.previous = Some(first.clone());
    let second = lay_out(&again).unwrap().layout;
    assert_eq!(
        first, second,
        "seed {seed} with a stage: laid out over itself, it moves"
    );
    every_edit_on(b, &format!("seed {seed} with a stage"));
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn stages_hold_over_their_own_layout_and_edits() {
    for seed in 1..=16 {
        stage(seed, 2 + (seed as usize % 5));
    }
}

#[test]
fn one_stage_quickly() {
    stage(7, 3);
}

/// Two sides each edit a seeded block (ours adds a part on a signal and
/// revalues U0, theirs adds one on the same signal and renames another
/// net), each
/// laid out over the base; their layouts merged over the merged block.
fn merges(seed: u64, ics: usize) {
    use ryusch::merge::{Repair, merge, resolve};
    let b = seeded(seed, ics);
    let base = lay_out(&b).unwrap().layout;
    let gnd = b.net("rail:gnd").unwrap();
    let signals: Vec<usize> = (0..b.nets.len())
        .filter(|n| b.nets[*n].kind == NetKind::Signal)
        .collect();
    let (s1, s2) = (signals[0], signals[signals.len() / 2]);
    let add_ours = |e: &mut Block| {
        e.parts
            .push(resistor("ro", "R901", "1k", [Some(s1), Some(gnd)]));
        let p = e.part("u0").unwrap();
        e.parts[p].value = "OURS".into();
    };
    let add_theirs = |e: &mut Block| {
        // Beside ours' part: both land in the same free space.
        e.parts
            .push(resistor("rt", "R902", "2k2", [Some(s1), Some(gnd)]));
        e.nets[s2].name = format!("{}_T", e.nets[s2].name);
    };
    let side = |f: &dyn Fn(&mut Block)| {
        let mut e = b.clone();
        f(&mut e);
        e.previous = Some(base.clone());
        lay_out(&e).unwrap().layout
    };
    let ours = side(&add_ours);
    let theirs = side(&add_theirs);
    let mut both = b.clone();
    add_ours(&mut both);
    add_theirs(&mut both);
    let m = merge(&both, &base, &ours, &theirs);
    let what = format!("seed {seed}");
    // Every key neither side touched keeps its bits.
    for (k, x) in &base.placed {
        if ours.placed.get(k) == Some(x) && theirs.placed.get(k) == Some(x) {
            assert_eq!(m.layout.placed.get(k), Some(x), "{what}: {k}");
        }
    }
    // Every part a side changed that overlaps one the other changed is in
    // one conflict.
    let changed = |l: &Layout| -> Vec<String> {
        l.placed
            .iter()
            .filter(|(k, x)| base.placed.get(*k) != Some(*x))
            .map(|(k, _)| format!("placed/{k}"))
            .collect()
    };
    let box_of = |l: &Layout, k: &str| {
        let key = k.trim_start_matches("placed/");
        let x = &l.placed[key];
        let p = both.part(key).unwrap();
        (
            x.sheet.clone(),
            ryusch::shape::Shape::of(
                &both.parts[p],
                ryusch::geom::Place::new(x.at, x.rotation, x.mirror),
            )
            .extent,
        )
    };
    for a in changed(&ours) {
        for t in changed(&theirs) {
            let ((sa, ra), (st, rt)) = (box_of(&ours, &a), box_of(&theirs, &t));
            if a != t && sa == st && ra.overlaps(&rt) {
                assert!(
                    m.conflicts
                        .iter()
                        .any(|c| c.ours.contains(&a) && c.theirs.contains(&t)),
                    "{what}: {a} and {t} overlap outside a conflict"
                );
            }
        }
    }
    // Laid out again where they conflict: every rule holds, and every
    // part no side moved and no conflict holds stays where it was.
    let repairs = vec![Repair::Relayout; m.conflicts.len()];
    let out = resolve(&both, &base, &ours, &theirs, &m, &repairs).unwrap();
    let broken = ryusch::rules::check(&both, &out.layout);
    assert!(broken.is_empty(), "{what}: {broken:#?}");
    let in_conflict: BTreeSet<String> = m
        .conflicts
        .iter()
        .flat_map(|c| c.ours.iter().chain(&c.theirs))
        .filter_map(|k| k.strip_prefix("placed/").map(str::to_owned))
        .collect();
    for (k, x) in &base.placed {
        if ours.placed.get(k) == Some(x)
            && theirs.placed.get(k) == Some(x)
            && !in_conflict.contains(k)
        {
            assert_eq!(out.layout.placed.get(k), Some(x), "{what}: {k} moved");
        }
    }
    for k in ["ro", "rt"] {
        assert!(out.layout.placed.contains_key(k), "{what}: {k} unplaced");
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn merges_keep_what_neither_side_touched() {
    for seed in 1..=64 {
        merges(seed, 2 + (seed as usize % 9));
    }
}

#[test]
fn one_merge_quickly() {
    merges(7, 3);
}
