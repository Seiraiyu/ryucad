use super::*;
use crate::geom::{Place, Side};
use crate::input::{Net, Part};
use crate::layout::{Placed, Sheet as LayoutSheet};
use crate::testing::{capacitor, ic, net, resistor};

const G: i64 = GRID;

/// A two-pin part drawn as a resistor, called by its prefix.
fn two(prefix: &str, key: &str, refdes: &str, nets: [Option<usize>; 2]) -> Part {
    let mut p = resistor(key, refdes, "x", nets);
    p.kind = PartKind::Symbol {
        prefix: prefix.into(),
    };
    p
}

/// A hand-drawn sheet: places, and wires by net.
struct Drawing {
    block: Block,
    layout: Layout,
}

impl Drawing {
    fn new(parts: Vec<Part>, nets: Vec<Net>) -> Drawing {
        let block = Block {
            parts,
            nets,
            ..Block::default()
        };
        let mut layout = Layout::default();
        layout.sheets.insert(
            "s1".into(),
            LayoutSheet {
                name: "Sheet 1".into(),
                at: [0, 0],
            },
        );
        Drawing { block, layout }
    }

    fn place(&mut self, key: &str, at: Point, rotation: u16) {
        self.layout.placed.insert(
            key.into(),
            Placed {
                sheet: "s1".into(),
                at,
                rotation,
                mirror: false,
                held: false,
            },
        );
    }

    /// Pin `name` (or number) of part `key`: where it ends and which way.
    fn end(&self, key: &str, pin: &str) -> (Point, Side) {
        let p = self.block.part(key).unwrap();
        let x = &self.layout.placed[key];
        let sh = Shape::of(&self.block.parts[p], Place::new(x.at, x.rotation, x.mirror));
        let k = self.block.parts[p]
            .pins
            .iter()
            .position(|q| q.name == pin || q.number == pin)
            .unwrap();
        (sh.pins[k].end, sh.pins[k].side)
    }

    fn wire(&mut self, net: &str, run: Vec<Point>) {
        self.layout
            .routes
            .entry(net.into())
            .or_default()
            .entry("s1".into())
            .or_default()
            .push(run);
    }

    fn judged(&self) -> Vec<Judged> {
        let shapes: BTreeMap<usize, Shape> = self
            .block
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let x = self.layout.placed.get(&p.key)?;
                Some((i, Shape::of(p, Place::new(x.at, x.rotation, x.mirror))))
            })
            .collect();
        judge(&self.block, &self.layout, "s1", &shapes)
    }
}

/// A buck converter: U1's PH pin, L1 to +3V3, D1 to ground, C1 on the
/// output, drawn in the standard shape.
fn buck() -> Drawing {
    let nets = vec![
        net("ph", "PH", NetKind::Signal),
        net("rail:v", "+3V3", NetKind::Supply),
        net("rail:gnd", "GND", NetKind::Ground),
        net("vin", "VIN", NetKind::Signal),
    ];
    let parts = vec![
        ic(
            "u1",
            "U1",
            "TPS54331",
            &[
                ("VIN", PinKind::PowerIn, Some(3)),
                ("GND", PinKind::PowerIn, Some(2)),
            ],
            &[("PH", PinKind::Output, Some(0))],
        ),
        two("L", "l1", "L1", [Some(0), Some(1)]),
        two("D", "d1", "D1", [Some(0), Some(2)]),
        capacitor("c1", "C1", "47uF", [Some(1), Some(2)]),
    ];
    let mut d = Drawing::new(parts, nets);
    d.place("u1", [0, 0], 0);
    let (e, _) = d.end("u1", "PH");
    // L1 across, its PH end four steps out.
    d.place("l1", [0, 0], 90);
    let (l0, _) = d.end("l1", "1");
    d.place("l1", [e[0] + 4 * G - l0[0], e[1] - l0[1]], 90);
    let (l1, _) = d.end("l1", "1");
    let (l2, _) = d.end("l1", "2");
    d.wire("ph", vec![e, l1]);
    // D1 standing under the switch node, cathode (pin 1) up.
    d.place("d1", [0, 0], 0);
    let (d0, _) = d.end("d1", "1");
    d.place("d1", [e[0] + 2 * G - d0[0], e[1] - 2 * G - d0[1]], 0);
    let (dt, _) = d.end("d1", "1");
    d.wire("ph", vec![dt, [dt[0], e[1]]]);
    // C1 standing on L1's right end.
    d.place("c1", [0, 0], 0);
    let (c0, _) = d.end("c1", "1");
    d.place("c1", [l2[0] + 3 * G - c0[0], l2[1] - 2 * G - c0[1]], 0);
    let (ct, _) = d.end("c1", "1");
    d.wire("rail:v", vec![l2, [ct[0], l2[1]], ct]);
    d
}

