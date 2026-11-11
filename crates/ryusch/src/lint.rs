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
/// `sch_off_sheet_local`'s weight: a net's connector where it stays, or a
/// region of a net that leaves with none.
pub const OFF_SHEET_LOCAL: i64 = 4;
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
        what: "A part of an idiom (a decoupling capacitor, a pull resistor, a divider, a crystal's capacitors) stands more than 16 grid steps from the part it serves; a bypass capacitor on its pin's line and a pull wired on its pin's row are the pin's however far out.",
        why: "An idiom reads at a glance only drawn together; scattered, the reader must work out again what each part is for.",
        fix: "Bring the part back (near X P, with P the pin it serves), or re-form the idiom (idiom X KIND).",
        evidence: "Design 236 names the idioms the engine draws and their canonical shapes, after the drafting convention of drawing decoupling at its pin and pull resistors beside the pin they pull. In round 2's blind review (plan 1.D, three judges a pair), the idiom rules' points fell on the Ethernet PHY (42 to 30) and the judges' idioms score rose (+0.7); against the hand-drawn references the judges scored our idioms 2 to 2.3 of 5 and the references' 3.7 to 5. A pull wired on its pin's row reads as the pin's however far out it stands (design 348, #429: banks past a column of flags).",
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
        what: "A part's signal pins all reach their nets by labels, though one of those nets has another pin on its sheet and none leaves it, or a pin on a net that leaves the sheet carries no label of its own and is wired to none of its region's off-sheet connectors.",
        why: "A part joined to nothing by a wire floats: the reader can't see what it belongs to without matching label names across the sheet.",
        fix: "Stand it beside the part it serves (near X P, with P the pin on its net), or ask for the net's wires (net N wire).",
        evidence: "Round 1's sessions and work item 4530 named floating parts as the corpus's main fault (35 laid out from nothing before plan 1.D, 16 after); a local part reads as part of its circuit only when wired to it. In round 2's blind review (plan 1.D, three judges a pair), the judges preferred the RP2040 split by function onto three sheets 3 to 0 though this rule's points rose from 32 to 80: they don't fault a label on a net that leaves its sheet, so since plan 2.A (design 348) such a label is the net's off-sheet connector and joins its pin; since plan 2.C (design 348) a pin wired to its region's off-sheet connector, not to a local label, is joined.",
    },
    Rule {
        code: "sch_bus_unbundled",
        weight: 2,
        unit: "a member",
        what: "Four or more members of a bus run between the same two parts' sides on a sheet but are drawn as separate labels or wires.",
        why: "A bus drawn as one bundle reads as one interface; drawn member by member it reads as unrelated signals and costs a label or a wire each.",
        fix: "Ask for the bus (net BUS bus); if it can't be drawn, the note bus_not_drawn says why.",
        evidence: "Design 348's grammar (figure 5): a bus of four members or more between two regions or parts is drawn as a bus wire with entries, as KiCad's buses and the hand-drawn references' data buses are. A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sch_off_sheet_local",
        weight: OFF_SHEET_LOCAL,
        unit: "a net",
        what: "An off-sheet connector on a net with no pin on another sheet, or a net that continues on another sheet drawn in some region of this one by local labels alone.",
        why: "The connector's outline is the reader's sign that a net leaves the sheet: on a net that stays it sends them looking for a page that isn't there, and where it is missing they take a net that continues for one that doesn't.",
        fix: "Lay the sheet out again (layout --relayout B/S), or ask for the net's wires where it stays (net N wire).",
        evidence: "Design 348's grammar (figure 3) and the owner's comment #11 on 1.B's renders: a net that leaves its sheet is drawn with a connector distinct from a local label. IEEE 315 and IEC 60617 both draw off-sheet connectors apart from net names. A drafting convention; no round has judged it yet.",
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
        fix: "Lay the sheet out again so paper fit chooses (layout --relayout BLOCK/SHEET); a paper of your own choosing can be pinned (paper SHEET a4).",
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
        "sch_bus_unbundled" => f.details["members"].as_i64().unwrap_or(1),
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
pub(crate) fn ink_boxes(block: &Block, layout: &crate::layout::Layout, sid: &str) -> Vec<Rect> {
    let mut out = Vec::new();
    for part in &block.parts {
        let Some(x) = layout.placed.get(&part.key).filter(|x| x.sheet == sid) else {
            continue;
        };
        out.push(Shape::of(part, Place::new(x.at, x.rotation, x.mirror)).extent);
        if let Some(fields) = layout.text.get(&part.key) {
            for (field, line) in part.fields() {
                if let Some(at) = fields.get(field) {
                    out.push(line.boxed(*at));
                }
            }
        }
    }
    for (key, list) in &layout.labels {
        for l in list.iter().filter(|l| l.sheet == sid) {
            out.push(label::boxed(block, key, l));
        }
    }
    for by_sheet in layout.routes.values() {
        for run in by_sheet.get(sid).into_iter().flatten() {
            for w in run.windows(2) {
                out.push(Rect::new(w[0], w[1]));
            }
        }
    }
    // Buses' entries, spines, trunks and connectors, and notes (plan 2.C).
    for by in layout.buses.values() {
        if let Some(bus) = by.get(sid) {
            let stub = bus.stub;
            let mut ends: Vec<crate::geom::Point> = Vec::new();
            for e in &bus.entries {
                let (_, on_spine) = crate::bus::entry(e.at, e.side, stub);
                out.push(Rect::new(e.at, on_spine));
                ends.push(on_spine);
                if let Some(n) = block.net(&e.net) {
                    out.push(label::label_box(
                        LabelKind::Net,
                        &block.nets[n].name,
                        e.at,
                        e.side,
                    ));
                }
            }
            let mut columns: BTreeMap<Nm, Vec<crate::geom::Point>> = BTreeMap::new();
            for p in ends {
                columns.entry(p[0]).or_default().push(p);
            }
            for pts in columns.values() {
                if let Some([a, b]) = crate::bus::spine(pts) {
                    out.push(Rect::new(a, b));
                }
            }
            for run in &bus.trunk {
                for w in run.windows(2) {
                    out.push(Rect::new(w[0], w[1]));
                }
            }
            if let Some(c) = &bus.connector {
                out.push(label::label_box(LabelKind::OffSheet, &c.text, c.at, c.side));
            }
        }
    }
    for (id, n) in &layout.notes {
        if n.sheet == sid
            && let Some(text) = block.note_text(id)
        {
            out.push(crate::paint::note_box(text, n.at));
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
                    LabelKind::Net | LabelKind::OffSheet if !l.side.horizontal() => {
                        ("sch_label_vertical", "label")
                    }
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
                // A stack's pin on a net per layer is labelled, never
                // wired: one wire can't carry several nets (plan 2.G).
                if block.nets[n].kind != NetKind::Signal || pin.stacked() {
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
                    // Its own label, or its bus's entry, or wired to one of
                    // the net's off-sheet connectors here (plan 2.C).
                    let on = format!("{}/{}", part.key, pin.number);
                    let key = &block.nets[n].key;
                    if layout.buses.values().any(|by| {
                        by.get(sid)
                            .is_some_and(|b| b.entries.iter().any(|e| e.on == on && &e.net == key))
                    }) {
                        wired = true;
                    }
                    let labels: Vec<route::Node> = layout
                        .labels
                        .get(key)
                        .into_iter()
                        .flatten()
                        .filter(|l| &l.sheet == sid)
                        .filter_map(|l| {
                            if l.on == on {
                                wired = true;
                            }
                            (l.kind == LabelKind::OffSheet).then(|| route::node_of(l.at))
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
        // Off-sheet connectors where their nets stay, and regions of a net
        // that leaves with none (plan 2.C).
        off_sheet_local(block, &out.idioms, layout, sid, &shapes, &mut found, &f);
        // Buses whose members could be one bundle here, drawn apart.
        bus_unbundled(block, layout, sid, &shapes, &mut found, &f);
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
                for (field, line) in block.parts[*p].fields() {
                    if let Some(at) = fields.get(field) {
                        ink += line.boxed(*at).area();
                    }
                }
            }
        }
        for (key, list) in &layout.labels {
            for l in list.iter().filter(|l| &l.sheet == sid) {
                ink += label::boxed(block, key, l).area();
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
                // So does a pull wired on its pin's row (a bank's member, or
                // what hangs two steps off its row), however far out (#429).
                if pull && i == 0 && wired_on_row(block, layout, sid, &shapes, idiom, *p) {
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
                // As paper fit measures a drawing: each part's room with
                // labels, and the regions' frames.
                .chain(shapes.keys().filter_map(|p| {
                    let x = layout.placed.get(&block.parts[*p].key)?;
                    let pl = Place::new(x.at, x.rotation, x.mirror);
                    Some(crate::cluster::Cluster::single(*p, pl).room_with_labels(block, [0, 0]))
                }))
                .chain(
                    layout
                        .regions
                        .values()
                        .filter(|g| &g.sheet == sid)
                        .map(|g| g.frame_on_sheet()),
                )
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
                // Its fix is a relayout, which fits the sheet's paper
                // again (pinning a smaller paper over the kept drawing
                // wouldn't hold it): text, not an instruction.
                found.push(f(
                    "sch_paper_fit",
                    format!(
                        "sheet {sid} is on {} though {why} (fill {sheet_fill} per mille): lay it out again (layout --relayout BLOCK/{sid})",
                        now.size
                    ),
                    vec![sid.clone()],
                    json!({
                        "size": now.size,
                        "smaller": to.size,
                        "fill": sheet_fill,
                        "extent": sheet_extent,
                        "fix": format!("layout --relayout BLOCK/{sid}"),
                    }),
                ));
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

/// `sch_bus_unbundled` on sheet `sid` (plan 2.C): each bus not drawn here
/// whose members make a bundle here as the engine finds one (placed
/// sides, nets that leave), its points two a member, its fix the bus
/// asked for.
fn bus_unbundled(
    block: &Block,
    layout: &crate::layout::Layout,
    sid: &str,
    shapes: &BTreeMap<usize, Shape>,
    found: &mut Vec<Finding>,
    f: &dyn Fn(&'static str, String, Vec<String>, Value) -> Finding,
) {
    let parts: Vec<usize> = shapes.keys().copied().collect();
    let here = crate::bus::sides_on_sheet(block, &parts);
    let leaves = |n: usize| {
        block.parts.iter().any(|q| {
            layout.placed.get(&q.key).is_some_and(|x| x.sheet != sid)
                && q.pins.iter().any(|x| x.nets().contains(&n))
        })
    };
    let side_of = |p: usize, k: usize| shapes[&p].pins[k].side;
    for (i, bus) in block.buses.iter().enumerate() {
        if layout
            .buses
            .get(&bus.key)
            .is_some_and(|b| b.contains_key(sid))
        {
            continue;
        }
        let Some(bundle) = crate::bus::bundle_with(block, i, &here, &leaves, &side_of) else {
            continue;
        };
        let name = crate::bus::name(block, i, &bundle.members);
        let mut refs = vec![bus.key.clone()];
        refs.extend(bundle.members.iter().map(|n| block.nets[*n].key.clone()));
        found.push(
            f(
                "sch_bus_unbundled",
                format!(
                    "{name}'s {} members run between the same parts' sides but are drawn apart (where it can't be drawn, the note bus_not_drawn says why)",
                    bundle.members.len()
                ),
                refs,
                json!({"members": bundle.members.len()}),
            )
            .fixed(vec![vec![Instruction::Net {
                net: bus.key.clone(),
                style: NetStyle::Bus,
            }]]),
        );
    }
}

/// `sch_off_sheet_local` on sheet `sid` (plan 2.C): each off-sheet
/// connector whose net has no pin on another sheet (its fix the net's
/// wires); and each net with a pin on another sheet, a stacked pin's net,
/// a ported bank row's ([`crate::idiom::BankRow::names`]) and a bus drawn
/// here aside, whose pins in some region of this sheet (a
/// part in no region a group of its own) carry no connector and are wired
/// to none.
fn off_sheet_local(
    block: &Block,
    idioms: &[crate::idiom::Idiom],
    layout: &crate::layout::Layout,
    sid: &str,
    shapes: &BTreeMap<usize, Shape>,
    found: &mut Vec<Finding>,
    f: &dyn Fn(&'static str, String, Vec<String>, Value) -> Finding,
) {
    let elsewhere = |n: usize| {
        block.parts.iter().any(|q| {
            layout.placed.get(&q.key).is_some_and(|x| x.sheet != sid)
                && q.pins.iter().any(|x| x.nets().contains(&n))
        })
    };
    for (key, list) in &layout.labels {
        let Some(n) = block.net(key) else { continue };
        for l in list
            .iter()
            .filter(|l| l.sheet == sid && l.kind == LabelKind::OffSheet)
        {
            if !elsewhere(n) {
                found.push(
                    f(
                        "sch_off_sheet_local",
                        format!(
                            "{} has an off-sheet connector at {} but no pin on another sheet",
                            block.nets[n].name, l.on
                        ),
                        vec![key.clone(), l.on.clone()],
                        json!({"why": "stays"}),
                    )
                    .fixed(vec![vec![Instruction::Net {
                        net: key.clone(),
                        style: NetStyle::Wire,
                    }]]),
                );
            }
        }
    }
    let region_of = layout.region_of();
    // A ported bank row's net keeps its labels in the row, as the engine
    // draws it: no connector is asked of it.
    let at: BTreeMap<usize, Place> = block
        .parts
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let x = layout.placed.get(&p.key)?;
            Some((i, Place::new(x.at, x.rotation, x.mirror)))
        })
        .collect();
    let rows = crate::idiom::bank_rows(block, idioms, &at);
    let bused: BTreeSet<&String> = layout
        .buses
        .values()
        .filter_map(|by| by.get(sid))
        .flat_map(|b| b.entries.iter().map(|e| &e.net))
        .collect();
    // Each leaving net's groups here: its pins' ends by region.
    let mut groups: BTreeMap<(usize, String), Vec<(String, route::Node)>> = BTreeMap::new();
    let mut stacked: BTreeSet<usize> = BTreeSet::new();
    for (p, sh) in shapes {
        let part = &block.parts[*p];
        if matches!(part.kind, PartKind::Port { .. }) {
            continue;
        }
        for (k, pin) in part.pins.iter().enumerate() {
            let Some(n) = pin.drawn_net() else { continue };
            if pin.stacked() {
                stacked.extend(pin.nets());
            }
            if block.nets[n].kind != NetKind::Signal || bused.contains(&block.nets[n].key) {
                continue;
            }
            let group = region_of
                .get(part.key.as_str())
                .map_or_else(|| part.key.clone(), |r| (*r).to_owned());
            groups.entry((n, group)).or_default().push((
                format!("{}/{}", part.key, pin.number),
                route::node_of(sh.pins[k].end),
            ));
        }
    }
    let mut leaves: BTreeMap<usize, bool> = BTreeMap::new();
    for ((n, group), pins) in &groups {
        if stacked.contains(n)
            || rows.iter().any(|r| r.names(*n))
            || !*leaves.entry(*n).or_insert_with(|| elsewhere(*n))
        {
            continue;
        }
        let key = &block.nets[*n].key;
        let connectors: Vec<&crate::layout::Label> = layout
            .labels
            .get(key)
            .into_iter()
            .flatten()
            .filter(|l| l.sheet == sid && l.kind == LabelKind::OffSheet)
            .collect();
        let carried = pins
            .iter()
            .any(|(on, _)| connectors.iter().any(|l| &l.on == on));
        let runs = layout
            .routes
            .get(key)
            .and_then(|r| r.get(sid))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let to: Vec<route::Node> = connectors.iter().map(|l| route::node_of(l.at)).collect();
        let wired = pins.iter().any(|(_, at)| wired_to_another(runs, *at, &to));
        if !carried && !wired {
            found.push(f(
                "sch_off_sheet_local",
                format!(
                    "{} continues on another sheet, but its pins in {group} carry no off-sheet connector",
                    block.nets[*n].name
                ),
                vec![key.clone(), group.clone()],
                json!({"why": "unmarked"}),
            ));
        }
    }
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

/// Whether pull `p` of `idiom` is wired to the pin it serves along that
/// pin's row: its pin on the pin's net within two steps of the row (on
/// it, or hanging off its wire), the two joined by the net's wires on
/// the sheet.
fn wired_on_row(
    block: &Block,
    layout: &crate::layout::Layout,
    sid: &str,
    shapes: &BTreeMap<usize, Shape>,
    idiom: &crate::idiom::Idiom,
    p: usize,
) -> bool {
    let Some((a, k)) = idiom.anchor else {
        return false;
    };
    let (Some(sa), Some(sp)) = (shapes.get(&a), shapes.get(&p)) else {
        return false;
    };
    if !sa.pins[k].side.horizontal() {
        return false;
    }
    let Some(n) = block.parts[a].pins[k].net else {
        return false;
    };
    let Some(kp) = block.parts[p].pins.iter().position(|x| x.net == Some(n)) else {
        return false;
    };
    let (row, end) = (sa.pins[k].end, sp.pins[kp].end);
    if (end[1] - row[1]).abs() > 2 * crate::geom::GRID {
        return false;
    }
    let runs: Vec<BTreeSet<route::Node>> = layout
        .routes
        .get(&block.nets[n].key)
        .and_then(|r| r.get(sid))
        .into_iter()
        .flatten()
        .map(|run| route::expand(run).into_iter().collect())
        .collect();
    // The runs joined to the pin, through shared nodes.
    let mut reached: BTreeSet<route::Node> = [route::node_of(row)].into();
    let mut taken = vec![false; runs.len()];
    loop {
        let mut grew = false;
        for (i, set) in runs.iter().enumerate() {
            if !taken[i] && set.iter().any(|x| reached.contains(x)) {
                taken[i] = true;
                reached.extend(set.iter().copied());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    reached.contains(&route::node_of(end)) && reached.len() > 1
}

/// Lays `fix` out over `out` as a dry run (its instructions added as
/// lasting ones) and judges it: `Ok` when the engine takes it, every
/// hard rule holds, the layout changes and `finding` (by code and refs)
/// is gone; else why not.
pub fn try_fix(
    block: &Block,
    out: &crate::engine::Outcome,
    finding: &Finding,
    fix: &[Instruction],
) -> Result<(), String> {
    let mut b = block.clone();
    b.previous = Some(out.layout.clone());
    b.moves.clear();
    b.relayout = crate::input::Relayout::None;
    for ins in fix {
        let id = crate::layout::next_id(b.instructions.keys());
        b.instructions.insert(id, ins.clone());
    }
    let tried = crate::engine::lay_out(&b).map_err(|e| e.message.clone())?;
    if let Some(v) = crate::rules::check(&b, &tried.layout).first() {
        return Err(format!("it breaks a hard rule ({}): {}", v.rule, v.message));
    }
    if tried.layout.placed == out.layout.placed
        && tried.layout.labels == out.layout.labels
        && tried.layout.routes == out.layout.routes
        && tried.layout.text == out.layout.text
    {
        return Err("it changes nothing".into());
    }
    let (after, _) = lint(&b, &tried);
    if after
        .iter()
        .any(|f| f.code == finding.code && f.refs == finding.refs)
    {
        return Err(format!("{} is still there after it", finding.code));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fix that holds is kept; an `align` that moves nothing, or a fix
    /// that leaves the finding, is dropped.
    #[test]
    fn fixes_are_tried_before_they_are_offered() {
        let block = crate::testing::with_stage(crate::testing::seeded(5, 2));
        let out = crate::engine::lay_out(&block).unwrap();
        let (found, _) = lint(&block, &out);
        let with_fix: Vec<&Finding> = found.iter().filter(|f| !f.fixes.is_empty()).collect();
        assert!(!with_fix.is_empty(), "the seed has a finding with a fix");
        for f in &with_fix {
            for fix in &f.fixes {
                // Whatever the verdict, it is the same twice.
                assert_eq!(try_fix(&block, &out, f, fix), try_fix(&block, &out, f, fix));
            }
        }
        // Lining a part up with itself changes nothing: dropped.
        let p = block.parts[1].key.clone();
        let idle = vec![Instruction::Align {
            parts: vec![p.clone(), p],
            axis: crate::layout::Axis::Row,
        }];
        assert!(try_fix(&block, &out, with_fix[0], &idle).is_err());
    }

    /// A pull wired on its pin's row is the pin's however far out along it
    /// (a bank past a column of flags, #429); moved off the row it is judged
    /// by its distance from the part, as before.
    #[test]
    fn a_pull_wired_on_its_row_is_in_reach() {
        let block = crate::testing::dense_block(false);
        let out = crate::engine::lay_out(&block).unwrap();
        // Push R1 and its row out by 20 steps: still on its row, and wired.
        let mut far = out.clone();
        let shift = 20 * GRID;
        if let Some(x) = far.layout.placed.get_mut("r1") {
            x.at[0] += shift;
        }
        // Its row's wire rebuilt from U1's pin to R1's moved signal pin.
        let end = |key: &str, net: usize| {
            let p = block.part(key).unwrap();
            let x = &far.layout.placed[key];
            let k = block.parts[p]
                .pins
                .iter()
                .position(|q| q.net == Some(net))
                .unwrap();
            Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror)).pins[k].end
        };
        let run = vec![end("u1", 2), end("r1", 2)];
        let runs = far
            .layout
            .routes
            .get_mut("s0")
            .unwrap()
            .values_mut()
            .next()
            .unwrap();
        *runs = vec![run];
        let (found, _) = lint(&block, &far);
        assert!(
            !found
                .iter()
                .any(|f| f.code == "sch_idiom_missed" && f.refs[0] == "r1"),
            "{:#?}",
            found
                .iter()
                .filter(|f| f.code == "sch_idiom_missed")
                .collect::<Vec<_>>()
        );
        assert!(
            !found
                .iter()
                .any(|f| f.code == "sch_floating" && f.refs.iter().any(|r| r == "r1")),
            "wired on its row, it floats nowhere"
        );
    }

    /// A part on a net that leaves its sheet, whose pin carries the net's
    /// label (its off-sheet connector until plan 2.C draws one), isn't
    /// floating: the judges don't fault a label on a net that leaves.
    #[test]
    fn a_label_on_a_net_leaving_the_sheet_doesnt_float_its_part() {
        use crate::layout::{IdiomChoice, Instruction, Label, LabelKind};
        let mut block = crate::testing::regions_block();
        block.parts.push(crate::testing::resistor(
            "r5",
            "R5",
            "33",
            [Some(6), Some(1)],
        ));
        // J1 (no jumper of a pull idiom here) on a sheet of its own: D+
        // leaves U1's and R5's.
        block.instructions.insert(
            "i1".into(),
            Instruction::Idiom {
                parts: vec!["j1".into(), "r5".into()],
                kind: IdiomChoice::None,
            },
        );
        block.instructions.insert(
            "i2".into(),
            Instruction::Sheet {
                parts: vec!["j1".into()],
                sheet: "Connectors".into(),
            },
        );
        let mut out = crate::engine::lay_out(&block).unwrap();
        let l = &mut out.layout;
        let sheet = l.placed["r5"].sheet.clone();
        assert_ne!(l.placed["j1"].sheet, sheet, "D+ leaves R5's sheet");
        // R5's D+ pin reaching its net by its own label alone.
        let p = block.part("r5").unwrap();
        let x = &l.placed["r5"];
        let end = crate::shape::Shape::of(
            &block.parts[p],
            crate::geom::Place::new(x.at, x.rotation, x.mirror),
        )
        .pins[0]
            .end;
        if let Some(by) = l.routes.get_mut("dp") {
            by.remove(&sheet);
        }
        let labels = l.labels.entry("dp".into()).or_default();
        labels.retain(|x| x.on != "r5/1");
        labels.push(Label {
            sheet: sheet.clone(),
            at: end,
            side: crate::geom::Side::Up,
            kind: LabelKind::Net,
            on: "r5/1".into(),
            text: "D+".into(),
        });
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

    /// A stack's pin carrying a net per layer can only be labelled (one
    /// wire can't carry several nets): a part whose every signal pin is
    /// such a pin isn't judged floating (plan 2.G; the front end's IV
    /// converter and the Ethernet circuit's status LEDs).
    #[test]
    fn a_stacked_pin_is_not_judged_floating() {
        let mut b = seeded(1, 2);
        let mut out = crate::engine::lay_out(&b).unwrap();
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
        let first = lint(&b, &out)
            .0
            .into_iter()
            .find(|f| f.code == "sch_floating")
            .expect("a part floats")
            .refs[0]
            .clone();
        let p = b.part(&first).unwrap();
        let signals: Vec<usize> = (0..b.nets.len())
            .filter(|n| b.nets[*n].kind == NetKind::Signal)
            .collect();
        for pin in &mut b.parts[p].pins {
            if let Some(n) = pin.net {
                let other = *signals.iter().find(|m| **m != n).unwrap();
                pin.layers = vec![Some(other)];
            }
        }
        assert!(
            !lint(&b, &out)
                .0
                .iter()
                .any(|f| f.code == "sch_floating" && f.refs[0] == first),
            "{first} is judged though every pin is a stack's"
        );
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
    fn resistors_are_judged_in_their_library_not_on_the_sheet() {
        let block = seeded(1, 2);
        let out = crate::engine::lay_out(&block).unwrap();
        assert!(!RULES.iter().any(|r| r.code == "sch_resistor_style"));
        // Whatever the preference, a sheet finds nothing of a resistor's
        // drawing: ryueda's symbol lint judges it once (sym_shape).
        let mut prefs = Prefs::default();
        prefs.prefer.resistor = ResistorStyle::Iec;
        prefs.weights.insert("sch_resistor_style".into(), 4);
        let (found, _) = lint_with(&block, &out, &prefs);
        assert!(found.iter().all(|f| f.code != "sch_resistor_style"));
    }

    #[test]
    fn a_sheet_scored_under_two_preferences_differs() {
        let b = seeded(1, 2);
        let out = crate::engine::lay_out(&b).unwrap();
        let (plain, _) = lint(&b, &out);
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

    #[test]
    fn off_sheet_connectors_where_nets_stay_or_leave_unmarked_are_found() {
        let b = crate::testing::leaving_block();
        let out = crate::engine::lay_out(&b).unwrap();
        let (found, _) = lint(&b, &out);
        assert!(
            !found.iter().any(|f| f.code == "sch_off_sheet_local"),
            "{found:?}"
        );
        // A connector on a net that stays: found, its fix the net's wires.
        let mut bad = out.clone();
        let (k, i) = bad
            .layout
            .labels
            .iter()
            .find_map(|(k, v)| {
                (k != "sig")
                    .then(|| {
                        v.iter()
                            .position(|l| l.kind == LabelKind::Net)
                            .map(|i| (k.clone(), i))
                    })
                    .flatten()
            })
            .expect("a local label");
        bad.layout.labels.get_mut(&k).unwrap()[i].kind = LabelKind::OffSheet;
        let (found, _) = lint(&b, &bad);
        let stays = found
            .iter()
            .find(|f| f.code == "sch_off_sheet_local" && f.refs.contains(&k))
            .expect("found");
        assert!(
            stays
                .fixes
                .iter()
                .flatten()
                .any(|i| matches!(i, Instruction::Net { net, style: NetStyle::Wire } if *net == k))
        );
        // A leaving net's region with no connector: found.
        let mut bad = out.clone();
        for l in bad.layout.labels.get_mut("sig").unwrap() {
            l.kind = LabelKind::Net;
        }
        let (found, _) = lint(&b, &bad);
        assert!(
            found
                .iter()
                .any(|f| f.code == "sch_off_sheet_local" && f.refs.iter().any(|r| r == "sig"))
        );
        // A pin wired to its region's connector doesn't float.
        let (found, _) = lint(&b, &out);
        assert!(
            !found
                .iter()
                .any(|f| f.code == "sch_floating" && f.refs.iter().any(|r| r == "r1" || r == "r2"))
        );
    }

    #[test]
    fn a_bus_drawn_as_labels_is_found() {
        let mut b = crate::testing::bus_block();
        b.instructions.insert(
            "i1".into(),
            Instruction::Net {
                net: "bus:q".into(),
                style: NetStyle::Label,
            },
        );
        let out = crate::engine::lay_out(&b).unwrap();
        let (found, _) = lint(&b, &out);
        let f = found
            .iter()
            .find(|f| f.code == "sch_bus_unbundled")
            .expect("found");
        assert!(f.refs.iter().any(|r| r == "bus:q"));
        assert_eq!(f.points, 8, "two a member");
        // Its fix asks for the bus.
        assert!(f.fixes.iter().flatten().any(
            |i| matches!(i, Instruction::Net { net, style: NetStyle::Bus } if net == "bus:q")
        ));
        // Drawn as a bus, not found.
        b.instructions.clear();
        let out = crate::engine::lay_out(&b).unwrap();
        assert!(
            !lint(&b, &out)
                .0
                .iter()
                .any(|f| f.code == "sch_bus_unbundled")
        );
    }
}
