//! Lint: soft findings about a layout, with stable codes, and a score per
//! sheet out of 100. Thresholds are fixed here (tuned on the corpus in
//! plan 1.A) and named in each finding's details. Each finding costs its
//! rule's weight per unit of it; a project's preferences ([`Prefs`]) set
//! weights, severities (`off` drops a rule) and styles, and a finding they
//! drove says so.

use crate::engine::Outcome;
use crate::geom::{GRID, Nm, Place, Rect, Side};
use crate::input::{Block, Flow, NetKind, PartKind};
use crate::label;
use crate::layout::{Axis, IdiomChoice, IdiomKind, Instruction, LabelKind, NetStyle};
use crate::route;
use crate::shape::{Shape, pins_on_grid};
use crate::text;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// A wire run longer than this share of the sheet's width.
pub const LONG_WIRE: (Nm, Nm) = (1, 4);
/// A net's wires longer in all than this share of the width read better
/// as labels.
pub const WIRE_NOT_LABEL: (Nm, Nm) = (1, 5);
/// A net whose wires cross more than this many others reads better as
/// labels.
pub const MANY_CROSSINGS: usize = 4;
/// A labelled net whose pins lie closer than this (a spanning tree's
/// length) could be a short wire.
pub const SHORT_NET: Nm = 12 * GRID;
/// Bends past the fewest a net could have, before it is a finding.
pub const EXTRA_BENDS: usize = 3;
/// What share of a sheet's area (per mille) may be ink before it is
/// dense.
pub const DENSITY: i64 = 300;

/// Per mille of a sheet's 10 mm cells holding ink below which
/// `sch_paper_fit` judges a sheet larger than its series' smallest paper
/// too empty: under the least of the references' fills, ivconverter's 115
/// (`measure.py`, plan 2.A's Task 1; the others 164 to 336).
pub const FILL_MIN: i64 = 100;

/// The cell `fill` counts in, as `measure.py` does: 10 mm.
pub const CELL: Nm = 10_000_000;
/// A signal running more than this far right to left is against the flow.
pub const AGAINST: Nm = 4 * GRID;
/// An idiom's part further than this from the part it serves is drawn
/// apart from its shape.
pub const IDIOM_REACH: Nm = 16 * GRID;

/// What a supply not pointing up, or ground not pointing down, costs.
pub const POWER_SIDEWAYS: i64 = 3;
/// What a net label or port reading up or down the page costs: it should
/// never be drawn so.
pub const LABEL_VERTICAL: i64 = 15;

/// What a bypass capacitor drawn apart from the pin it serves costs.
pub const BYPASS_APART: i64 = 3;
/// What a part joined to nothing by a wire costs, where a wire could have
/// joined it: schematics are read along their wires.
pub const FLOATING: i64 = 8;
/// What a port's flag whose wire turns costs (besides its jogs).
pub const PORT_BENT: i64 = 3;
/// What a pin's name past its body, or into another's, costs.
pub const NAME_OUTSIDE: i64 = 3;
/// What a jog costs: a wire stepping aside and carrying on the way it
/// went, where it could have run straight.
pub const JOG: i64 = 2;

/// A rule of the lint: its code (stable, never reused), its default weight
/// (points per unit) and unit, and what it teaches. `lint.get` answers the
/// catalogue whole and `ryueda lint --explain CODE` one rule.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Rule {
    pub code: &'static str,
    pub weight: i64,
    pub unit: &'static str,
    /// What the rule finds, in the drawing's terms.
    pub what: &'static str,
    /// What a reader loses where it is found.
    pub why: &'static str,
    /// How to fix it: the instruction to send, as text.
    pub fix: &'static str,
    /// What the rule rests on: the references' habits (measured by
    /// `scripts/schematic-refs/measure.py`), the agent rounds' findings,
    /// conventions and standards, by name.
    pub evidence: &'static str,
}

