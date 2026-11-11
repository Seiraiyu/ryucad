//! The requests the CLI and the daemon share, and what `version` says:
//! one definition, so they never diverge.

use crate::error::{Code, Error, Result};
use ryu_session::live::Kind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use ryu_session::rpc::Answer;

/// The protocol version. A CLI and a daemon talk only when theirs match.
pub const PROTOCOL_VERSION: u32 = 1;

/// What `ryueda version` reports. Its `ryueda` key is how a client tells
/// a ryueda daemon from another product's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub ryueda: String,
    pub commit: String,
    pub target: String,
    pub protocol: u32,
}

impl VersionInfo {
    /// This binary's version information.
    pub fn current() -> Self {
        Self {
            ryueda: env!("CARGO_PKG_VERSION").to_owned(),
            commit: env!("RYUEDA_COMMIT").to_owned(),
            target: env!("RYUEDA_TARGET").to_owned(),
            protocol: PROTOCOL_VERSION,
        }
    }

    /// Whether `v` (a `version` object as JSON) is the same build: the same
    /// ryueda, commit and protocol. The target may differ, as ryucad's may.
    pub fn same_build(&self, v: &Value) -> bool {
        let me = json!(self);
        ["ryueda", "commit", "protocol"]
            .iter()
            .all(|k| v.get(k) == me.get(k))
    }
}

