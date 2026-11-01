//! The corpus's op-amp front end: a DAC's current outputs converted to
//! voltage and driven out balanced, following yanava's mhd314
//! (BSD-3-Clause, commit 977c5ab5, its `amp.kicad_sch`, which places
//! `ivconverter.kicad_sch` and `BalDiffAmp.kicad_sch` twice each, left
//! and right). Blocks `ivconv` (an OPA1612 as a transimpedance pair, its
//! feedback and decoupling) and `baldiff` (an OPA1622 balanced difference
//! amplifier, its dividers and decoupling) have the ports their sheets'
//! hierarchical labels give; the root block `amp` holds what
//! `amp.kicad_sch` holds itself (the BAV99 clamps on every output, and
//! the enable's divider R65 and R66) and places each child twice, as a
//! stack: `iv_l` and `iv_r`, `bd_l` and `bd_r`, their designators
//! suffixed `_A` (left) and `_B` (right). The reference has no voltage
//! reference: the converters' non-inverting inputs sit on GND. Cut: the
//! single-ended difference amplifier (`sediffamp`), the DAC and the power
//! sheets. The front end's inputs, the outputs the clamps guard (the
//! single-ended ones driven by the cut sheet included) and the balanced
//! amplifiers' enable become ports of `amp`; the supplies are the rails
//! +5VA, -5VA and GND; the single-ended amplifier's enable, which only
//! that sheet reads, goes with it. The connectivity is the reference's,
//! part for part and pin for pin; the symbols are our own drawings
//! (`draw`). Generated once from the reference's netlist; edit it as code
//! from now on.

#![allow(dead_code)]

