//! The corpus's buck supply: a TPS54331 stepping 5 V down to 3.3 V,
//! following maudeve-it's 3.3V-3A-step-down-converter-powered-by-USB-C
//! (MIT, commit 86388564, its `USB power port.kicad_sch`). Only the
//! converter's network is taken: U1 and everything on its pins' nets
//! (input capacitors C1 to C3, C4 on SS, C5, C6 and R1 on COMP, the VSNS
//! divider R7 to R9, the boot capacitor C7, D1, L1, the output capacitors
//! C8 and C9, R22 and the jumper J5 on EN, and the test point TP3 on
//! GND), with the 3.3 V output connector J4. Cut: the USB-C receptacle and
//! its CC detection, the LM358 comparators, the transistors, LEDs and
//! selector headers, which reach the converter only through +5V and GND.
//! VIN is the rail +5V, VOUT the rail +3V3. The connectivity is the
//! reference's, part for part and pin for pin; the symbols are our own
//! drawings (`draw`). Generated once from the reference's netlist; edit it
//! as code from now on.

#![allow(dead_code)]

use super::draw;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn tps54331() -> String {
    let (p, h, wide) = draw::sides(
        &[
            ("2", "VIN", "power_in"),
            ("3", "EN", "input"),
            ("4", "SS", "input"),
            ("6", "COMP", "input"),
        ],
        &[
            ("1", "BOOT", "passive"),
            ("8", "PH", "output"),
            ("5", "VSNS", "input"),
        ],
        &[],
        &[("7", "GND", "power_in"), ("9", "GPAD", "power_in")],
        15.24,
    );
    draw::ic(wide, h, &p)
}

/// The library: symbols and library components, by command.
pub fn library() -> Vec<Vec<String>> {
    let symbols: Vec<(&str, &str, u32, String)> = vec![
        ("R", "r", 1, draw::resistor()),
        ("C", "c", 1, draw::capacitor()),
        ("L", "l", 1, draw::inductor()),
        ("D_Schottky", "schottky", 1, draw::diode()),
        ("TestPoint", "tp", 1, draw::test_point()),
        ("Conn_01x02", "conn2", 1, draw::connector(2)),
        ("TPS54331", "tps54331", 1, tps54331()),
    ];
    let mut out = Vec::new();
    for (name, id, units, ops) in &symbols {
        let units = units.to_string();
        out.push(args(&[
            "symbol", "create", name, "--id", id, "--units", &units, "--ops", ops,
        ]));
    }
    for (id, name, symbol, prefix, value) in LIBCOMPS {
        out.push(args(&[
            "libcomp", "create", name, "--id", id, "--symbol", symbol, "--prefix", prefix,
            "--value", value,
        ]));
    }
    out
}

/// Library components: `(id, name, symbol, prefix, value)`.
const LIBCOMPS: &[(&str, &str, &str, &str, &str)] = &[
    ("c_0_1uf", "c 0.1uF", "c", "C", "0.1uF"),
    ("c_1_5nf", "c 1.5nF", "c", "C", "1.5nF"),
    ("c_27pf", "c 27pF", "c", "C", "27pF"),
    ("c_47uf", "c 47uF", "c", "C", "47uF"),
    ("c_4_7uf", "c 4.7uF", "c", "C", "4.7uF"),
    ("conn2_3_3v", "conn2 3.3V conn.", "conn2", "J", "3.3V conn."),
    ("conn2_5v", "conn2 5V conn.", "conn2", "J", "5V conn."),
    ("l_3_3uh", "l 3.3uH", "l", "L", "3.3uH"),
    ("r_10k", "r 10k", "r", "R", "10k"),
    ("r_3_3k", "r 3.3K", "r", "R", "3.3K"),
    ("r_330", "r 330", "r", "R", "330"),
    ("r_47k", "r 47k", "r", "R", "47k"),
    ("schottky_ss34", "schottky SS34", "schottky", "D", "SS34"),
    ("tp_gnd", "tp GND", "tp", "TP", "GND"),
    (
        "tps54331_tps54331",
        "tps54331 TPS54331",
        "tps54331",
        "U",
        "TPS54331",
    ),
];