#[test]
fn a_buck_in_its_standard_shape_has_no_problem() {
    let d = buck();
    let j = d.judged();
    let b = j
        .iter()
        .find(|x| x.kind == "buck_converter")
        .expect("recognised");
    assert_eq!(b.parts, ["u1", "l1", "d1", "c1"]);
    assert!(b.problems.is_empty(), "{:?}", b.problems);
    assert!(
        b.what
            .contains("switch node PH: U1.PH, L1, D1; output +3V3 through C1"),
        "{}",
        b.what
    );
    assert!(
        b.standard.contains("L1 lies across the page"),
        "{}",
        b.standard
    );
}

#[test]
fn a_buck_out_of_shape_says_what_breaks_it() {
    let mut d = buck();
    // L1 stood upright, and D1 joined by a label.
    d.layout.placed.get_mut("l1").unwrap().rotation = 0;
    d.layout
        .routes
        .get_mut("ph")
        .unwrap()
        .get_mut("s1")
        .unwrap()
        .pop();
    let j = d.judged();
    let b = j.iter().find(|x| x.kind == "buck_converter").unwrap();
    assert!(
        b.problems.contains(&"L1 stands upright".to_owned()),
        "{:?}",
        b.problems
    );
    assert!(
        b.problems
            .contains(&"D1 is joined to the switch node by a label".to_owned()),
        "{:?}",
        b.problems
    );
}

/// An inverting stage: U2's OUT to its `-` through R1, R2 into `-`, drawn
/// in the standard shape.
fn stage() -> Drawing {
    let nets = vec![
        net("out", "OUT", NetKind::Signal),
        net("inn", "INN", NetKind::Signal),
        net("in", "IN", NetKind::Signal),
        net("rail:gnd", "GND", NetKind::Ground),
    ];
    let parts = vec![
        ic(
            "u2",
            "U2",
            "OPA1612",
            &[
                ("+", PinKind::Input, Some(3)),
                ("-", PinKind::Input, Some(1)),
            ],
            &[("OUT", PinKind::Output, Some(0))],
        ),
        resistor("r1", "R1", "10k", [Some(1), Some(0)]),
        resistor("r2", "R2", "1k", [Some(2), Some(1)]),
    ];
    let mut d = Drawing::new(parts, nets);
    d.place("u2", [0, 0], 0);
    let (m, _) = d.end("u2", "-");
    let (o, _) = d.end("u2", "OUT");
    // R2 across, its right end two steps left of `-`.
    d.place("r2", [0, 0], 90);
    let (r2b, _) = d.end("r2", "2");
    d.place("r2", [m[0] - 2 * G - r2b[0], m[1] - r2b[1]], 90);
    let (r2r, _) = d.end("r2", "2");
    d.wire("inn", vec![r2r, m]);
    // R1 across, above U2, from a step left of `-` to over OUT.
    d.place("r1", [0, 0], 90);
    let (r1a, _) = d.end("r1", "1");
    let top = 8 * G;
    d.place("r1", [m[0] - G - r1a[0], top - r1a[1]], 90);
    let (r1l, _) = d.end("r1", "1");
    let (r1r, _) = d.end("r1", "2");
    d.wire("inn", vec![r1l, [r1l[0], m[1]]]);
    d.wire("out", vec![r1r, [o[0], r1r[1]], o]);
    d
}

#[test]
fn an_opamp_stage_in_its_standard_shape_has_no_problem() {
    let d = stage();
    let j = d.judged();
    let s = j
        .iter()
        .find(|x| x.kind == "opamp_stage")
        .expect("recognised");
    assert_eq!(s.parts, ["u2", "r1", "r2"]);
    assert!(s.problems.is_empty(), "{:?}", s.problems);
    assert!(
        s.standard.contains("lies across the page above U2"),
        "{}",
        s.standard
    );
}

