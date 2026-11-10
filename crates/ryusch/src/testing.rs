//! Test helpers: symbols drawn as KiCad's library draws them (our own
//! numbers), and seeded blocks for the property tests, the fuzz target
//! and the benchmarks.

use crate::geom::{GRID, Nm, Point, Side};
use crate::input::{Block, Fill, Graphic, Net, NetKind, Part, PartKind, Pin, PinKind};

const MIL: Nm = 25_400;

fn pin(
    number: &str,
    name: &str,
    at: Point,
    side: Side,
    length: Nm,
    kind: PinKind,
    net: Option<usize>,
) -> Pin {
    Pin {
        number: number.into(),
        name: name.into(),
        at,
        side,
        length,
        kind,
        net,
        nc: false,
        show_number: false,
        layers: Vec::new(),
    }
}

fn symbol(
    key: &str,
    prefix: &str,
    refdes: &str,
    value: &str,
    graphics: Vec<Graphic>,
    pins: Vec<Pin>,
) -> Part {
    Part {
        key: key.into(),
        kind: PartKind::Symbol {
            prefix: prefix.into(),
        },
        refdes: refdes.into(),
        value: value.into(),
        function: String::new(),
        graphics,
        pins,
    }
}

/// A resistor standing up, drawn as the US zigzag: pin 1 at the top.
pub fn resistor(key: &str, refdes: &str, value: &str, nets: [Option<usize>; 2]) -> Part {
    let zig: Vec<Point> = [
        (0, 100),
        (40, 83),
        (-40, 50),
        (40, 17),
        (-40, -17),
        (40, -50),
        (-40, -83),
        (0, -100),
    ]
    .iter()
    .map(|(x, y)| [x * MIL, y * MIL])
    .collect();
    symbol(
        key,
        "R",
        refdes,
        value,
        vec![Graphic::Polyline {
            points: zig,
            fill: Fill::None,
        }],
        vec![
            pin(
                "1",
                "",
                [0, 3 * GRID],
                Side::Up,
                GRID,
                PinKind::Passive,
                nets[0],
            ),
            pin(
                "2",
                "",
                [0, -3 * GRID],
                Side::Down,
                GRID,
                PinKind::Passive,
                nets[1],
            ),
        ],
    )
}

/// A capacitor standing up: pin 1 at the top.
pub fn capacitor(key: &str, refdes: &str, value: &str, nets: [Option<usize>; 2]) -> Part {
    symbol(
        key,
        "C",
        refdes,
        value,
        vec![
            Graphic::Polyline {
                points: vec![[-80 * MIL, 20 * MIL], [80 * MIL, 20 * MIL]],
                fill: Fill::None,
            },
            Graphic::Polyline {
                points: vec![[-80 * MIL, -20 * MIL], [80 * MIL, -20 * MIL]],
                fill: Fill::None,
            },
        ],
        vec![
            pin(
                "1",
                "",
                [0, 2 * GRID],
                Side::Up,
                2 * GRID - 20 * MIL,
                PinKind::Passive,
                nets[0],
            ),
            pin(
                "2",
                "",
                [0, -2 * GRID],
                Side::Down,
                2 * GRID - 20 * MIL,
                PinKind::Passive,
                nets[1],
            ),
        ],
    )
}

/// An LED pointing right: anode (1) left, cathode (2) right.
pub fn led(key: &str, refdes: &str, value: &str, nets: [Option<usize>; 2]) -> Part {
    symbol(
        key,
        "D",
        refdes,
        value,
        vec![
            Graphic::Polyline {
                points: vec![
                    [-50 * MIL, 50 * MIL],
                    [50 * MIL, 0],
                    [-50 * MIL, -50 * MIL],
                    [-50 * MIL, 50 * MIL],
                ],
                fill: Fill::None,
            },
            Graphic::Polyline {
                points: vec![[50 * MIL, 50 * MIL], [50 * MIL, -50 * MIL]],
                fill: Fill::None,
            },
        ],
        vec![
            pin(
                "1",
                "A",
                [-3 * GRID, 0],
                Side::Left,
                2 * GRID - 50 * MIL,
                PinKind::Passive,
                nets[0],
            ),
            pin(
                "2",
                "K",
                [3 * GRID, 0],
                Side::Right,
                2 * GRID - 50 * MIL,
                PinKind::Passive,
                nets[1],
            ),
        ],
    )
}