/// Every request a session answers, by JSON-RPC method name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum Request {
    /// The project, its libraries and their revisions.
    #[serde(rename = "project.info")]
    ProjectInfo {},
    /// The project's properties (text sets, `null` removes) and its
    /// schematic settings: the sheet's paper size and orientation.
    #[serde(rename = "project.edit")]
    ProjectEdit {
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        properties: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet_size: Option<crate::project::PaperSize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        orientation: Option<crate::project::Orientation>,
        /// Paper fit on or off (plan 2.A).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<bool>,
        /// The papers paper fit takes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sizes: Option<Vec<crate::project::PaperSize>>,
        /// The library alias whose style.json is the organisation's
        /// style; "" for none (plan 2.E).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style_from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Names a library project by alias, at a path relative to this one.
    #[serde(rename = "lib.add")]
    LibAdd {
        alias: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Every library with its revision, what it holds and its faults.
    #[serde(rename = "lib.list")]
    LibList {},
    /// A library's symbols judged by the standards that apply to it (this
    /// project's own library by default, or one symbol).
    #[serde(rename = "lib.check")]
    LibCheck {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbol: Option<String>,
    },
    /// A symbol called `name`, its ID from the name unless given, with
    /// `units` (1 by default) and a batch of ops applied.
    #[serde(rename = "symbol.create")]
    SymbolCreate {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        units: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ops: Vec<Value>,
        /// The symbol's class (`resistor`, `ic`, …); on an edit `none`
        /// takes it off.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        /// Draw it from the standards' template for this class first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        /// The pins a box template places (`number`, `name`, `type`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pins: Vec<crate::template::PinSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A batch of `add`, `remove`, `set` and `move` ops on a symbol's
    /// pins and graphics, and its name and unit count.
    #[serde(rename = "symbol.edit")]
    SymbolEdit {
        symbol: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        units: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ops: Vec<Value>,
        /// The symbol's class (`resistor`, `ic`, …); on an edit `none`
        /// takes it off.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "symbol.show")]
    SymbolShow { symbol: String },
    #[serde(rename = "symbol.list")]
    SymbolList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<String>,
    },
    #[serde(rename = "symbol.delete")]
    SymbolDelete {
        symbol: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "footprint.show")]
    FootprintShow { footprint: String },
    #[serde(rename = "footprint.list")]
    FootprintList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<String>,
    },
    /// A library component, its ID made from `name` unless given.
    #[serde(rename = "libcomp.create")]
    LibcompCreate {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        fields: crate::libcmd::CompFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The fields given replace the component's; its parameters merge.
    #[serde(rename = "libcomp.edit")]
    LibcompEdit {
        libcomp: String,
        #[serde(default, flatten)]
        fields: crate::libcmd::CompFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "libcomp.show")]
    LibcompShow { libcomp: String },
    #[serde(rename = "libcomp.list")]
    LibcompList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<String>,
    },
    #[serde(rename = "libcomp.delete")]
    LibcompDelete {
        libcomp: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Parts by category and typed conditions: the project's libraries
    /// (6.A); outside sources come with later plans behind `in`.
    #[serde(rename = "parts.search")]
    PartsSearch {
        #[serde(default, flatten)]
        search: crate::parts::Search,
    },
    /// Downloads a catalogue into this machine's parts cache: long, in
    /// phase `fetch`, cancelled between files.
    #[serde(rename = "parts.sync")]
    PartsSync { source: String },
    /// Every source of parts, and each catalogue's state on this machine.
    #[serde(rename = "parts.sources")]
    PartsSources {},
    /// Syncs each catalogue the placed parts' offers draw on that is
    /// missing or past `parts.max_age`, or every one with `force`: long,
    /// as `parts.sync` is.
    #[serde(rename = "parts.refresh")]
    PartsRefresh {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    /// Sets the project's sourcing in `parts.json`: `assembly` (`jlcpcb`,
    /// `null` for none), `build_quantity` (`null` for 1), `sources` (the
    /// whole list, `[]` for none), severities by supply code (`null` for
    /// the default). Nothing given answers it as it is.
    #[serde(rename = "parts.configure")]
    PartsConfigure {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assembly: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build_quantity: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sources: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        severity: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The records a key names (`lcsc:C1525`, `mpn:<part number>`,
    /// `library:<reference>`). Catalogues never fetch per part, so
    /// `refresh` says how to refresh one and `offline` changes nothing.
    #[serde(rename = "parts.show")]
    PartsShow {
        key: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        refresh: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        offline: bool,
    },
    /// KiCad 6 to 9 libraries (`.kicad_sym` files, `.pretty` and
    /// `.3dshapes` folders, by absolute path) into this project's
    /// library, or with `into` a library project's it names.
    #[serde(rename = "import.kicad")]
    ImportKicad {
        paths: Vec<String>,
        /// Where to look for footprints symbols name.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        footprints: Vec<String>,
        /// What KiCad's model path variables stand for.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        models: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        into: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A project-wide net, its ID random unless given.
    #[serde(rename = "rail.add")]
    RailAdd {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        intent: crate::topcmd::IntentArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Adds and removes tags on a component, a net or a rail (plan 2.E).
    #[serde(rename = "tag")]
    Tag {
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        add: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A rail's name, class or intent; an empty text clears a field.
    #[serde(rename = "rail.edit")]
    RailEdit {
        rail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, flatten)]
        intent: crate::topcmd::IntentArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "rail.delete")]
    RailDelete {
        rail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "rail.list")]
    RailList {},
    #[serde(rename = "class.add")]
    ClassAdd {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "class.edit")]
    ClassEdit {
        class: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "class.delete")]
    ClassDelete {
        class: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "class.list")]
    ClassList {},
    /// A design: a name and its root block.
    #[serde(rename = "design.create")]
    DesignCreate {
        name: String,
        root: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "design.edit")]
    DesignEdit {
        design: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "design.delete")]
    DesignDelete {
        design: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "design.list")]
    DesignList {},
    /// A functional block, its ID random unless given.
    #[serde(rename = "block.create")]
    BlockCreate {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A block's name and description, and ops on its ports and buses.
    #[serde(rename = "block.edit")]
    BlockEdit {
        block: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ports: Vec<crate::topcmd::PortOp>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        buses: Vec<crate::topcmd::BusOp>,
        /// Defaults for every placement: text sets, `null` removes.
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        properties: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A block with what it holds, its faults and revision.
    #[serde(rename = "block.show")]
    BlockShow { block: String },
    #[serde(rename = "block.list")]
    BlockList {},
    /// A block's folder, refused while instanced or a design's root.
    #[serde(rename = "block.delete")]
    BlockDelete {
        block: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A library component placed in a block; its refdes the prefix and
    /// the project's next free number unless given.
    #[serde(rename = "component.add")]
    ComponentAdd {
        block: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        fields: crate::partcmd::ComponentFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The fields given replace the component's; its parameters merge.
    #[serde(rename = "component.edit")]
    ComponentEdit {
        component: String,
        #[serde(default, flatten)]
        fields: crate::partcmd::ComponentFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "component.delete")]
    ComponentDelete {
        component: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "component.show")]
    ComponentShow { component: String },
    /// A pin (`<block>/<component>/<pin>`) onto a net, `rail:<name>`,
    /// `port:<name>`, or with `new` a new net of the block by that name.
    #[serde(rename = "connect")]
    Connect {
        pin: String,
        net: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        new: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A pin off its net, unconnected.
    #[serde(rename = "disconnect")]
    Disconnect {
        pin: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A pin marked not connected.
    #[serde(rename = "nc")]
    Nc {
        pin: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A net of a block, with its class and intent.
    #[serde(rename = "net.add")]
    NetAdd {
        block: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        intent: crate::topcmd::IntentArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A net's class, intent and pair (`pair_with` empty for none).
    #[serde(rename = "net.edit")]
    NetEdit {
        net: String,
        #[serde(default, flatten)]
        intent: crate::topcmd::IntentArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pair_with: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        polarity: Option<crate::conn::Polarity>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "net.rename")]
    NetRename {
        net: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Two nets become one: every pin on `net` moves `into` another net
    /// of its block or a rail, in one change.
    #[serde(rename = "net.join")]
    NetJoin {
        net: String,
        into: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A net nothing names.
    #[serde(rename = "net.delete")]
    NetDelete {
        net: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A net with the pins and ports on it.
    #[serde(rename = "net.show")]
    NetShow { net: String },
    /// A block placed in another, its ports connected there.
    #[serde(rename = "instance.add")]
    InstanceAdd {
        block: String,
        of: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        ports: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refdes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// An instance's ports, channel rule and overrides.
    #[serde(rename = "instance.edit")]
    InstanceEdit {
        instance: String,
        #[serde(default, flatten)]
        fields: crate::partcmd::InstanceFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "instance.delete")]
    InstanceDelete {
        instance: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "instance.show")]
    InstanceShow { instance: String },
    /// A design's flattened netlist: the project's one design when none
    /// is named.
    #[serde(rename = "netlist")]
    Netlist {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        design: Option<String>,
    },
    /// ERC over every design, or the one named.
    #[serde(rename = "erc")]
    Erc {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        design: Option<String>,
    },
    /// Accepts a finding with a reason (in `erc.json`), or with `remove`
    /// withdraws a waiver.
    #[serde(rename = "erc.waive")]
    ErcWaive {
        finding: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        remove: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Each ERC code's severity: `error`, `warning`, `off`, or `null` for
    /// its default.
    #[serde(rename = "erc.configure")]
    ErcConfigure {
        severity: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The schematic lint's preferences (`lint.json`) and its rules; with
    /// `explain`, one rule and the project's weight and severity for it.
    #[serde(rename = "lint.get")]
    LintGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explain: Option<String>,
    },
    /// Sets the lint's preferences: weights and severities by rule, and
    /// preferred styles; `null` for a default.
    #[serde(rename = "lint.configure")]
    LintConfigure {
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        weights: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        severity: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        prefer: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
        /// What the preferences would change, written nowhere.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
    },
    /// The style: the project's, the organisation's, the built-in one.
    #[serde(rename = "style.get")]
    StyleGet {},
    /// One object's properties and where each came from.
    #[serde(rename = "style.explain")]
    StyleExplain {
        target: String,
        block: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
    },
    /// Properties on an object's override, or on a selector's rule.
    #[serde(rename = "style.set")]
    StyleSet {
        /// The agent's name for an object (with `block`), `rail:NAME`, or
        /// a selector `key=value,…` (a rule).
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block: Option<String>,
        /// Property to its text, in order.
        set: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Properties off an object's override or a selector's rule.
    #[serde(rename = "style.unset")]
    StyleUnset {
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block: Option<String>,
        names: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// `add` a rule (`select`, `set`) at the end, or `remove` the one at
    /// `index`.
    #[serde(rename = "style.rule")]
    StyleRule {
        op: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        set: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A palette entry: `#light/#dark`, or `default` to remove it.
    #[serde(rename = "style.palette")]
    StylePalette {
        name: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The standards that judge a library's symbols (this project's own by
    /// default), whole, and where they come from.
    #[serde(rename = "standards.get")]
    StandardsGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<String>,
    },
    /// A JSON merge patch on this project's own standards
    /// (`library/standards.json`): `null` puts a field back.
    #[serde(rename = "standards.set")]
    StandardsSet {
        patch: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Lays out a design's blocks (or one block) over their layout files,
    /// writing those that change; `relayout` (`<block>` or
    /// `<block>/<sheet>`) lays out from nothing what it names.
    #[serde(rename = "layout")]
    Layout {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        design: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        relayout: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
        /// A dry run's picture of one block, its bytes after the answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render: Option<crate::schemcmd::RenderAsk>,
    },
    /// A batch of instructions as text (`beside C1 U1 left`, `release
    /// C1`), lasting, and moves applied once and held, all or nothing.
    #[serde(rename = "layout.place")]
    LayoutPlace {
        block: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        instructions: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        moves: Vec<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
        /// A dry run's picture, its bytes after the answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render: Option<crate::schemcmd::RenderAsk>,
    },
    /// A block's lasting instructions as text, each with whether it holds.
    #[serde(rename = "layout.instructions")]
    LayoutInstructions { block: String },
    /// A design's blocks (or one block, or one sheet) in relative terms:
    /// neighbours, idioms, nets, instructions, lint and scores.
    #[serde(rename = "describe")]
    Describe {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        design: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
    },
    /// A block's canvas (a design's root by default) or one sheet, as SVG
    /// or PNG, its bytes after the answer.
    #[serde(rename = "render")]
    Render {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        design: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        /// Which occurrence's designators to draw: an instance path
        /// (`ch_b`) or a stack path (`B`), else the first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layer: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<crate::schemcmd::Format>,
        /// Pixels per millimetre (PNG; 4 by default).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f32>,
        /// Crop the view to what is drawn.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fit: bool,
    },
    /// A design's canvases paged into a PDF, its bytes after the answer:
    /// a page per sheet, a stack's sheets per layer (`all`) or once
    /// (`first`).
    #[serde(rename = "output")]
    Output {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        design: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layers: Option<crate::output::Layers>,
    },
    /// The project loaded from nothing: every file's faults and every
    /// design's ERC; `check_failed` on any error.
    #[serde(rename = "check")]
    Check {
        /// Stay off the network: check never fetches (its supply findings
        /// read the catalogues this machine holds), so it changes nothing
        /// yet.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        offline: bool,
    },
    #[serde(rename = "activity.log")]
    ActivityLog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
    },
    #[serde(rename = "undo")]
    Undo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<u64>,
    },
    #[serde(rename = "redo")]
    Redo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<u64>,
    },
    #[serde(rename = "session.hello")]
    SessionHello {
        kind: Kind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    #[serde(rename = "participant.list")]
    ParticipantList {},
    #[serde(rename = "subscribe")]
    Subscribe {
        #[serde(default)]
        topics: Vec<String>,
    },
    #[serde(rename = "daemon.status")]
    DaemonStatus {},
    /// Cancels the caller's running long request named `token` (the
    /// connection's participant's, or on a connection that said no hello
    /// the agent `as`'s). Answered without waiting for the request.
    #[serde(rename = "request.cancel")]
    RequestCancel {
        token: String,
        #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    #[serde(rename = "daemon.shutdown")]
    DaemonShutdown {},
}

impl Request {
    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("requests serialize")
    }

    /// Reads a request from its method and params.
    pub fn from_rpc(method: &str, params: Option<Value>) -> Result<Request> {
        let params = params.unwrap_or_else(|| json!({}));
        serde_json::from_value(json!({"method": method, "params": params})).map_err(|e| {
            let msg = e.to_string();
            let code = if msg.starts_with("unknown variant") {
                Code::Usage
            } else {
                Code::InvalidDocument
            };
            Error::new(code, format!("{method}: {msg}")).with("method", method)
        })
    }

    pub fn method(&self) -> String {
        self.to_value()["method"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    pub fn params(&self) -> Value {
        self.to_value()["params"].take()
    }

    /// Whether the request writes the project's files.
    pub fn mutates(&self) -> bool {
        matches!(
            self,
            Request::LibAdd { .. }
                | Request::ProjectEdit { .. }
                | Request::SymbolCreate { .. }
                | Request::SymbolEdit { .. }
                | Request::SymbolDelete { .. }
                | Request::LibcompCreate { .. }
                | Request::LibcompEdit { .. }
                | Request::LibcompDelete { .. }
                | Request::ImportKicad { .. }
                | Request::RailAdd { .. }
                | Request::RailEdit { .. }
                | Request::Tag { .. }
                | Request::StyleSet { .. }
                | Request::StyleUnset { .. }
                | Request::StyleRule { .. }
                | Request::StylePalette { .. }
                | Request::RailDelete { .. }
                | Request::ClassAdd { .. }
                | Request::ClassEdit { .. }
                | Request::ClassDelete { .. }
                | Request::DesignCreate { .. }
                | Request::DesignEdit { .. }
                | Request::DesignDelete { .. }
                | Request::BlockCreate { .. }
                | Request::BlockEdit { .. }
                | Request::BlockDelete { .. }
                | Request::ComponentAdd { .. }
                | Request::ComponentEdit { .. }
                | Request::ComponentDelete { .. }
                | Request::Connect { .. }
                | Request::Disconnect { .. }
                | Request::Nc { .. }
                | Request::NetAdd { .. }
                | Request::NetEdit { .. }
                | Request::NetRename { .. }
                | Request::NetJoin { .. }
                | Request::NetDelete { .. }
                | Request::InstanceAdd { .. }
                | Request::InstanceEdit { .. }
                | Request::InstanceDelete { .. }
                | Request::ErcWaive { .. }
                | Request::ErcConfigure { .. }
                | Request::LintConfigure { .. }
                | Request::StandardsSet { .. }
                | Request::PartsConfigure { .. }
                | Request::Layout { .. }
                | Request::LayoutPlace { .. }
                | Request::Undo { .. }
                | Request::Redo { .. }
        )
    }
}

impl ryu_session::client::Call for Request {
    fn method(&self) -> String {
        Request::method(self)
    }

    fn params(&self) -> Value {
        Request::params(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_requests_are_named_and_cancelled_by_token() {
        use ryu_session::Serve;
        assert_eq!(
            crate::Eda::LONG,
            ["parts.sync", "parts.refresh", "parts.search", "parts.show"]
        );
        let r = Request::from_rpc(
            "request.cancel",
            Some(json!({"token": "t1", "as": "agent"})),
        )
        .unwrap();
        assert!(!r.mutates());
        assert!(matches!(
            <crate::Eda as Serve>::control(&r),
            ryu_session::daemon::Control::Cancel { ref token, ref name }
                if token == "t1" && name.as_deref() == Some("agent")
        ));
        let topics =
            ryu_session::notify::topics::<crate::notify::Topic>(&["progress".to_owned()]).unwrap();
        assert_eq!(topics, [crate::notify::Topic::Progress]);
    }

    #[test]
    fn the_same_build_may_have_another_target() {
        let me = VersionInfo::current();
        let mut v = json!(me);
        assert!(me.same_build(&v));
        v["target"] = json!("x86_64-unknown-linux-musl");
        assert!(me.same_build(&v));
        v["commit"] = json!("0123456789ab");
        assert!(!me.same_build(&v));
    }

    #[test]
    fn requests_round_trip_through_rpc() {
        let reqs = [
            Request::ProjectInfo {},
            Request::LibAdd {
                alias: "passives".into(),
                path: "../passives".into(),
                expected_revision: Some("r".into()),
            },
            Request::Undo { entry: None },
            Request::Subscribe {
                topics: vec!["project".into()],
            },
        ];
        for r in reqs {
            assert_eq!(Request::from_rpc(&r.method(), Some(r.params())).unwrap(), r);
        }
        assert_eq!(Request::Undo { entry: None }.method(), "undo");
        assert_eq!(
            Request::from_rpc("nope", None).unwrap_err().code,
            Code::Usage
        );
        let e = Request::from_rpc("lib.add", Some(json!({"alias": 1}))).unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
        // A library component's fields are flattened into the params, and
        // a misspelt one is still refused.
        let e = Request::from_rpc(
            "libcomp.create",
            Some(json!({"name": "x", "colour": "red"})),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
        assert!(e.message.contains("colour"), "{}", e.message);
    }

    #[test]
    fn a_ryucad_version_is_never_ryuedas() {
        let me = VersionInfo::current();
        let ryucad = json!({"ryucad": me.ryueda, "ryukei": "0.0.0", "commit": me.commit, "target": me.target, "protocol": 2});
        assert!(!me.same_build(&ryucad));
        assert!(json!(me).get("ryucad").is_none());
    }
}