/// The references' drawing habits the catalogue cites, measured on the
/// committed KiCad references by `scripts/schematic-refs/measure.py` and
/// recorded in ryueda's `tests/schematic_corpus/manifest.txt` (plan 1.D's
/// Task 12): crossings within each circuit's cut buck 0, rp2040 1,
/// breakout 3, frontend 5, ethernet 33; power symbols up/down/sideways
/// buck 10/17/2, Mitayi 12/17/8, BalDiffAmp 1/2/2, ivconverter 1/2/1,
/// amp 0/1/12.
///
/// Every rule, its default weight and what it teaches. Codes are stable
/// and never reused.
pub const RULES: &[Rule] = &[
    Rule {
        code: "sch_crossings",
        weight: 2,
        unit: "a crossing",
        what: "Two wires of different nets cross on the sheet.",
        why: "At a crossing the eye must check whether the wires join, and following either wire past it takes longer.",
        fix: "Move a part so the wires needn't cross (beside X Y left|right|above|below, or a shift move), or draw one net by labels (net N label).",
        evidence: "Within the cut each corpus circuit takes, the references draw few crossings: the buck 0, Mitayi-Pico-D1 1, the BME280 breakout 3, the front end's three sheets 5, and only the ESP32-PoE's dense Ethernet panel 33. Design 236's quality bar holds each circuit to 1.5 times its reference's. In round 2's blind review (plan 1.D, three judges a pair), where this rule's points fell the judges scored crossings higher: the breakout's 4 to 2 (+2.7 of 5) and the Ethernet PHY's 10 to 8 (+1.7).",
    },
    Rule {
        code: "sch_bends",
        weight: 1,
        unit: "a bend past the fewest",
        what: "A net's wires bend more than three times past the fewest bends its pins' places allow.",
        why: "Every bend is a turn the eye has to follow; a wire that winds hides where it goes.",
        fix: "Line the parts up so the wire runs straight (align X... row|column, or align on pins: align U1.OUT R2.1 row).",
        evidence: "A drafting convention: wires run straight and turn only where they must. measure.py doesn't count bends; plan 1.D's blind review weighs this rule. In round 2's blind review (plan 1.D, three judges a pair), the judges preferred the breakout's straight I2C wires to the dog-legs before them, 3 to 0.",
    },
    Rule {
        code: "sch_long_wire",
        weight: 4,
        unit: "a long wire",
        what: "One run of a net's wire is longer than a quarter of the sheet's width.",
        why: "A long wire drags the eye across the sheet and through everything between its ends; a reader loses which net it is half way.",
        fix: "Draw the net by labels (net N label), or bring its parts together (beside X Y left|right|above|below).",
        evidence: "The references label what travels far: Mitayi-Pico-D1 uses 106 net labels, the ESP32-PoE board 92, and the buck 17 global labels, rather than wires across the sheet. In round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covers this rule alone.",
    },
    Rule {
        code: "sch_label_where_wire",
        weight: 2,
        unit: "a net",
        what: "A net drawn by labels whose labelled pins all lie within 12 grid steps of each other.",
        why: "A label sends the reader searching the sheet for its twin; where the twin is beside it, a short wire says the same at a glance.",
        fix: "Ask for the net's wires (net N wire), or bring its parts together (beside X Y left|right|above|below).",
        evidence: "Round 1's sessions asked for wires between neighbours and found the request dropped (findings F6 to F8); labels are for what travels (see sch_long_wire). In round 2's blind review (plan 1.D, three judges a pair), the judges' labels-against-wires score rose with the buck's local wiring (+1.3 of 5), but also on the Ethernet PHY, where the label rules' points rose from 52 to 72 (+1.0).",
    },
    Rule {
        code: "sch_wire_where_label",
        weight: 2,
        unit: "a net",
        what: "A signal net's wires run longer than a fifth of the sheet's width, or cross more than four other wires.",
        why: "A wire that long or that tangled is harder to follow than two labels of the same name; it also blocks the space it crosses.",
        fix: "Draw the net by labels (net N label).",
        evidence: "Mitayi-Pico-D1 and the ESP32-PoE board label their buses and far signals (106 and 92 net labels) instead of routing them across the sheet; a rail's wires are never judged, since a rail is drawn by ports. In round 2's blind review (plan 1.D, three judges a pair), the judges preferred the Ethernet PHY's strap resistors joined by labels in one row to the long looping wires before them, 3 to 0.",
    },
    Rule {
        code: "sch_density",
        weight: 1,
        unit: "10 per mille of ink past the threshold",
        what: "The sheet's ink (bodies, wires and text over its area) passes 300 per mille.",
        why: "A crowded sheet leaves no white space between groups, so circuits run into each other and text is hard to read.",
        fix: "Change the layout by hand: no instruction says it; move a group to a sheet of its own (sheet X... NAME).",
        evidence: "Design 236's Readability section asks for groups with space between them. The threshold is a house style, tunable by weight. In round 2's blind review (plan 1.D, three judges a pair), the judges scored paper fit lowest of all (1 to 2 of 5 for our sheets, 4 for the references) while this rule found nothing on any sheet: it sees crowding, not the empty paper the judges faulted.",
    },
    Rule {
        code: "sch_flow_reversed",
        weight: 3,
        unit: "a net",
        what: "A signal runs from right to left: its driver stands more than four steps right of what it drives, with no flow instruction.",
        why: "Schematics are read left to right, inputs to outputs; a reversed signal makes the reader trace backwards.",
        fix: "Move the driven part to the driver's right (beside X Y right), or say the flow is meant (flow X reversed).",
        evidence: "The left-to-right signal flow convention of IEEE 315 and ASME Y14.44's schematic practice. In round 2's blind review (plan 1.D, three judges a pair), no flow rule found anything before or after, yet the judges' flow score rose by up to 1 of 5: what they read as flow these rules don't yet measure.",
    },
    Rule {
        code: "sch_pin_off_grid",
        weight: 1,
        unit: "a part",
        what: "A symbol's pin ends off the 50 mil grid.",
        why: "A pin off the grid can't be met squarely by a wire on the grid, which then jogs or misses it.",
        fix: "Change the symbol in its library so its pins land on the 50 mil grid: no instruction says it.",
        evidence: "KiCad's libraries put every pin on the 50 mil grid and Eagle's on the 100 mil grid; wires are drawn on it. In round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covers this rule alone.",
    },
    Rule {
        code: "sch_idiom_missed",
        weight: 3,
        unit: "a part",
        what: "A part of an idiom (a decoupling capacitor, a pull resistor, a divider, a crystal's capacitors) stands more than 16 grid steps from the part it serves.",
        why: "An idiom reads at a glance only drawn together; scattered, the reader must work out again what each part is for.",
        fix: "Bring the part back (near X P, with P the pin it serves), or re-form the idiom (idiom X KIND).",
        evidence: "Design 236 names the idioms the engine draws and their canonical shapes, after the drafting convention of drawing decoupling at its pin and pull resistors beside the pin they pull. In round 2's blind review (plan 1.D, three judges a pair), the idiom rules' points fell on the Ethernet PHY (42 to 30) and the judges' idioms score rose (+0.7); against the hand-drawn references the judges scored our idioms 2 to 2.3 of 5 and the references' 3.7 to 5.",
    },
    Rule {
        code: "sch_power_sideways",
        weight: POWER_SIDEWAYS,
        unit: "a port",
        what: "A supply's port points anywhere but up.",
        why: "Supplies up and grounds down let a reader see power at a glance; a sideways supply reads like a signal.",
        fix: "Turn the part whose pin takes the port (orient X 0|90|180|270), or leave it if the house style allows it.",
        evidence: "The buck draws 10 supplies up against 2 sideways and Mitayi-Pico-D1 12 up against 8, but frontend's amp draws 12 of its 13 power symbols sideways: a house style, tunable by weight. In round 2's blind review (plan 1.D, three judges a pair), the judges named supplies up and grounds down among what made the preferred sheet read well.",
    },
    Rule {
        code: "sch_ground_sideways",
        weight: POWER_SIDEWAYS,
        unit: "a port",
        what: "A ground's port points anywhere but down.",
        why: "Grounds down mark the bottom of each circuit; a sideways ground reads like a signal or a supply.",
        fix: "Turn the part whose pin takes the port (orient X 0|90|180|270).",
        evidence: "Grounds point down in the references 39 times against 25 sideways symbols of either kind (buck 17 down, Mitayi-Pico-D1 17 down); the convention of IEEE 315. In round 2's blind review (plan 1.D, three judges a pair), the judges named supplies up and grounds down among what made the preferred sheet read well.",
    },
    Rule {
        code: "sch_label_vertical",
        weight: LABEL_VERTICAL,
        unit: "a label or port",
        what: "A net label or a port's flag reads up or down the page.",
        why: "Text turned on its side must be read with the head tilted; it is the slowest thing on a sheet to read.",
        fix: "Turn the part whose pin takes the label so the pin points sideways (orient X 0|90|180|270).",
        evidence: "Text on its side is read with the head turned; no committed reference turns a label's text. It carries the catalogue's highest weight. In round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covers this rule alone.",
    },
    Rule {
        code: "sch_jog",
        weight: JOG,
        unit: "a jog",
        what: "A wire steps aside and carries on the way it went (a Z of two turns) where it could run straight.",
        why: "A jog looks like a join or a change of net; it adds two bends for nothing.",
        fix: "Line the ends up (align on pins: align U1.EN R1.1 row, or a shift move of a step or two).",
        evidence: "1.B's review of the buck named the EN wire's jog under VIN's capacitors; a jog of two turns is never needed where the ends can be lined up. In round 2's blind review (plan 1.D, three judges a pair), the judges preferred the breakout's straight I2C wires to the dog-legs before them, 3 to 0.",
    },
    Rule {
        code: "sch_bypass_apart",
        weight: BYPASS_APART,
        unit: "a capacitor",
        what: "A decoupling capacitor isn't wired to the pin it serves (it reaches its supply by a port of its own).",
        why: "A bypass capacitor only reads as its pin's when drawn on that pin's wire; apart, the reader can't tell which pin it decouples.",
        fix: "Re-form the decoupling (idiom C decoupling), or bring it to its pin (near C U.PIN).",
        evidence: "The drafting convention design 236 follows: a bypass capacitor stands on its pin's supply wire, its rail's one port on that wire. In round 2's blind review (plan 1.D, three judges a pair), judges faulted decoupling capacitors standing away from the pins they serve on the front end and the Ethernet PHY.",
    },
    Rule {
        code: "sch_pin_name_outside",
        weight: NAME_OUTSIDE,
        unit: "a pin",
        what: "A pin's name runs past its symbol's body, or into another pin's name.",
        why: "A name outside its body reads as a label of the net, and overlapping names can't be read at all.",
        fix: "Widen the symbol in its library so its names fit: no instruction says it.",
        evidence: "Every committed reference's symbols hold their pin names inside the body; the corpus's own symbols are sized so their names fit (tests/support/draw.rs). In round 2's blind review (plan 1.D, three judges a pair), this rule found nothing on any sheet, and the judges scored text alike before and after.",
    },
    Rule {
        code: "sch_port_bent",
        weight: PORT_BENT,
        unit: "a port",
        what: "A port's flag reaches its pin by a wire that turns.",
        why: "A flag is the block's edge; a bent wire behind it hides which pin the signal enters by.",
        fix: "Line the flag up with its pin (align port:N U.PIN row).",
        evidence: "A drafting convention: a flag sits on its pin's line, so the signal enters by a straight wire. In round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covers this rule alone.",
    },
    Rule {
        code: "sch_floating",
        weight: FLOATING,
        unit: "a part",
        what: "A part's signal pins all reach their nets by labels, though one of those nets has another pin on its sheet and none leaves it, or a pin on a net that leaves the sheet is wired to neither its own label nor the net's.",
        why: "A part joined to nothing by a wire floats: the reader can't see what it belongs to without matching label names across the sheet.",
        fix: "Stand it beside the part it serves (near X P, with P the pin on its net), or ask for the net's wires (net N wire).",
        evidence: "Round 1's sessions and work item 4530 named floating parts as the corpus's main fault (35 laid out from nothing before plan 1.D, 16 after); a local part reads as part of its circuit only when wired to it. In round 2's blind review (plan 1.D, three judges a pair), the judges preferred the RP2040 split by function onto three sheets 3 to 0 though this rule's points rose from 32 to 80: they don't fault a label on a net that leaves its sheet, so since plan 2.A (design 348) such a label is the net's off-sheet connector and joins its pin.",
    },
    Rule {
        code: "sch_resistor_style",
        weight: 1,
        unit: "a resistor",
        what: "A resistor is drawn in the style the project doesn't prefer (the US zigzag or the IEC box).",
        why: "Two styles of one symbol on a sheet make the reader wonder whether they are different parts.",
        fix: "Draw the resistor's symbol in the preferred style in its library, or prefer the other style (lint configure --prefer resistor=us|iec).",
        evidence: "IEEE 315 draws the zigzag, IEC 60617 the box; a sheet keeps to one. A preference, not a fault of the drawing. In round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covers this rule alone.",
    },
    Rule {
        code: "sch_out_of_line",
        weight: 2,
        unit: "a part",
        what: "A part's body middle, or a two-pin part's pin axis, lines up with no other part across or down.",
        why: "Parts on common lines read as an ordered drawing; a part off every line looks dropped where it fell.",
        fix: "Put it in a row or a column with its neighbours (align X Y row|column).",
        evidence: "A drafting convention of rows and columns; round 1 (F20) asked that a part beside its idiom's pin count as in line, which it does. In round 2's blind review (plan 1.D, three judges a pair), this rule and the judges' grouping score moved together on the buck and the front end but apart on the breakout (2 to 4 points, +0.3 of 5) and the RP2040 (26 to 28, +2.0): the judges grouped by function, which lines don't show.",
    },
    Rule {
        code: "sch_bank_not_in_row",
        weight: 3,
        unit: "a bank",
        what: "Parts of one prefix between the same two nets (a bank of capacitors, of pull-ups) aren't in one row or column.",
        why: "A bank reads as one function only lined up; scattered, the reader counts its members one by one.",
        fix: "Line the bank up (align C1 C2 C3 row).",
        evidence: "A drafting convention: a bank of one function between the same nets drawn as one row or column. In round 2's blind review (plan 1.D, three judges a pair), the judges named decoupling banks drawn as rows on their rail among what read well on the RP2040.",
    },
    Rule {
        code: "sch_topology",
        weight: 10,
        unit: "a topology",
        what: "A recognised circuit (a buck converter, an op-amp stage) is drawn out of its standard shape.",
        why: "Engineers recognise these circuits by their shape; drawn another way, the reader must trace every wire to know what they are looking at.",
        fix: "Re-form it round its main part (idiom U1 buck, idiom U2 opamp_stage), as the finding's message says.",
        evidence: "The TPS54331 reference and the datasheets' application circuits draw the buck one way, mhd314's BalDiffAmp and ivconverter the op-amp stage one way; round 1 (F4) asked for the shape by name. In round 2's blind review (plan 1.D, three judges a pair), the judges preferred the TPS54331 reference's standard drawing to ours 3 to 0, naming the shape this rule asks for: the inductor across, the diode down, the input capacitors beside VIN.",
    },
    Rule {
        code: "sch_part_flipped",
        weight: 3,
        unit: "a part",
        what: "A two-pin part has its ground end up or its supply end down.",
        why: "A flipped part sends its rail's port across its body or through a bend, against supplies up and grounds down.",
        fix: "Turn it over (orient X 180).",
        evidence: "Round 1 found flipped parts the score didn't name (F19); supplies up and grounds down (see sch_power_sideways). In round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covers this rule alone.",
    },
    Rule {
        code: "sch_wire_wraps",
        weight: 3,
        unit: "a wire",
        what: "A wire goes round two opposite sides of the part it ends on.",
        why: "A wire wrapping a part looks like it joins pins on both sides; the reader must trace it to be sure.",
        fix: "Turn the part so its pin faces the wire (orient X 0|90|180|270), or move it (beside X Y left|right|above|below).",
        evidence: "Round 1 found wires wrapping parts that the score didn't name (F19). In round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covers this rule alone.",
    },
    Rule {
        code: "sch_paper_fit",
        weight: 5,
        unit: "a sheet",
        what: "A sheet on paper larger than its drawing needs: a smaller paper of the project's series would hold it, or it fills under 100 per mille of its 10 mm cells on paper larger than the series' smallest.",
        why: "A small drawing on a large sheet prints small and reads as empty; the reader zooms and pans for what one smaller page would show at once.",
        fix: "Lay the sheet out again so paper fit chooses (layout --relayout BLOCK/SHEET), or pin a smaller paper (paper SHEET a4).",
        evidence: "The references fill 115 to 336 per mille of their drawing area's 10 mm cells and draw on the smallest paper that holds them: the buck, the front end's three sheets on A4, Mitayi-Pico-D1 on A3, the ESP32-PoE on A2 (measure.py, plan 2.A's Task 1). In round 2's blind review (plan 1.D, three judges a pair), the judges preferred the reference on all four circuits on paper fit, 3 to 0.",
    },
    Rule {
        code: "sch_function_split",
        weight: 6,
        unit: "a function",
        what: "One function's parts (a recognised circuit, a dense IC with its satellites and guests, a connector and its breakout) are in more than one region, or some in none.",
        why: "A reader finds a function by its titled region; split, its parts read as two functions, or one is missing from where the title says it is.",
        fix: "Name its region with all of them (region TITLE X...), as the finding's fix says.",
        evidence: "Design 348's regions: the references group each function on its own part of the sheet (Mitayi-Pico-D1's USB, regulator and RP2040 corners; the ESP32-PoE's Ethernet panel). In round 2's blind review (plan 1.D, three judges a pair), the judges preferred the reference on all four circuits on grouping by function, 3 to 0.",
    },
    Rule {
        code: "sch_region_untitled",
        weight: 2,
        unit: "a region",
        what: "A region titled Misc: nothing in it names its function.",
        why: "A region's title is what a reader scans for; Misc says nothing about what the parts do.",
        fix: "Give it a title (region rename REGION TITLE), or a function property to its main component.",
        evidence: "Design 348: every region is titled by its function. The references title their panels (the ESP32-PoE's ETHERNET, Mitayi-Pico-D1's power and USB blocks).",
    },
];

/// How much a finding counts: a rule's lint, warned of, or an error that
/// fails a check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Off,
    #[default]
    Warning,
    Error,
}

impl Severity {
    /// As files write it.
    pub fn word(self) -> &'static str {
        match self {
            Severity::Off => "off",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// How a resistor is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResistorStyle {
    /// The zigzag.
    #[default]
    Us,
    /// The box.
    Iec,
}

/// How a net between parts on one sheet is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalNets {
    /// Wires: a part joined only by labels floats.
    #[default]
    Wires,
    /// Labels: no part floats, and no labels want a wire.
    Labels,
}

