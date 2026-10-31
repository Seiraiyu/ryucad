//! The corpus's second board: an RP2040 with USB-C, following
//! CIRCUITSTATE's Mitayi-Pico-D1 (MIT, commit 8411224b, its
//! `Mitayi-Pico-D1.kicad_sch`): the RP2040 drawn as three units (GPIO,
//! power, system), its QSPI flash, 12 MHz crystal, MIC5219 LDO, USB-C and
//! micro-B, decoupling, buttons, jumpers and headers. The connectivity is
//! the reference's, part for part and pin for pin; the symbols are our own
//! drawings (`draw`). Generated once from the reference's netlist; edit it
//! as code from now on.

#![allow(dead_code)]

use super::draw;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// The RP2040's units: GPIO, power, system.
fn rp2040() -> String {
    let gpio_l: Vec<(&str, &str, &str)> = vec![
        ("2", "GPIO0", "bidirectional"),
        ("3", "GPIO1", "bidirectional"),
        ("4", "GPIO2", "bidirectional"),
        ("5", "GPIO3", "bidirectional"),
        ("6", "GPIO4", "bidirectional"),
        ("7", "GPIO5", "bidirectional"),
        ("8", "GPIO6", "bidirectional"),
        ("9", "GPIO7", "bidirectional"),
        ("11", "GPIO8", "bidirectional"),
        ("12", "GPIO9", "bidirectional"),
        ("13", "GPIO10", "bidirectional"),
        ("14", "GPIO11", "bidirectional"),
        ("15", "GPIO12", "bidirectional"),
        ("16", "GPIO13", "bidirectional"),
        ("17", "GPIO14", "bidirectional"),
    ];
    let gpio_r: Vec<(&str, &str, &str)> = vec![
        ("18", "GPIO15", "bidirectional"),
        ("27", "GPIO16", "bidirectional"),
        ("28", "GPIO17", "bidirectional"),
        ("29", "GPIO18", "bidirectional"),
        ("30", "GPIO19", "bidirectional"),
        ("31", "GPIO20", "bidirectional"),
        ("32", "GPIO21", "bidirectional"),
        ("34", "GPIO22", "bidirectional"),
        ("35", "GPIO23", "bidirectional"),
        ("36", "GPIO24", "bidirectional"),
        ("37", "GPIO25", "bidirectional"),
        ("38", "GPIO26/ADC0", "bidirectional"),
        ("39", "GPIO27/ADC1", "bidirectional"),
        ("40", "GPIO28/ADC2", "bidirectional"),
        ("41", "GPIO29/ADC3", "bidirectional"),
    ];
    let (p1, h1) = draw::sides(&gpio_l, &gpio_r, &[], &[], 25.4);
    let (p2, h2) = draw::sides(
        &[
            ("1", "IOVDD", "power_in"),
            ("10", "IOVDD", "power_in"),
            ("22", "IOVDD", "power_in"),
            ("33", "IOVDD", "power_in"),
            ("42", "IOVDD", "power_in"),
            ("49", "IOVDD", "power_in"),
            ("48", "USB-VDD", "power_in"),
            ("43", "ADC-AVDD", "power_in"),
            ("44", "VREG-VIN", "power_in"),
        ],
        &[
            ("45", "VREG-VOUT", "power_out"),
            ("23", "DVDD", "power_in"),
            ("50", "DVDD", "power_in"),
        ],
        &[],
        &[("57", "GND", "power_in")],
        20.32,
    );
    let (p3, h3) = draw::sides(
        &[
            ("20", "XIN", "bidirectional"),
            ("21", "XOUT", "output"),
            ("26", "RUN", "input"),
            ("19", "TESTEN", "input"),
            ("24", "SWCLK", "unspecified"),
            ("25", "SWDIO", "unspecified"),
        ],
        &[
            ("47", "USB-DP", "bidirectional"),
            ("46", "USB-DM", "bidirectional"),
            ("56", "QSPI-SS-N", "bidirectional"),
            ("52", "QSPI-SCLK", "bidirectional"),
            ("53", "QSPI-SD0", "bidirectional"),
            ("55", "QSPI-SD1", "bidirectional"),
            ("54", "QSPI-SD2", "bidirectional"),
            ("51", "QSPI-SD3", "bidirectional"),
        ],
        &[],
        &[],
        25.4,
    );
    let mut list = draw::ic_unit(25.4, h1, &p1, 1);
    list.extend(draw::ic_unit(20.32, h2, &p2, 2));
    list.extend(draw::ic_unit(25.4, h3, &p3, 3));
    draw::ops(list)
}

