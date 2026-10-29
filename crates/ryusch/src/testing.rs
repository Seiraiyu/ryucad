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
        graphics,
        pins,
    }
}

/// A resistor standing up: pin 1 at the top.
pub fn resistor(key: &str, refdes: &str, value: &str, nets: [Option<usize>; 2]) -> Part {
    symbol(
        key,
        "R",
        refdes,
        value,
        vec![Graphic::Rect {
            start: [-40 * MIL, 100 * MIL],
            end: [40 * MIL, -100 * MIL],
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