/// The styles a project prefers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefer {
    #[serde(default)]
    pub resistor: ResistorStyle,
    #[serde(default)]
    pub local_nets: LocalNets,
}

/// A project's lint preferences: weights and severities by rule, and the
/// styles it prefers. The default is lint as this module sets it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefs {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub weights: BTreeMap<String, i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub severity: BTreeMap<String, Severity>,
    #[serde(default)]
    pub prefer: Prefer,
}

impl Prefs {
    /// A rule's weight, and the preference that set it if one did.
    pub fn weight(&self, code: &str) -> (i64, Option<String>) {
        match self.weights.get(code) {
            Some(w) => (*w, Some(format!("weights.{code} = {w}"))),
            None => (default_weight(code), None),
        }
    }

    /// A rule's severity, and the preference that set it if one did.
    pub fn severity(&self, code: &str) -> (Severity, Option<String>) {
        match self.severity.get(code) {
            Some(s) => (*s, Some(format!("severity.{code} = {}", s.word()))),
            None => (Severity::Warning, None),
        }
    }
}

/// A rule's weight as this module sets it (0 for a code it doesn't have).
pub fn default_weight(code: &str) -> i64 {
    RULES
        .iter()
        .find(|r| r.code == code)
        .map_or(0, |r| r.weight)
}

/// One finding.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub sheet: String,
    pub message: String,
    /// The parts and nets concerned, by key.
    pub refs: Vec<String>,
    pub details: Value,
    /// What it costs the sheet's score: its rule's weight per unit.
    pub points: i64,
    pub severity: Severity,
    /// The preferences that drove it, as the project's file writes them
    /// (`prefer.resistor = us`, `weights.sch_jog = 5`), when any did.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prefs: Vec<String>,
    /// Batches that remove it, where the finding alone says which: each
    /// ready to send (ryueda writes them in the agent's names), the first
    /// the one to try first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Vec<Instruction>>,
}

impl Finding {
    /// The finding with these fixes.
    fn fixed(mut self, fixes: Vec<Vec<Instruction>>) -> Finding {
        self.fixes = fixes;
        self
    }
}

/// How many units of its rule a finding is.
fn units(f: &Finding) -> i64 {
    match f.code {
        "sch_crossings" => f.details["crossings"].as_i64().unwrap_or(1),
        "sch_bends" => f.details["extra"].as_i64().unwrap_or(1),
        "sch_jog" => f.details["jogs"].as_i64().unwrap_or(1),
        "sch_density" => (f.details["per_mille"].as_i64().unwrap_or(DENSITY) - DENSITY).max(0) / 10,
        _ => 1,
    }
}

/// A sheet's measures and its score.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Score {
    pub sheet: String,
    /// Out of 100: 100 less the penalty, never below 0.
    pub score: u32,
    /// The findings' points, before the score clamps them.
    pub penalty: i64,
    pub crossings: usize,
    pub bends: usize,
    /// Every wire's length, in nanometres.
    pub wire_nm: Nm,
    /// Ink per sheet area, per mille.
    pub density: i64,
    /// The box of everything drawn over the drawing area, per mille.
    pub extent: i64,
    /// The share of the drawing area's 10 mm cells holding ink, per mille.
    pub fill: i64,
    pub findings: usize,
    /// Preferences that took rules out of play on the sheet.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Everything sheet `sid` draws that takes room: parts' extents, text,
/// labels and wire segments.
fn ink_boxes(block: &Block, layout: &crate::layout::Layout, sid: &str) -> Vec<Rect> {
    let mut out = Vec::new();
    for part in &block.parts {
        let Some(x) = layout.placed.get(&part.key).filter(|x| x.sheet == sid) else {
            continue;
        };
        out.push(Shape::of(part, Place::new(x.at, x.rotation, x.mirror)).extent);
        if let Some(fields) = layout.text.get(&part.key) {
            for (field, value) in [("refdes", &part.refdes), ("value", &part.value)] {
                if let Some(at) = fields.get(field) {
                    out.push(text::boxes(&[value.as_str()], *at)[0]);
                }
            }
        }
    }
    for list in layout.labels.values() {
        for l in list.iter().filter(|l| l.sheet == sid) {
            out.push(label::label_box(l.kind, &l.text, l.at, l.side));
        }
    }
    for by_sheet in layout.routes.values() {
        for run in by_sheet.get(sid).into_iter().flatten() {
            for w in run.windows(2) {
                out.push(Rect::new(w[0], w[1]));
            }
        }
    }
    out
}

/// The share of sheet `sid`'s drawing area's whole 10 mm cells that a
/// part, a text, a label or a wire touches, per mille (as `measure.py`).
pub fn fill(block: &Block, layout: &crate::layout::Layout, sid: &str) -> i64 {
    let area = block.area_in(layout, sid);
    let (nx, ny) = (area[0] / CELL, area[1] / CELL);
    if nx <= 0 || ny <= 0 {
        return 0;
    }
    let mut cells: BTreeSet<(Nm, Nm)> = BTreeSet::new();
    for b in ink_boxes(block, layout, sid) {
        let (x0, x1) = ((b.min[0] / CELL).max(0), (b.max[0] / CELL).min(nx - 1));
        let (y0, y1) = ((b.min[1] / CELL).max(0), (b.max[1] / CELL).min(ny - 1));
        for i in x0..=x1 {
            for j in y0..=y1 {
                cells.insert((i, j));
            }
        }
    }
    cells.len() as i64 * 1000 / (nx * ny)
}

/// The box of everything sheet `sid` draws, over its drawing area, per
/// mille.
pub fn extent(block: &Block, layout: &crate::layout::Layout, sid: &str) -> i64 {
    let area = Rect::new([0, 0], block.area_in(layout, sid));
    let Some(b) = ink_boxes(block, layout, sid)
        .into_iter()
        .reduce(Rect::union)
    else {
        return 0;
    };
    let clipped = Rect::new(
        [b.min[0].max(area.min[0]), b.min[1].max(area.min[1])],
        [b.max[0].min(area.max[0]), b.max[1].min(area.max[1])],
    );
    (clipped.area() * 1000 / area.area().max(1)) as i64
}

/// The lint of a laid out block, sheet by sheet: findings in a fixed
/// order (code, then what they name) and each sheet's score, under the
/// default preferences.
pub fn lint(block: &Block, out: &Outcome) -> (Vec<Finding>, Vec<Score>) {
    lint_with(block, out, &Prefs::default())
}

/// A bank's key: prefix, pin count, nets, and for decoupling capacitors
/// the part and side they serve.
type BankKey = (String, usize, Vec<usize>, Option<(usize, Side)>);

