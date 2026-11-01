//! The corpus's 10/100 Ethernet PHY: a LAN8710A and its RJ45, following
//! OLIMEX's ESP32-POE (hardware Apache-2.0, commit deb38a37, its
//! `ESP32-PoE_Rev_M2.kicad_sch`). Taken: U4 and its network, the line
//! terminations R10 to R13, the ESD array TVS1, the ferrite L1 feeding the
//! analog supply, RBIAS (R26), the reset's R25 and C21 (fitted NA in the
//! reference, kept as it is), the strap R41 and the strap arrays RM1 and
//! RM2 (each drawn as one symbol of four resistor units, unit `k` between
//! pins `k` and `9 - k`: the reference's pin `k.2` is pin `k`, its `k.1`
//! pin `9 - k`; RM1's fourth unit is unused there and not connected here),
//! the decoupling on VDDIO, VDDA and VDDCR, the jack LAN_CON1 (magnetics
//! inside) with its shield capacitor C2. The ESP32's RMII pins are ports of
//! the root block `eth`; the supplies are the reference's +3.3VLAN, the
//! analog +3.3VA after L1 (unnamed in the reference) and GND. Cut: the
//! ESP32, U8 (fitted NA, the clock inverter), TVS2 and TVS3 (NA), the PoE
//! controller and the jack's PW+ and PW- (not connected here), the
//! chargers, USB, SD and the rest of the board.
//!
//! The status LEDs are a block `status_led` (an LED from its port DRIVE,
//! through a 2.2k resistor to GND) placed twice as a stack, `led_act` on
//! the PHY's LED1 and `led_lnk` on LED2. The reference's ACT1 is wired
//! so (LED1, ACT1, R15, GND), but LNK1 the other way round: +3.3VA, R14,
//! LNK1's anode, its cathode on LED2, the pin sinking (the LAN8710's
//! LED2/nINTSEL strap reads the polarity). One block can't hold both, so
//! `led_lnk` follows ACT1's wiring: the one place this circuit departs
//! from its reference. The LEDs' colours (yellow, green) are one value,
//! `LED/0603`. The jack holds no LEDs. The rest of the connectivity is
//! the reference's, part for part and pin for pin; the symbols are our
//! own drawings (`draw`). Generated once from the reference's netlist;
//! edit it as code from now on.

#![allow(dead_code)]

use super::draw;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn lan8710() -> String {
    let (p, h) = draw::sides(
        &[
            ("12", "VDDIO", "power_in"),
            ("27", "VDD1A", "power_in"),
            ("1", "VDD2A", "power_in"),
            ("6", "VDDCR", "power_in"),
            ("5", "XTAL1/CLKIN", "input"),
            ("4", "XTAL2", "output"),
            ("3", "LED1/REGOFF", "bidirectional"),
            ("2", "LED2/NINTSEL", "bidirectional"),
            ("29", "TXP", "output"),
            ("28", "TXN", "output"),
            ("31", "RXP", "input"),
            ("30", "RXN", "input"),
        ],
        &[
            ("22", "TXD0", "input"),
            ("23", "TXD1", "input"),
            ("24", "TXD2", "input"),
            ("25", "TXD3", "input"),
            ("21", "TXEN", "input"),
            ("20", "TXCLK", "output"),
            ("18", "NINT/TXER/TXD4", "bidirectional"),
            ("11", "RXD0/MODE0", "bidirectional"),
            ("10", "RXD1/MODE1", "bidirectional"),
            ("9", "RXD2/RMIISEL", "bidirectional"),
            ("8", "RXD3/PHYAD2", "bidirectional"),
            ("13", "RXER/RXD4/PHYAD0", "bidirectional"),
            ("7", "RXCLK/PHYAD1", "bidirectional"),
            ("26", "RXDV", "output"),
            ("14", "CRS", "output"),
            ("15", "COL/CRS_DV/MODE2", "bidirectional"),
            ("17", "MDC", "input"),
            ("16", "MDIO", "bidirectional"),
            ("19", "NRST", "input"),
            ("32", "RBIAS", "input"),
        ],
        &[],
        &[("33", "GND", "power_in")],
        30.48,
    );
    draw::ic(30.48, h, &p)
}