/// An IC: a box with `left` and `right` pins, `(name, kind, net)`, two
/// steps apart, numbered down the left and up the right.
pub fn ic(
    key: &str,
    refdes: &str,
    value: &str,
    left: &[(&str, PinKind, Option<usize>)],
    right: &[(&str, PinKind, Option<usize>)],
) -> Part {
    let rows = left.len().max(right.len()).max(1) as Nm;
    let widest = |list: &[(&str, PinKind, Option<usize>)]| {
        list.iter()
            .map(|(n, _, _)| crate::font::width(n, crate::font::SIZE))
            .max()
            .unwrap_or(0)
    };
    let w = ((widest(left) + widest(right) + 3 * GRID) / (2 * GRID) + 1) * 2 * GRID;
    let h = 2 * GRID * rows;
    let mut pins = Vec::new();
    let mut n = 1;
    for (i, (name, kind, net)) in left.iter().enumerate() {
        let mut p = pin(
            &n.to_string(),
            name,
            [-w / 2 - 3 * GRID, h / 2 - GRID - 2 * GRID * i as Nm],
            Side::Left,
            3 * GRID,
            *kind,
            *net,
        );
        p.show_number = true;
        pins.push(p);
        n += 1;
    }
    for (i, (name, kind, net)) in right.iter().enumerate().rev() {
        let mut p = pin(
            &n.to_string(),
            name,
            [w / 2 + 3 * GRID, h / 2 - GRID - 2 * GRID * i as Nm],
            Side::Right,
            3 * GRID,
            *kind,
            *net,
        );
        p.show_number = true;
        pins.push(p);
        n += 1;
    }
    symbol(
        key,
        "U",
        refdes,
        value,
        vec![Graphic::Rect {
            start: [-w / 2, h / 2],
            end: [w / 2, -h / 2],
            fill: Fill::Body,
        }],
        pins,
    )
}

/// A connector: one column of pins pointing left.
pub fn connector(key: &str, refdes: &str, nets: &[Option<usize>]) -> Part {
    let n = nets.len() as Nm;
    let mut p = symbol(
        key,
        "J",
        refdes,
        &format!("Conn_01x{:02}", nets.len()),
        vec![Graphic::Rect {
            start: [-GRID, GRID],
            end: [GRID, -2 * GRID * n + GRID],
            fill: Fill::Body,
        }],
        nets.iter()
            .enumerate()
            .map(|(i, net)| {
                pin(
                    &(i + 1).to_string(),
                    "",
                    [-3 * GRID, -2 * GRID * i as Nm],
                    Side::Left,
                    2 * GRID,
                    PinKind::Passive,
                    *net,
                )
            })
            .collect(),
    );
    for pin in &mut p.pins {
        pin.show_number = true;
    }
    p
}

/// A port of the block: its flag, one pin on `net`, as `schem::build`
/// makes ports (keyed `port:<id>`, named as written).
pub fn port(key: &str, name: &str, flow: crate::input::Flow, net: Option<usize>) -> Part {
    crate::shape::port_symbol(key, name, flow, net)
}

/// A net of the block.
pub fn net(key: &str, name: &str, kind: NetKind) -> Net {
    Net {
        key: key.into(),
        name: name.into(),
        kind,
    }
}

/// A small deterministic generator (SplitMix64).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn draw(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.draw() % n.max(1)
    }
}