/// [`lint`] under a project's preferences.
pub fn lint_with(block: &Block, out: &Outcome, prefs: &Prefs) -> (Vec<Finding>, Vec<Score>) {
    let layout = &out.layout;
    let mut findings = Vec::new();
    let mut scores = Vec::new();
    for sid in layout.sheets.keys() {
        let size = block.area_in(layout, sid);
        let width = size[0];
        let mut found: Vec<Finding> = Vec::new();
        let f = |code: &'static str, message: String, refs: Vec<String>, details: Value| Finding {
            code,
            sheet: sid.clone(),
            message,
            refs,
            details,
            points: 0,
            severity: Severity::Warning,
            prefs: Vec::new(),
            fixes: Vec::new(),
        };
        let shapes: BTreeMap<usize, Shape> = block
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let x = layout.placed.get(&p.key).filter(|x| &x.sheet == sid)?;
                Some((i, Shape::of(p, Place::new(x.at, x.rotation, x.mirror))))
            })
            .collect();
        // The circuits with a standard drawing on this sheet, judged once;
        // the parts at the heart of its buck converters.
        let judged = crate::topology::judge(block, layout, sid, &shapes);
        let bucks: BTreeSet<String> = judged
            .iter()
            .filter(|j| j.kind == "buck_converter")
            .map(|j| j.parts[0].clone())
            .collect();
        // Wires: each node's nets, with arms.
        let mut at_node: BTreeMap<route::Node, BTreeSet<&str>> = BTreeMap::new();
        let mut bends = 0usize;
        let mut wire_nm = 0;
        let mut crossed_by: BTreeMap<&str, usize> = BTreeMap::new();
        for (key, by_sheet) in &layout.routes {
            let Some(runs) = by_sheet.get(sid) else {
                continue;
            };
            let mut extra = 0usize;
            let mut total = 0;
            let mut net_jogs = 0usize;
            let mut around: BTreeSet<usize> = BTreeSet::new();
            for run in runs {
                net_jogs += jogs(run);
                around.extend(crate::topology::jog_obstacles(run, &shapes));
                let turns = run.len().saturating_sub(2);
                bends += turns;
                let (a, b) = (run[0], *run.last().expect("a run has ends"));
                let fewest = usize::from(a[0] != b[0] && a[1] != b[1]);
                extra += turns.saturating_sub(fewest);
                let len = route::length(run);
                total += len;
                if len > width * LONG_WIRE.0 / LONG_WIRE.1 {
                    found.push(f(
                        "sch_long_wire",
                        format!("a wire of {key} runs {} mm", len / 1_000_000),
                        vec![key.clone()],
                        json!({"length_nm": len, "threshold_nm": width * LONG_WIRE.0 / LONG_WIRE.1}),
                    ));
                }
                for n in route::expand(run) {
                    at_node.entry(n).or_default().insert(key.as_str());
                }
            }
            wire_nm += total;
            if net_jogs > 0 {
                let name = block
                    .net(key)
                    .map_or(key.clone(), |n| block.nets[n].name.clone());
                // A net's own parts are what it reaches, not what it steps
                // round.
                let n = block.net(key);
                around.retain(|p| {
                    !block.parts[*p]
                        .pins
                        .iter()
                        .any(|x| n.is_some() && x.net == n)
                });
                let mut message = format!(
                    "{name}'s wires jog {} where they could run straight on",
                    times(net_jogs)
                );
                if !around.is_empty() {
                    let names: Vec<String> = around.iter().map(|p| part_shown(block, *p)).collect();
                    message.push_str(&format!(", stepping round {}", names.join(", ")));
                }
                // Round a pin's bypass capacitors: say what the standard
                // drawing does instead.
                let mut served: Vec<(usize, usize)> = out
                    .idioms
                    .iter()
                    .filter(|i| i.kind == IdiomKind::Decoupling && around.contains(&i.parts[0]))
                    .filter_map(|i| i.anchor)
                    .collect();
                served.sort();
                served.dedup();
                if let Some(&(a, k)) = served.first() {
                    let pin = &block.parts[a].pins[k].name;
                    let caps: Vec<String> = out
                        .idioms
                        .iter()
                        .filter(|i| i.kind == IdiomKind::Decoupling && i.anchor == Some((a, k)))
                        .map(|i| who(&block.parts[i.parts[0]]))
                        .collect();
                    let standard = if bucks.contains(&block.parts[a].key) {
                        "Standard buck input:"
                    } else {
                        "Standard:"
                    };
                    message = format!(
                        "{name} jogs under {pin}'s bypass caps ({}). {standard} the caps stand on the input wire beside {pin}, and {name} is taken off before them. ({message}.)",
                        caps.join(", ")
                    );
                }
                found.push(f(
                    "sch_jog",
                    message,
                    vec![key.clone()],
                    json!({"jogs": net_jogs, "around": around.iter().map(|p| block.parts[*p].key.clone()).collect::<Vec<_>>()}),
                ));
            }
            if extra > EXTRA_BENDS {
                found.push(f(
                    "sch_bends",
                    format!(
                        "{}'s wires bend {extra} times more than they need",
                        net_shown(block, key)
                    ),
                    vec![key.clone()],
                    json!({"extra": extra, "threshold": EXTRA_BENDS}),
                ));
            }
            // A rail is drawn by ports; its wires are bypass and local ties.
            let rail = block
                .net(key)
                .is_some_and(|n| block.nets[n].kind != NetKind::Signal);
            if total > width * WIRE_NOT_LABEL.0 / WIRE_NOT_LABEL.1 && !rail {
                found.push(
                    f(
                    "sch_wire_where_label",
                    format!("{}'s wires run {} mm; labels would read better", net_shown(block, key), total / 1_000_000),
                    vec![key.clone()],
                    json!({"length_nm": total, "threshold_nm": width * WIRE_NOT_LABEL.0 / WIRE_NOT_LABEL.1}),
                    )
                    .fixed(vec![vec![Instruction::Net {
                        net: (*key).to_owned(),
                        style: NetStyle::Label,
                    }]]),
                );
            }
        }
        let mut crossings = 0;
        for nets in at_node.values() {
            if nets.len() >= 2 {
                crossings += 1;
                for n in nets {
                    *crossed_by.entry(n).or_default() += 1;
                }
            }
        }
        if crossings > 0 {
            found.push(f(
                "sch_crossings",
                format!("wires cross {} on sheet {sid}", times(crossings)),
                Vec::new(),
                json!({"crossings": crossings}),
            ));
        }
        for (key, n) in &crossed_by {
            if *n > MANY_CROSSINGS
                && !found
                    .iter()
                    .any(|x| x.code == "sch_wire_where_label" && x.refs[0] == *key)
            {
                found.push(
                    f(
                        "sch_wire_where_label",
                        format!(
                            "{}'s wires cross {n} others; labels would read better",
                            net_shown(block, key)
                        ),
                        vec![(*key).to_owned()],
                        json!({"crossings": n, "threshold": MANY_CROSSINGS}),
                    )
                    .fixed(vec![vec![Instruction::Net {
                        net: (*key).to_owned(),
                        style: NetStyle::Label,
                    }]]),
                );
            }
        }
        // Labels a short wire would replace.
        for (key, list) in &layout.labels {
            let Some(n) = block.net(key) else { continue };
            if block.nets[n].kind != NetKind::Signal {
                continue;
            }
            let here: Vec<_> = list
                .iter()
                .filter(|l| &l.sheet == sid && l.kind == LabelKind::Net)
                .collect();
            if here.len() < 2 {
                continue;
            }
            let pts: Vec<_> = here.iter().map(|l| l.at).collect();
            let span = spanning(&pts);
            if span < SHORT_NET {
                found.push(
                    f(
                        "sch_label_where_wire",
                        format!(
                            "{}'s labels are {} mm apart; a wire would read better",
                            net_shown(block, key),
                            span / 1_000_000
                        ),
                        vec![key.clone()],
                        json!({"span_nm": span, "threshold_nm": SHORT_NET}),
                    )
                    .fixed(vec![vec![Instruction::Net {
                        net: key.clone(),
                        style: NetStyle::Wire,
                    }]]),
                );
            }
        }
        // Ports and labels the way they read best: supplies up, ground
        // down, names across the page.
        for (key, list) in &layout.labels {
            for l in list.iter().filter(|l| &l.sheet == sid) {
                let (code, what) = match l.kind {
                    LabelKind::Power if l.side != Side::Up => ("sch_power_sideways", "supply"),
                    LabelKind::Ground if l.side != Side::Down => ("sch_ground_sideways", "ground"),
                    LabelKind::Net if !l.side.horizontal() => ("sch_label_vertical", "label"),
                    _ => continue,
                };
                found.push(f(
                    code,
                    format!(
                        "{}'s {what} at {} points {}",
                        net_shown(block, key),
                        l.on,
                        l.side.as_str()
                    ),
                    vec![key.clone(), l.on.clone()],
                    json!({"side": l.side.as_str()}),
                ));
            }
        }
        for (p, s) in &shapes {
            let part = &block.parts[*p];
            if matches!(part.kind, PartKind::Port { .. })
                && s.pins.first().is_some_and(|pin| !pin.side.horizontal())
            {
                found.push(f(
                    "sch_label_vertical",
                    format!("port {} is drawn up the page", part.key),
                    vec![part.key.clone()],
                    json!({"side": s.pins[0].side.as_str()}),
                ));
            }
        }
        // A part floating in space: every signal pin reaches its net by a
        // label though some net of it has another pin on this sheet, so a
        // wire could have joined it. Ports and connectors (whose pins
        // labels name, as schematics draw them) aren't judged.
        // Every pin end on this sheet, by net.
        let mut ends_of: BTreeMap<usize, Vec<route::Node>> = BTreeMap::new();
        for (p, sh) in &shapes {
            for (k, pin) in block.parts[*p].pins.iter().enumerate() {
                if let Some(n) = pin.drawn_net() {
                    ends_of
                        .entry(n)
                        .or_default()
                        .push(route::node_of(sh.pins[k].end));
                }
            }
        }
        // Nets with a pin on another sheet (plan 2.A): there a label is the
        // net's off-sheet connector, until plan 2.C draws one.
        let leaving: BTreeSet<usize> = block
            .parts
            .iter()
            .filter(|q| layout.placed.get(&q.key).is_some_and(|x| &x.sheet != sid))
            .flat_map(|q| q.pins.iter().flat_map(|x| x.nets()))
            .collect();
        for (p, sh) in &shapes {
            let part = &block.parts[*p];
            if matches!(part.kind, PartKind::Port { .. }) || crate::cluster::is_connector(block, *p)
            {
                continue;
            }
            let mut signals = 0;
            let mut wired = false;
            let mut local = false;
            // The nearest other pin on one of its nets on this sheet: what
            // `near` would stand it beside.
            let mut nearest: Option<(Nm, String)> = None;
            for (k, pin) in part.pins.iter().enumerate() {
                let Some(n) = pin.drawn_net() else { continue };
                if block.nets[n].kind != NetKind::Signal {
                    continue;
                }
                signals += 1;
                let ends = ends_of.get(&n).map(Vec::as_slice).unwrap_or_default();
                local |= ends.len() > 1;
                let e = sh.pins[k].end;
                for (q, sq) in &shapes {
                    if q == p || matches!(block.parts[*q].kind, PartKind::Port { .. }) {
                        continue;
                    }
                    for (j, x) in block.parts[*q].pins.iter().enumerate() {
                        if x.drawn_net() != Some(n) {
                            continue;
                        }
                        let o = sq.pins[j].end;
                        let d = (o[0] - e[0]).abs() + (o[1] - e[1]).abs();
                        let to = format!("{}/{}", block.parts[*q].key, x.number);
                        if nearest.as_ref().is_none_or(|(b, t)| (d, &to) < (*b, t)) {
                            nearest = Some((d, to));
                        }
                    }
                }
                let runs = layout
                    .routes
                    .get(&block.nets[n].key)
                    .and_then(|r| r.get(sid))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                wired |= wired_to_another(runs, route::node_of(sh.pins[k].end), ends);
                if leaving.contains(&n) {
                    // Its own label, or wired to one of the net's labels here.
                    let on = format!("{}/{}", part.key, pin.number);
                    let labels: Vec<route::Node> = layout
                        .labels
                        .get(&block.nets[n].key)
                        .into_iter()
                        .flatten()
                        .filter(|l| &l.sheet == sid)
                        .map(|l| {
                            if l.on == on {
                                wired = true;
                            }
                            route::node_of(l.at)
                        })
                        .collect();
                    wired |= wired_to_another(runs, route::node_of(sh.pins[k].end), &labels);
                }
            }
            if signals > 0 && !wired && local {
                let fixes = nearest
                    .map(|(_, to)| {
                        vec![vec![Instruction::Near {
                            parts: vec![part.key.clone()],
                            to,
                        }]]
                    })
                    .unwrap_or_default();
                found.push(
                    f(
                        "sch_floating",
                        format!(
                            "{} is joined to nothing by a wire: its nets on this sheet are labels",
                            part_shown(block, *p)
                        ),
                        vec![part.key.clone()],
                        json!({"signals": signals}),
                    )
                    .fixed(fixes),
                );
            }
        }
        // A two-pin part standing upside down against its rail: ground up,
        // or a supply down.
        for (p, sh) in &shapes {
            let part = &block.parts[*p];
            if part.pins.len() != 2 || matches!(part.kind, PartKind::Port { .. }) {
                continue;
            }
            for (k, pin) in part.pins.iter().enumerate() {
                let kind = pin.net.map(|n| block.nets[n].kind);
                let side = sh.pins[k].side;
                let flipped = match kind {
                    Some(NetKind::Ground) => side == Side::Up,
                    Some(NetKind::Supply) => side == Side::Down,
                    _ => false,
                };
                // Between two rails, only the ground end judges.
                let both = part
                    .pins
                    .iter()
                    .all(|x| x.net.is_some_and(|n| block.nets[n].kind != NetKind::Signal));
                if flipped && !(both && kind == Some(NetKind::Supply)) {
                    let rail = if kind == Some(NetKind::Ground) {
                        "ground end up"
                    } else {
                        "supply end down"
                    };
                    let (rotation, mirror) = layout
                        .placed
                        .get(&part.key)
                        .map_or((0, false), |x| (x.rotation, x.mirror));
                    found.push(
                        f(
                            "sch_part_flipped",
                            format!(
                                "{} stands with its {rail}: turn it over (orient {} 180)",
                                part_shown(block, *p),
                                part_shown(block, *p)
                            ),
                            vec![part.key.clone()],
                            json!({"pin": pin.number}),
                        )
                        .fixed(vec![vec![Instruction::Orient {
                            parts: vec![part.key.clone()],
                            rotation: (rotation + 180) % 360,
                            mirror,
                        }]]),
                    );
                    break;
                }
            }
        }
        // A wire running round its own part to reach it: on two opposite
        // sides of the body it ends on.
        for (key, by_sheet) in &layout.routes {
            let Some(runs) = by_sheet.get(sid) else {
                continue;
            };
            let mut wrapped: BTreeSet<usize> = BTreeSet::new();
            for run in runs {
                for end in [run[0], *run.last().expect("a run has ends")] {
                    for (p, sh) in &shapes {
                        if wrapped.contains(p) || !sh.pins.iter().any(|x| x.end == end) {
                            continue;
                        }
                        let b = sh.body;
                        let (mut left, mut right, mut up, mut down) = (false, false, false, false);
                        for w in run.windows(2) {
                            let r = Rect::new(w[0], w[1]);
                            let across_y = r.min[1] < b.max[1] && r.max[1] > b.min[1];
                            let across_x = r.min[0] < b.max[0] && r.max[0] > b.min[0];
                            left |= across_y && r.max[0] <= b.min[0] && w[0][0] == w[1][0];
                            right |= across_y && r.min[0] >= b.max[0] && w[0][0] == w[1][0];
                            down |= across_x && r.max[1] <= b.min[1] && w[0][1] == w[1][1];
                            up |= across_x && r.min[1] >= b.max[1] && w[0][1] == w[1][1];
                        }
                        if (left && right) || (up && down) {
                            wrapped.insert(*p);
                            found.push(f(
                                "sch_wire_wraps",
                                format!(
                                    "{}'s wire runs round {} to reach it",
                                    net_shown(block, key),
                                    part_shown(block, *p)
                                ),
                                vec![key.clone(), block.parts[*p].key.clone()],
                                Value::Null,
                            ));
                        }
                    }
                }
            }
        }
        // Clean rows: every part in line with another, across or down, and
        // like parts sharing their nets (a bank) in one row or column.
        let middles: Vec<(usize, [Nm; 2])> = shapes
            .iter()
            .filter(|(p, _)| !matches!(block.parts[**p].kind, PartKind::Port { .. }))
            .map(|(p, sh)| (*p, sh.body.center()))
            .collect();
        let near = |a: Nm, b: Nm| (a - b).abs() <= GRID / 2;
        // The lines a part stands on: its body's middle across and down,
        // and a two-pin part's axis through its pins too.
        let lines = |p: usize| -> (Vec<Nm>, Vec<Nm>) { part_lines(&block.parts[p], &shapes[&p]) };
        // An idiom's satellite standing on the line out of the pin it
        // serves (a pull-up), or a bypass capacitor wired to its pin, is in
        // line with that part as the idiom draws it, and the part with it:
        // the lines of their middles needn't meet.
        let mut on_pin_line: BTreeSet<(usize, usize)> = BTreeSet::new();
        for idiom in &out.idioms {
            if !matches!(
                idiom.kind,
                IdiomKind::Decoupling | IdiomKind::PullUp | IdiomKind::PullDown
            ) {
                continue;
            }
            let Some((a, k)) = idiom.anchor else { continue };
            let Some(pin) = shapes.get(&a).and_then(|sh| sh.pins.get(k)) else {
                continue;
            };
            let (e, d) = (pin.end, pin.side.step());
            // A bypass capacitor is its pin's when it is wired to it,
            // standing on the pin's line or, from a pin pointing up or
            // down, beside the part.
            let nodes: BTreeSet<route::Node> = block.parts[a].pins[k]
                .net
                .and_then(|n| layout.routes.get(&block.nets[n].key))
                .and_then(|r| r.get(sid))
                .into_iter()
                .flatten()
                .flat_map(|run| route::expand(run))
                .collect();
            for c in &idiom.parts {
                let Some(sc) = shapes.get(c) else { continue };
                let wired = idiom.kind == IdiomKind::Decoupling
                    && nodes.contains(&route::node_of(e))
                    && sc
                        .pins
                        .iter()
                        .any(|x| nodes.contains(&route::node_of(x.end)));
                let stands = wired
                    || sc.pins.iter().any(|x| {
                        let (across, along) = if pin.side.horizontal() {
                            ((x.end[1] - e[1]).abs(), (x.end[0] - e[0]) * d[0].signum())
                        } else {
                            ((x.end[0] - e[0]).abs(), (x.end[1] - e[1]) * d[1].signum())
                        };
                        across <= 2 * GRID && along >= 0
                    });
                if stands {
                    on_pin_line.insert((a, *c));
                    on_pin_line.insert((*c, a));
                }
            }
        }
        if middles.len() > 1 {
            for (p, m) in &middles {
                let (xs, ys) = lines(*p);
                let lined = middles.iter().any(|(q, _)| {
                    let (ox, oy) = lines(*q);
                    q != p
                        && (on_pin_line.contains(&(*p, *q))
                            || xs.iter().any(|a| ox.iter().any(|b| near(*a, *b)))
                            || ys.iter().any(|a| oy.iter().any(|b| near(*a, *b))))
                });
                if !lined {
                    let near_part = middles
                        .iter()
                        .filter(|(q, _)| q != p)
                        .min_by_key(|(q, o)| ((m[0] - o[0]).abs() + (m[1] - o[1]).abs(), *q))
                        .map(|(q, o)| (*q, *o));
                    let nearest = near_part
                        .map(|(q, _)| part_shown(block, q))
                        .unwrap_or_default();
                    // In line with the nearest part, the nearer way: its
                    // row when they are further apart across than down.
                    let fixes = near_part
                        .map(|(q, o)| {
                            let axis = if (m[1] - o[1]).abs() <= (m[0] - o[0]).abs() {
                                Axis::Row
                            } else {
                                Axis::Column
                            };
                            vec![vec![Instruction::Align {
                                parts: vec![
                                    block.parts[q].key.clone(),
                                    block.parts[*p].key.clone(),
                                ],
                                axis,
                            }]]
                        })
                        .unwrap_or_default();
                    found.push(
                        f(
                            "sch_out_of_line",
                            format!(
                                "{} lines up with no other part, across or down: put it in a row or a column with its neighbours (the nearest is {nearest})",
                                part_shown(block, *p)
                            ),
                            vec![block.parts[*p].key.clone()],
                            json!({"nearest": nearest}),
                        )
                        .fixed(fixes),
                    );
                }
            }
        }
        // A decoupling capacitor belongs to the pin it serves (the bypass
        // wiring holds it there): a bank of them is one part's on one
        // side, which stand in a row; another part's are another bank.
        let serves: BTreeMap<usize, (usize, Side)> = out
            .idioms
            .iter()
            .filter(|i| i.kind == IdiomKind::Decoupling)
            .filter_map(|i| {
                let (a, k) = i.anchor?;
                Some((i.parts[0], (a, shapes.get(&a)?.pins[k].side)))
            })
            .collect();
        let mut banks: BTreeMap<BankKey, Vec<usize>> = BTreeMap::new();
        for (p, _) in &middles {
            let part = &block.parts[*p];
            let PartKind::Symbol { prefix } = &part.kind else {
                continue;
            };
            let nets: Option<Vec<usize>> = part.pins.iter().map(|x| x.drawn_net()).collect();
            let Some(mut nets) = nets else { continue };
            nets.sort();
            nets.dedup();
            if nets.len() < 2 {
                continue;
            }
            banks
                .entry((
                    prefix.clone(),
                    part.pins.len(),
                    nets,
                    serves.get(p).copied(),
                ))
                .or_default()
                .push(*p);
        }
        for ((_, _, nets, _), members) in &banks {
            if members.len() < 2 {
                continue;
            }
            let at = |p: &usize| middles.iter().find(|(q, _)| q == p).expect("a member").1;
            let first = at(&members[0]);
            let row = members.iter().all(|p| near(at(p)[1], first[1]));
            let column = members.iter().all(|p| near(at(p)[0], first[0]));
            if !row && !column {
                let mut heights: Vec<Nm> = members.iter().map(|p| at(p)[1] / (GRID / 2)).collect();
                heights.sort();
                heights.dedup();
                let names: Vec<String> = members.iter().map(|p| part_shown(block, *p)).collect();
                let between: Vec<&str> =
                    nets.iter().map(|n| block.nets[*n].name.as_str()).collect();
                found.push(f(
                    "sch_bank_not_in_row",
                    format!(
                        "{} all sit between {}: stand them in one row, their tops on one line and evenly spaced, or in one column (now at {} heights)",
                        names.join(", "),
                        between.join(" and "),
                        heights.len()
                    ),
                    members.iter().map(|p| block.parts[*p].key.clone()).collect(),
                    json!({"nets": between, "heights": heights.len()}),
                ).fixed(vec![vec![Instruction::Align {
                    parts: members.iter().map(|p| block.parts[*p].key.clone()).collect(),
                    axis: Axis::Row,
                }]]));
            }
        }
        // Circuits with a standard drawing (a buck converter, an op-amp
        // stage) drawn out of it.
        for j in judged {
            if j.problems.is_empty() {
                continue;
            }
            let mut what = j.what.clone();
            if let Some(c) = what.get(..1) {
                what = c.to_uppercase() + &what[1..];
            }
            found.push(f(
                "sch_topology",
                format!(
                    "{what} is drawn out of its standard shape: {}. Draw it the standard way: {}. Send \"{}\" to draw it so.",
                    j.problems.join("; "),
                    j.standard,
                    j.fix
                ),
                j.parts.clone(),
                json!({"topology": j.kind, "problems": j.problems, "standard": j.standard, "fix": j.fix}),
            ).fixed(vec![vec![Instruction::Idiom {
                parts: vec![j.parts[0].clone()],
                kind: IdiomChoice::Kind(if j.kind == "buck_converter" {
                    IdiomKind::Buck
                } else {
                    IdiomKind::OpampStage
                }),
            }]]));
        }
        // A port's flag reads best straight out to the pin it meets.
        for (p, sh) in &shapes {
            let part = &block.parts[*p];
            if !matches!(part.kind, PartKind::Port { .. }) || part.pins.len() != 1 {
                continue;
            }
            let Some(n) = part.pins[0].net else { continue };
            let end = sh.pins[0].end;
            let Some(runs) = layout
                .routes
                .get(&block.nets[n].key)
                .and_then(|r| r.get(sid))
            else {
                continue;
            };
            let turns: usize = runs
                .iter()
                .filter(|r| r.first() == Some(&end) || r.last() == Some(&end))
                .map(|r| r.len().saturating_sub(2))
                .sum();
            if turns > 0 {
                found.push(f(
                    "sch_port_bent",
                    format!("port {}'s wire turns {turns} times", part.key),
                    vec![part.key.clone()],
                    json!({"turns": turns}),
                ));
            }
        }
        // Pins' names inside their body, apart from each other.
        for (p, sh) in &shapes {
            let part = &block.parts[*p];
            if !crate::paint::draws_names(part) {
                continue;
            }
            let boxes: Vec<(usize, Rect)> = part
                .pins
                .iter()
                .enumerate()
                .filter(|(_, pin)| !pin.name.is_empty())
                .map(|(k, pin)| (k, crate::paint::name_box(&pin.name, &sh.pins[k])))
                .collect();
            for (i, (k, b)) in boxes.iter().enumerate() {
                let outside = !sh.body.contains(b);
                let crowded = boxes[i + 1..].iter().any(|(_, o)| o.overlaps(b));
                if outside || crowded {
                    let pin = &part.pins[*k];
                    found.push(f(
                        "sch_pin_name_outside",
                        format!(
                            "{}'s pin {} name {} {}",
                            part.key,
                            pin.number,
                            pin.name,
                            if outside {
                                "runs past its body"
                            } else {
                                "runs into another pin's"
                            }
                        ),
                        vec![part.key.clone()],
                        json!({"pin": pin.number, "outside": outside}),
                    ));
                }
            }
        }
        // Density: the ink's area against the sheet's.
        let mut ink: i128 = 0;
        for (p, s) in &shapes {
            ink += s.extent.area();
            if let Some(fields) = layout.text.get(&block.parts[*p].key) {
                for (field, v) in [
                    ("refdes", &block.parts[*p].refdes),
                    ("value", &block.parts[*p].value),
                ] {
                    if let Some(at) = fields.get(field) {
                        ink += text::boxes(&[v.as_str()], *at)[0].area();
                    }
                }
            }
        }
        for list in layout.labels.values() {
            for l in list.iter().filter(|l| &l.sheet == sid) {
                ink += label::label_box(l.kind, &l.text, l.at, l.side).area();
            }
        }
        let density = (ink * 1000 / Rect::new([0, 0], size).area().max(1)) as i64;
        if density > DENSITY {
            found.push(f(
                "sch_density",
                format!("sheet {sid} is {}% ink", density / 10),
                Vec::new(),
                json!({"per_mille": density, "threshold": DENSITY}),
            ));
        }
        // Flow: drivers left of what they drive.
        // Each reversed net with its first driver and the part it drives.
        let mut reversed: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for (n, net) in block.nets.iter().enumerate() {
            if net.kind != NetKind::Signal {
                continue;
            }
            let mut drivers = Vec::new();
            let mut sinks = Vec::new();
            for (p, s) in &shapes {
                let part = &block.parts[*p];
                for (k, pin) in part.pins.iter().enumerate() {
                    if pin.net != Some(n) {
                        continue;
                    }
                    let (drives, takes) = match part.kind {
                        PartKind::Port { flow } => (flow != Flow::Out, flow == Flow::Out),
                        _ => (pin.kind.drives(), pin.kind.takes()),
                    };
                    if drives {
                        drivers.push((*p, s.pins[k].end[0]));
                    }
                    if takes {
                        sinks.push((*p, s.pins[k].end[0]));
                    }
                }
            }
            for (d, dx) in &drivers {
                for (t, tx) in &sinks {
                    let free = out.flow.contains(&block.parts[*d].key)
                        || out.flow.contains(&block.parts[*t].key);
                    if *tx < *dx - AGAINST && !free {
                        reversed.entry(net.key.clone()).or_insert((*d, *t));
                    }
                }
            }
        }
        for (key, (d, t)) in reversed {
            // Move what it drives to its right; else say the flow is meant.
            let (dk, tk) = (block.parts[d].key.clone(), block.parts[t].key.clone());
            let fixes = if d == t {
                vec![vec![Instruction::Flow { parts: vec![tk] }]]
            } else {
                vec![
                    vec![Instruction::Beside {
                        parts: vec![tk.clone()],
                        of: dk,
                        side: Side::Right,
                    }],
                    vec![Instruction::Flow { parts: vec![tk] }],
                ]
            };
            found.push(
                f(
                    "sch_flow_reversed",
                    format!(
                        "{} runs right to left; `flow` lets it, or move what it drives",
                        net_shown(block, &key)
                    ),
                    vec![key.clone()],
                    json!({"threshold_nm": AGAINST}),
                )
                .fixed(fixes),
            );
        }
        for p in shapes.keys() {
            if !pins_on_grid(&block.parts[*p]) {
                let key = block.parts[*p].key.clone();
                found.push(f(
                    "sch_pin_off_grid",
                    format!("{}'s pins aren't on the 50 mil grid relative to each other; it is placed by its first", part_shown(block, *p)),
                    vec![key],
                    Value::Null,
                ));
            }
        }
        let mut bypass_apart: BTreeSet<usize> = BTreeSet::new();
        // A bypass capacitor reads best on the line out of the pin it
        // serves, wired to it, not standing apart under ports of its own.
        for idiom in &out.idioms {
            if idiom.kind != IdiomKind::Decoupling {
                continue;
            }
            let Some((a, k)) = idiom.anchor else { continue };
            let c = idiom.parts[0];
            let (Some(sa), Some(sc)) = (shapes.get(&a), shapes.get(&c)) else {
                continue;
            };
            let Some(n) = block.parts[a].pins[k].net else {
                continue;
            };
            let Some(kc) = block.parts[c].pins.iter().position(|x| x.net == Some(n)) else {
                continue;
            };
            let nodes: BTreeSet<route::Node> = layout
                .routes
                .get(&block.nets[n].key)
                .and_then(|r| r.get(sid))
                .into_iter()
                .flatten()
                .flat_map(|run| route::expand(run))
                .collect();
            let wired = [sa.pins[k].end, sc.pins[kc].end]
                .iter()
                .all(|e| nodes.contains(&route::node_of(*e)));
            if !wired {
                bypass_apart.insert(c);
                let (ka, kc) = (block.parts[a].key.clone(), block.parts[c].key.clone());
                found.push(f(
                    "sch_bypass_apart",
                    format!("{kc} decouples {ka}, but isn't drawn on the line from its pin"),
                    vec![kc, ka],
                    json!({"net": block.nets[n].key}),
                ));
            }
        }
        for idiom in &out.idioms {
            let Some((a, _)) = idiom.anchor else { continue };
            let Some(anchor) = shapes.get(&a) else {
                continue;
            };
            let pull = matches!(idiom.kind, IdiomKind::PullUp | IdiomKind::PullDown);
            for (i, p) in idiom.parts.iter().enumerate() {
                let Some(s) = shapes.get(p) else { continue };
                // A jumper joined to a pull idiom stands beside its
                // resistor, which stands by the pin: it is judged against
                // the resistor.
                let anchor = match shapes.get(&idiom.parts[0]) {
                    Some(r) if pull && i > 0 => r,
                    _ => anchor,
                };
                // A bypass capacitor wired on its pin's line reads as its
                // pin's however far out along it.
                if idiom.kind == IdiomKind::Decoupling && !bypass_apart.contains(p) {
                    continue;
                }
                if !s.extent.grow(IDIOM_REACH).overlaps(&anchor.extent) {
                    let key = block.parts[*p].key.clone();
                    found.push(f(
                        "sch_idiom_missed",
                        format!(
                            "{} is the {} of {}, but drawn far from it",
                            part_shown(block, *p),
                            idiom.kind.as_str(),
                            part_shown(block, a)
                        ),
                        vec![key, block.parts[a].key.clone()],
                        json!({"idiom": idiom.kind.as_str(), "threshold_nm": IDIOM_REACH}),
                    ));
                }
            }
        }
        // Styles the project prefers.
        let mut notes = Vec::new();
        let resistor = match prefs.prefer.resistor {
            ResistorStyle::Us => "prefer.resistor = us",
            ResistorStyle::Iec => "prefer.resistor = iec",
        };
        for p in shapes.keys() {
            let part = &block.parts[*p];
            let Some(drawn) = resistor_style(part) else {
                continue;
            };
            if drawn != prefs.prefer.resistor {
                let (is, wants) = match drawn {
                    ResistorStyle::Iec => ("a box (IEC)", "the US zigzag"),
                    ResistorStyle::Us => ("a zigzag (US)", "the IEC box"),
                };
                let mut x = f(
                    "sch_resistor_style",
                    format!(
                        "{} is drawn as {is}; this project prefers {wants} ({resistor}{})",
                        who(part),
                        if prefs.prefer == Prefer::default() {
                            ", the default"
                        } else {
                            ""
                        }
                    ),
                    vec![part.key.clone()],
                    Value::Null,
                );
                x.prefs.push(resistor.to_owned());
                found.push(x);
            }
        }
        // Paper fit (plan 2.A): a smaller paper would hold the drawing, or
        // it is too empty for paper larger than the series' smallest.
        let sheet = &layout.sheets[sid];
        let sheet_fill = fill(block, layout, sid);
        let sheet_extent = extent(block, layout, sid);
        if !sheet.pinned
            && let Some(now) = block.paper_of(sheet)
            && let Some(at) = block.papers.iter().position(|p| p == now)
            && let Some(drawn) = ink_boxes(block, layout, sid)
                .into_iter()
                .reduce(Rect::union)
        {
            let need = 2 * crate::place::MARGIN + 2 * GRID;
            let smaller = block.papers[..at]
                .iter()
                .find(|p| drawn.width() + need <= p.area[0] && drawn.height() + need <= p.area[1]);
            let empty = at > 0 && sheet_fill < FILL_MIN;
            if smaller.is_some() || empty {
                let to = smaller.unwrap_or(&block.papers[0]);
                let why = match smaller {
                    Some(p) => format!("{} would hold it", p.size),
                    None => format!("it fills {sheet_fill} per mille, under {FILL_MIN}"),
                };
                found.push(
                    f(
                        "sch_paper_fit",
                        format!(
                            "sheet {sid} is on {} though {why} (fill {sheet_fill} per mille)",
                            now.size
                        ),
                        vec![sid.clone()],
                        json!({
                            "size": now.size,
                            "smaller": to.size,
                            "fill": sheet_fill,
                            "extent": sheet_extent,
                            "fix": format!("layout --relayout {}/{sid}", block.name),
                        }),
                    )
                    .fixed(vec![vec![Instruction::Paper {
                        sheet: sid.clone(),
                        size: to.size.clone(),
                        orientation: None,
                    }]]),
                );
            }
        }
        // Regions (plan 2.A): one function in one region; every region
        // titled.
        let region_of = layout.region_of();
        if let Ok(r) = crate::instr::resolve(block) {
            for g in crate::region::propose(block, &out.idioms, &r) {
                if !matches!(
                    g.basis,
                    crate::region::Basis::Topology
                        | crate::region::Basis::Ic
                        | crate::region::Basis::Connector
                ) {
                    continue;
                }
                let here: Vec<&str> = g
                    .parts
                    .iter()
                    .map(|p| block.parts[*p].key.as_str())
                    .filter(|k| layout.placed.get(*k).is_some_and(|x| &x.sheet == sid))
                    .collect();
                if here.is_empty() {
                    continue;
                }
                let homes: BTreeSet<Option<&&str>> =
                    here.iter().map(|k| region_of.get(k)).collect();
                if homes.len() > 1 || homes.contains(&None) {
                    let shown: Vec<String> =
                        g.parts.iter().map(|p| part_shown(block, *p)).collect();
                    found.push(
                        f(
                            "sch_function_split",
                            format!(
                                "{}'s parts ({}) are in {} regions{}",
                                g.title,
                                shown.join(", "),
                                homes.iter().filter(|h| h.is_some()).count(),
                                if homes.contains(&None) {
                                    ", some in none"
                                } else {
                                    ""
                                }
                            ),
                            here.iter().map(|k| (*k).to_owned()).collect(),
                            json!({"function": g.title}),
                        )
                        .fixed(vec![vec![Instruction::Region {
                            title: g.title.clone(),
                            parts: here.iter().map(|k| (*k).to_owned()).collect(),
                        }]]),
                    );
                }
            }
        }
        for (id, g) in layout.regions.iter().filter(|(_, g)| &g.sheet == sid) {
            if g.title == "Misc" {
                found.push(f(
                    "sch_region_untitled",
                    format!("region {id} is titled Misc: give it its function's title"),
                    vec![id.clone()],
                    json!({"fix": format!("region rename {id} TITLE")}),
                ));
            }
        }
        match prefs.prefer.local_nets {
            LocalNets::Wires => {
                for x in found.iter_mut().filter(|x| x.code == "sch_floating") {
                    x.message.push_str(" (prefer.local_nets = wires)");
                    x.prefs.push("prefer.local_nets = wires".into());
                }
            }
            LocalNets::Labels => {
                let before = found.len();
                found.retain(|x| !matches!(x.code, "sch_floating" | "sch_label_where_wire"));
                notes.push(format!(
                    "prefer.local_nets = labels: sch_floating and sch_label_where_wire not judged ({} findings left out)",
                    before - found.len()
                ));
            }
        }
        // Weights and severities.
        found.retain_mut(|x| {
            let (severity, why) = prefs.severity(x.code);
            if severity == Severity::Off {
                return false;
            }
            let (weight, set) = prefs.weight(x.code);
            x.severity = severity;
            x.points = units(x) * weight;
            for p in [why, set].into_iter().flatten() {
                x.message.push_str(&format!(" ({p})"));
                x.prefs.push(p);
            }
            true
        });
        for (code, s) in &prefs.severity {
            if *s == Severity::Off {
                notes.push(format!("severity.{code} = off: not judged"));
            }
        }
        found.sort_by(|a, b| (a.code, &a.refs).cmp(&(b.code, &b.refs)));
        let penalty: i64 = found.iter().map(|x| x.points).sum();
        scores.push(Score {
            sheet: sid.clone(),
            score: (100 - penalty).clamp(0, 100) as u32,
            penalty,
            crossings,
            bends,
            wire_nm,
            density,
            extent: sheet_extent,
            fill: sheet_fill,
            findings: found.len(),
            notes,
        });
        findings.extend(found);
    }
    (findings, scores)
}