use super::draw;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// An op amp's power unit: `top` and `bottom` pins on a small box.
fn power_unit<'a>(
    unit: u32,
    top: &[(&'a str, &'a str, &'a str)],
    bottom: &[(&'a str, &'a str, &'a str)],
) -> Vec<serde_json::Value> {
    let w = 2.54 * 2.0 * top.len().max(bottom.len()) as f64;
    let (p, h) = draw::sides(&[], &[], top, bottom, w);
    draw::ic_unit(w, h, &p, unit)
}

/// The OPA1612: amplifiers A and B, then power.
fn opa1612() -> String {
    let mut list = draw::opamp_unit(1, "3", "2", "1", &[]);
    list.extend(draw::opamp_unit(2, "5", "6", "7", &[]));
    list.extend(power_unit(
        3,
        &[("8", "V+", "power_in")],
        &[("4", "V-", "power_in")],
    ));
    draw::ops(list)
}

/// The OPA1622: amplifiers A (with the enable) and B, then power.
fn opa1622() -> String {
    let mut list = draw::opamp_unit(
        1,
        "1",
        "10",
        "9",
        &[("8", "EN", "input", 0.0, 5.08, "up", 2.54)],
    );
    list.extend(draw::opamp_unit(2, "5", "6", "7", &[]));
    list.extend(power_unit(
        3,
        &[("2", "V+", "power_in")],
        &[
            ("4", "V-", "power_in"),
            ("11", "V-", "power_in"),
            ("3", "GND", "power_in"),
        ],
    ));
    draw::ops(list)
}

/// The library: symbols and library components, by command.
pub fn library() -> Vec<Vec<String>> {
    let symbols: Vec<(&str, &str, u32, String)> = vec![
        ("R", "r", 1, draw::resistor()),
        ("C", "c", 1, draw::capacitor()),
        ("BAV99", "bav99", 1, draw::diode_pair()),
        ("OPA1612AxD", "opa1612", 3, opa1612()),
        ("OPA1622", "opa1622", 3, opa1622()),
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
    ("bav99_bav99", "bav99 BAV99", "bav99", "D", "BAV99"),
    ("c_100nf", "c 100nF", "c", "C", "100nF"),
    ("c_1uf", "c 1uF", "c", "C", "1uF"),
    ("c_820pf", "c 820pF", "c", "C", "820pF"),
    (
        "opa1612_opa1612axd",
        "opa1612 OPA1612AxD",
        "opa1612",
        "U",
        "OPA1612AxD",
    ),
    (
        "opa1622_opa1622",
        "opa1622 OPA1622",
        "opa1622",
        "U",
        "OPA1622",
    ),
    ("r_10k", "r 10k", "r", "R", "10k"),
    ("r_1_05k", "r 1.05k", "r", "R", "1.05k"),
    ("r_22r", "r 22R", "r", "R", "22R"),
    ("r_430r", "r 430R", "r", "R", "430R"),
    ("r_470r", "r 470R", "r", "R", "470R"),
];

/// The rails: `(id, name, voltage)`.
const RAILS: &[(&str, &str, &str)] = &[
    ("v5va", "+5VA", "5 V"),
    ("vm5va", "-5VA", "-5 V"),
    ("gnd", "GND", ""),
];

/// A block: its ID, name, ports (`NAME:DIRECTION`), nets (`(id, name)`),
/// parts (`(refdes, library component)`) and connections (`(<part>/<pin>,
/// net)`, a rail as `rail:<name>`, a port as `port:<name>`).
struct Block {
    id: &'static str,
    name: &'static str,
    ports: &'static [&'static str],
    nets: &'static [(&'static str, &'static str)],
    parts: &'static [(&'static str, &'static str)],
    wires: &'static [(&'static str, &'static str)],
}

/// The I/V converter: both halves of an OPA1612, each a transimpedance
/// stage (R23 or R24 with C19 or C20 across it).
const IVCONV: Block = Block {
    id: "ivconv",
    name: "IV Converter",
    ports: &[
        "IVCONV_IN_P:in",
        "IVCONV_IN_N:in",
        "IVCONV_OUT_P:out",
        "IVCONV_OUT_N:out",
    ],
    nets: &[],
    parts: &[
        ("C15", "c_100nf"),
        ("C16", "c_100nf"),
        ("C17", "c_1uf"),
        ("C18", "c_1uf"),
        ("C19", "c_820pf"),
        ("C20", "c_820pf"),
        ("R23", "r_1_05k"),
        ("R24", "r_1_05k"),
        ("U3", "opa1612_opa1612axd"),
    ],
    wires: &[
        ("c15/1", "rail:+5VA"),
        ("c15/2", "rail:GND"),
        ("c16/1", "rail:GND"),
        ("c16/2", "rail:-5VA"),
        ("c17/1", "rail:+5VA"),
        ("c17/2", "rail:GND"),
        ("c18/1", "rail:GND"),
        ("c18/2", "rail:-5VA"),
        ("c19/1", "port:IVCONV_IN_P"),
        ("c19/2", "port:IVCONV_OUT_P"),
        ("c20/1", "port:IVCONV_IN_N"),
        ("c20/2", "port:IVCONV_OUT_N"),
        ("r23/1", "port:IVCONV_IN_P"),
        ("r23/2", "port:IVCONV_OUT_P"),
        ("r24/1", "port:IVCONV_IN_N"),
        ("r24/2", "port:IVCONV_OUT_N"),
        ("u3/1", "port:IVCONV_OUT_P"),
        ("u3/2", "port:IVCONV_IN_P"),
        ("u3/3", "rail:GND"),
        ("u3/4", "rail:-5VA"),
        ("u3/5", "rail:GND"),
        ("u3/6", "port:IVCONV_IN_N"),
        ("u3/7", "port:IVCONV_OUT_N"),
        ("u3/8", "rail:+5VA"),
    ],
};

/// The balanced difference amplifier: an OPA1622's halves, each with its
/// input and feedback resistors and the capacitors across them.
const BALDIFF: Block = Block {
    id: "baldiff",
    name: "Balanced Diff Amp",
    ports: &[
        "BALDIFFAMP_IN_P:in",
        "BALDIFFAMP_IN_N:in",
        "BALDIFFAMP_OUT_P:out",
        "BALDIFFAMP_OUT_N:out",
        "BALDIFFAMP_ENABLE:in",
    ],
    nets: &[
        ("n_c36_1", "N_C36_1"),
        ("n_c53_1", "N_C53_1"),
        ("n_c55_2", "N_C55_2"),
        ("n_c57_1", "N_C57_1"),
    ],
    parts: &[
        ("C36", "c_820pf"),
        ("C53", "c_820pf"),
        ("C55", "c_820pf"),
        ("C57", "c_820pf"),
        ("C64", "c_100nf"),
        ("C65", "c_100nf"),
        ("R45", "r_470r"),
        ("R46", "r_470r"),
        ("R47", "r_470r"),
        ("R48", "r_470r"),
        ("R49", "r_430r"),
        ("R50", "r_430r"),
        ("R51", "r_430r"),
        ("R52", "r_430r"),
        ("U11", "opa1622_opa1622"),
    ],
    wires: &[
        ("c36/1", "N_C36_1"),
        ("c36/2", "port:BALDIFFAMP_OUT_P"),
        ("c53/1", "N_C53_1"),
        ("c53/2", "rail:GND"),
        ("c55/1", "rail:GND"),
        ("c55/2", "N_C55_2"),
        ("c57/1", "N_C57_1"),
        ("c57/2", "port:BALDIFFAMP_OUT_N"),
        ("c64/1", "rail:+5VA"),
        ("c64/2", "rail:GND"),
        ("c65/1", "rail:GND"),
        ("c65/2", "rail:-5VA"),
        ("r45/1", "port:BALDIFFAMP_IN_P"),
        ("r45/2", "N_C36_1"),
        ("r46/1", "port:BALDIFFAMP_IN_N"),
        ("r46/2", "N_C53_1"),
        ("r47/1", "port:BALDIFFAMP_IN_P"),
        ("r47/2", "N_C55_2"),
        ("r48/1", "port:BALDIFFAMP_IN_N"),
        ("r48/2", "N_C57_1"),
        ("r49/1", "rail:GND"),
        ("r49/2", "N_C53_1"),
        ("r50/1", "N_C55_2"),
        ("r50/2", "rail:GND"),
        ("r51/1", "N_C36_1"),
        ("r51/2", "port:BALDIFFAMP_OUT_P"),
        ("r52/1", "N_C57_1"),
        ("r52/2", "port:BALDIFFAMP_OUT_N"),
        ("u11/1", "N_C53_1"),
        ("u11/10", "N_C36_1"),
        ("u11/11", "rail:-5VA"),
        ("u11/2", "rail:+5VA"),
        ("u11/3", "rail:GND"),
        ("u11/4", "rail:-5VA"),
        ("u11/5", "N_C55_2"),
        ("u11/6", "N_C57_1"),
        ("u11/7", "port:BALDIFFAMP_OUT_N"),
        ("u11/8", "port:BALDIFFAMP_ENABLE"),
        ("u11/9", "port:BALDIFFAMP_OUT_P"),
    ],
};

/// The front end itself: the clamps, the enable's divider, and the
/// children placed below.
const AMP: Block = Block {
    id: "amp",
    name: "Amp",
    ports: &[
        "AMP_IN_L_P:in",
        "AMP_IN_L_N:in",
        "AMP_IN_R_P:in",
        "AMP_IN_R_N:in",
        "AMP_IN_BAL_ENABLE:in",
        "AMP_OUT_BAL_L_P:out",
        "AMP_OUT_BAL_L_N:out",
        "AMP_OUT_BAL_R_P:out",
        "AMP_OUT_BAL_R_N:out",
        "AMP_OUT_SE_L:out",
        "AMP_OUT_SE_R:out",
    ],
    nets: &[
        ("bal_en", "BAL_EN"),
        ("iv_l_n", "IV_L_N"),
        ("iv_l_p", "IV_L_P"),
        ("iv_r_n", "IV_R_N"),
        ("iv_r_p", "IV_R_P"),
    ],
    parts: &[
        ("D6", "bav99_bav99"),
        ("D7", "bav99_bav99"),
        ("D8", "bav99_bav99"),
        ("D9", "bav99_bav99"),
        ("D10", "bav99_bav99"),
        ("D11", "bav99_bav99"),
        ("R65", "r_10k"),
        ("R66", "r_22r"),
    ],
    wires: &[
        ("d6/1", "rail:-5VA"),
        ("d6/2", "rail:+5VA"),
        ("d6/3", "port:AMP_OUT_SE_L"),
        ("d7/1", "rail:-5VA"),
        ("d7/2", "rail:+5VA"),
        ("d7/3", "port:AMP_OUT_SE_R"),
        ("d8/1", "rail:-5VA"),
        ("d8/2", "rail:+5VA"),
        ("d8/3", "port:AMP_OUT_BAL_L_P"),
        ("d9/1", "rail:-5VA"),
        ("d9/2", "rail:+5VA"),
        ("d9/3", "port:AMP_OUT_BAL_L_N"),
        ("d10/1", "rail:-5VA"),
        ("d10/2", "rail:+5VA"),
        ("d10/3", "port:AMP_OUT_BAL_R_P"),
        ("d11/1", "rail:-5VA"),
        ("d11/2", "rail:+5VA"),
        ("d11/3", "port:AMP_OUT_BAL_R_N"),
        ("r65/1", "rail:GND"),
        ("r65/2", "BAL_EN"),
        ("r66/1", "port:AMP_IN_BAL_ENABLE"),
        ("r66/2", "BAL_EN"),
    ],
};

/// The root's instances: `(id, block, its ports onto the root's nets)`.
const INSTANCES: &[(&str, &str, &[&str])] = &[
    (
        "iv_l",
        "ivconv",
        &[
            "IVCONV_IN_P=port:AMP_IN_L_P",
            "IVCONV_IN_N=port:AMP_IN_L_N",
            "IVCONV_OUT_P=IV_L_P",
            "IVCONV_OUT_N=IV_L_N",
        ],
    ),
    (
        "iv_r",
        "ivconv",
        &[
            "IVCONV_IN_P=port:AMP_IN_R_P",
            "IVCONV_IN_N=port:AMP_IN_R_N",
            "IVCONV_OUT_P=IV_R_P",
            "IVCONV_OUT_N=IV_R_N",
        ],
    ),
    (
        "bd_l",
        "baldiff",
        &[
            "BALDIFFAMP_IN_P=IV_L_P",
            "BALDIFFAMP_IN_N=IV_L_N",
            "BALDIFFAMP_OUT_P=port:AMP_OUT_BAL_L_P",
            "BALDIFFAMP_OUT_N=port:AMP_OUT_BAL_L_N",
            "BALDIFFAMP_ENABLE=BAL_EN",
        ],
    ),
    (
        "bd_r",
        "baldiff",
        &[
            "BALDIFFAMP_IN_P=IV_R_P",
            "BALDIFFAMP_IN_N=IV_R_N",
            "BALDIFFAMP_OUT_P=port:AMP_OUT_BAL_R_P",
            "BALDIFFAMP_OUT_N=port:AMP_OUT_BAL_R_N",
            "BALDIFFAMP_ENABLE=BAL_EN",
        ],
    ),
];

/// A block's commands: itself, its ports, nets, parts and connections.
fn block(b: &Block) -> Vec<Vec<String>> {
    let mut out = vec![args(&["block", "create", b.name, "--id", b.id])];
    if !b.ports.is_empty() {
        let mut a = args(&["block", "edit", b.id]);
        for p in b.ports {
            a.extend(args(&["--add-port", p]));
        }
        out.push(a);
    }
    for (id, name) in b.nets {
        out.push(args(&["net", "add", b.id, name, "--id", id]));
    }
    for (refdes, of) in b.parts {
        let id = refdes.to_lowercase();
        out.push(args(&[
            "component",
            "add",
            b.id,
            "--of",
            of,
            "--id",
            &id,
            "--refdes",
            refdes,
        ]));
    }
    for (pin, net) in b.wires {
        out.push(args(&["connect", &format!("{}/{pin}", b.id), net]));
    }
    out
}

/// The design: rails, the two children, the root placing them twice
/// each, and the design.
pub fn design() -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (id, name, v) in RAILS {
        // -5VA's name and voltage start with a dash: given after `--`
        // and with `=`, so neither reads as a flag.
        let mut a = args(&["rail", "add", "--id", id]);
        if !v.is_empty() {
            a.push(format!("--voltage={v}"));
        }
        a.extend(args(&["--", name]));
        out.push(a);
    }
    for b in [&IVCONV, &BALDIFF, &AMP] {
        out.extend(block(b));
    }
    for (id, of, ports) in INSTANCES {
        let mut a = args(&[
            "instance",
            "add",
            "amp",
            "--of",
            of,
            "--id",
            id,
            "--refdes",
            "suffix_letter",
        ]);
        for p in *ports {
            a.extend(args(&["--port", p]));
        }
        out.push(a);
    }
    out.push(args(&[
        "design",
        "create",
        "Front end",
        "--root",
        "amp",
        "--id",
        "frontend",
    ]));
    out
}

/// Every command, the library first.
pub fn script() -> Vec<Vec<String>> {
    let mut out = library();
    out.extend(design());
    out
}