/// A seeded block of `ics` ICs, each with decoupling and pull-ups, joined
/// by signals, fed from a connector, on an A3 sheet less its margin.
pub fn seeded(seed: u64, ics: usize) -> Block {
    let mut rng = Rng::new(seed);
    let mut nets = vec![
        net("rail:vcc", "3V3", NetKind::Supply),
        net("rail:gnd", "GND", NetKind::Ground),
    ];
    let (vcc, gnd) = (Some(0), Some(1));
    let mut parts = Vec::new();
    let mut signals: Vec<usize> = Vec::new();
    let mut conn_nets = vec![vcc, gnd];
    for i in 0..ics {
        let ins = 2 + rng.below(4) as usize;
        let outs = 1 + rng.below(4) as usize;
        let mut left = vec![("VDD", PinKind::PowerIn, vcc)];
        let mut right = vec![("GND", PinKind::PowerIn, gnd)];
        for k in 0..ins {
            // An input: an earlier signal, or a new one from the connector.
            let n = if !signals.is_empty() && rng.below(3) > 0 {
                signals[rng.below(signals.len() as u64) as usize]
            } else {
                nets.push(net(
                    &format!("in{i}_{k}"),
                    &format!("IN{i}_{k}"),
                    NetKind::Signal,
                ));
                conn_nets.push(Some(nets.len() - 1));
                nets.len() - 1
            };
            left.push((["A", "B", "C", "D", "E", "F"][k], PinKind::Input, Some(n)));
        }
        for k in 0..outs {
            nets.push(net(
                &format!("out{i}_{k}"),
                &format!("OUT{i}_{k}"),
                NetKind::Signal,
            ));
            let n = nets.len() - 1;
            signals.push(n);
            right.push((["Q", "R", "S", "T"][k], PinKind::Output, Some(n)));
        }
        if rng.below(2) == 0 {
            nets.push(net(&format!("sda{i}"), &format!("SDA{i}"), NetKind::Signal));
            let n = nets.len() - 1;
            right.push(("SDA", PinKind::Bidirectional, Some(n)));
            parts.push(resistor(
                &format!("rp{i}"),
                &format!("R{}", 100 + i),
                "4k7",
                [vcc, Some(n)],
            ));
            conn_nets.push(Some(n));
        }
        parts.push(ic(
            &format!("u{i}"),
            &format!("U{}", i + 1),
            "LOGIC",
            &left,
            &right,
        ));
        parts.push(capacitor(
            &format!("c{i}"),
            &format!("C{}", i + 1),
            "100n",
            [vcc, gnd],
        ));
    }
    conn_nets.truncate(12);
    parts.push(connector("j1", "J1", &conn_nets));
    Block {
        name: format!("Seeded {seed}"),
        sheet: [420_000_000 - 20_000_000, 297_000_000 - 20_000_000],
        parts,
        nets,
        ..Block::default()
    }
}

/// `b` with an op-amp stage added: an amplifier whose inverting input
/// takes a resistor from the block's first signal net and a resistor and a
/// capacitor from its output, its non-inverting input on ground, its
/// output the block's out port `AMP_OUT`.
pub fn with_stage(mut b: Block) -> Block {
    let gnd = b.net("rail:gnd");
    let from = (0..b.nets.len()).find(|n| b.nets[*n].kind == NetKind::Signal);
    b.nets.push(net("amp_inn", "AMP_INN", NetKind::Signal));
    let inn = Some(b.nets.len() - 1);
    b.nets.push(net("port:amp_out", "AMP_OUT", NetKind::Signal));
    let out = Some(b.nets.len() - 1);
    b.parts.push(ic(
        "ua",
        "U90",
        "OPA",
        &[("IN-", PinKind::Input, inn), ("IN+", PinKind::Input, gnd)],
        &[("OUT", PinKind::Output, out)],
    ));
    b.parts.push(resistor("rf", "R90", "10k", [inn, out]));
    b.parts.push(capacitor("cf", "C90", "100p", [out, inn]));
    b.parts.push(resistor("ri", "R91", "1k", [from, inn]));
    b.parts.push(crate::shape::port_symbol(
        "port:amp_out",
        "AMP_OUT",
        crate::input::Flow::Out,
        out,
    ));
    b
}

/// The bytes, read one at a time: zeros once they run out.
struct Bytes<'a>(&'a [u8], usize);

impl Bytes<'_> {
    fn next(&mut self) -> usize {
        let b = self.0.get(self.1).copied().unwrap_or(0);
        self.1 += 1;
        usize::from(b)
    }
}