/// Whether the wires of a net touching the pin end `at` reach another of
/// the net's pin ends (`ends`), not only its own label's stub.
fn wired_to_another(runs: &[Vec<[Nm; 2]>], at: route::Node, ends: &[route::Node]) -> bool {
    let nodes: Vec<BTreeSet<route::Node>> = runs
        .iter()
        .map(|r| route::expand(r).into_iter().collect())
        .collect();
    let mut reached: BTreeSet<route::Node> = [at].into();
    let mut taken = vec![false; nodes.len()];
    loop {
        let mut grew = false;
        for (i, n) in nodes.iter().enumerate() {
            if !taken[i] && n.iter().any(|x| reached.contains(x)) {
                taken[i] = true;
                reached.extend(n.iter().copied());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    ends.iter().any(|e| *e != at && reached.contains(e))
}

/// How many times, in words: once, twice, then `n times`.
fn times(n: usize) -> String {
    match n {
        1 => "once".into(),
        2 => "twice".into(),
        n => format!("{n} times"),
    }
}

/// A part as messages name it: its designator, else its key.
/// The lines a part stands on, down and across: its body's middle, and a
/// two-pin part's axis through its pins too. `sch_out_of_line` asks that
/// each part share one with another, within half a step; `align` holds
/// when its parts share one.
pub fn part_lines(part: &crate::input::Part, sh: &Shape) -> (Vec<Nm>, Vec<Nm>) {
    let m = sh.body.center();
    let (mut xs, mut ys) = (vec![m[0]], vec![m[1]]);
    if part.pins.len() == 2 {
        for pin in &sh.pins {
            if pin.side.horizontal() {
                ys.push(pin.end[1]);
            } else {
                xs.push(pin.end[0]);
            }
        }
    }
    (xs, ys)
}

/// A net as messages name it: by its name, else its key.
fn net_shown(block: &Block, key: &str) -> String {
    block
        .net(key)
        .map_or_else(|| key.to_owned(), |n| block.nets[n].name.clone())
}

/// A part as messages name it: its designator, a port by its net's name
/// (`port:SDA`), else its key.
pub(crate) fn part_shown(block: &Block, p: usize) -> String {
    let part = &block.parts[p];
    if matches!(part.kind, PartKind::Port { .. })
        && let Some(n) = part.pins.first().and_then(|x| x.net)
    {
        return format!("port:{}", block.nets[n].name);
    }
    who(part)
}

pub(crate) fn who(part: &crate::input::Part) -> String {
    if part.refdes.is_empty() {
        part.key.clone()
    } else {
        part.refdes.clone()
    }
}

/// How a resistor is drawn, read from its graphics: a box, or a zigzag (a
/// polyline of six points or more); `None` for a part that isn't a
/// resistor of two pins drawn either way.
pub fn resistor_style(part: &crate::input::Part) -> Option<ResistorStyle> {
    let PartKind::Symbol { prefix } = &part.kind else {
        return None;
    };
    if part.pins.len() != 2
        || !(prefix == "R" || ["RN", "RM", "RA"].iter().any(|x| prefix.starts_with(x)))
    {
        return None;
    }
    use crate::input::Graphic;
    if part
        .graphics
        .iter()
        .any(|g| matches!(g, Graphic::Rect { .. }))
    {
        return Some(ResistorStyle::Iec);
    }
    if part
        .graphics
        .iter()
        .any(|g| matches!(g, Graphic::Polyline { points, .. } if points.len() >= 6))
    {
        return Some(ResistorStyle::Us);
    }
    None
}

/// A run's jogs: each two turns that leave it going the way it went
/// before them (a step aside, a Z).
pub fn jogs(run: &[[Nm; 2]]) -> usize {
    let dir = |a: [Nm; 2], b: [Nm; 2]| [(b[0] - a[0]).signum(), (b[1] - a[1]).signum()];
    let dirs: Vec<[Nm; 2]> = run.windows(2).map(|w| dir(w[0], w[1])).collect();
    dirs.windows(3)
        .filter(|w| w[0] == w[2] && w[0] != w[1])
        .count()
}

/// The length of a spanning tree through `pts`, by Manhattan distance.
fn spanning(pts: &[[Nm; 2]]) -> Nm {
    let mut joined = vec![false; pts.len()];
    let mut best = vec![Nm::MAX; pts.len()];
    let mut total = 0;
    if pts.is_empty() {
        return 0;
    }
    best[0] = 0;
    for _ in 0..pts.len() {
        let (i, d) = best
            .iter()
            .enumerate()
            .filter(|(i, _)| !joined[*i])
            .min_by_key(|(_, d)| **d)
            .map(|(i, d)| (i, *d))
            .expect("one left");
        joined[i] = true;
        total += d;
        for j in 0..pts.len() {
            let dd = (pts[i][0] - pts[j][0]).abs() + (pts[i][1] - pts[j][1]).abs();
            if !joined[j] && dd < best[j] {
                best[j] = dd;
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A part on a net that leaves its sheet, whose pin carries the net's
    /// label (its off-sheet connector until plan 2.C draws one), isn't
    /// floating: the judges don't fault a label on a net that leaves.
    #[test]
    fn a_label_on_a_net_leaving_the_sheet_doesnt_float_its_part() {
        use crate::layout::Instruction;
        let mut block = crate::testing::regions_block();
        // Put J1 on a sheet of its own: D+ leaves U1's sheet.
        block.instructions.insert(
            "i1".into(),
            Instruction::Sheet {
                parts: vec!["j1".into()],
                sheet: "Connectors".into(),
            },
        );
        block.parts.push(crate::testing::resistor(
            "r5",
            "R5",
            "33",
            [Some(6), Some(1)],
        ));
        block.instructions.insert(
            "i2".into(),
            Instruction::Net {
                net: "dp".into(),
                style: crate::layout::NetStyle::Label,
            },
        );
        let out = crate::engine::lay_out(&block).unwrap();
        let (found, _) = lint(&block, &out);
        assert!(
            !found
                .iter()
                .any(|f| f.code == "sch_floating" && f.refs.iter().any(|r| r == "r5")),
            "{found:#?}"
        );
    }

    #[test]
    fn paper_and_region_rules_teach_fit_and_function() {
        use crate::input::Paper;
        use crate::layout::{Orientation, Region};
        let mut block = crate::testing::regions_block();
        let a4 = Paper {
            size: "a4".into(),
            orientation: Orientation::Landscape,
            area: [277_000_000, 190_000_000],
        };
        let a3 = Paper {
            size: "a3".into(),
            orientation: Orientation::Landscape,
            area: [400_000_000, 277_000_000],
        };
        block.sheet = a3.area;
        block.papers = vec![a4, a3];
        let mut out = crate::engine::lay_out(&block).unwrap();
        let (found, _) = lint(&block, &out);
        assert!(
            !found.iter().any(|f| matches!(
                f.code,
                "sch_paper_fit" | "sch_function_split" | "sch_region_untitled"
            )),
            "{found:#?}"
        );
        let f = fill(&block, &out.layout, "s1");
        assert!(f > 0 && f <= 1000);
        // On A3 by hand, it would fit A4: sch_paper_fit, fixed by a relayout.
        out.layout.sheets.get_mut("s1").unwrap().size = None;
        let (found, _) = lint(&block, &out);
        let fit = found
            .iter()
            .find(|f| f.code == "sch_paper_fit")
            .expect("paper fit");
        assert!(fit.message.contains("a4"), "{}", fit.message);
        // A buck's inductor outside its region: its function is split.
        let mut split = crate::engine::lay_out(&block).unwrap();
        let conv = split
            .layout
            .regions
            .iter()
            .find(|(_, r)| r.title.ends_with("converter"))
            .map(|(id, _)| id.clone())
            .unwrap();
        split
            .layout
            .regions
            .get_mut(&conv)
            .unwrap()
            .parts
            .retain(|k| k != "l1");
        split.layout.regions.insert(
            "r9".into(),
            Region {
                sheet: "s1".into(),
                at: [0, 0],
                frame: Rect::new([0, 0], [1, 1]),
                title: "Misc".into(),
                named: false,
                parts: vec!["l1".into()],
            },
        );
        let (found, _) = lint(&block, &split);
        assert!(found.iter().any(|f| f.code == "sch_function_split"));
        assert!(found.iter().any(|f| f.code == "sch_region_untitled"));
    }

    /// Every rule says what it means, what a reader loses, how to fix it
    /// and what it rests on, in plain sentences.
    #[test]
    fn every_rule_teaches() {
        for r in RULES {
            for (field, text) in [
                ("what", r.what),
                ("why", r.why),
                ("fix", r.fix),
                ("evidence", r.evidence),
            ] {
                assert!(
                    text.len() >= 20 && text.ends_with('.'),
                    "{}: {field} is no sentence: {text:?}",
                    r.code
                );
            }
            assert!(r.code.starts_with("sch_") && r.weight >= 0);
        }
        let mut codes: Vec<&str> = RULES.iter().map(|r| r.code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), RULES.len(), "codes are unique");
    }
    use crate::testing::seeded;

    #[test]
    fn seeded_sheets_score_and_findings_are_stable() {
        let b = seeded(3, 6);
        let out = crate::engine::lay_out(&b).unwrap();
        let (findings, scores) = lint(&b, &out);
        assert_eq!(scores.len(), out.layout.sheets.len());
        assert!(scores.iter().all(|s| s.score <= 100));
        // Sorted and the same run to run.
        let (again, _) = lint(&b, &crate::engine::lay_out(&b).unwrap());
        assert_eq!(findings, again);
        let codes: Vec<&str> = findings.iter().map(|f| f.code).collect();
        let mut sorted = codes.clone();
        sorted.sort();
        assert_eq!(codes, sorted);
    }

    #[test]
    fn a_pin_is_wired_when_its_wires_reach_another_pin() {
        let g = GRID;
        let n = |p: [Nm; 2]| route::node_of(p);
        let (a, b, c) = ([0, 0], [6 * g, 0], [6 * g, 4 * g]);
        // A stub to its own label reaches no other pin.
        let stub = vec![vec![a, [2 * g, 0]]];
        assert!(!wired_to_another(&stub, n(a), &[n(a), n(b)]));
        // A wire to b does, and c through b's wire on to it.
        let wires = vec![vec![a, b], vec![c, [6 * g, 0]]];
        assert!(wired_to_another(&wires, n(a), &[n(a), n(b)]));
        assert!(wired_to_another(&wires, n(c), &[n(a), n(c)]));
    }

    #[test]
    fn a_part_joined_only_by_labels_floats() {
        let b = seeded(1, 2);
        let mut out = crate::engine::lay_out(&b).unwrap();
        // Every signal's wires taken away and each pin labelled instead.
        let wired: Vec<String> = out
            .layout
            .routes
            .keys()
            .filter(|k| b.net(k).is_some_and(|n| b.nets[n].kind == NetKind::Signal))
            .cloned()
            .collect();
        for key in &wired {
            out.layout.routes.remove(key);
        }
        let (findings, _) = lint(&b, &out);
        let floats: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.code == "sch_floating")
            .collect();
        assert!(!floats.is_empty());
        assert!(
            floats
                .iter()
                .all(|f| f.message.ends_with("(prefer.local_nets = wires)"))
        );
        // A project that prefers labels for local nets judges none.
        let mut prefs = Prefs::default();
        prefs.prefer.local_nets = LocalNets::Labels;
        let (labelled, scores) = lint_with(&b, &out, &prefs);
        assert!(!labelled.iter().any(|f| f.code == "sch_floating"));
        assert!(scores[0].notes[0].starts_with("prefer.local_nets = labels"));
    }

    #[test]
    fn a_sheet_scored_under_two_preferences_differs() {
        let b = seeded(1, 2);
        let out = crate::engine::lay_out(&b).unwrap();
        let (plain, plain_scores) = lint(&b, &out);
        // Seeded resistors are zigzags: under `iec` each is a finding.
        assert!(!plain.iter().any(|f| f.code == "sch_resistor_style"));
        let mut prefs = Prefs::default();
        prefs.prefer.resistor = ResistorStyle::Iec;
        prefs.weights.insert("sch_resistor_style".into(), 4);
        let (tuned, tuned_scores) = lint_with(&b, &out, &prefs);
        let style: Vec<&Finding> = tuned
            .iter()
            .filter(|f| f.code == "sch_resistor_style")
            .collect();
        assert!(!style.is_empty());
        for f in &style {
            assert!(f.message.contains("prefer.resistor = iec"), "{}", f.message);
            assert!(
                f.message.contains("weights.sch_resistor_style = 4"),
                "{}",
                f.message
            );
            assert_eq!(f.points, 4);
            assert_eq!(
                f.prefs,
                ["prefer.resistor = iec", "weights.sch_resistor_style = 4"]
            );
        }
        let total = |s: &[Score]| s.iter().map(|x| x.score).sum::<u32>();
        assert!(total(&tuned_scores) < total(&plain_scores));
        // A rule turned off drops its findings and says so.
        let mut off = Prefs::default();
        off.severity.insert("sch_crossings".into(), Severity::Off);
        let (quiet, quiet_scores) = lint_with(&b, &out, &off);
        assert!(!quiet.iter().any(|f| f.code == "sch_crossings"));
        assert!(quiet_scores.iter().all(|s| {
            s.notes
                .iter()
                .any(|n| n.contains("severity.sch_crossings = off"))
        }));
        // The defaults are lint itself.
        assert_eq!(lint_with(&b, &out, &Prefs::default()).0, plain);
        // Every rule has a weight.
        assert!(RULES.iter().all(|r| r.weight > 0));
    }

    #[test]
    fn a_part_out_of_line_and_a_bank_out_of_its_row_are_found() {
        let b = seeded(2, 3);
        let mut out = crate::engine::lay_out(&b).unwrap();
        // Capacitors serving no pin: one bank, whichever parts they stand
        // beside (each IC's own decoupling capacitor is a bank of its own).
        out.idioms.retain(|i| i.kind != IdiomKind::Decoupling);
        // A bank: two parts of one prefix between the same nets.
        let nets = |p: &crate::input::Part| {
            let mut v: Vec<usize> = p.pins.iter().filter_map(|x| x.drawn_net()).collect();
            v.sort();
            v.dedup();
            v
        };
        let (a, c) = (0..b.parts.len())
            .flat_map(|i| (0..b.parts.len()).map(move |j| (i, j)))
            .find(|(i, j)| {
                i < j
                    && b.parts[*i].pins.len() == 2
                    && b.parts[*i].kind == b.parts[*j].kind
                    && nets(&b.parts[*i]).len() == 2
                    && nets(&b.parts[*i]) == nets(&b.parts[*j])
                    && out.layout.placed[&b.parts[*i].key].sheet
                        == out.layout.placed[&b.parts[*j].key].sheet
            })
            .expect("a bank in the seeded block");
        let (ka, kc) = (b.parts[a].key.clone(), b.parts[c].key.clone());
        // Stand the whole bank in one row, turned alike, then move one off
        // it, and off every line.
        let first = out.layout.placed[&ka].clone();
        let row = first.at[1];
        for (j, part) in b.parts.iter().enumerate() {
            if part.kind == b.parts[a].kind && nets(part) == nets(&b.parts[a]) && j != a {
                let x = out.layout.placed.get_mut(&part.key).unwrap();
                if x.sheet == first.sheet {
                    x.at[1] = row;
                    x.rotation = first.rotation;
                    x.mirror = first.mirror;
                }
            }
        }
        let (lined, _) = lint(&b, &out);
        assert!(
            !lined
                .iter()
                .any(|f| f.code == "sch_bank_not_in_row" && f.refs.contains(&ka))
        );
        let at = out.layout.placed[&kc].at;
        out.layout.placed.get_mut(&kc).unwrap().at =
            [at[0] + 3 * GRID + GRID / 4, row - 7 * GRID - GRID / 4];
        let (moved, _) = lint(&b, &out);
        let bank = moved
            .iter()
            .find(|f| f.code == "sch_bank_not_in_row" && f.refs.contains(&kc))
            .expect("the bank out of its row");
        assert!(
            bank.message.contains("stand them in one row"),
            "{}",
            bank.message
        );
        let off = moved
            .iter()
            .find(|f| f.code == "sch_out_of_line" && f.refs == [kc.clone()])
            .expect("the part out of line");
        assert!(
            off.message.contains("lines up with no other part"),
            "{}",
            off.message
        );
    }

    #[test]
    fn a_pin_name_past_its_body_is_found() {
        let mut b = seeded(1, 2);
        let out = crate::engine::lay_out(&b).unwrap();
        let (before, _) = lint(&b, &out);
        assert!(!before.iter().any(|f| f.code == "sch_pin_name_outside"));
        let ic = b
            .parts
            .iter()
            .position(|p| crate::paint::draws_names(p) && !p.pins.is_empty())
            .unwrap();
        b.parts[ic].pins[0].name = "A_NAME_FAR_TOO_LONG_FOR_ANY_BODY".into();
        let (after, _) = lint(&b, &out);
        assert!(after.iter().any(|f| f.code == "sch_pin_name_outside"));
    }

    #[test]
    fn a_step_aside_is_a_jog() {
        let g = GRID;
        assert_eq!(jogs(&[[0, 0], [4 * g, 0]]), 0);
        assert_eq!(jogs(&[[0, 0], [4 * g, 0], [4 * g, 2 * g]]), 0);
        assert_eq!(
            jogs(&[[0, 0], [2 * g, 0], [2 * g, 2 * g], [4 * g, 2 * g]]),
            1
        );
        // Round a corner and back is a U, not a jog.
        assert_eq!(jogs(&[[0, 0], [2 * g, 0], [2 * g, 2 * g], [0, 2 * g]]), 0);
    }

    #[test]
    fn a_short_labelled_net_is_a_label_where_a_wire_would_do() {
        let b = seeded(1, 2);
        let mut out = crate::engine::lay_out(&b).unwrap();
        // Draw a wired net by two labels a step apart instead.
        let key = out
            .layout
            .routes
            .keys()
            .find(|k| !k.starts_with("rail"))
            .unwrap()
            .clone();
        out.layout.routes.remove(&key);
        for at in [[50 * GRID, 50 * GRID], [52 * GRID, 50 * GRID]] {
            out.layout
                .labels
                .entry(key.clone())
                .or_default()
                .push(crate::layout::Label {
                    sheet: "s1".into(),
                    at,
                    side: crate::geom::Side::Right,
                    kind: LabelKind::Net,
                    on: "x/1".into(),
                    text: "X".into(),
                });
        }
        let (findings, _) = lint(&b, &out);
        assert!(
            findings
                .iter()
                .any(|f| f.code == "sch_label_where_wire" && f.refs == [key.clone()])
        );
    }

    /// An IC and a resistor from its output to ground, laid out.
    fn flipped_case() -> (crate::input::Block, Outcome) {
        use crate::input::{Block, NetKind, PinKind};
        use crate::testing::{ic, net, resistor};
        let block = Block {
            nets: vec![
                net("rail:gnd", "GND", NetKind::Ground),
                net("a", "A", NetKind::Signal),
            ],
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "x",
                    &[("GND", PinKind::PowerIn, Some(0))],
                    &[("A", PinKind::Output, Some(1))],
                ),
                resistor("r1", "R1", "10k", [Some(1), Some(0)]),
            ],
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let out = crate::engine::lay_out(&block).unwrap();
        (block, out)
    }

    /// A finding whose rule an instruction fixes carries that batch; sent
    /// back, the finding goes.
    #[test]
    fn findings_carry_fixes_that_work() {
        let (block, mut out) = flipped_case();
        let was = out.layout.placed["r1"].clone();
        // Turned over, R1 stands with its ground end up.
        let x = out.layout.placed.get_mut("r1").unwrap();
        x.rotation = (x.rotation + 180) % 360;
        let (found, _) = lint(&block, &out);
        let f = found.iter().find(|f| f.code == "sch_part_flipped").unwrap();
        assert_eq!(
            f.fixes,
            vec![vec![Instruction::Orient {
                parts: vec!["r1".into()],
                rotation: was.rotation,
                mirror: was.mirror,
            }]]
        );
        let mut block = block;
        block
            .instructions
            .insert("i1".into(), f.fixes[0][0].clone());
        block.previous = Some(out.layout.clone());
        let again = crate::engine::lay_out(&block).unwrap();
        assert!(
            !lint(&block, &again)
                .0
                .iter()
                .any(|f| f.code == "sch_part_flipped")
        );
    }

    #[test]
    fn a_part_upside_down_against_its_rail_and_a_wrapping_wire_are_found() {
        use crate::geom::Place;
        let (block, mut out) = flipped_case();
        let (found, scores) = lint(&block, &out);
        assert!(
            !found.iter().any(|f| f.code == "sch_part_flipped"),
            "{found:#?}"
        );
        assert!(scores.iter().all(|s| s.penalty >= 0));
        // Turned over, R1 stands with its ground end up.
        let x = out.layout.placed.get_mut("r1").unwrap();
        x.rotation = (x.rotation + 180) % 360;
        let (found, _) = lint(&block, &out);
        assert!(
            found
                .iter()
                .any(|f| f.code == "sch_part_flipped" && f.refs == ["r1"]),
            "{found:#?}"
        );
        // A run from R1's pin round both sides of its body wraps it.
        let x = &out.layout.placed["r1"];
        let sh = Shape::of(&block.parts[1], Place::new(x.at, x.rotation, x.mirror));
        let (e, b) = (sh.pins[0].end, sh.body);
        let run = vec![
            e,
            [b.min[0] - 2 * GRID, e[1]],
            [b.min[0] - 2 * GRID, b.max[1] + 2 * GRID],
            [b.max[0] + 2 * GRID, b.max[1] + 2 * GRID],
            [b.max[0] + 2 * GRID, b.min[1] - 2 * GRID],
        ];
        let sid = x.sheet.clone();
        out.layout
            .routes
            .insert("a".into(), [(sid, vec![run])].into_iter().collect());
        let (found, _) = lint(&block, &out);
        assert!(
            found.iter().any(|f| f.code == "sch_wire_wraps"),
            "{found:#?}"
        );
    }
}