fn w25q() -> String {
    let (p, h) = draw::sides(
        &[
            ("1", "~{CS}", "input"),
            ("6", "CLK", "input"),
            ("5", "DI(IO0)", "bidirectional"),
            ("2", "DO(IO1)", "bidirectional"),
        ],
        &[("3", "IO2", "bidirectional"), ("7", "IO3", "bidirectional")],
        &[("8", "VCC", "power_in")],
        &[("4", "GND", "power_in")],
        15.24,
    );
    draw::ic(15.24, h, &p)
}

fn mic5219() -> String {
    let (p, h) = draw::sides(
        &[("1", "IN", "power_in"), ("3", "EN", "input")],
        &[("5", "OUT", "power_out"), ("4", "BP", "input")],
        &[],
        &[("2", "GND", "power_in")],
        10.16,
    );
    draw::ic(10.16, h, &p)
}

fn usb_c() -> String {
    let (p, h) = draw::sides(
        &[],
        &[
            ("A4", "VBUS", "passive"),
            ("A9", "VBUS", "passive"),
            ("B4", "VBUS", "passive"),
            ("B9", "VBUS", "passive"),
            ("A6", "D+", "bidirectional"),
            ("B6", "D+", "bidirectional"),
            ("A7", "D-", "bidirectional"),
            ("B7", "D-", "bidirectional"),
            ("A5", "CC1", "bidirectional"),
            ("B5", "CC2", "bidirectional"),
            ("A8", "SBU1", "bidirectional"),
            ("B8", "SBU2", "bidirectional"),
        ],
        &[],
        &[
            ("A1", "GND", "passive"),
            ("A12", "GND", "passive"),
            ("B1", "GND", "passive"),
            ("B12", "GND", "passive"),
            ("S1", "SHIELD", "passive"),
        ],
        15.24,
    );
    draw::ic(15.24, h, &p)
}

fn usb_micro() -> String {
    let (p, h) = draw::sides(
        &[],
        &[
            ("1", "VBUS", "power_out"),
            ("2", "D-", "bidirectional"),
            ("3", "D+", "bidirectional"),
            ("4", "ID", "passive"),
        ],
        &[],
        &[("5", "GND", "power_out"), ("6", "Shield", "passive")],
        10.16,
    );
    draw::ic(10.16, h, &p)
}