/// The `schematic` fuzz target's case, from its bytes: a seeded block
/// (its size and seed from the bytes) and a batch of instructions and
/// moves over its parts and nets, laid out. Panics unless the result is
/// a typed refusal or a layout keeping every hard rule, the same bytes
/// lay out to the same bits twice, and laid out again over its own layout
/// after one part's value changes, every other part keeps its place and
/// text. `tests/fuzz_regressions.rs` replays what it found.
pub fn check_bytes(data: &[u8]) {
    use crate::layout::{Axis, IdiomChoice, IdiomKind, Instruction, NetStyle};
    const SIDES: [Side; 4] = [Side::Left, Side::Right, Side::Up, Side::Down];

    let mut b = Bytes(data, 0);
    let seed = (b.next() as u64) << 8 | b.next() as u64;
    let ics = 1 + b.next() % 6;
    let mut block = crate::testing::seeded(seed, ics);
    let keys: Vec<String> = block.parts.iter().map(|p| p.key.clone()).collect();
    let nets: Vec<String> = block.nets.iter().map(|n| n.key.clone()).collect();
    let pick = |b: &mut Bytes| keys[b.next() % keys.len()].clone();
    let count = b.next() % 5;
    for i in 0..count {
        let parts = vec![pick(&mut b)];
        let ins = match b.next() % 11 {
            0 => Instruction::Sheet {
                parts,
                sheet: format!("Sheet {}", 1 + b.next() % 3),
            },
            1 => Instruction::Beside {
                parts,
                of: pick(&mut b),
                side: SIDES[b.next() % 4],
            },
            2 => Instruction::Near {
                parts,
                to: pick(&mut b),
            },
            3 => Instruction::Align {
                parts: vec![parts[0].clone(), pick(&mut b)],
                axis: if b.next() % 2 == 0 {
                    Axis::Row
                } else {
                    Axis::Column
                },
            },
            4 => Instruction::Group {
                parts: vec![parts[0].clone(), pick(&mut b)],
            },
            5 => Instruction::Edge {
                parts,
                side: SIDES[b.next() % 4],
            },
            6 => Instruction::Orient {
                parts,
                rotation: [0, 90, 180, 270][b.next() % 4],
                mirror: b.next() % 2 == 1,
            },
            7 => Instruction::Net {
                net: nets[b.next() % nets.len()].clone(),
                style: [NetStyle::Wire, NetStyle::Label, NetStyle::Port][b.next() % 3],
            },
            8 => Instruction::Idiom {
                parts,
                kind: match b.next() % 3 {
                    0 => IdiomChoice::None,
                    k => IdiomChoice::Kind(IdiomKind::ALL[(k + b.next()) % IdiomKind::ALL.len()]),
                },
            },
            9 => Instruction::Flow { parts },
            // A shift is a move only.
            _ => Instruction::Shift {
                parts,
                side: SIDES[b.next() % 4],
                steps: 1 + (b.next() % 8) as u16,
            },
        };
        if matches!(ins, Instruction::Shift { .. }) || b.next() % 4 == 0 {
            block.moves.push(ins);
        } else {
            block.instructions.insert(format!("i{}", i + 1), ins);
        }
    }
    // Plan 2.A, from the bytes past the 48th (so the replays of earlier
    // finds, all shorter, keep their cases): the A4/A3 series, and region,
    // region none and paper instructions.
    let mut tail = Bytes(data.get(48..).unwrap_or(&[]), 0);
    if tail.next() & 1 == 1 {
        let a4 = crate::input::Paper {
            size: "a4".into(),
            orientation: crate::layout::Orientation::Landscape,
            area: [277_000_000, 190_000_000],
        };
        let a3 = crate::input::Paper {
            size: "a3".into(),
            orientation: crate::layout::Orientation::Landscape,
            area: [400_000_000, 277_000_000],
        };
        block.sheet = a3.area;
        block.papers = vec![a4, a3];
    }
    const TITLES: [&str; 4] = ["Power", "Sensor", "Filter", "Misc"];
    for i in 0..tail.next() % 3 {
        let mut parts = Vec::new();
        for _ in 0..1 + tail.next() % 3 {
            parts.push(keys[tail.next() % keys.len()].clone());
        }
        let ins = match tail.next() % 3 {
            0 => Instruction::Region {
                title: TITLES[tail.next() % TITLES.len()].into(),
                parts,
            },
            1 => Instruction::RegionNone { parts },
            _ => Instruction::Paper {
                sheet: "s1".into(),
                size: ["a4", "a3"][tail.next() % 2].into(),
                orientation: None,
            },
        };
        block.instructions.insert(format!("r{}", i + 1), ins);
    }
    let out = match crate::engine::lay_out(&block) {
        Ok(o) => o,
        Err(r) => {
            assert!(
                matches!(r.code, "instruction_conflict" | "layout_rule"),
                "{r:?}"
            );
            return;
        }
    };
    let broken = crate::rules::check(&block, &out.layout);
    assert!(broken.is_empty(), "{broken:#?}");
    assert_eq!(crate::engine::lay_out(&block).unwrap().layout, out.layout);
    // One part revalued, over the layout: nothing else moves.
    let p = b.next() % block.parts.len();
    let mut edited = block.clone();
    edited.moves.clear();
    edited.parts[p].value.push('X');
    edited.previous = Some(out.layout.clone());
    let again = crate::engine::lay_out(&edited).unwrap();
    for (k, x) in &out.layout.placed {
        assert_eq!(again.layout.placed.get(k), Some(x), "{k} moved");
        if *k != edited.parts[p].key {
            assert_eq!(
                again.layout.text.get(k),
                out.layout.text.get(k),
                "{k}'s text moved"
            );
        }
    }
}