#[test]
fn an_opamp_stage_out_of_shape_says_what_breaks_it() {
    let mut d = stage();
    // R1 moved under U2 and its output end left to a label.
    let at = d.layout.placed["r1"].at;
    d.layout.placed.get_mut("r1").unwrap().at = [at[0], at[1] - 20 * G];
    d.layout.routes.remove("out");
    let j = d.judged();
    let s = j.iter().find(|x| x.kind == "opamp_stage").unwrap();
    assert!(
        s.problems.contains(&"R1 isn't above U2".to_owned()),
        "{:?}",
        s.problems
    );
    assert!(
        s.problems
            .contains(&"R1 is joined to U2 by a label".to_owned()),
        "{:?}",
        s.problems
    );
}

#[test]
fn switch_pins_go_by_their_names() {
    for n in [
        "SW", "sw", "SW1", "SW_A", "SWITCH", "LX", "LX2", "LX_B", "PH",
    ] {
        assert!(switch_pin(n), "{n}");
    }
    // A debug port's pins aren't a switch node.
    for n in [
        "VIN", "EN", "PHASE_X", "S", "SWCLK", "SWDIO", "SWO", "SWD", "LXOUT",
    ] {
        assert!(!switch_pin(n), "{n}");
    }
}

#[test]
fn a_jog_names_what_it_steps_round() {
    let d = buck();
    let shapes: BTreeMap<usize, Shape> = d
        .block
        .parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let x = &d.layout.placed[&p.key];
            (i, Shape::of(p, Place::new(x.at, x.rotation, x.mirror)))
        })
        .collect();
    let c1 = d.block.part("c1").unwrap();
    let e = shapes[&c1].extent;
    let y = e.center()[1];
    // A wire along C1's middle that steps under it and back up.
    let run = vec![
        [e.min[0] - 4 * G, y],
        [e.min[0] - 2 * G, y],
        [e.min[0] - 2 * G, e.min[1] - 2 * G],
        [e.max[0] + 2 * G, e.min[1] - 2 * G],
        [e.max[0] + 2 * G, y],
        [e.max[0] + 4 * G, y],
    ];
    // Its first Z steps down and on under C1: the straight way on crosses it.
    assert!(jog_obstacles(&run, &shapes).contains(&c1));
    // A straight wire steps round nothing.
    assert!(jog_obstacles(&[[0, -40 * G], [9 * G, -40 * G]], &shapes).is_empty());
}

#[test]
fn the_input_wire_stepping_round_its_own_capacitor_is_no_jog_under_it() {
    // U1's VIN pin, C2 from VIN to ground, and VIN's own wire stepping
    // round C2 on its way to it: VIN reaches C2, so it doesn't jog under it.
    let mut d = buck();
    d.block
        .parts
        .push(capacitor("c2", "C2", "10uF", [Some(3), Some(2)]));
    let (v, _) = d.end("u1", "VIN");
    d.place("c2", [0, 0], 0);
    let (c0, _) = d.end("c2", "1");
    d.place("c2", [v[0] - 6 * G - c0[0], v[1] + 2 * G - c0[1]], 0);
    let (ct, _) = d.end("c2", "1");
    let x = ct[0];
    // Out of VIN, down past C2's top, on beyond it and back up: a Z round C2.
    d.wire(
        "vin",
        vec![
            v,
            [x + 2 * G, v[1]],
            [x + 2 * G, v[1] - 12 * G],
            [x - 4 * G, v[1] - 12 * G],
            [x - 4 * G, v[1]],
            [x - 6 * G, v[1]],
        ],
    );
    d.wire("vin", vec![[x, v[1]], ct]);
    let b = d
        .judged()
        .into_iter()
        .find(|j| j.kind == "buck_converter")
        .unwrap();
    assert!(
        !b.problems
            .iter()
            .any(|p| p.contains("jogs under the input capacitors")),
        "{:?}",
        b.problems
    );
}

/// A circuit out of shape names the instruction that draws it in shape
/// (round 1, F4: an agent couldn't say "in parallel" for a stage's
/// feedback), and the lint's message ends with it.
#[test]
fn a_circuit_out_of_shape_names_the_instruction_that_reforms_it() {
    let mut d = stage();
    let at = d.layout.placed["r1"].at;
    d.layout.placed.get_mut("r1").unwrap().at = [at[0], at[1] - 20 * G];
    let j = d.judged();
    let s = j.iter().find(|x| x.kind == "opamp_stage").unwrap();
    assert_eq!(s.fix, "idiom U2 opamp_stage");
    let b = buck();
    let j = b.judged();
    let k = j.iter().find(|x| x.kind == "buck_converter").unwrap();
    assert!(k.fix.starts_with("idiom U1 buck"), "{}", k.fix);
}
