//! The corpus's first board: a sensor breakout. A BME280 and its
//! decoupling in a block of their own, used from the board's root block
//! with a 4-pin connector, an AP2112K LDO and its capacitors, and the I2C
//! pull-ups, over rails VIN, 3V3 and GND. Its footprints are files (they
//! come from import, plan 0.D); everything else is entered by command.

#![allow(dead_code)]

use serde_json::{Value, json};

/// A footprint's file: `pads` as `(number, x, y, w, h)` in micrometres,
/// SMD unless `drill` (micrometres) is given, with a courtyard round them.
fn footprint(name: &str, pads: &[(&str, i64, i64, i64, i64)], drill: Option<i64>) -> String {
    let um = 1_000;
    let mut list = serde_json::Map::new();
    for (i, (number, x, y, w, h)) in pads.iter().enumerate() {
        let mut pad = json!({
            "number": number,
            "kind": "smd",
            "shape": {"type": "roundrect", "ratio_ppm": 250_000},
            "at": [x * um, y * um],
            "size": [w * um, h * um],
            "layers": ["copper.top", "mask.top", "paste.top"],
        });
        if let Some(d) = drill {
            pad["kind"] = json!("through");
            pad["shape"] = json!({"type": "circle"});
            pad["drill"] = json!({"shape": "round", "diameter_nm": d * um});
            pad["layers"] = json!(["copper.all", "mask.top", "mask.bottom"]);
        }
        list.insert(format!("p{}", i + 1), pad);
    }
    let reach =
        |f: fn(&(&str, i64, i64, i64, i64)) -> i64| pads.iter().map(f).max().unwrap_or(0) + 250;
    let (x, y) = (
        reach(|p| p.1.abs() + p.3 / 2),
        reach(|p| p.2.abs() + p.4 / 2),
    );
    let v = json!({
        "kind": "ryueda.footprint", "format": 1, "name": name, "mount": if drill.is_some() { "through" } else { "smd" },
        "pads": list,
        "graphics": {"g1": {"layer": "courtyard.top", "type": "rect",
            "start": [-x * um, y * um], "end": [x * um, -y * um], "width_nm": 50_000}},
    });
    ryueda::canonical::to_string(&v)
}

/// The library's footprints, by path within the project.
pub fn footprints() -> Vec<(String, String)> {
    let two = [("1", -480, 0, 560, 620), ("2", 480, 0, 560, 620)];
    vec![
        ("r_0402", footprint("R_0402_1005Metric", &two, None)),
        ("c_0402", footprint("C_0402_1005Metric", &two, None)),
        (
            "sot23_5",
            footprint(
                "SOT-23-5",
                &[
                    ("1", -1_140, 950, 1_060, 650),
                    ("2", -1_140, 0, 1_060, 650),
                    ("3", -1_140, -950, 1_060, 650),
                    ("4", 1_140, -950, 1_060, 650),
                    ("5", 1_140, 950, 1_060, 650),
                ],
                None,
            ),
        ),
        (
            "lga8",
            footprint(
                "Bosch_LGA-8_2.5x2.5mm",
                &[
                    ("1", -975, 975, 500, 350),
                    ("2", -975, 325, 500, 350),
                    ("3", -975, -325, 500, 350),
                    ("4", -975, -975, 500, 350),
                    ("5", 975, -975, 500, 350),
                    ("6", 975, -325, 500, 350),
                    ("7", 975, 325, 500, 350),
                    ("8", 975, 975, 500, 350),
                ],
                None,
            ),
        ),
        (
            "pinheader_1x04",
            footprint(
                "PinHeader_1x04_P2.54mm_Vertical",
                &[
                    ("1", 0, 3_810, 1_700, 1_700),
                    ("2", 0, 1_270, 1_700, 1_700),
                    ("3", 0, -1_270, 1_700, 1_700),
                    ("4", 0, -3_810, 1_700, 1_700),
                ],
                Some(1_000),
            ),
        ),
    ]
    .into_iter()
    .map(|(id, text)| (format!("library/footprints/{id}.json"), text))
    .collect()
}