/// A block for plan 2.A's regions: a dense IC (U1, 10 pins) with a bypass
/// capacitor and a pull-up, a buck (U2 TPS54331 with L1 and C2 on its own
/// output rail), a connector (J1) on U1's D+, a loose RC on its own nets,
/// and a port.
pub fn regions_block() -> Block {
    use crate::input::{Flow, PinKind::*};
    let nets: Vec<Net> = vec![
        net("rail:v", "3V3", NetKind::Supply),     // 0
        net("rail:gnd", "GND", NetKind::Ground),   // 1
        net("sda", "SDA", NetKind::Signal),        // 2
        net("scl", "SCL", NetKind::Signal),        // 3
        net("sw", "SW", NetKind::Signal),          // 4
        net("rail:vin", "VIN", NetKind::Supply),   // 5
        net("dp", "D+", NetKind::Signal),          // 6
        net("f1", "F1", NetKind::Signal),          // 7
        net("f2", "F2", NetKind::Signal),          // 8
        net("rst", "RST", NetKind::Signal),        // 9
        net("rail:vout", "VOUT", NetKind::Supply), // 10
    ];
    let u1_left: Vec<(&str, PinKind, Option<usize>)> = vec![
        ("VDD", PowerIn, Some(0)),
        ("SDA", Bidirectional, Some(2)),
        ("SCL", Input, Some(3)),
        ("RST", Input, Some(9)),
        ("D+", Bidirectional, Some(6)),
    ];
    let u1_right: Vec<(&str, PinKind, Option<usize>)> = vec![
        ("GND", PowerIn, Some(1)),
        ("NC1", Passive, None),
        ("NC2", Passive, None),
        ("NC3", Passive, None),
        ("NC4", Passive, None),
    ];
    // An inductor: drawn as a resistor, its designator's letter L.
    let mut l1 = resistor("l1", "L1", "10u", [Some(4), Some(10)]);
    l1.kind = PartKind::Symbol { prefix: "L".into() };
    let mut j1 = connector("j1", "J1", &[Some(6), Some(1)]);
    j1.value = String::new();
    Block {
        parts: vec![
            ic("u1", "U1", "LAN8710A", &u1_left, &u1_right),
            capacitor("c1", "C1", "100n", [Some(0), Some(1)]),
            resistor("r1", "R1", "10k", [Some(0), Some(9)]),
            ic(
                "u2",
                "U2",
                "TPS54331",
                &[("VIN", PowerIn, Some(5))],
                &[("SW", Output, Some(4)), ("GND", PowerIn, Some(1))],
            ),
            l1,
            capacitor("c2", "C2", "22u", [Some(10), Some(1)]),
            j1,
            resistor("r9", "R9", "1k", [Some(7), Some(8)]),
            capacitor("c9", "C9", "1n", [Some(8), Some(1)]),
            port("port:a", "A", Flow::In, Some(7)),
        ],
        nets,
        sheet: [400_000_000, 277_000_000],
        ..Block::default()
    }
}