fn rj45() -> String {
    let (p, h) = draw::sides(
        &[
            ("1", "TD+", "bidirectional"),
            ("4", "TCT", "passive"),
            ("2", "TD-", "bidirectional"),
            ("3", "RD+", "bidirectional"),
            ("5", "RCT", "passive"),
            ("6", "RD-", "bidirectional"),
        ],
        &[("9", "PW+", "passive"), ("10", "PW-", "passive")],
        &[],
        &[
            ("7", "GND/NC", "passive"),
            ("8", "NC/GND", "passive"),
            ("0", "Shield", "passive"),
        ],
        15.24,
    );
    draw::ic(15.24, h, &p)
}

fn esds314() -> String {
    let (p, h) = draw::sides(
        &[
            ("1", "I/O1", "passive"),
            ("2", "GND", "passive"),
            ("3", "I/O2", "passive"),
        ],
        &[("5", "I/O4", "passive"), ("4", "I/O3", "passive")],
        &[],
        &[],
        10.16,
    );
    draw::ic(10.16, h, &p)
}

/// The library: symbols and library components, by command.
pub fn library() -> Vec<Vec<String>> {
    let symbols: Vec<(&str, &str, u32, String)> = vec![
        ("R", "r", 1, draw::resistor()),
        ("C", "c", 1, draw::capacitor()),
        ("LED", "led", 1, draw::led()),
        ("FerriteBead", "ferrite", 1, draw::ferrite()),
        ("RA1206_4x0603", "rarray4", 4, draw::resistor_array(4)),
        ("ESDS314DBVR", "esds314", 1, esds314()),
        ("LAN8710A", "lan8710", 1, lan8710()),
        ("RJP-003TC1", "rj45", 1, rj45()),
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
    ("c_100nf", "c 100nF", "c", "C", "100nF"),
    ("c_1nf_2000v", "c 1nF/2000V", "c", "C", "1nF/2000V"),
    ("c_22uf", "c 22uF", "c", "C", "22uF"),
    ("c_2_2uf", "c 2.2uF", "c", "C", "2.2uF"),
    ("c_na_10uf", "c NA(10uF)", "c", "C", "NA(10uF)"),
    (
        "esds314_esds314dbvr",
        "esds314 ESDS314DBVR",
        "esds314",
        "TVS",
        "ESDS314DBVR",
    ),
    (
        "ferrite_fb0805",
        "ferrite FB0805/600R/2A",
        "ferrite",
        "L",
        "FB0805/600R/2A",
    ),
    (
        "lan8710_lan8710a_ezc",
        "lan8710 LAN8710A-EZC",
        "lan8710",
        "U",
        "LAN8710A-EZC",
    ),
    ("led_0603", "led LED/0603", "led", "D", "LED/0603"),
    ("r_10k", "r 10k", "r", "R", "10k"),
    ("r_12_1k", "r 12.1k/1%", "r", "R", "12.1k/1%"),
    ("r_2_2k", "r 2.2k", "r", "R", "2.2k"),
    ("r_49_9r", "r 49.9R/1%", "r", "R", "49.9R/1%"),
    ("rarray4_2_2k", "rarray4 4x2.2k", "rarray4", "RM", "4x2.2k"),
    (
        "rj45_rjp_003tc1",
        "rj45 RJP-003TC1(LPJ4112CNL)",
        "rj45",
        "J",
        "RJP-003TC1(LPJ4112CNL)",
    ),
];

/// The root block's parts: `(refdes, library component)`.
const PARTS: &[(&str, &str)] = &[
    ("C2", "c_1nf_2000v"),
    ("C9", "c_22uf"),
    ("C11", "c_100nf"),
    ("C13", "c_22uf"),
    ("C14", "c_100nf"),
    ("C15", "c_100nf"),
    ("C16", "c_100nf"),
    ("C17", "c_2_2uf"),
    ("C18", "c_100nf"),
    ("C21", "c_na_10uf"),
    ("L1", "ferrite_fb0805"),
    ("LAN_CON1", "rj45_rjp_003tc1"),
    ("R10", "r_49_9r"),
    ("R11", "r_49_9r"),
    ("R12", "r_49_9r"),
    ("R13", "r_49_9r"),
    ("R25", "r_10k"),
    ("R26", "r_12_1k"),
    ("R41", "r_2_2k"),
    ("RM1", "rarray4_2_2k"),
    ("RM2", "rarray4_2_2k"),
    ("TVS1", "esds314_esds314dbvr"),
    ("U4", "lan8710_lan8710a_ezc"),
];

/// Every connection of the root block: `(<part>/<pin>, net)`, a rail as
/// `rail:<name>`, a port as `port:<name>`.
const WIRES: &[(&str, &str)] = &[
    ("c2/1", "SHIELD"),
    ("c2/2", "rail:GND"),
    ("c9/1", "rail:+3.3VLAN"),
    ("c9/2", "rail:GND"),
    ("c11/1", "rail:+3.3VA"),
    ("c11/2", "rail:GND"),
    ("c13/1", "rail:+3.3VA"),
    ("c13/2", "rail:GND"),
    ("c14/1", "rail:+3.3VLAN"),
    ("c14/2", "rail:GND"),
    ("c15/1", "rail:+3.3VA"),
    ("c15/2", "rail:GND"),
    ("c16/1", "rail:+3.3VA"),
    ("c16/2", "rail:GND"),
    ("c17/1", "VDDCR"),
    ("c17/2", "rail:GND"),
    ("c18/1", "VDDCR"),
    ("c18/2", "rail:GND"),
    ("c21/1", "rail:GND"),
    ("c21/2", "NRST"),
    ("l1/1", "rail:+3.3VLAN"),
    ("l1/2", "rail:+3.3VA"),
    ("lan_con1/0", "SHIELD"),
    ("lan_con1/1", "TD+"),
    ("lan_con1/2", "TD-"),
    ("lan_con1/3", "RD+"),
    ("lan_con1/4", "rail:+3.3VA"),
    ("lan_con1/5", "rail:+3.3VA"),
    ("lan_con1/6", "RD-"),
    ("lan_con1/7", "rail:GND"),
    ("lan_con1/8", "rail:GND"),
    ("r10/1", "RD-"),
    ("r10/2", "rail:+3.3VA"),
    ("r11/1", "RD+"),
    ("r11/2", "rail:+3.3VA"),
    ("r12/1", "TD-"),
    ("r12/2", "rail:+3.3VA"),
    ("r13/1", "TD+"),
    ("r13/2", "rail:+3.3VA"),
    ("r25/1", "NRST"),
    ("r25/2", "rail:+3.3VLAN"),
    ("r26/1", "RBIAS"),
    ("r26/2", "rail:GND"),
    ("r41/1", "rail:GND"),
    ("r41/2", "PHYAD0"),
    ("rm1/1", "PHYAD1"),
    ("rm1/2", "PHYAD2"),
    ("rm1/3", "RMIISEL"),
    ("rm1/6", "rail:+3.3VLAN"),
    ("rm1/7", "rail:GND"),
    ("rm1/8", "rail:GND"),
    ("rm2/1", "port:RXD1"),
    ("rm2/2", "port:RXD0"),
    ("rm2/3", "port:CRS_DV"),
    ("rm2/4", "port:MDIO"),
    ("rm2/5", "rail:+3.3VLAN"),
    ("rm2/6", "rail:+3.3VLAN"),
    ("rm2/7", "rail:+3.3VLAN"),
    ("rm2/8", "rail:+3.3VLAN"),
    ("tvs1/1", "RD+"),
    ("tvs1/2", "rail:GND"),
    ("tvs1/3", "TD+"),
    ("tvs1/4", "TD-"),
    ("tvs1/5", "RD-"),
    ("u4/1", "rail:+3.3VA"),
    ("u4/2", "LED2"),
    ("u4/3", "LED1"),
    ("u4/5", "port:EMAC_CLK"),
    ("u4/6", "VDDCR"),
    ("u4/7", "PHYAD1"),
    ("u4/8", "PHYAD2"),
    ("u4/9", "RMIISEL"),
    ("u4/10", "port:RXD1"),
    ("u4/11", "port:RXD0"),
    ("u4/12", "rail:+3.3VLAN"),
    ("u4/13", "PHYAD0"),
    ("u4/15", "port:CRS_DV"),
    ("u4/16", "port:MDIO"),
    ("u4/17", "port:MDC"),
    ("u4/19", "NRST"),
    ("u4/21", "port:TX_EN"),
    ("u4/22", "port:TXD0"),
    ("u4/23", "port:TXD1"),
    ("u4/24", "rail:GND"),
    ("u4/25", "rail:GND"),
    ("u4/27", "rail:+3.3VA"),
    ("u4/28", "TD-"),
    ("u4/29", "TD+"),
    ("u4/30", "RD-"),
    ("u4/31", "RD+"),
    ("u4/32", "RBIAS"),
    ("u4/33", "rail:GND"),
];

/// Pins marked not connected.
const NC: &[&str] = &[
    "lan_con1/9",
    "lan_con1/10",
    "rm1/4",
    "rm1/5",
    "u4/4",
    "u4/14",
    "u4/18",
    "u4/20",
    "u4/26",
];

/// The root block's ports, the ESP32's RMII pins: `NAME:DIRECTION`.
const PORTS: &[&str] = &[
    "TXD0:in",
    "TXD1:in",
    "TX_EN:in",
    "RXD0:out",
    "RXD1:out",
    "CRS_DV:out",
    "MDC:in",
    "MDIO:bidirectional",
    "EMAC_CLK:in",
];

/// The root block's nets: `(id, name)`.
const NETS: &[(&str, &str)] = &[
    ("led1", "LED1"),
    ("led2", "LED2"),
    ("nrst", "NRST"),
    ("phyad0", "PHYAD0"),
    ("phyad1", "PHYAD1"),
    ("phyad2", "PHYAD2"),
    ("rbias", "RBIAS"),
    ("rd_n", "RD-"),
    ("rd_p", "RD+"),
    ("rmiisel", "RMIISEL"),
    ("shield", "SHIELD"),
    ("td_n", "TD-"),
    ("td_p", "TD+"),
    ("vddcr", "VDDCR"),
];

/// The rails: `(id, name, voltage)`.
const RAILS: &[(&str, &str, &str)] = &[
    ("v3v3_lan", "+3.3VLAN", "3.3 V"),
    ("v3v3_a", "+3.3VA", "3.3 V"),
    ("gnd", "GND", ""),
];

/// The design: rails, the LED block, the root block with its ports, nets,
/// parts, connections and the LEDs' stack, and the design.
pub fn design() -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (id, name, v) in RAILS {
        let mut a = args(&["rail", "add", name, "--id", id]);
        if !v.is_empty() {
            a.extend(args(&["--voltage", v]));
        }
        out.push(a);
    }
    out.extend([
        args(&["block", "create", "Status LED", "--id", "status_led"]),
        args(&["block", "edit", "status_led", "--add-port", "DRIVE:in"]),
        args(&["net", "add", "status_led", "K", "--id", "k"]),
        args(&[
            "component",
            "add",
            "status_led",
            "--of",
            "led_0603",
            "--id",
            "d1",
            "--refdes",
            "D1",
        ]),
        args(&[
            "component",
            "add",
            "status_led",
            "--of",
            "r_2_2k",
            "--id",
            "r15",
            "--refdes",
            "R15",
        ]),
        args(&["connect", "status_led/d1/2", "port:DRIVE"]),
        args(&["connect", "status_led/d1/1", "K"]),
        args(&["connect", "status_led/r15/1", "K"]),
        args(&["connect", "status_led/r15/2", "rail:GND"]),
    ]);
    out.push(args(&["block", "create", "Ethernet PHY", "--id", "eth"]));
    let mut ports = args(&["block", "edit", "eth"]);
    for p in PORTS {
        ports.extend(args(&["--add-port", p]));
    }
    out.push(ports);
    for (id, name) in NETS {
        out.push(args(&["net", "add", "eth", name, "--id", id]));
    }
    for (refdes, of) in PARTS {
        let id = refdes.to_lowercase();
        out.push(args(&[
            "component",
            "add",
            "eth",
            "--of",
            of,
            "--id",
            &id,
            "--refdes",
            refdes,
        ]));
    }
    for (pin, net) in WIRES {
        out.push(args(&["connect", &format!("eth/{pin}"), net]));
    }
    for pin in NC {
        out.push(args(&["nc", &format!("eth/{pin}")]));
    }
    for (id, net) in [("led_act", "LED1"), ("led_lnk", "LED2")] {
        out.push(args(&[
            "instance",
            "add",
            "eth",
            "--of",
            "status_led",
            "--id",
            id,
            "--refdes",
            "suffix_letter",
            "--port",
            &format!("DRIVE={net}"),
        ]));
    }
    out.push(args(&[
        "design",
        "create",
        "Ethernet PHY",
        "--root",
        "eth",
        "--id",
        "phy",
    ]));
    out
}

/// Every command, the library first.
pub fn script() -> Vec<Vec<String>> {
    let mut out = library();
    out.extend(design());
    out
}