/// A symbol's pins as `symbol create` ops: `(number, name, type, side)`,
/// 2.54 mm apart down each side.
fn pins(list: &[(&str, &str, &str, &str)]) -> String {
    let ops: Vec<Value> = list
        .iter()
        .enumerate()
        .map(|(i, (number, name, ty, side))| {
            let x = if *side == "left" { "-7.62 mm" } else { "7.62 mm" };
            let y = format!("{} um", 2_540 * (4 - i % 4));
            json!({"op": "add", "pin": number, "name": name, "type": ty, "at": [x, y], "side": side})
        })
        .chain(std::iter::once(json!(
            {"op": "add", "graphic": "rect", "start": ["-5.08 mm", "12.7 mm"], "end": ["5.08 mm", "-2.54 mm"], "fill": "body"}
        )))
        .collect();
    serde_json::to_string(&ops).unwrap()
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// The library: symbols and library components, by command.
pub fn library() -> Vec<Vec<String>> {
    let bme280 = pins(&[
        ("1", "GND", "power_in", "left"),
        ("2", "CSB", "input", "left"),
        ("3", "SDI", "bidirectional", "left"),
        ("4", "SCK", "input", "left"),
        ("5", "SDO", "bidirectional", "right"),
        ("6", "VDDIO", "power_in", "right"),
        ("7", "GND", "power_in", "right"),
        ("8", "VDD", "power_in", "right"),
    ]);
    let ldo = pins(&[
        ("1", "VIN", "power_in", "left"),
        ("2", "GND", "power_in", "left"),
        ("3", "EN", "input", "left"),
        ("4", "NC", "no_connect", "right"),
        ("5", "VOUT", "power_out", "right"),
    ]);
    let two = pins(&[
        ("1", "~", "passive", "left"),
        ("2", "~", "passive", "right"),
    ]);
    let conn = pins(&[
        ("1", "Pin_1", "passive", "left"),
        ("2", "Pin_2", "passive", "left"),
        ("3", "Pin_3", "passive", "left"),
        ("4", "Pin_4", "passive", "left"),
    ]);
    vec![
        args(&[
            "symbol", "create", "BME280", "--id", "bme280", "--ops", &bme280,
        ]),
        args(&[
            "symbol",
            "create",
            "AP2112K-3.3",
            "--id",
            "ldo",
            "--ops",
            &ldo,
        ]),
        args(&["symbol", "create", "R", "--id", "r", "--ops", &two]),
        args(&["symbol", "create", "C", "--id", "c", "--ops", &two]),
        args(&[
            "symbol",
            "create",
            "Conn_01x04",
            "--id",
            "conn4",
            "--ops",
            &conn,
        ]),
        args(&[
            "libcomp",
            "create",
            "BME280",
            "--symbol",
            "bme280",
            "--prefix",
            "U",
            "--footprint",
            "lga8",
            "--manufacturer",
            "Bosch",
            "--mpn",
            "BME280",
        ]),
        args(&[
            "libcomp",
            "create",
            "AP2112K-3.3",
            "--id",
            "ap2112k",
            "--symbol",
            "ldo",
            "--prefix",
            "U",
            "--value",
            "3.3 V",
            "--footprint",
            "sot23_5",
            "--mpn",
            "AP2112K-3.3TRG1",
            "--offer",
            "LCSC=C51118",
        ]),
        args(&[
            "libcomp",
            "create",
            "R 4k7",
            "--symbol",
            "r",
            "--prefix",
            "R",
            "--value",
            "4k7",
            "--footprint",
            "r_0402",
        ]),
        args(&[
            "libcomp",
            "create",
            "C 100n",
            "--symbol",
            "c",
            "--prefix",
            "C",
            "--value",
            "100nF",
            "--footprint",
            "c_0402",
        ]),
        args(&[
            "libcomp",
            "create",
            "C 1u",
            "--symbol",
            "c",
            "--prefix",
            "C",
            "--value",
            "1uF",
            "--footprint",
            "c_0402",
        ]),
        args(&[
            "libcomp",
            "create",
            "Conn 1x04",
            "--id",
            "conn4",
            "--symbol",
            "conn4",
            "--prefix",
            "J",
            "--footprint",
            "pinheader_1x04",
        ]),
    ]
}

/// The design: rails, the sensor's block, the root block and the design.
pub fn design() -> Vec<Vec<String>> {
    let mut out = vec![
        args(&["rail", "add", "VIN", "--id", "vin", "--voltage", "5 V"]),
        args(&["rail", "add", "3V3", "--id", "v3v3", "--voltage", "3.3 V"]),
        args(&["rail", "add", "GND", "--id", "gnd"]),
        args(&["block", "create", "Sensor", "--id", "sensor"]),
        args(&[
            "block",
            "edit",
            "sensor",
            "--add-port",
            "SDA:bidirectional",
            "--add-port",
            "SCL:bidirectional",
        ]),
        args(&[
            "block",
            "edit",
            "sensor",
            "--port-voltage",
            "SDA=3.3 V",
            "--port-voltage",
            "SCL=3.3 V",
        ]),
        args(&["component", "add", "sensor", "--of", "bme280", "--id", "u1"]),
        args(&["component", "add", "sensor", "--of", "c_100n", "--id", "c1"]),
        args(&["block", "create", "Breakout", "--id", "breakout"]),
        args(&[
            "net",
            "add",
            "breakout",
            "SDA",
            "--id",
            "sda",
            "--voltage",
            "3.3 V",
        ]),
        args(&[
            "net",
            "add",
            "breakout",
            "SCL",
            "--id",
            "scl",
            "--voltage",
            "3.3 V",
        ]),
        args(&[
            "component",
            "add",
            "breakout",
            "--of",
            "conn4",
            "--id",
            "j1",
        ]),
        args(&[
            "component",
            "add",
            "breakout",
            "--of",
            "ap2112k",
            "--id",
            "u2",
        ]),
        args(&["component", "add", "breakout", "--of", "c_1u", "--id", "c2"]),
        args(&["component", "add", "breakout", "--of", "c_1u", "--id", "c3"]),
        args(&[
            "component",
            "add",
            "breakout",
            "--of",
            "r_4k7",
            "--id",
            "r1",
        ]),
        args(&[
            "component",
            "add",
            "breakout",
            "--of",
            "r_4k7",
            "--id",
            "r2",
        ]),
    ];
    let wires = [
        ("sensor/u1/1", "rail:GND"),
        ("sensor/u1/2", "rail:3V3"),
        ("sensor/u1/3", "port:SDA"),
        ("sensor/u1/4", "port:SCL"),
        ("sensor/u1/5", "rail:GND"),
        ("sensor/u1/6", "rail:3V3"),
        ("sensor/u1/7", "rail:GND"),
        ("sensor/u1/8", "rail:3V3"),
        ("sensor/c1/1", "rail:3V3"),
        ("sensor/c1/2", "rail:GND"),
        ("breakout/j1/1", "rail:VIN"),
        ("breakout/j1/2", "rail:GND"),
        ("breakout/j1/3", "SCL"),
        ("breakout/j1/4", "SDA"),
        ("breakout/u2/1", "rail:VIN"),
        ("breakout/u2/2", "rail:GND"),
        ("breakout/u2/3", "rail:VIN"),
        ("breakout/u2/5", "rail:3V3"),
        ("breakout/c2/1", "rail:VIN"),
        ("breakout/c2/2", "rail:GND"),
        ("breakout/c3/1", "rail:3V3"),
        ("breakout/c3/2", "rail:GND"),
        ("breakout/r1/1", "SDA"),
        ("breakout/r1/2", "rail:3V3"),
        ("breakout/r2/1", "SCL"),
        ("breakout/r2/2", "rail:3V3"),
    ];
    for (pin, net) in wires {
        out.push(args(&["connect", pin, net]));
    }
    out.push(args(&["nc", "breakout/u2/4"]));
    out.push(args(&[
        "instance", "add", "breakout", "--of", "sensor", "--id", "s1", "--port", "SDA=SDA",
        "--port", "SCL=SCL",
    ]));
    out.push(args(&[
        "design", "create", "Breakout", "--root", "breakout", "--id", "board",
    ]));
    out
}

/// Every command, the library first.
pub fn script() -> Vec<Vec<String>> {
    let mut out = library();
    out.extend(design());
    out
}