/// The parts: `(refdes, library component)`.
const PARTS: &[(&str, &str)] = &[
    ("C1", "c_4_7uf"),
    ("C2", "c_4_7uf"),
    ("C3", "c_0_1uf"),
    ("C4", "c_0_1uf"),
    ("C5", "c_1_5nf"),
    ("C6", "c_27pf"),
    ("C7", "c_0_1uf"),
    ("C8", "c_47uf"),
    ("C9", "c_47uf"),
    ("D1", "schottky_ss34"),
    ("J4", "conn2_3_3v"),
    ("J5", "conn2_5v"),
    ("L1", "l_3_3uh"),
    ("R1", "r_47k"),
    ("R7", "r_330"),
    ("R8", "r_10k"),
    ("R9", "r_3_3k"),
    ("R22", "r_10k"),
    ("TP3", "tp_gnd"),
    ("U1", "tps54331_tps54331"),
];

/// Every connection: `(<part>/<pin>, net)`, a rail as `rail:<name>`.
const WIRES: &[(&str, &str)] = &[
    ("c1/1", "rail:+5V"),
    ("c1/2", "rail:GND"),
    ("c2/1", "rail:+5V"),
    ("c2/2", "rail:GND"),
    ("c3/1", "rail:GND"),
    ("c3/2", "rail:+5V"),
    ("c4/1", "rail:GND"),
    ("c4/2", "SS"),
    ("c5/1", "COMP"),
    ("c5/2", "N_C5_2"),
    ("c6/1", "rail:GND"),
    ("c6/2", "COMP"),
    ("c7/1", "BOOT"),
    ("c7/2", "PH"),
    ("c8/1", "rail:GND"),
    ("c8/2", "rail:+3V3"),
    ("c9/1", "rail:GND"),
    ("c9/2", "rail:+3V3"),
    ("d1/1", "PH"),
    ("d1/2", "rail:GND"),
    ("j4/1", "rail:GND"),
    ("j4/2", "rail:+3V3"),
    ("j5/1", "rail:GND"),
    ("j5/2", "BUCK-EN"),
    ("l1/1", "PH"),
    ("l1/2", "rail:+3V3"),
    ("r1/1", "rail:GND"),
    ("r1/2", "N_C5_2"),
    ("r7/1", "N_R7_1"),
    ("r7/2", "rail:+3V3"),
    ("r8/1", "VSNS"),
    ("r8/2", "N_R7_1"),
    ("r9/1", "rail:GND"),
    ("r9/2", "VSNS"),
    ("r22/1", "rail:GND"),
    ("r22/2", "BUCK-EN"),
    ("tp3/1", "rail:GND"),
    ("u1/1", "BOOT"),
    ("u1/2", "rail:+5V"),
    ("u1/3", "BUCK-EN"),
    ("u1/4", "SS"),
    ("u1/5", "VSNS"),
    ("u1/6", "COMP"),
    ("u1/7", "rail:GND"),
    ("u1/8", "PH"),
    ("u1/9", "rail:GND"),
];

/// The block's nets: `(id, name)`.
const NETS: &[(&str, &str)] = &[
    ("boot", "BOOT"),
    ("buck_en", "BUCK-EN"),
    ("comp", "COMP"),
    ("n_c5_2", "N_C5_2"),
    ("n_r7_1", "N_R7_1"),
    ("ph", "PH"),
    ("ss", "SS"),
    ("vsns", "VSNS"),
];

/// The rails: `(id, name, voltage)`.
const RAILS: &[(&str, &str, &str)] = &[
    ("v5v", "+5V", "5 V"),
    ("v3v3", "+3V3", "3.3 V"),
    ("gnd", "GND", ""),
];

/// The design: rails, one block holding the converter, its nets, parts
/// and connections, and the design.
pub fn design() -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (id, name, v) in RAILS {
        let mut a = args(&["rail", "add", name, "--id", id]);
        if !v.is_empty() {
            a.extend(args(&["--voltage", v]));
        }
        out.push(a);
    }
    out.push(args(&["block", "create", "Buck", "--id", "buck"]));
    for (id, name) in NETS {
        out.push(args(&["net", "add", "buck", name, "--id", id]));
    }
    for (refdes, of) in PARTS {
        let id = refdes.to_lowercase();
        out.push(args(&[
            "component",
            "add",
            "buck",
            "--of",
            of,
            "--id",
            &id,
            "--refdes",
            refdes,
        ]));
    }
    for (pin, net) in WIRES {
        out.push(args(&["connect", &format!("buck/{pin}"), net]));
    }
    out.push(args(&[
        "design", "create", "Buck", "--root", "buck", "--id", "supply",
    ]));
    out
}

/// Every command, the library first.
pub fn script() -> Vec<Vec<String>> {
    let mut out = library();
    out.extend(design());
    out
}
