//! The symbol lint: a library's symbols judged against the standards that
//! apply to them (`Project::standards_for`). Its findings judge a symbol,
//! never a sheet: each costs the symbol's own score (100 less its
//! findings' points) in `lib check`, and `check` reports them. Every rule
//! carries what it finds, why, how to fix it and its evidence; its weight
//! and severity are `lint.json`'s, as the sheet lint's are.

use ryusch::lint::{Prefs, Rule};

/// The old sheet rule whose judgement `sym_shape` took (plan 2.D):
/// `lint.json` and `lint configure` read it as the new one.
pub const ALIAS: (&str, &str) = ("sch_resistor_style", "sym_shape");

pub const RULES: [Rule; 7] = [
    Rule {
        code: "sym_pin_off_grid",
        weight: 2,
        unit: "a pin",
        what: "A pin's connection point isn't on the standards' grid (50 mil by default) from the symbol's origin.",
        why: "Wires meet pins on the grid; a pin off it takes a jog or a dangling end wherever the part is placed, and the engine places a part by its first pin.",
        fix: "Move the pin onto the grid (the finding's symbol edit ops), or set the grid the library draws to (standards set '{\"grid_nm\": …}').",
        evidence: "KiCad's library convention (KLC S4.1) puts every pin on a 100 mil grid and KiCad's schematic grid is 50 mil; ryusch places parts on the 50 mil grid and its sheet lint judges pins off it relative to each other (sch_pin_off_grid). A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_pin_side",
        weight: 3,
        unit: "a pin",
        what: "A pin stands on a side its class's standards don't put its role on: a supply off the top, ground off the bottom, an input on the right, an output on the left (a regulator's supply runs left to right).",
        why: "Readers expect power from above, ground below and signals flowing left to right; a pin on the wrong side sends its wire across the body or against the flow.",
        fix: "Move the pin to the free place the finding's ops name, or change the class's sides (standards set).",
        evidence: "IEEE 315 and the hand-drawn references put supplies up, ground down, inputs left and outputs right; phase 1's lint already faults sideways power (sch_power_sideways) and signals drawn against the flow, which a pin on the wrong side causes. A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_pin_group",
        weight: 2,
        unit: "a group",
        what: "A function's pins (I2C, SPI, USB, GPIO, by the standards' groups) stand on more than one side or with other pins between them, or a numbered group isn't in order down its side.",
        why: "A bus's pins together read as one interface and their wires run as a bundle; split, each wire goes its own way, and GPIO12 above GPIO3 hides the one being looked for.",
        fix: "Reorder the side's pins as the finding's ops do; bring a split group to one side first.",
        evidence: "Design 348: pins grouped by function, GPIOs in numeric order, as the references' MCUs and KiCad's library draw them. A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_pin_length",
        weight: 1,
        unit: "a pin",
        what: "A pin of a part whose class judges lengths (an IC, a regulator, an op-amp) isn't the standards' length (100 mil by default).",
        why: "Pins of one length line a sheet's wires up at one distance from every body; a long or short pin reads as a different kind of pin.",
        fix: "Set the pin's length keeping where it meets the body (the finding's ops), or the standards' pin_length_nm.",
        evidence: "KiCad's library convention (KLC S4.2) draws pins 100 mil long; every IC of the corpus (tests/support/draw.rs) is drawn so. A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_shape",
        weight: 1,
        unit: "a symbol",
        what: "A resistor is drawn to the other standard than its library's: the IEC box under ANSI standards, or the US zigzag under IEC ones.",
        why: "Two styles of one symbol on a sheet make the reader wonder whether they are different parts.",
        fix: "Redraw the body to the standard (the finding's ops), or set the standards' shapes (standards set '{\"shapes\": \"iec\"}').",
        evidence: "IEEE 315 draws the zigzag, IEC 60617 the box; an organisation keeps to one, ANSI by default (the owner's answer, design 348). Until plan 2.D the sheet lint judged it (sch_resistor_style, which reads as this rule); in round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covered it alone.",
    },
    Rule {
        code: "sym_body_margin",
        weight: 2,
        unit: "a pin",
        what: "A pin's name, as drawn, comes nearer its body's far edge, or another pin's name, than the standards' margin (25 mil by default).",
        why: "A name outside its body reads as a label of the net, and names that touch can't be read: the cause of the sheet lint's sch_pin_name_outside, judged once in the library rather than on every sheet.",
        fix: "Widen the body (the finding's ops move its far edge and the pins on that edge), or shorten the name.",
        evidence: "Every committed reference's symbols hold their pin names inside the body with room to spare; the corpus's symbols are sized so their names fit (tests/support/draw.rs). A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_units",
        weight: 2,
        unit: "a unit",
        what: "A unit of a symbol has more pins than its class draws in one (64 for an IC by default).",
        why: "A body with that many pins fills a sheet on its own and pulls every satellite far from its pin; drawn in units (power, ports, functions), each sits beside the circuit it serves.",
        fix: "Split the symbol into units in its library (symbol edit --units N, then each pin's unit): no op does it for you.",
        evidence: "The references draw large MCUs in units, power apart from I/O; design 143 asks for units past a pin count. A drafting convention; no round has judged it yet.",
    },
];

/// A rule of the symbol lint, by code.
pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// A rule's weight, and the preference that set it if one did.
pub fn weight(prefs: &Prefs, code: &str) -> (i64, Option<String>) {
    match prefs.weights.get(code) {
        Some(w) => (*w, Some(format!("weights.{code} = {w}"))),
        None => (rule(code).map_or(0, |r| r.weight), None),
    }
}
