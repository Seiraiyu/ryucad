//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code, as ryucad's is.

use clap::{Args, Parser, Subcommand};
use ryueda::error::{Code, Error, Result};
use ryueda::project::find_root;
use ryueda::proto::{Answer, Request, VersionInfo};
use ryueda::units::Unit;
use ryueda::{client, daemon, server};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ryueda",
    about = "Agent-driven electronics CAD",
    disable_version_flag = true
)]
struct Cli {
    /// Run as if started in this folder.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    dir: Option<PathBuf>,
    /// Indent the JSON output.
    #[arg(long, global = true)]
    pretty: bool,
    /// Run in this process instead of through the project's daemon (also
    /// RYUEDA_NO_DAEMON=1).
    #[arg(long, global = true)]
    no_daemon: bool,
    /// Act as the agent participant with this name (also
    /// RYUEDA_PARTICIPANT); `agent` by default.
    #[arg(long = "as", global = true, value_name = "NAME")]
    as_name: Option<String>,
    #[command(subcommand)]
    command: Command,
}

/// The revision a write is guarded by: refused with `conflict` when the
/// files have moved on since it was read.
#[derive(Args)]
struct Expect {
    #[arg(long = "expect", value_name = "REV")]
    expect: Option<String>,
}

// Parsed once per run; the size difference costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// This build's version.
    Version,
    /// Create a project in this folder, or with --library a library
    /// project, which other projects name with `lib add`.
    Init {
        /// The unit of lengths typed as bare numbers (mm by default).
        #[arg(long, value_parser = unit)]
        unit: Option<Unit>,
        #[arg(long)]
        library: bool,
    },
    /// The project, its libraries and their revisions.
    Info,
    /// Name library projects, and list the libraries.
    #[command(subcommand)]
    Lib(LibCmd),
    /// Create, edit, show, list or delete symbols: units, pins with type,
    /// side, place and length, and the body's graphics.
    #[command(subcommand)]
    Symbol(SymbolCmd),
    /// Show or list footprints (they come from import).
    #[command(subcommand)]
    Footprint(FootprintCmd),
    /// Create, edit, show, list or delete library components: a symbol
    /// tied to footprints, pin to pad, with the part's value, parameters,
    /// maker and supplier part numbers.
    #[command(subcommand)]
    Libcomp(LibcompCmd),
    /// Create, edit, show, list or delete functional blocks: their
    /// names, ports and buses.
    #[command(subcommand)]
    Block(BlockCmd),
    /// Add, edit, delete or show components: a library component placed
    /// in a block.
    #[command(subcommand)]
    Component(ComponentCmd),
    /// Connect a pin (<block>/<component>/<pin>) to a net (by ID or name),
    /// rail:<name> or port:<name>; --new makes a net of the block by that
    /// name.
    Connect {
        pin: String,
        net: String,
        #[arg(long)]
        new: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// Take a pin off its net.
    Disconnect {
        pin: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Mark a pin not connected.
    Nc {
        pin: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Add, edit, rename, join, delete or show nets of a block.
    #[command(subcommand)]
    Net(NetCmd),
    /// Add, edit, delete or show instances: blocks placed in blocks.
    #[command(subcommand)]
    Instance(InstanceCmd),
    /// A design's flattened netlist: every component at every instance
    /// path, and every net joined through the hierarchy.
    Netlist {
        /// By ID or name; the project's one design when left out.
        design: Option<String>,
    },
    /// ERC: every design's findings, or one design's; `erc waive` and
    /// `erc configure` write erc.json.
    #[command(args_conflicts_with_subcommands = true)]
    Erc {
        /// By ID or name; every design when left out.
        design: Option<String>,
        #[command(subcommand)]
        action: Option<ErcCmd>,
    },
    /// Add, edit, delete or list rails: project-wide nets (GND, 3V3).
    #[command(subcommand)]
    Rail(RailCmd),
    /// Add, edit, delete or list net classes.
    #[command(subcommand)]
    Class(ClassCmd),
    /// Create, edit, delete or list designs: a name and a root block.
    #[command(subcommand)]
    Design(DesignCmd),
    /// The daemon's activity log: every change it made or saw, newest
    /// last.
    History {
        #[arg(long)]
        limit: Option<usize>,
        /// Only entries before this one.
        #[arg(long)]
        before: Option<u64>,
    },
    /// Who is attached to the project's daemon.
    Participants,
    /// Undo your latest change, or the entry given.
    Undo { entry: Option<u64> },
    /// Redo your latest undo, or the entry given.
    Redo { entry: Option<u64> },
    /// Wait for the daemon's notifications.
    Watch {
        /// project, participants, activity; all when none.
        #[arg(long = "topic")]
        topics: Vec<String>,
        #[arg(long, default_value_t = 1)]
        count: usize,
        /// Seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    #[command(subcommand)]
    Daemon(DaemonCmd),
}

#[derive(Subcommand)]
enum LibCmd {
    /// Name a library project by an alias: its folder, relative to this
    /// project, inside the same repository.
    Add {
        alias: String,
        path: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Every library with its revision, what it holds and its faults.
    List,
}

#[derive(Subcommand)]
enum SymbolCmd {
    /// A symbol in this project's library, its ID made from its name
    /// unless given, with --ops applied.
    Create {
        name: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        units: Option<u32>,
        /// A JSON list of ops: {"op": "add", "pin": "1", "type": "passive",
        /// "at": ["0", "3.81 mm"], "side": "down"}, {"op": "add", "graphic":
        /// "rect", "start": [...], "end": [...]}, ...
        #[arg(long, value_parser = ops, default_value = "[]")]
        ops: Ops,
        #[command(flatten)]
        expect: Expect,
    },
    /// Apply a batch of ops (add, remove, set, move on pins and graphics)
    /// all or nothing, and rename or change the unit count.
    Edit {
        symbol: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        units: Option<u32>,
        #[arg(long, value_parser = ops, default_value = "[]")]
        ops: Ops,
        #[command(flatten)]
        expect: Expect,
    },
    /// A symbol (<id>, or lib/<alias>/symbol/<id>) with its faults.
    Show { symbol: String },
    /// A library's symbols (this project's by default).
    List {
        #[arg(long)]
        library: Option<String>,
    },
    /// Delete a symbol no library component places.
    Delete {
        symbol: String,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum FootprintCmd {
    /// A footprint (<id>, or lib/<alias>/footprint/<id>) with its faults.
    Show { footprint: String },
    /// A library's footprints (this project's by default).
    List {
        #[arg(long)]
        library: Option<String>,
    },
}

/// A library component's fields; an edit keeps those left out.
#[derive(Args)]
struct CompArgs {
    /// <id>, or lib/<alias>/symbol/<id>.
    #[arg(long)]
    symbol: Option<String>,
    /// The reference designator's letters: R, C, U.
    #[arg(long)]
    prefix: Option<String>,
    #[arg(long)]
    value: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    datasheet: Option<String>,
    /// name=value; repeat for more.
    #[arg(long = "param", value_parser = pair)]
    params: Vec<(String, String)>,
    /// Remove this parameter; repeat for more.
    #[arg(long = "unset-param", value_name = "NAME")]
    unset: Vec<String>,
    #[arg(long)]
    manufacturer: Option<String>,
    #[arg(long)]
    mpn: Option<String>,
    /// supplier=sku (LCSC=C25741); repeat for more. Replaces the list.
    #[arg(long = "offer", value_parser = pair)]
    offers: Vec<(String, String)>,
    /// A footprint (<id>, or lib/<alias>/footprint/<id>), pins mapped to
    /// the pads of their numbers; repeat for more, the default first.
    /// Replaces the list.
    #[arg(long = "footprint")]
    footprint: Vec<String>,
    /// The whole footprint list as JSON, with pin maps:
    /// [{"footprint": "sot23", "pins": {"1": ["3"]}}].
    #[arg(long, conflicts_with = "footprint")]
    footprints: Option<String>,
}

impl CompArgs {
    fn fields(&self) -> Result<ryueda::libcmd::CompFields> {
        let mut params = serde_json::Map::new();
        for (k, v) in &self.params {
            params.insert(k.clone(), json!(v));
        }
        for k in &self.unset {
            params.insert(k.clone(), Value::Null);
        }
        let footprints = match (&self.footprints, self.footprint.is_empty()) {
            (Some(text), _) => Some(serde_json::from_str(text).map_err(|e| {
                Error::new(
                    Code::Usage,
                    format!("--footprints isn't a list of {{footprint, pins}}: {e}"),
                )
            })?),
            (None, false) => Some(
                self.footprint
                    .iter()
                    .map(|f| ryueda::libcmd::FootprintArg {
                        footprint: f.clone(),
                        pins: Default::default(),
                    })
                    .collect(),
            ),
            (None, true) => None,
        };
        Ok(ryueda::libcmd::CompFields {
            symbol: self.symbol.clone(),
            prefix: self.prefix.clone(),
            value: self.value.clone(),
            description: self.description.clone(),
            datasheet: self.datasheet.clone(),
            parameters: (!params.is_empty()).then_some(params),
            manufacturer: self.manufacturer.clone(),
            mpn: self.mpn.clone(),
            offers: (!self.offers.is_empty()).then(|| {
                self.offers
                    .iter()
                    .map(|(supplier, sku)| ryueda::libcomp::Offer {
                        supplier: supplier.clone(),
                        sku: sku.clone(),
                    })
                    .collect()
            }),
            footprints,
        })
    }
}

#[derive(Subcommand)]
enum LibcompCmd {
    /// A library component in this project's library, its ID made from
    /// its name unless given.
    Create {
        name: String,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        fields: CompArgs,
        #[command(flatten)]
        expect: Expect,
    },
    /// Replace the fields given.
    Edit {
        libcomp: String,
        #[command(flatten)]
        fields: CompArgs,
        #[command(flatten)]
        expect: Expect,
    },
    /// A library component (<id>, or lib/<alias>/libcomp/<id>) with its
    /// faults.
    Show { libcomp: String },
    /// A library's components (this project's by default).
    List {
        #[arg(long)]
        library: Option<String>,
    },
    Delete {
        libcomp: String,
        #[command(flatten)]
        expect: Expect,
    },
}

/// A net's or rail's intent; an empty value clears a field.
#[derive(Args)]
struct IntentFlags {
    /// A net class, by ID or name.
    #[arg(long)]
    class: Option<String>,
    /// "3.3 V"
    #[arg(long)]
    voltage: Option<String>,
    /// "500 mA"
    #[arg(long)]
    current: Option<String>,
    /// "90 Ohm"
    #[arg(long)]
    impedance: Option<String>,
}

impl IntentFlags {
    fn args(&self) -> ryueda::topcmd::IntentArgs {
        ryueda::topcmd::IntentArgs {
            class: self.class.clone(),
            voltage: self.voltage.clone(),
            current: self.current.clone(),
            impedance: self.impedance.clone(),
        }
    }
}

// Parsed once per run, as `Command` is.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum BlockCmd {
    /// A block, its ID random unless given.
    Create {
        name: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Rename a block, and add, change or remove its ports and buses.
    Edit {
        block: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// NAME:DIRECTION (in, out, bidirectional, power, passive); repeat
        /// for more.
        #[arg(long = "add-port", value_name = "NAME:DIRECTION")]
        add_port: Vec<String>,
        /// PORT=NAME
        #[arg(long = "rename-port", value_parser = pair)]
        rename_port: Vec<(String, String)>,
        /// PORT=DIRECTION
        #[arg(long = "port-direction", value_parser = pair)]
        port_direction: Vec<(String, String)>,
        /// PORT=VOLTAGE ("3.3 V"; empty clears it): what the block expects
        /// on the port.
        #[arg(long = "port-voltage", value_parser = pair)]
        port_voltage: Vec<(String, String)>,
        #[arg(long = "remove-port", value_name = "PORT")]
        remove_port: Vec<String>,
        /// NAME=NET,NET,...
        #[arg(long = "add-bus", value_parser = pair)]
        add_bus: Vec<(String, String)>,
        /// BUS=NET,NET,...: the bus's whole list.
        #[arg(long = "bus-nets", value_parser = pair)]
        bus_nets: Vec<(String, String)>,
        /// BUS=NAME
        #[arg(long = "rename-bus", value_parser = pair)]
        rename_bus: Vec<(String, String)>,
        #[arg(long = "remove-bus", value_name = "BUS")]
        remove_bus: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// A block with what it holds, its faults and its revision.
    Show {
        block: String,
    },
    List,
    /// Delete a block no instance places and no design is rooted on.
    Delete {
        block: String,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum RailCmd {
    Add {
        name: String,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        intent: IntentFlags,
        #[command(flatten)]
        expect: Expect,
    },
    Edit {
        /// By ID or name.
        rail: String,
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        intent: IntentFlags,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete a rail no pin or port names.
    Delete {
        rail: String,
        #[command(flatten)]
        expect: Expect,
    },
    List,
}

#[derive(Subcommand)]
enum ClassCmd {
    Add {
        name: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Edit {
        class: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete a class no net or rail names.
    Delete {
        class: String,
        #[command(flatten)]
        expect: Expect,
    },
    List,
}

#[derive(Subcommand)]
enum DesignCmd {
    Create {
        name: String,
        /// The root block.
        #[arg(long)]
        root: String,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Edit {
        design: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        root: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Delete {
        design: String,
        #[command(flatten)]
        expect: Expect,
    },
    List,
}

/// A component's fields; an edit keeps those left out.
#[derive(Args)]
struct ComponentArgs {
    /// The library component: <id>, or lib/<alias>/libcomp/<id>.
    #[arg(long)]
    of: Option<String>,
    #[arg(long)]
    refdes: Option<String>,
    /// One of the library component's footprints.
    #[arg(long)]
    footprint: Option<String>,
    #[arg(long)]
    value: Option<String>,
    /// name=value; repeat for more.
    #[arg(long = "param", value_parser = pair)]
    params: Vec<(String, String)>,
    /// Remove this parameter; repeat for more.
    #[arg(long = "unset-param", value_name = "NAME")]
    unset: Vec<String>,
}

impl ComponentArgs {
    fn fields(&self) -> ryueda::partcmd::ComponentFields {
        let mut params = serde_json::Map::new();
        for (k, v) in &self.params {
            params.insert(k.clone(), json!(v));
        }
        for k in &self.unset {
            params.insert(k.clone(), Value::Null);
        }
        ryueda::partcmd::ComponentFields {
            of: self.of.clone(),
            refdes: self.refdes.clone(),
            footprint: self.footprint.clone(),
            value: self.value.clone(),
            parameters: (!params.is_empty()).then_some(params),
        }
    }
}

#[derive(Subcommand)]
enum ComponentCmd {
    /// Place a library component (--of) in a block.
    Add {
        block: String,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        fields: ComponentArgs,
        #[command(flatten)]
        expect: Expect,
    },
    /// Replace the fields given (<block>/<component>).
    Edit {
        component: String,
        #[command(flatten)]
        fields: ComponentArgs,
        #[command(flatten)]
        expect: Expect,
    },
    Delete {
        component: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// A component with its connections and faults.
    Show { component: String },
}

#[derive(Subcommand)]
enum NetCmd {
    Add {
        block: String,
        name: String,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        intent: IntentFlags,
        #[command(flatten)]
        expect: Expect,
    },
    /// A net's (<block>/<net>) class, intent and pair.
    Edit {
        net: String,
        #[command(flatten)]
        intent: IntentFlags,
        /// The pair's other net, of the same block.
        #[arg(long = "pair-with")]
        pair_with: Option<String>,
        /// p or n: this net's half (the other takes the other).
        #[arg(long)]
        polarity: Option<String>,
        /// Unpair the net.
        #[arg(long = "no-pair", conflicts_with = "pair_with")]
        no_pair: bool,
        #[command(flatten)]
        expect: Expect,
    },
    Rename {
        net: String,
        name: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Move every pin on NET onto INTO (a net of its block, or
    /// rail:<name>), and delete NET.
    Join {
        net: String,
        into: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete a net with no pins.
    Delete {
        net: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// A net with the pins on it.
    Show { net: String },
}

#[derive(Subcommand)]
enum ErcCmd {
    /// Accept a finding (by its ID) with a reason, or --remove a waiver.
    Waive {
        finding: String,
        #[arg(long, required_unless_present = "remove")]
        reason: Option<String>,
        #[arg(long, conflicts_with = "reason")]
        remove: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// Set codes' severities: CODE=error|warning|off|default; repeat.
    Configure {
        #[arg(long = "severity", value_parser = pair, required = true)]
        severity: Vec<(String, String)>,
        #[command(flatten)]
        expect: Expect,
    },
}

/// A channel rule as flags.
#[derive(Args)]
struct ChannelFlags {
    /// Reference designators of this instance's occurrences take this
    /// suffix (R5 becomes R5_A).
    #[arg(long, conflicts_with = "offset")]
    suffix: Option<String>,
    /// ... or have this added to their number (R5 becomes R105).
    #[arg(long)]
    offset: Option<u32>,
}

impl ChannelFlags {
    fn channel(&self) -> Option<ryueda::conn::Channel> {
        use ryueda::conn::Channel;
        match (&self.suffix, self.offset) {
            (Some(s), _) => Some(Channel::Suffix(s.clone())),
            (None, Some(n)) => Some(Channel::Offset(n)),
            (None, None) => None,
        }
    }
}

#[derive(Subcommand)]
enum InstanceCmd {
    /// Place block --of in a block.
    Add {
        block: String,
        #[arg(long)]
        of: String,
        #[arg(long)]
        id: Option<String>,
        /// PORT=NET: a port of the placed block onto a net here (by ID or
        /// name, rail:<name>, port:<name>, nc); repeat for more.
        #[arg(long = "port", value_parser = pair)]
        ports: Vec<(String, String)>,
        #[command(flatten)]
        channel: ChannelFlags,
        #[command(flatten)]
        expect: Expect,
    },
    /// Change an instance's (<block>/<instance>) ports, channel rule and
    /// overrides.
    Edit {
        instance: String,
        #[arg(long = "port", value_parser = pair)]
        ports: Vec<(String, String)>,
        /// Leave this port unconnected; repeat for more.
        #[arg(long = "unport", value_name = "PORT")]
        unport: Vec<String>,
        #[command(flatten)]
        channel: ChannelFlags,
        /// Drop the channel rule.
        #[arg(long = "no-refdes")]
        no_refdes: bool,
        /// PATH=REFDES: an occurrence below the instance; repeat for more.
        #[arg(long = "override", value_parser = pair)]
        overrides: Vec<(String, String)>,
        #[arg(long = "unoverride", value_name = "PATH")]
        unoverride: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Delete {
        instance: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// An instance with its connections and faults.
    Show { instance: String },
}

/// `NAME=VALUE` pairs and names to drop as a JSON object of strings and
/// nulls.
fn settings(set: &[(String, String)], drop: &[String]) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    for (k, v) in set {
        m.insert(k.clone(), json!(v));
    }
    for k in drop {
        m.insert(k.clone(), Value::Null);
    }
    m
}

/// A port's direction, as typed.
fn direction(s: &str) -> Result<ryueda::conn::Direction> {
    serde_json::from_value(json!(s)).map_err(|_| {
        Error::new(
            Code::Usage,
            format!("{s:?} isn't a direction: in, out, bidirectional, power or passive"),
        )
    })
}

/// A list of nets as typed: `A,B,C`.
fn nets(s: &str) -> Vec<String> {
    s.split(',')
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Serve the project (the CLI starts this itself).
    Run {
        /// Seconds idle before it exits.
        #[arg(long)]
        idle_timeout: Option<u64>,
    },
    Status,
    Stop {
        /// Every ryueda daemon of this user.
        #[arg(long)]
        all: bool,
    },
}

/// A batch of ops, as JSON.
#[derive(Clone)]
struct Ops(Vec<Value>);

fn ops(s: &str) -> core::result::Result<Ops, String> {
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Array(list)) => Ok(Ops(list)),
        Ok(_) => Err("ops are a JSON list".into()),
        Err(e) => Err(format!("ops aren't JSON: {e}")),
    }
}

fn pair(s: &str) -> core::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .ok_or_else(|| format!("{s:?} isn't name=value"))
}

fn unit(s: &str) -> core::result::Result<Unit, String> {
    Unit::parse(s).ok_or_else(|| format!("unknown unit {s:?}: m, cm, mm, um, in or ft"))
}

/// The request a command sends, for those that are requests.
fn request(cmd: &Command) -> Result<Option<Request>> {
    Ok(Some(match cmd {
        Command::Info => Request::ProjectInfo {},
        Command::Lib(LibCmd::Add {
            alias,
            path,
            expect,
        }) => Request::LibAdd {
            alias: alias.clone(),
            path: path.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Lib(LibCmd::List) => Request::LibList {},
        Command::Symbol(SymbolCmd::Create {
            name,
            id,
            units,
            ops,
            expect,
        }) => Request::SymbolCreate {
            name: name.clone(),
            id: id.clone(),
            units: *units,
            ops: ops.0.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Symbol(SymbolCmd::Edit {
            symbol,
            name,
            units,
            ops,
            expect,
        }) => Request::SymbolEdit {
            symbol: symbol.clone(),
            name: name.clone(),
            units: *units,
            ops: ops.0.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Symbol(SymbolCmd::Show { symbol }) => Request::SymbolShow {
            symbol: symbol.clone(),
        },
        Command::Symbol(SymbolCmd::List { library }) => Request::SymbolList {
            library: library.clone(),
        },
        Command::Symbol(SymbolCmd::Delete { symbol, expect }) => Request::SymbolDelete {
            symbol: symbol.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::History { limit, before } => Request::ActivityLog {
            limit: *limit,
            before: *before,
        },
        Command::Participants => Request::ParticipantList {},
        Command::Undo { entry } => Request::Undo { entry: *entry },
        Command::Redo { entry } => Request::Redo { entry: *entry },
        Command::Footprint(FootprintCmd::Show { footprint }) => Request::FootprintShow {
            footprint: footprint.clone(),
        },
        Command::Footprint(FootprintCmd::List { library }) => Request::FootprintList {
            library: library.clone(),
        },
        Command::Libcomp(LibcompCmd::Create {
            name,
            id,
            fields,
            expect,
        }) => Request::LibcompCreate {
            name: name.clone(),
            id: id.clone(),
            fields: fields.fields()?,
            expected_revision: expect.expect.clone(),
        },
        Command::Libcomp(LibcompCmd::Edit {
            libcomp,
            fields,
            expect,
        }) => Request::LibcompEdit {
            libcomp: libcomp.clone(),
            fields: fields.fields()?,
            expected_revision: expect.expect.clone(),
        },
        Command::Libcomp(LibcompCmd::Show { libcomp }) => Request::LibcompShow {
            libcomp: libcomp.clone(),
        },
        Command::Libcomp(LibcompCmd::List { library }) => Request::LibcompList {
            library: library.clone(),
        },
        Command::Libcomp(LibcompCmd::Delete { libcomp, expect }) => Request::LibcompDelete {
            libcomp: libcomp.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Block(BlockCmd::Create {
            name,
            id,
            description,
            expect,
        }) => Request::BlockCreate {
            name: name.clone(),
            id: id.clone(),
            description: description.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Block(BlockCmd::Edit {
            block,
            name,
            description,
            add_port,
            rename_port,
            port_direction,
            port_voltage,
            remove_port,
            add_bus,
            bus_nets,
            rename_bus,
            remove_bus,
            expect,
        }) => {
            use ryueda::topcmd::{BusOp, PortOp};
            let mut ports = Vec::new();
            for a in add_port {
                let (name, dir) = a.split_once(':').ok_or_else(|| {
                    Error::new(
                        Code::Usage,
                        format!("--add-port {a:?} isn't NAME:DIRECTION"),
                    )
                })?;
                ports.push(PortOp::Add {
                    name: name.to_owned(),
                    direction: direction(dir)?,
                    voltage: None,
                    id: None,
                });
            }
            for (port, name) in rename_port {
                ports.push(PortOp::Set {
                    port: port.clone(),
                    name: Some(name.clone()),
                    direction: None,
                    voltage: None,
                });
            }
            for (port, dir) in port_direction {
                ports.push(PortOp::Set {
                    port: port.clone(),
                    name: None,
                    direction: Some(direction(dir)?),
                    voltage: None,
                });
            }
            for (port, v) in port_voltage {
                ports.push(PortOp::Set {
                    port: port.clone(),
                    name: None,
                    direction: None,
                    voltage: Some(v.clone()),
                });
            }
            for port in remove_port {
                ports.push(PortOp::Remove { port: port.clone() });
            }
            let mut buses = Vec::new();
            for (name, list) in add_bus {
                buses.push(BusOp::Add {
                    name: name.clone(),
                    nets: nets(list),
                    id: None,
                });
            }
            for (bus, list) in bus_nets {
                buses.push(BusOp::Set {
                    bus: bus.clone(),
                    name: None,
                    nets: Some(nets(list)),
                });
            }
            for (bus, name) in rename_bus {
                buses.push(BusOp::Set {
                    bus: bus.clone(),
                    name: Some(name.clone()),
                    nets: None,
                });
            }
            for bus in remove_bus {
                buses.push(BusOp::Remove { bus: bus.clone() });
            }
            Request::BlockEdit {
                block: block.clone(),
                name: name.clone(),
                description: description.clone(),
                ports,
                buses,
                expected_revision: expect.expect.clone(),
            }
        }
        Command::Block(BlockCmd::Show { block }) => Request::BlockShow {
            block: block.clone(),
        },
        Command::Block(BlockCmd::List) => Request::BlockList {},
        Command::Block(BlockCmd::Delete { block, expect }) => Request::BlockDelete {
            block: block.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Component(ComponentCmd::Add {
            block,
            id,
            fields,
            expect,
        }) => Request::ComponentAdd {
            block: block.clone(),
            id: id.clone(),
            fields: fields.fields(),
            expected_revision: expect.expect.clone(),
        },
        Command::Component(ComponentCmd::Edit {
            component,
            fields,
            expect,
        }) => Request::ComponentEdit {
            component: component.clone(),
            fields: fields.fields(),
            expected_revision: expect.expect.clone(),
        },
        Command::Component(ComponentCmd::Delete { component, expect }) => {
            Request::ComponentDelete {
                component: component.clone(),
                expected_revision: expect.expect.clone(),
            }
        }
        Command::Component(ComponentCmd::Show { component }) => Request::ComponentShow {
            component: component.clone(),
        },
        Command::Connect {
            pin,
            net,
            new,
            expect,
        } => Request::Connect {
            pin: pin.clone(),
            net: net.clone(),
            new: *new,
            expected_revision: expect.expect.clone(),
        },
        Command::Disconnect { pin, expect } => Request::Disconnect {
            pin: pin.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Nc { pin, expect } => Request::Nc {
            pin: pin.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Net(NetCmd::Add {
            block,
            name,
            id,
            intent,
            expect,
        }) => Request::NetAdd {
            block: block.clone(),
            name: name.clone(),
            id: id.clone(),
            intent: intent.args(),
            expected_revision: expect.expect.clone(),
        },
        Command::Net(NetCmd::Edit {
            net,
            intent,
            pair_with,
            polarity,
            no_pair,
            expect,
        }) => Request::NetEdit {
            net: net.clone(),
            intent: intent.args(),
            pair_with: if *no_pair {
                Some(String::new())
            } else {
                pair_with.clone()
            },
            polarity: polarity
                .as_deref()
                .map(|p| {
                    serde_json::from_value(json!(p))
                        .map_err(|_| Error::new(Code::Usage, format!("--polarity {p:?} is p or n")))
                })
                .transpose()?,
            expected_revision: expect.expect.clone(),
        },
        Command::Net(NetCmd::Rename { net, name, expect }) => Request::NetRename {
            net: net.clone(),
            name: name.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Net(NetCmd::Join { net, into, expect }) => Request::NetJoin {
            net: net.clone(),
            into: into.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Net(NetCmd::Delete { net, expect }) => Request::NetDelete {
            net: net.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Net(NetCmd::Show { net }) => Request::NetShow { net: net.clone() },
        Command::Instance(InstanceCmd::Add {
            block,
            of,
            id,
            ports,
            channel,
            expect,
        }) => Request::InstanceAdd {
            block: block.clone(),
            of: of.clone(),
            id: id.clone(),
            ports: settings(ports, &[]),
            refdes: channel.channel(),
            expected_revision: expect.expect.clone(),
        },
        Command::Instance(InstanceCmd::Edit {
            instance,
            ports,
            unport,
            channel,
            no_refdes,
            overrides,
            unoverride,
            expect,
        }) => Request::InstanceEdit {
            instance: instance.clone(),
            fields: ryueda::partcmd::InstanceFields {
                ports: settings(ports, unport),
                refdes: channel.channel(),
                no_refdes: *no_refdes,
                overrides: settings(overrides, unoverride),
            },
            expected_revision: expect.expect.clone(),
        },
        Command::Instance(InstanceCmd::Delete { instance, expect }) => Request::InstanceDelete {
            instance: instance.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Instance(InstanceCmd::Show { instance }) => Request::InstanceShow {
            instance: instance.clone(),
        },
        Command::Netlist { design } => Request::Netlist {
            design: design.clone(),
        },
        Command::Erc {
            design,
            action: None,
        } => Request::Erc {
            design: design.clone(),
        },
        Command::Erc {
            action:
                Some(ErcCmd::Waive {
                    finding,
                    reason,
                    remove,
                    expect,
                }),
            ..
        } => Request::ErcWaive {
            finding: finding.clone(),
            reason: reason.clone(),
            remove: *remove,
            expected_revision: expect.expect.clone(),
        },
        Command::Erc {
            action: Some(ErcCmd::Configure { severity, expect }),
            ..
        } => Request::ErcConfigure {
            severity: severity
                .iter()
                .map(|(code, level)| {
                    let v = match level.as_str() {
                        "default" => Value::Null,
                        l => json!(l),
                    };
                    (code.clone(), v)
                })
                .collect(),
            expected_revision: expect.expect.clone(),
        },
        Command::Rail(RailCmd::Add {
            name,
            id,
            intent,
            expect,
        }) => Request::RailAdd {
            name: name.clone(),
            id: id.clone(),
            intent: intent.args(),
            expected_revision: expect.expect.clone(),
        },
        Command::Rail(RailCmd::Edit {
            rail,
            name,
            intent,
            expect,
        }) => Request::RailEdit {
            rail: rail.clone(),
            name: name.clone(),
            intent: intent.args(),
            expected_revision: expect.expect.clone(),
        },
        Command::Rail(RailCmd::Delete { rail, expect }) => Request::RailDelete {
            rail: rail.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Rail(RailCmd::List) => Request::RailList {},
        Command::Class(ClassCmd::Add {
            name,
            id,
            description,
            expect,
        }) => Request::ClassAdd {
            name: name.clone(),
            id: id.clone(),
            description: description.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Class(ClassCmd::Edit {
            class,
            name,
            description,
            expect,
        }) => Request::ClassEdit {
            class: class.clone(),
            name: name.clone(),
            description: description.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Class(ClassCmd::Delete { class, expect }) => Request::ClassDelete {
            class: class.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Class(ClassCmd::List) => Request::ClassList {},
        Command::Design(DesignCmd::Create {
            name,
            root,
            id,
            expect,
        }) => Request::DesignCreate {
            name: name.clone(),
            root: root.clone(),
            id: id.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Design(DesignCmd::Edit {
            design,
            name,
            root,
            expect,
        }) => Request::DesignEdit {
            design: design.clone(),
            name: name.clone(),
            root: root.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Design(DesignCmd::Delete { design, expect }) => Request::DesignDelete {
            design: design.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Design(DesignCmd::List) => Request::DesignList {},
        Command::Version | Command::Init { .. } | Command::Watch { .. } | Command::Daemon(_) => {
            return Ok(None);
        }
    }))
}

fn no_daemon(cli: &Cli) -> bool {
    cli.no_daemon || std::env::var("RYUEDA_NO_DAEMON").is_ok_and(|v| v == "1")
}

/// The agent participant to act as.
fn participant(cli: &Cli) -> Option<String> {
    cli.as_name
        .clone()
        .or_else(|| std::env::var("RYUEDA_PARTICIPANT").ok())
}

/// Runs the command, collecting warnings for the output.
fn run(cli: &Cli, warnings: &mut Vec<String>) -> Result<Value> {
    let here = match &cli.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::io(std::path::Path::new("."), &e))?,
    };
    match &cli.command {
        Command::Version => Ok(json!(VersionInfo::current())),
        Command::Init { unit, library } => {
            let root =
                ryueda::source::plain(here.canonicalize().map_err(|e| Error::io(&here, &e))?);
            server::init(&root, *unit, *library)
        }
        Command::Daemon(DaemonCmd::Run { idle_timeout }) => {
            let idle = idle_timeout.map_or(daemon::DEFAULT_IDLE, Duration::from_secs);
            daemon::run(&find_root(&here)?, idle).map(|()| Value::Null)
        }
        Command::Daemon(DaemonCmd::Status) => client::status(&find_root(&here)?),
        Command::Daemon(DaemonCmd::Stop { all: true }) => client::stop_all(),
        Command::Daemon(DaemonCmd::Stop { all: false }) => client::stop(&find_root(&here)?),
        Command::Watch {
            topics,
            count,
            timeout,
        } => {
            let root = find_root(&here)?;
            if no_daemon(cli) {
                return Err(ryu_session::live::no_live_session("subscribe"));
            }
            client::watch(
                &root,
                participant(cli).as_deref(),
                topics,
                *count,
                Duration::from_secs(*timeout),
            )
        }
        cmd => {
            let req = request(cmd)?.expect("every other command is a request");
            let root = find_root(&here)?;
            if !no_daemon(cli) {
                match client::request(&root, &req, participant(cli).as_deref()) {
                    Ok(answer) => return answer.map(|a: Answer| a.result),
                    // Never delivered, so running it here is safe.
                    Err(e) => warnings.push(format!("{}; ran in process", e.message)),
                }
            }
            let me = participant(cli);
            server::handle_safely(&mut None, None, me.as_deref(), &root, req).map(|a| a.result)
        }
    }
}

/// Whether the arguments ask only for the version the way most tools take
/// it (`--version`, `-V`), beside `--pretty`: answered as `ryueda version`.
fn version_flag(args: &[String]) -> bool {
    let (flags, rest): (Vec<&String>, Vec<&String>) = args
        .iter()
        .skip(1)
        .partition(|a| *a == "--version" || *a == "-V");
    !flags.is_empty() && rest.iter().all(|a| *a == "--pretty")
}

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if version_flag(&args) {
        let pretty = args.iter().any(|a| a == "--pretty");
        return finish(Ok(json!(VersionInfo::current())), Vec::new(), pretty);
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind;
            // `ryueda` alone, like --help, is a person asking: but a command
            // missing its subcommand is a usage error, in JSON, like any
            // other.
            let bare = args.len() <= 1;
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion)
                || (e.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand && bare)
            {
                let _ = e.print();
                return ExitCode::SUCCESS;
            }
            let err = Error::new(Code::Usage, e.render().to_string().trim().to_owned());
            return finish(Err(err), Vec::new(), args.iter().any(|a| a == "--pretty"));
        }
    };
    let mut warnings = Vec::new();
    let outcome = run(&cli, &mut warnings);
    finish(outcome, warnings, cli.pretty)
}

fn finish(outcome: Result<Value>, warnings: Vec<String>, pretty: bool) -> ExitCode {
    let (doc, code) = match outcome {
        Ok(result) => (
            json!({"ok": true, "result": result, "warnings": warnings}),
            0,
        ),
        Err(e) => {
            let code = e.code.exit_code();
            (json!({"ok": false, "error": e, "warnings": warnings}), code)
        }
    };
    let s = if pretty {
        serde_json::to_string_pretty(&doc)
    } else {
        serde_json::to_string(&doc)
    };
    // A reader that stops early (`| head`) has what it wanted.
    use std::io::Write;
    let _ = writeln!(
        std::io::stdout().lock(),
        "{}",
        s.expect("JSON values always serialize")
    );
    ExitCode::from(code)
}