/// The library: symbols and library components, by command.
pub fn library() -> Vec<Vec<String>> {
    let symbols: Vec<(&str, &str, u32, String)> = vec![
        ("R", "r", 1, draw::resistor()),
        ("C", "c", 1, draw::capacitor()),
        ("C_Polarized", "cp", 1, draw::capacitor_polarized()),
        ("LED", "led", 1, draw::led()),
        ("D_Schottky", "schottky", 1, draw::diode()),
        ("Crystal_GND24", "xtal4", 1, draw::crystal_gnd()),
        ("SW_Push", "sw_push", 1, draw::button()),
        ("SolderJumper_2", "jumper2", 1, draw::jumper2()),
        ("SolderJumper_3", "jumper3", 1, draw::jumper3()),
        ("W25Q", "w25q", 1, w25q()),
        ("MIC5219-3.3", "mic5219", 1, mic5219()),
        ("RP2040", "rp2040", 3, rp2040()),
        ("USB_C_Receptacle_USB2.0", "usb_c", 1, usb_c()),
        ("USB_B_Micro", "usb_micro", 1, usb_micro()),
        ("Conn_01x01", "conn1", 1, draw::connector(1)),
        ("Conn_01x03", "conn3", 1, draw::connector(3)),
        ("Conn_01x20", "conn20", 1, draw::connector(20)),
        ("Conn_02x04", "conn8", 1, draw::connector(8)),
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
    ("c_10uf", "c 10uF", "c", "C", "10uF"),
    ("c_1uf", "c 1uF", "c", "C", "1uF"),
    ("c_30pf", "c 30pF", "c", "C", "30pF"),
    ("c_470pf", "c 470pF", "c", "C", "470pF"),
    ("conn1_dm", "conn1 DM", "conn1", "J", "DM"),
    ("conn1_dp", "conn1 DP", "conn1", "J", "DP"),
    ("conn1_gnd", "conn1 GND", "conn1", "J", "GND"),
    ("conn1_vbus", "conn1 VBUS", "conn1", "J", "VBUS"),
    (
        "conn20_header_1",
        "conn20 Header-1",
        "conn20",
        "J",
        "Header-1",
    ),
    (
        "conn20_header_2",
        "conn20 Header-2",
        "conn20",
        "J",
        "Header-2",
    ),
    ("conn3_swd", "conn3 SWD", "conn3", "J", "SWD"),
    ("conn8_exp8", "conn8 EXP8", "conn8", "J", "EXP8"),
    ("cp_2_2uf", "cp 2.2uF", "cp", "C", "2.2uF"),
    ("jumper2_0r", "jumper2 0R", "jumper2", "JP", "0R"),
    (
        "jumper3_btn_sel",
        "jumper3 BTN_SEL",
        "jumper3",
        "JP",
        "BTN_SEL",
    ),
    ("led_blue", "led BLUE", "led", "D", "BLUE"),
    ("led_red", "led RED", "led", "D", "RED"),
    (
        "mic5219_mic5219_3_3ym5",
        "mic5219 MIC5219-3.3YM5",
        "mic5219",
        "U",
        "MIC5219-3.3YM5",
    ),
    ("r_10k", "r 10K", "r", "R", "10K"),
    ("r_1k", "r 1K", "r", "R", "1K"),
    ("r_27r", "r 27R", "r", "R", "27R"),
    ("r_470r", "r 470R", "r", "R", "470R"),
    ("r_5_1k", "r 5.1K", "r", "R", "5.1K"),
    ("r_dnp", "r DNP", "r", "R", "DNP"),
    ("rp2040_rp2040", "rp2040 RP2040", "rp2040", "U", "RP2040"),
    (
        "schottky_bs5817ws",
        "schottky BS5817WS",
        "schottky",
        "D",
        "BS5817WS",
    ),
    (
        "sw_push_uk_b0273_sp25y",
        "sw_push UK-B0273-SP25Y",
        "sw_push",
        "SW",
        "UK-B0273-SP25Y",
    ),
    (
        "usb_c_usb_c_2_0",
        "usb_c USB-C-2.0",
        "usb_c",
        "J",
        "USB-C-2.0",
    ),
    (
        "usb_micro_usb_b_micro",
        "usb_micro USB_B_Micro",
        "usb_micro",
        "J",
        "USB_B_Micro",
    ),
    (
        "w25q_w25q32jvssiq",
        "w25q W25Q32JVSSIQ",
        "w25q",
        "U",
        "W25Q32JVSSIQ",
    ),
    (
        "xtal4_yic_12m20p2_xt324_10_10",
        "xtal4 YIC-12M20P2/XT324-10/10",
        "xtal4",
        "Y",
        "YIC-12M20P2/XT324-10/10",
    ),
];

/// The parts: `(refdes, library component)`.
const PARTS: &[(&str, &str)] = &[
    ("C1", "c_0_1uf"),
    ("C2", "c_0_1uf"),
    ("C3", "c_0_1uf"),
    ("C4", "c_1uf"),
    ("C5", "c_1uf"),
    ("C6", "c_0_1uf"),
    ("C7", "c_0_1uf"),
    ("C8", "c_0_1uf"),
    ("C9", "c_0_1uf"),
    ("C10", "c_0_1uf"),
    ("C11", "c_0_1uf"),
    ("C12", "c_0_1uf"),
    ("C13", "c_10uf"),
    ("C14", "cp_2_2uf"),
    ("C15", "c_10uf"),
    ("C16", "c_470pf"),
    ("C17", "c_30pf"),
    ("C18", "c_30pf"),
    ("C19", "c_1uf"),
    ("D1", "schottky_bs5817ws"),
    ("D2", "led_blue"),
    ("D3", "led_red"),
    ("J1", "conn8_exp8"),
    ("J2", "usb_c_usb_c_2_0"),
    ("J3", "conn20_header_1"),
    ("J4", "conn20_header_2"),
    ("J5", "conn3_swd"),
    ("J6", "usb_micro_usb_b_micro"),
    ("J7", "conn1_vbus"),
    ("J8", "conn1_dm"),
    ("J9", "conn1_dp"),
    ("J10", "conn1_gnd"),
    ("JP1", "jumper2_0r"),
    ("JP4", "jumper2_0r"),
    ("JP5", "jumper2_0r"),
    ("JP6", "jumper3_btn_sel"),
    ("R1", "r_dnp"),
    ("R2", "r_1k"),
    ("R3", "r_5_1k"),
    ("R4", "r_5_1k"),
    ("R5", "r_27r"),
    ("R6", "r_27r"),
    ("R7", "r_470r"),
    ("R8", "r_470r"),
    ("R9", "r_10k"),
    ("R10", "r_1k"),
    ("R11", "r_5_1k"),
    ("SW1", "sw_push_uk_b0273_sp25y"),
    ("SW2", "sw_push_uk_b0273_sp25y"),
    ("U1", "w25q_w25q32jvssiq"),
    ("U2", "mic5219_mic5219_3_3ym5"),
    ("U3", "rp2040_rp2040"),
    ("Y1", "xtal4_yic_12m20p2_xt324_10_10"),
];

/// Every connection: `(<part>/<pin>, net)`, a rail as `rail:<name>`.
const WIRES: &[(&str, &str)] = &[
    ("c1/1", "rail:+3V3"),
    ("c1/2", "rail:GND"),
    ("c2/1", "rail:+1V1"),
    ("c2/2", "rail:GND"),
    ("c3/1", "rail:+1V1"),
    ("c3/2", "rail:GND"),
    ("c4/1", "rail:+1V1"),
    ("c4/2", "rail:GND"),
    ("c5/1", "rail:+3V3"),
    ("c5/2", "rail:GND"),
    ("c6/1", "rail:+3V3"),
    ("c6/2", "rail:GND"),
    ("c7/1", "rail:+3V3"),
    ("c7/2", "rail:GND"),
    ("c8/1", "rail:+3V3"),
    ("c8/2", "rail:GND"),
    ("c9/1", "rail:+3V3"),
    ("c9/2", "rail:GND"),
    ("c10/1", "rail:+3V3"),
    ("c10/2", "rail:GND"),
    ("c11/1", "rail:+3V3"),
    ("c11/2", "rail:GND"),
    ("c12/1", "rail:+3V3"),
    ("c12/2", "rail:GND"),
    ("c13/1", "rail:VSYS"),
    ("c13/2", "rail:GND"),
    ("c14/1", "rail:+3V3"),
    ("c14/2", "rail:GND"),
    ("c15/1", "rail:+3V3"),
    ("c15/2", "rail:GND"),
    ("c16/1", "N_C16_1"),
    ("c16/2", "rail:GND"),
    ("c17/1", "XIN"),
    ("c17/2", "rail:GND"),
    ("c18/1", "N_C18_1"),
    ("c18/2", "rail:GND"),
    ("c19/1", "RUN"),
    ("c19/2", "rail:GND"),
    ("d1/1", "rail:VSYS"),
    ("d1/2", "rail:VBUS"),
    ("d2/1", "N_D2_1"),
    ("d2/2", "N_D2_2"),
    ("d3/1", "N_D3_1"),
    ("d3/2", "rail:+3V3"),
    ("j1/1", "rail:GND"),
    ("j1/2", "rail:+3V3"),
    ("j1/3", "GPIO12"),
    ("j1/4", "GPIO13"),
    ("j1/5", "GPIO15"),
    ("j1/6", "GPIO14"),
    ("j1/7", "GPIO8"),
    ("j1/8", "GPIO9"),
    ("j2/A1", "rail:GND"),
    ("j2/A12", "rail:GND"),
    ("j2/A4", "rail:VBUS"),
    ("j2/A5", "N_J2_A5"),
    ("j2/A6", "USB_D+"),
    ("j2/A7", "USB_D-"),
    ("j2/A9", "rail:VBUS"),
    ("j2/B1", "rail:GND"),
    ("j2/B12", "rail:GND"),
    ("j2/B4", "rail:VBUS"),
    ("j2/B5", "N_J2_B5"),
    ("j2/B6", "USB_D+"),
    ("j2/B7", "USB_D-"),
    ("j2/B9", "rail:VBUS"),
    ("j2/S1", "rail:GND"),
    ("j3/1", "GPIO0"),
    ("j3/10", "GPIO9"),
    ("j3/11", "GPIO10"),
    ("j3/12", "GPIO11"),
    ("j3/13", "GPIO12"),
    ("j3/14", "GPIO13"),
    ("j3/15", "GPIO14"),
    ("j3/16", "GPIO15"),
    ("j3/17", "GPIO16"),
    ("j3/18", "GPIO17"),
    ("j3/19", "GPIO18"),
    ("j3/2", "GPIO1"),
    ("j3/20", "rail:GND"),
    ("j3/3", "GPIO2"),
    ("j3/4", "GPIO3"),
    ("j3/5", "GPIO4"),
    ("j3/6", "GPIO5"),
    ("j3/7", "GPIO6"),
    ("j3/8", "GPIO7"),
    ("j3/9", "GPIO8"),
    ("j4/1", "rail:VBUS"),
    ("j4/10", "GPIO27"),
    ("j4/11", "GPIO26"),
    ("j4/12", "rail:GND"),
    ("j4/13", "GPIO25"),
    ("j4/14", "GPIO24"),
    ("j4/15", "GPIO23"),
    ("j4/16", "GPIO22"),
    ("j4/17", "GPIO21"),
    ("j4/18", "GPIO20"),
    ("j4/19", "GPIO19"),
    ("j4/2", "rail:VSYS"),
    ("j4/20", "rail:GND"),
    ("j4/3", "rail:GND"),
    ("j4/4", "V_EN"),
    ("j4/5", "rail:+3V3"),
    ("j4/6", "AREF"),
    ("j4/7", "RUN"),
    ("j4/8", "GPIO29"),
    ("j4/9", "GPIO28"),
    ("j5/1", "SWCLK"),
    ("j5/2", "rail:GND"),
    ("j5/3", "SWDIO"),
    ("j6/1", "rail:VBUS"),
    ("j6/2", "USB_D-"),
    ("j6/3", "USB_D+"),
    ("j6/5", "rail:GND"),
    ("j6/6", "rail:GND"),
    ("j7/1", "rail:VBUS"),
    ("j8/1", "USB_D-"),
    ("j9/1", "USB_D+"),
    ("j10/1", "rail:GND"),
    ("jp1/1", "AREF"),
    ("jp1/2", "rail:+3V3"),
    ("jp4/1", "rail:VSYS"),
    ("jp4/2", "V_EN"),
    ("jp5/1", "GPIO25"),
    ("jp5/2", "N_D2_2"),
    ("jp6/1", "USB_BOOT"),
    ("jp6/2", "N_JP6_2"),
    ("jp6/3", "GPIO2"),
    ("r1/1", "rail:+3V3"),
    ("r1/2", "QSPI_SS"),
    ("r2/1", "QSPI_SS"),
    ("r2/2", "USB_BOOT"),
    ("r3/1", "N_J2_A5"),
    ("r3/2", "rail:GND"),
    ("r4/1", "N_J2_B5"),
    ("r4/2", "rail:GND"),
    ("r5/1", "USB_D+"),
    ("r5/2", "N_R5_2"),
    ("r6/1", "USB_D-"),
    ("r6/2", "N_R6_2"),
    ("r7/1", "rail:GND"),
    ("r7/2", "N_D2_1"),
    ("r8/1", "rail:GND"),
    ("r8/2", "N_D3_1"),
    ("r9/1", "rail:+3V3"),
    ("r9/2", "RUN"),
    ("r10/1", "XOUT"),
    ("r10/2", "N_C18_1"),
    ("r11/1", "GPIO2"),
    ("r11/2", "rail:+3V3"),
    ("sw1/1", "RUN"),
    ("sw1/2", "rail:GND"),
    ("sw2/1", "N_JP6_2"),
    ("sw2/2", "rail:GND"),
    ("u1/1", "QSPI_SS"),
    ("u1/2", "QSPI_SD1"),
    ("u1/3", "QSPI_SD2"),
    ("u1/4", "rail:GND"),
    ("u1/5", "QSPI_SD0"),
    ("u1/6", "QSPI_SCLK"),
    ("u1/7", "QSPI_SD3"),
    ("u1/8", "rail:+3V3"),
    ("u2/1", "rail:VSYS"),
    ("u2/2", "rail:GND"),
    ("u2/3", "V_EN"),
    ("u2/4", "N_C16_1"),
    ("u2/5", "rail:+3V3"),
    ("u3/1", "rail:+3V3"),
    ("u3/10", "rail:+3V3"),
    ("u3/11", "GPIO8"),
    ("u3/12", "GPIO9"),
    ("u3/13", "GPIO10"),
    ("u3/14", "GPIO11"),
    ("u3/15", "GPIO12"),
    ("u3/16", "GPIO13"),
    ("u3/17", "GPIO14"),
    ("u3/18", "GPIO15"),
    ("u3/19", "rail:GND"),
    ("u3/2", "GPIO0"),
    ("u3/20", "XIN"),
    ("u3/21", "XOUT"),
    ("u3/22", "rail:+3V3"),
    ("u3/23", "rail:+1V1"),
    ("u3/24", "SWCLK"),
    ("u3/25", "SWDIO"),
    ("u3/26", "RUN"),
    ("u3/27", "GPIO16"),
    ("u3/28", "GPIO17"),
    ("u3/29", "GPIO18"),
    ("u3/3", "GPIO1"),
    ("u3/30", "GPIO19"),
    ("u3/31", "GPIO20"),
    ("u3/32", "GPIO21"),
    ("u3/33", "rail:+3V3"),
    ("u3/34", "GPIO22"),
    ("u3/35", "GPIO23"),
    ("u3/36", "GPIO24"),
    ("u3/37", "GPIO25"),
    ("u3/38", "GPIO26"),
    ("u3/39", "GPIO27"),
    ("u3/4", "GPIO2"),
    ("u3/40", "GPIO28"),
    ("u3/41", "GPIO29"),
    ("u3/42", "rail:+3V3"),
    ("u3/43", "AREF"),
    ("u3/44", "rail:+3V3"),
    ("u3/45", "rail:+1V1"),
    ("u3/46", "N_R6_2"),
    ("u3/47", "N_R5_2"),
    ("u3/48", "rail:+3V3"),
    ("u3/49", "rail:+3V3"),
    ("u3/5", "GPIO3"),
    ("u3/50", "rail:+1V1"),
    ("u3/51", "QSPI_SD3"),
    ("u3/52", "QSPI_SCLK"),
    ("u3/53", "QSPI_SD0"),
    ("u3/54", "QSPI_SD2"),
    ("u3/55", "QSPI_SD1"),
    ("u3/56", "QSPI_SS"),
    ("u3/57", "rail:GND"),
    ("u3/6", "GPIO4"),
    ("u3/7", "GPIO5"),
    ("u3/8", "GPIO6"),
    ("u3/9", "GPIO7"),
    ("y1/1", "XIN"),
    ("y1/2", "rail:GND"),
    ("y1/3", "N_C18_1"),
    ("y1/4", "rail:GND"),
];

/// Pins marked not connected.
const NC: &[&str] = &["j2/A8", "j2/B8", "j6/4"];

/// The block's nets: `(id, name)`.
const NETS: &[(&str, &str)] = &[
    ("aref", "AREF"),
    ("gpio0", "GPIO0"),
    ("gpio1", "GPIO1"),
    ("gpio10", "GPIO10"),
    ("gpio11", "GPIO11"),
    ("gpio12", "GPIO12"),
    ("gpio13", "GPIO13"),
    ("gpio14", "GPIO14"),
    ("gpio15", "GPIO15"),
    ("gpio16", "GPIO16"),
    ("gpio17", "GPIO17"),
    ("gpio18", "GPIO18"),
    ("gpio19", "GPIO19"),
    ("gpio2", "GPIO2"),
    ("gpio20", "GPIO20"),
    ("gpio21", "GPIO21"),
    ("gpio22", "GPIO22"),
    ("gpio23", "GPIO23"),
    ("gpio24", "GPIO24"),
    ("gpio25", "GPIO25"),
    ("gpio26", "GPIO26"),
    ("gpio27", "GPIO27"),
    ("gpio28", "GPIO28"),
    ("gpio29", "GPIO29"),
    ("gpio3", "GPIO3"),
    ("gpio4", "GPIO4"),
    ("gpio5", "GPIO5"),
    ("gpio6", "GPIO6"),
    ("gpio7", "GPIO7"),
    ("gpio8", "GPIO8"),
    ("gpio9", "GPIO9"),
    ("n_c16_1", "N_C16_1"),
    ("n_c18_1", "N_C18_1"),
    ("n_d2_1", "N_D2_1"),
    ("n_d2_2", "N_D2_2"),
    ("n_d3_1", "N_D3_1"),
    ("n_j2_a5", "N_J2_A5"),
    ("n_j2_b5", "N_J2_B5"),
    ("n_jp6_2", "N_JP6_2"),
    ("n_r5_2", "N_R5_2"),
    ("n_r6_2", "N_R6_2"),
    ("qspi_sclk", "QSPI_SCLK"),
    ("qspi_sd0", "QSPI_SD0"),
    ("qspi_sd1", "QSPI_SD1"),
    ("qspi_sd2", "QSPI_SD2"),
    ("qspi_sd3", "QSPI_SD3"),
    ("qspi_ss", "QSPI_SS"),
    ("run", "RUN"),
    ("swclk", "SWCLK"),
    ("swdio", "SWDIO"),
    ("usb_boot", "USB_BOOT"),
    ("usb_dp", "USB_D+"),
    ("usb_dm", "USB_D-"),
    ("v_en", "V_EN"),
    ("xin", "XIN"),
    ("xout", "XOUT"),
];

/// The rails: `(id, name, voltage)`.
const RAILS: &[(&str, &str, &str)] = &[
    ("v3v3", "+3V3", "3.3 V"),
    ("v1v1", "+1V1", "1.1 V"),
    ("gnd", "GND", ""),
    ("vbus", "VBUS", "5 V"),
    ("vsys", "VSYS", "5 V"),
];

/// The design: rails, one block holding the board, its nets, parts and
/// connections, and the design.
pub fn design() -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (id, name, v) in RAILS {
        let mut a = args(&["rail", "add", name, "--id", id]);
        if !v.is_empty() {
            a.extend(args(&["--voltage", v]));
        }
        out.push(a);
    }
    out.push(args(&["block", "create", "Mitayi Pico", "--id", "mitayi"]));
    for (id, name) in NETS {
        out.push(args(&["net", "add", "mitayi", name, "--id", id]));
    }
    for (refdes, of) in PARTS {
        let id = refdes.to_lowercase();
        out.push(args(&[
            "component",
            "add",
            "mitayi",
            "--of",
            of,
            "--id",
            &id,
            "--refdes",
            refdes,
        ]));
    }
    for (pin, net) in WIRES {
        out.push(args(&["connect", &format!("mitayi/{pin}"), net]));
    }
    for pin in NC {
        out.push(args(&["nc", &format!("mitayi/{pin}")]));
    }
    out.push(args(&[
        "design",
        "create",
        "Mitayi Pico",
        "--root",
        "mitayi",
        "--id",
        "pico",
    ]));
    out
}

/// Every command, the library first.
pub fn script() -> Vec<Vec<String>> {
    let mut out = library();
    out.extend(design());
    out
}