/// A dense part (#429's shape): a 24-pin IC whose right side's rows each
/// carry a signal, ten of them pulled, pull-ups and pull-downs
/// interleaved as the Ethernet PHY's are (from the top: up, up, up, down,
/// down, down, up, up, up, down), two of the pulled nets block ports
/// (`with_ports`), and one row (the ninth) with a capacitor to ground as
/// well. Nets: 0 the supply, 1 ground, 2.. one a row.
pub fn dense_block(with_ports: bool) -> Block {
    use crate::input::Flow;
    let ups = [
        true, true, true, false, false, false, true, true, true, false,
    ];
    let names: Vec<String> = (0..12).map(|i| format!("S{i}")).collect();
    let mut nets = vec![
        net("rail:v", "3V3", NetKind::Supply),
        net("rail:gnd", "GND", NetKind::Ground),
    ];
    for n in &names {
        nets.push(net(&n.to_lowercase(), n, NetKind::Signal));
    }
    let right: Vec<(&str, PinKind, Option<usize>)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), PinKind::Bidirectional, Some(2 + i)))
        .collect();
    let left: Vec<(&str, PinKind, Option<usize>)> = vec![
        ("VDD", PinKind::PowerIn, Some(0)),
        ("GND", PinKind::PowerIn, Some(1)),
    ]
    .into_iter()
    .chain((0..10).map(|_| ("NC", PinKind::Passive, None)))
    .collect();
    let mut parts = vec![ic("u1", "U1", "PHY", &left, &right)];
    for (i, up) in ups.iter().enumerate() {
        let rail = if *up { Some(0) } else { Some(1) };
        parts.push(resistor(
            &format!("r{}", i + 1),
            &format!("R{}", i + 1),
            "10k",
            [Some(2 + i), rail],
        ));
    }
    parts.push(capacitor("c1", "C1", "100n", [Some(10), Some(1)]));
    // Two signals out of the block, each pulled: ports.
    if with_ports {
        parts.push(port("port:a", "S0", Flow::Bidirectional, Some(2)));
        parts.push(port("port:b", "S6", Flow::Bidirectional, Some(8)));
    }
    // The two unpulled rows go somewhere: a connector.
    parts.push(connector("j1", "J1", &[Some(12), Some(13)]));
    Block {
        parts,
        nets,
        sheet: [400_000_000, 277_000_000],
        ..Block::default()
    }
}

/// A one-symbol resistor network of `n` elements (prefix RN): element
/// `k` between pin `k` on the left and pin `2n + 1 - k` on the right,
/// level, two steps apart, top to bottom; `nets` gives each pin's net in
/// pin number order.
pub fn network(key: &str, refdes: &str, value: &str, nets: &[Option<usize>]) -> Part {
    let n = nets.len() / 2;
    let h = 2 * GRID * n as Nm;
    let mut pins = Vec::new();
    for (k, net) in nets[..n].iter().enumerate() {
        let y = h / 2 - GRID - 2 * GRID * k as Nm;
        pins.push(pin(
            &(k + 1).to_string(),
            "",
            [-3 * GRID, y],
            Side::Left,
            2 * GRID,
            PinKind::Passive,
            *net,
        ));
    }
    for k in (0..n).rev() {
        let y = h / 2 - GRID - 2 * GRID * k as Nm;
        let number = 2 * n - k;
        pins.push(pin(
            &number.to_string(),
            "",
            [3 * GRID, y],
            Side::Right,
            2 * GRID,
            PinKind::Passive,
            nets[number - 1],
        ));
    }
    pins.sort_by_key(|p| p.number.parse::<usize>().unwrap_or(0));
    symbol(
        key,
        "RN",
        refdes,
        value,
        vec![Graphic::Rect {
            start: [-GRID, h / 2],
            end: [GRID, -h / 2],
            fill: Fill::Body,
        }],
        pins,
    )
}
