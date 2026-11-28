//! Footprints by ops, for the tests (ryueda 7.F).

/// `tests/data/kicad/footprints/Ryu_Test.pretty/R_0402.kicad_mod`, drawn
/// by ops: in millimetres, y up (KiCad's y points down, so its y's are
/// negated).
pub const R_0402: &str = r#"[
    {"op": "add", "pad": null, "number": "1", "kind": "smd", "shape": {"type": "roundrect", "ratio_ppm": 250000},
     "at": ["-0.51", 0], "size": ["0.54", "0.64"], "layers": ["copper.top", "mask.top", "paste.top"]},
    {"op": "add", "pad": null, "number": "2", "kind": "smd", "shape": {"type": "roundrect", "ratio_ppm": 250000},
     "at": ["0.51", 0], "size": ["0.54", "0.64"], "layers": ["copper.top", "mask.top", "paste.top"]},
    {"op": "add", "graphic": null, "type": "line", "layer": "silk.top",
     "start": ["-0.153641", "0.38"], "end": ["0.153641", "0.38"], "width": "0.12"},
    {"op": "add", "graphic": null, "type": "rect", "layer": "courtyard.top",
     "start": ["-0.93", "0.47"], "end": ["0.93", "-0.47"], "width": "0.05"},
    {"op": "add", "graphic": null, "type": "text", "layer": "fab.top", "text": "${REFERENCE}",
     "at": [0, 0], "size": ["0.26", "0.26"], "thickness": "0.04"},
    {"op": "add", "field": "reference", "layer": "silk.top", "at": [0, "1.17"], "size": ["1", "1"], "thickness": "0.15"},
    {"op": "add", "field": "value", "layer": "fab.top", "at": [0, "-1.17"], "size": ["1", "1"], "thickness": "0.15"}
]"#;
