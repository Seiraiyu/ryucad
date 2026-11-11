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
    /// Edit the project's properties and schematic settings.
    #[command(subcommand)]
    Project(ProjectCmd),
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
    /// Find parts by category and typed parameters
    /// (capacitance=100nF voltage_rating>=16V package=0402) in the
    /// project's libraries.
    #[command(subcommand)]
    Parts(PartsCmd),
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
    /// The schematic lint's preferences (lint.json) and its rules, each
    /// with its default weight, what it finds, why, how to fix it and its
    /// evidence; `lint configure` writes them.
    #[command(args_conflicts_with_subcommands = true)]
    Lint {
        /// One rule, with the project's weight and severity for it.
        #[arg(long, value_name = "CODE")]
        explain: Option<String>,
        #[command(subcommand)]
        action: Option<LintCmd>,
    },
    /// The symbol standards that judge a library (this project's own by
    /// default): resistor shapes, grid, pin lengths, name margins, ground
    /// and group name patterns, and each class's side rules; `standards
    /// set` changes this project's own.
    #[command(args_conflicts_with_subcommands = true)]
    /// Add or remove tags (critical, hv, dnp, or your own) on a component
    /// or net (by designator or name, with --block) or a rail (rail:NAME).
    Tag {
        target: String,
        #[arg(long)]
        block: Option<String>,
        #[arg(long = "add", value_name = "TAG")]
        add: Vec<String>,
        #[arg(long = "remove", value_name = "TAG")]
        remove: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// The style (the project's, the organisation's, the built-in one),
    /// or a subcommand.
    Style {
        #[command(subcommand)]
        cmd: Option<StyleCmd>,
    },
    Standards {
        /// A library project's alias, to read the standards that judge it.
        #[arg(long)]
        library: Option<String>,
        #[command(subcommand)]
        action: Option<StandardsCmd>,
    },
    /// Load the project from nothing and report every file's faults and
    /// every design's ERC, and supply findings from the catalogues this
    /// machine holds; fails (check_failed) on any error.
    Check {
        /// Stay off the network (check never fetches).
        #[arg(long)]
        offline: bool,
    },
    /// Lay out a design's schematics (the project's one design, or
    /// --design), or one --block, over their layout files, writing those
    /// that change.
    Layout {
        #[arg(long)]
        design: Option<String>,
        #[arg(long)]
        block: Option<String>,
        /// Lay out from nothing <block> or <block>/<sheet>, keeping its
        /// instructions.
        #[arg(long, value_name = "BLOCK[/SHEET]")]
        relayout: Option<String>,
        /// Say what would change; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Draw the dry run's result into FILE (svg or png, by --format or
        /// its extension), as render draws; one block (--block or
        /// --relayout).
        #[arg(long, value_name = "FILE", requires = "dry_run")]
        render: Option<PathBuf>,
        /// svg or png for --render; from its extension when left out.
        #[arg(long, requires = "render")]
        format: Option<String>,
        /// Pixels per millimetre for a PNG (4 by default).
        #[arg(long, requires = "render")]
        scale: Option<f32>,
        /// Crop the picture to what is drawn.
        #[arg(long, requires = "render")]
        fit: bool,
        /// Draw one sheet (by ID or name) with --render.
        #[arg(long, requires = "render")]
        sheet: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Compose a block's schematic: instructions an agent gives, each
    /// lasting until released ("sheet X... S", "beside X... Y
    /// left|right|above|below", "near X... P", "align X... row|column",
    /// "group X...", "edge X... left|right|top|bottom", "orient X...
    /// 0|90|180|270 [mirror]", "text X... refdes|value|both
    /// left|right|above|below", "net N wire|label|port", "idiom X...
    /// KIND|none", "flow X... reversed", "release X..."). Parts by
    /// designator (U3), unit (U1:2), instance (instance:<id>) or port
    /// (port:SDA); pins as U3.VDD or U3.4; nets by name or rail:<name>.
    Place {
        block: String,
        instructions: Vec<String>,
        /// An instruction applied once, its result held where it puts it
        /// (repeat for more).
        #[arg(long = "move", value_name = "INSTRUCTION")]
        moves: Vec<String>,
        /// List the block's lasting instructions and whether each holds.
        #[arg(long, conflicts_with_all = ["instructions", "moves", "dry_run"])]
        list: bool,
        /// Answer the result (its description) and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Draw the dry run's result into FILE (svg or png, by --format or
        /// its extension), as render draws.
        #[arg(long, value_name = "FILE", requires = "dry_run")]
        render: Option<PathBuf>,
        /// svg or png for --render; from its extension when left out.
        #[arg(long, requires = "render")]
        format: Option<String>,
        /// Pixels per millimetre for a PNG (4 by default).
        #[arg(long, requires = "render")]
        scale: Option<f32>,
        /// Crop the picture to what is drawn.
        #[arg(long, requires = "render")]
        fit: bool,
        /// Draw one sheet (by ID or name) with --render.
        #[arg(long, requires = "render")]
        sheet: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// A schematic in relative terms, for reading instead of a picture:
    /// each sheet's parts with their neighbours, idioms, nets, the
    /// instructions in force, the lint and the score.
    Describe {
        #[arg(long)]
        design: Option<String>,
        #[arg(long)]
        block: Option<String>,
        #[arg(long)]
        sheet: Option<String>,
    },
    /// Draw a block's canvas (a design's root by default) or one sheet,
    /// as SVG or PNG, into --out.
    Render {
        #[arg(long)]
        design: Option<String>,
        #[arg(long)]
        block: Option<String>,
        #[arg(long)]
        sheet: Option<String>,
        /// Draw that occurrence's designators: an instance path (ch_b,
        /// front/ch_a) or a stack path (B); the first when left out.
        #[arg(long)]
        layer: Option<String>,
        /// svg or png; from --out's extension when left out.
        #[arg(long)]
        format: Option<String>,
        /// Pixels per millimetre for PNG (4 by default).
        #[arg(long)]
        scale: Option<f32>,
        /// Crop to what is drawn, rather than the whole paper.
        #[arg(long)]
        fit: bool,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
    /// Page a design's canvases into a PDF (--out): a page per sheet at
    /// the project's paper, bookmarks, links and searchable text.
    Output {
        /// The design (the project's only one when left out).
        design: Option<String>,
        /// all: a stack's sheets once per layer; first: once, its layers
        /// listed.
        #[arg(long)]
        layers: Option<String>,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
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
    /// Import libraries from other tools: KiCad's for now.
    #[command(subcommand)]
    Import(ImportCmd),
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
    /// Cancel a running long request by its token (through the daemon).
    Cancel { token: String },
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
enum ImportCmd {
    /// KiCad 6 to 9 libraries, with their STEP models, into this
    /// project's library (or --into a library project it names):
    /// .kicad_sym files, .pretty folders (or .kicad_mod files in one) and
    /// .3dshapes folders. Importing again updates what changed and
    /// deletes nothing.
    Kicad {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Where to look for the footprints symbols name: a folder of
        /// <library>.pretty folders, or a .pretty folder (repeatable).
        #[arg(long = "footprints", value_name = "DIR")]
        footprints: Vec<PathBuf>,
        /// What KiCad's model variables (${KICAD9_3DMODEL_DIR},
        /// ${KIPRJMOD}, ...) stand for, tried in turn (repeatable).
        #[arg(long = "models", value_name = "DIR")]
        models: Vec<PathBuf>,
        /// A library project's alias to write instead of this project's
        /// library.
        #[arg(long, value_name = "ALIAS")]
        into: Option<String>,
        /// Read and check everything, write nothing, and say what would
        /// change.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        expect: Expect,
    },
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
    /// Judge a library's symbols by the standards that apply to it
    /// (`ryueda standards`): each symbol's class, score and findings, with
    /// symbol edit ops that fix them. This project's own library by
    /// default.
    Check {
        /// A library project's alias.
        #[arg(long)]
        library: Option<String>,
        /// One symbol (<id>, or lib/<alias>/symbol/<id>).
        #[arg(long)]
        symbol: Option<String>,
    },
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
        /// The class of part it draws: resistor, capacitor, inductor,
        /// diode, led, transistor, opamp, connector, regulator, ic (none
        /// takes it off an edit).
        #[arg(long)]
        class: Option<String>,
        /// Draw it from the standards' template for a class: resistor,
        /// capacitor, inductor, diode, led (whole), ic, regulator,
        /// connector (a box round --pins); --ops apply after.
        #[arg(long)]
        template: Option<String>,
        /// A box template's pins, as JSON: [{"number": "1", "name":
        /// "VDD", "type": "power_in"}, ...].
        #[arg(long, value_parser = pin_specs, default_value = "[]")]
        pins: PinSpecs,
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
        /// The class of part it draws: resistor, capacitor, inductor,
        /// diode, led, transistor, opamp, connector, regulator, ic (none
        /// takes it off an edit).
        #[arg(long)]
        class: Option<String>,
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
    /// Take maker, part number, description, datasheet, category and
    /// supplier number from a part record (lcsc:C1525, mpn:...); never its
    /// parameters. Fields given win.
    #[arg(long, value_name = "SOURCE:KEY")]
    from: Option<String>,
    /// <id>, or lib/<alias>/symbol/<id>. With --from, the one the
    /// library's components of the record's category and package use most
    /// when left out.
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
    /// resistor, capacitor, inductor, ferrite_bead, diode, led,
    /// transistor, regulator, ic, crystal, connector, switch, fuse or
    /// other; "" for the one the prefix gives.
    #[arg(long)]
    category: Option<String>,
    /// Another maker's part it may be bought as: MAKER:MPN; repeat for
    /// more. One with a part number already listed replaces it.
    #[arg(long = "alternate", value_name = "MAKER:MPN")]
    alternates: Vec<String>,
    /// Take the alternate of this part number off; repeat for more.
    #[arg(long = "remove-alternate", value_name = "MPN")]
    remove_alternates: Vec<String>,
    /// An offer for an alternate: its part number, then supplier=sku;
    /// repeat for more.
    #[arg(long = "alternate-offer", num_args = 2, value_names = ["MPN", "SUPPLIER=SKU"])]
    alternate_offers: Vec<String>,
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
        let alternates = self
            .alternates
            .iter()
            .map(|t| {
                let (maker, mpn) = t
                    .split_once(':')
                    .filter(|(a, b)| !a.trim().is_empty() && !b.trim().is_empty())
                    .ok_or_else(|| {
                        Error::new(Code::Usage, format!("--alternate is MAKER:MPN, not {t:?}"))
                    })?;
                Ok(ryueda::libcomp::Alternate {
                    manufacturer: maker.trim().to_owned(),
                    mpn: mpn.trim().to_owned(),
                    offers: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let alternate_offers = self
            .alternate_offers
            .chunks(2)
            .map(|pair| {
                let (supplier, sku) = pair[1].split_once('=').ok_or_else(|| {
                    Error::new(
                        Code::Usage,
                        format!(
                            "--alternate-offer's offer is supplier=sku, not {:?}",
                            pair[1]
                        ),
                    )
                })?;
                Ok(ryueda::libcmd::AlternateOffer {
                    mpn: pair[0].clone(),
                    supplier: supplier.to_owned(),
                    sku: sku.to_owned(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ryueda::libcmd::CompFields {
            from: self.from.clone(),
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
            category: self.category.clone(),
            alternates: None,
            remove_alternates: (!self.remove_alternates.is_empty())
                .then(|| self.remove_alternates.clone()),
            add_alternates: (!alternates.is_empty()).then_some(alternates),
            alternate_offers: (!alternate_offers.is_empty()).then_some(alternate_offers),
        })
    }
}

#[derive(Subcommand)]
enum PartsCmd {
    /// Parts by category and conditions: name=value, name>=value,
    /// name<=value or name~text, each name in the part vocabulary.
    Search {
        /// A category first if wanted (capacitor), then conditions.
        terms: Vec<String>,
        /// Where to search, in order: library (the default), a catalogue
        /// (jlc) or a distributor (digikey, called live with your keys).
        /// Repeat for more.
        #[arg(long = "in", value_name = "SOURCE")]
        sources: Vec<String>,
        /// At least this many in stock.
        #[arg(long)]
        stock: Option<u64>,
        /// JLC Basic (or Preferred) parts only.
        #[arg(long)]
        basic: bool,
        /// How many results (20 by default, 1000 at most).
        #[arg(long)]
        limit: Option<usize>,
        /// Search with this component's category and parameters: <id>,
        /// lib/<alias>/libcomp/<id>, or a placed <block>/<component>.
        #[arg(long)]
        like: Option<String>,
        /// Stay off the network: distributors are left out.
        #[arg(long)]
        offline: bool,
        #[command(flatten)]
        long: LongArgs,
    },
    /// The records a key names: <source>:<key> (lcsc:C1525,
    /// digikey:311-1338-1-ND), mpn:<part number> for every source, or
    /// library:<reference>. A distributor is called when this machine
    /// keeps no record of the key.
    Show {
        key: String,
        /// Call distributors again; say how to refresh a catalogue.
        #[arg(long)]
        refresh: bool,
        /// Stay off the network: only what this machine keeps.
        #[arg(long)]
        offline: bool,
        #[command(flatten)]
        long: LongArgs,
    },
    /// Keep a distributor's keys on this machine, owner-only, beside the
    /// preferences: its key values on stdin, one a line, in its order
    /// (digikey: client ID, then client secret). Never in a project.
    Login {
        /// The distributor (`ryueda parts sources` lists them).
        distributor: String,
    },
    /// Forget a distributor's keys kept on this machine.
    Logout { distributor: String },
    /// The project's sourcing (parts.json): the assembly house, how many
    /// boards a build makes, the sources searches ask, supply findings'
    /// severities. Nothing given shows it.
    Configure {
        /// jlcpcb, or none.
        #[arg(long)]
        assembly: Option<String>,
        /// Boards a build makes (1 or more), or default.
        #[arg(long)]
        build_quantity: Option<String>,
        /// A source a search without --in asks, in order; repeat for
        /// more. Replaces the list.
        #[arg(long = "source", conflicts_with = "no_sources")]
        sources: Vec<String>,
        /// Searches without --in ask the library alone again.
        #[arg(long)]
        no_sources: bool,
        /// CODE=error|warning|off|default for a supply finding; repeat
        /// for more.
        #[arg(long, value_parser = pair)]
        severity: Vec<(String, String)>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Sync each catalogue the placed parts' offers draw on that is
    /// missing or older than parts.max_age, and fetch again each
    /// distributor's record of their offers this machine doesn't keep
    /// (every one with --force): a long request, as sync is.
    Refresh {
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        long: LongArgs,
    },
    /// Every source of parts, and each catalogue's state on this machine:
    /// when it was synced, how many parts, whether it is stale.
    Sources,
    /// Download a catalogue into this machine's parts cache, replacing
    /// the one there once it is whole. A long request: --token names it
    /// for `ryueda cancel`, --progress prints its progress on stderr.
    Sync {
        /// The catalogue's name (`ryueda parts sources` lists them).
        source: String,
        #[command(flatten)]
        long: LongArgs,
    },
}

/// What a long request takes.
#[derive(Args)]
struct LongArgs {
    /// A name for `ryueda cancel` (one is made when left out).
    #[arg(long)]
    token: Option<String>,
    /// Print progress on stderr as it goes.
    #[arg(long)]
    progress: bool,
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
        properties: PropertyFlags,
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

#[derive(Subcommand)]
enum StyleCmd {
    /// An object's properties (by designator, net name, rail:NAME,
    /// port:NAME or a region's title) and where each came from.
    Explain {
        target: String,
        #[arg(long)]
        block: String,
        #[arg(long)]
        sheet: Option<String>,
    },
    /// Set properties (NAME=VALUE: colour=hv, fill=#ffffff/#202020,
    /// line_weight=heavy, text_size=large, emphasis=true, dash=true,
    /// visible.pin_numbers=false) on an object (by the agent's name, with
    /// --block; rail:NAME without) or on the rule of a selector
    /// (tag=hv,kind=net).
    Set {
        target: String,
        #[arg(value_parser = pair, required = true, value_name = "NAME=VALUE")]
        set: Vec<(String, String)>,
        #[arg(long)]
        block: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Take properties off an object's override or a selector's rule.
    Unset {
        target: String,
        #[arg(required = true)]
        names: Vec<String>,
        #[arg(long)]
        block: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Add a rule (a selector and NAME=VALUE...) or remove one by index.
    Rule {
        #[command(subcommand)]
        cmd: StyleRuleCmd,
    },
    /// Set a palette colour: NAME #light/#dark, or NAME default.
    Palette {
        #[command(subcommand)]
        cmd: StylePaletteCmd,
    },
}

#[derive(Subcommand)]
enum StyleRuleCmd {
    /// A rule at the end: a selector (tag=hv,kind=net) and NAME=VALUE...
    Add {
        select: String,
        #[arg(value_parser = pair, required = true, value_name = "NAME=VALUE")]
        set: Vec<(String, String)>,
        #[command(flatten)]
        expect: Expect,
    },
    /// The rule at INDEX (from 0) removed.
    Remove {
        index: usize,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum StylePaletteCmd {
    /// A palette colour: NAME #light/#dark, or NAME default.
    Set {
        name: String,
        value: String,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum StandardsCmd {
    /// Change this project's own standards by a JSON merge patch:
    /// '{"shapes": "iec"}', '{"classes": {"ic": {"max_pins_per_unit": 100}}}';
    /// null puts a field back.
    Set {
        #[arg(value_parser = json_value)]
        patch: Value,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum LintCmd {
    /// Set rules' weights (CODE=N) and severities (CODE=error|warning|off),
    /// and preferred styles (resistor=us|iec, local_nets=wires|labels);
    /// `default` for a value puts it back; repeat each.
    Configure {
        #[arg(long = "weight", value_parser = pair, value_name = "CODE=N")]
        weight: Vec<(String, String)>,
        #[arg(long = "severity", value_parser = pair, value_name = "CODE=LEVEL")]
        severity: Vec<(String, String)>,
        #[arg(long = "prefer", value_parser = pair, value_name = "STYLE=VALUE")]
        prefer: Vec<(String, String)>,
        /// Answer what the preferences would change (each laid-out
        /// sheet's score before and after, the findings that come and
        /// go); write nothing.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        expect: Expect,
    },
}

/// Properties set and removed, as flags.
#[derive(Args)]
struct PropertyFlags {
    /// NAME=VALUE: a property (text) to set; repeat for more. Text on
    /// sheets and designator formats read it as {NAME}.
    #[arg(long = "property", value_parser = pair, value_name = "NAME=VALUE")]
    set: Vec<(String, String)>,
    /// A property to remove; repeat for more.
    #[arg(long = "unset-property", value_name = "NAME")]
    unset: Vec<String>,
}

impl PropertyFlags {
    fn map(&self) -> serde_json::Map<String, Value> {
        settings(&self.set, &self.unset)
    }
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Set the project's properties (the outermost level every block's
    /// text and designator formats read) and its schematic sheet.
    Edit {
        #[command(flatten)]
        properties: PropertyFlags,
        /// The paper every sheet is laid out to fit: a4, a3, a2, a1, a0,
        /// ansi_a to ansi_e (a3 by default).
        #[arg(long, value_name = "SIZE")]
        sheet_size: Option<String>,
        /// landscape (the default) or portrait.
        #[arg(long)]
        orientation: Option<String>,
        /// Paper fit: each sheet on the smallest of --sizes that holds it
        /// (true, the default), or every sheet on --sheet-size (false).
        #[arg(long, value_name = "true|false")]
        fit: Option<bool>,
        /// The papers paper fit takes, comma separated, up to
        /// --sheet-size (a4,a3 by default).
        #[arg(long, value_name = "SIZES")]
        sizes: Option<String>,
        /// The library project (an alias) whose style.json is the
        /// organisation's style; "" for none.
        #[arg(long)]
        style_from: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
}

/// A JSON enum value as typed.
fn named<T: serde::de::DeserializeOwned>(what: &str, s: &str, choices: &str) -> Result<T> {
    serde_json::from_value(json!(s))
        .map_err(|_| Error::new(Code::Usage, format!("{s:?} isn't a {what}: {choices}")))
}

/// How an instance's occurrences' designators are named, as flags.
#[derive(Args)]
struct ChannelFlags {
    /// A designator format ("{refdes}_{stack_letter}",
    /// "{prefix}{number + stack_index * 100}{rest}") or a preset:
    /// suffix_letter, suffix_index, offset_hundreds, prefix_block.
    #[arg(long, conflicts_with_all = ["suffix", "offset"])]
    refdes: Option<String>,
    /// Reference designators of this instance's occurrences take this
    /// suffix (R5 becomes R5_A).
    #[arg(long, conflicts_with = "offset")]
    suffix: Option<String>,
    /// ... or have this added to their number (R5 becomes R105).
    #[arg(long)]
    offset: Option<u32>,
}

impl ChannelFlags {
    fn format(&self) -> Option<String> {
        use ryueda::conn::Channel;
        match (&self.refdes, &self.suffix, self.offset) {
            (Some(f), _, _) => Some(f.clone()),
            (None, Some(s), _) => Some(Channel::Suffix(s.clone()).format()),
            (None, None, Some(n)) => Some(Channel::Offset(n).format()),
            (None, None, None) => None,
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
        properties: PropertyFlags,
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

fn json_value(s: &str) -> std::result::Result<Value, String> {
    serde_json::from_str(s).map_err(|e| format!("not JSON: {e}"))
}

#[derive(Clone)]
struct PinSpecs(Vec<ryueda::template::PinSpec>);

fn pin_specs(s: &str) -> std::result::Result<PinSpecs, String> {
    serde_json::from_str(s)
        .map(PinSpecs)
        .map_err(|e| format!("not a list of pins: {e}"))
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
/// `p` as an absolute path, relative to `here` (where the CLI runs as if
/// started), for the daemon, which has no folder of the caller's.
fn absolute(here: &std::path::Path, p: &std::path::Path) -> String {
    let joined = here.join(p);
    let p = joined.canonicalize().unwrap_or(joined);
    ryueda::source::plain(p).to_string_lossy().into_owned()
}

fn request(cmd: &Command, here: &std::path::Path) -> Result<Option<Request>> {
    let all = |list: &[PathBuf]| list.iter().map(|p| absolute(here, p)).collect::<Vec<_>>();
    Ok(Some(match cmd {
        Command::Import(ImportCmd::Kicad {
            paths,
            footprints,
            models,
            into,
            dry_run,
            expect,
        }) => Request::ImportKicad {
            paths: all(paths),
            footprints: all(footprints),
            models: all(models),
            into: into.clone(),
            dry_run: *dry_run,
            expected_revision: expect.expect.clone(),
        },
        Command::Info => Request::ProjectInfo {},
        Command::Project(ProjectCmd::Edit {
            properties,
            sheet_size,
            orientation,
            fit,
            sizes,
            style_from,
            expect,
        }) => Request::ProjectEdit {
            style_from: style_from.clone(),
            fit: *fit,
            sizes: sizes
                .as_deref()
                .map(|list| {
                    list.split(',')
                        .map(|s| {
                            ryueda::project::PaperSize::parse(s.trim()).ok_or_else(|| {
                                Error::new(
                                    Code::InvalidParam,
                                    format!("{s:?} isn't a paper size: a4, a3, a2, a1, a0, ansi_a, ansi_b, ansi_c, ansi_d, ansi_e"),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?,
            properties: properties.map(),
            sheet_size: sheet_size
                .as_deref()
                .map(|s| {
                    named(
                        "sheet size",
                        s,
                        "a4, a3, a2, a1, a0, ansi_a, ansi_b, ansi_c, ansi_d, ansi_e",
                    )
                })
                .transpose()?,
            orientation: orientation
                .as_deref()
                .map(|s| named("orientation", s, "landscape or portrait"))
                .transpose()?,
            expected_revision: expect.expect.clone(),
        },
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
        Command::Lib(LibCmd::Check { library, symbol }) => Request::LibCheck {
            library: library.clone(),
            symbol: symbol.clone(),
        },
        Command::Symbol(SymbolCmd::Create {
            name,
            id,
            units,
            ops,
            class,
            template,
            pins,
            expect,
        }) => Request::SymbolCreate {
            name: name.clone(),
            id: id.clone(),
            units: *units,
            ops: ops.0.clone(),
            class: class.clone(),
            template: template.clone(),
            pins: pins.0.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Symbol(SymbolCmd::Edit {
            symbol,
            name,
            units,
            ops,
            class,
            expect,
        }) => Request::SymbolEdit {
            symbol: symbol.clone(),
            name: name.clone(),
            units: *units,
            ops: ops.0.clone(),
            class: class.clone(),
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
        Command::Parts(PartsCmd::Sync { source, .. }) => Request::PartsSync {
            source: source.clone(),
        },
        Command::Parts(PartsCmd::Sources) => Request::PartsSources {},
        Command::Parts(PartsCmd::Refresh { force, .. }) => Request::PartsRefresh { force: *force },
        Command::Parts(PartsCmd::Configure {
            assembly,
            build_quantity,
            sources,
            no_sources,
            severity,
            expect,
        }) => Request::PartsConfigure {
            assembly: assembly.as_deref().map(|a| json!(a)),
            build_quantity: build_quantity
                .as_deref()
                .map(|b| b.parse::<u64>().map_or_else(|_| json!(b), |n| json!(n))),
            sources: if *no_sources {
                Some(Vec::new())
            } else {
                (!sources.is_empty()).then(|| sources.clone())
            },
            severity: severity
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect(),
            expected_revision: expect.expect.clone(),
        },
        Command::Parts(PartsCmd::Show {
            key,
            refresh,
            offline,
            ..
        }) => Request::PartsShow {
            key: key.clone(),
            refresh: *refresh,
            offline: *offline,
        },
        Command::Cancel { token } => Request::RequestCancel {
            token: token.clone(),
            name: None,
        },
        Command::Parts(PartsCmd::Search {
            terms,
            sources,
            stock,
            basic,
            limit,
            like,
            offline,
            ..
        }) => {
            // A first word with no operator in it is the category.
            let (category, conditions) = match terms.split_first() {
                Some((first, rest)) if !first.contains(['=', '<', '>', '~']) => {
                    (Some(first.clone()), rest.to_vec())
                }
                _ => (None, terms.clone()),
            };
            Request::PartsSearch {
                search: ryueda::parts::Search {
                    category,
                    conditions,
                    sources: sources.clone(),
                    stock: *stock,
                    basic: *basic,
                    limit: *limit,
                    like: like.clone(),
                    offline: *offline,
                },
            }
        }
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
            properties,
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
                properties: properties.map(),
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
            refdes: channel.format(),
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
            properties,
            expect,
        }) => Request::InstanceEdit {
            instance: instance.clone(),
            fields: ryueda::partcmd::InstanceFields {
                ports: settings(ports, unport),
                refdes: channel.format(),
                no_refdes: *no_refdes,
                overrides: settings(overrides, unoverride),
                properties: properties.map(),
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
        Command::Lint {
            action: None,
            explain,
        } => Request::LintGet {
            explain: explain.clone(),
        },
        Command::Lint {
            explain: _,
            action:
                Some(LintCmd::Configure {
                    weight,
                    severity,
                    prefer,
                    dry_run,
                    expect,
                }),
        } => {
            let map = |list: &[(String, String)], number: bool| -> serde_json::Map<String, Value> {
                list.iter()
                    .map(|(k, v)| {
                        let v = match v.as_str() {
                            "default" => Value::Null,
                            v if number => v.parse::<i64>().map_or_else(|_| json!(v), |n| json!(n)),
                            v => json!(v),
                        };
                        (k.clone(), v)
                    })
                    .collect()
            };
            Request::LintConfigure {
                weights: map(weight, true),
                severity: map(severity, false),
                prefer: map(prefer, false),
                expected_revision: expect.expect.clone(),
                dry_run: *dry_run,
            }
        }
        Command::Style { cmd: None } => Request::StyleGet {},
        Command::Style {
            cmd:
                Some(StyleCmd::Set {
                    target,
                    set,
                    block,
                    expect,
                }),
        } => Request::StyleSet {
            target: target.clone(),
            block: block.clone(),
            set: set.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Style {
            cmd:
                Some(StyleCmd::Unset {
                    target,
                    names,
                    block,
                    expect,
                }),
        } => Request::StyleUnset {
            target: target.clone(),
            block: block.clone(),
            names: names.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Style {
            cmd:
                Some(StyleCmd::Rule {
                    cmd: StyleRuleCmd::Add { select, set, expect },
                }),
        } => Request::StyleRule {
            op: "add".into(),
            select: Some(select.clone()),
            set: set.clone(),
            index: None,
            expected_revision: expect.expect.clone(),
        },
        Command::Style {
            cmd:
                Some(StyleCmd::Rule {
                    cmd: StyleRuleCmd::Remove { index, expect },
                }),
        } => Request::StyleRule {
            op: "remove".into(),
            select: None,
            set: Vec::new(),
            index: Some(*index),
            expected_revision: expect.expect.clone(),
        },
        Command::Style {
            cmd:
                Some(StyleCmd::Palette {
                    cmd: StylePaletteCmd::Set { name, value, expect },
                }),
        } => Request::StylePalette {
            name: name.clone(),
            value: value.clone(),
            expected_revision: expect.expect.clone(),
        },
        Command::Style {
            cmd:
                Some(StyleCmd::Explain {
                    target,
                    block,
                    sheet,
                }),
        } => Request::StyleExplain {
            target: target.clone(),
            block: block.clone(),
            sheet: sheet.clone(),
        },
        Command::Standards {
            library,
            action: None,
        } => Request::StandardsGet {
            library: library.clone(),
        },
        Command::Standards {
            library: _,
            action: Some(StandardsCmd::Set { patch, expect }),
        } => Request::StandardsSet {
            patch: patch.clone(),
            expected_revision: expect.expect.clone(),
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
        Command::Check { offline } => Request::Check { offline: *offline },
        Command::Layout {
            design,
            block,
            relayout,
            dry_run,
            render,
            format,
            scale,
            fit,
            sheet,
            expect,
        } => Request::Layout {
            design: design.clone(),
            block: block.clone(),
            relayout: relayout.clone(),
            dry_run: *dry_run,
            expected_revision: expect.expect.clone(),
            render: match render {
                None => None,
                Some(out) => Some(ryueda::schemcmd::RenderAsk {
                    format: picture_format(format.as_deref(), out)?,
                    scale: *scale,
                    fit: *fit,
                    sheet: sheet.clone(),
                }),
            },
        },
        Command::Place {
            block, list: true, ..
        } => Request::LayoutInstructions {
            block: block.clone(),
        },
        Command::Place {
            block,
            instructions,
            moves,
            dry_run,
            render,
            format,
            scale,
            fit,
            sheet,
            expect,
            ..
        } => Request::LayoutPlace {
            block: block.clone(),
            instructions: instructions.clone(),
            moves: moves.clone(),
            dry_run: *dry_run,
            expected_revision: expect.expect.clone(),
            render: match render {
                None => None,
                Some(out) => Some(ryueda::schemcmd::RenderAsk {
                    format: picture_format(format.as_deref(), out)?,
                    scale: *scale,
                    fit: *fit,
                    sheet: sheet.clone(),
                }),
            },
        },
        Command::Describe {
            design,
            block,
            sheet,
        } => Request::Describe {
            design: design.clone(),
            block: block.clone(),
            sheet: sheet.clone(),
        },
        Command::Render {
            design,
            block,
            sheet,
            layer,
            format,
            scale,
            fit,
            out,
        } => {
            let format = picture_format(format.as_deref(), out)?;
            Request::Render {
                design: design.clone(),
                block: block.clone(),
                sheet: sheet.clone(),
                layer: layer.clone(),
                format: Some(format),
                scale: *scale,
                fit: *fit,
            }
        }
        Command::Output {
            design,
            layers,
            out: _,
        } => Request::Output {
            design: design.clone(),
            layers: match layers.as_deref() {
                None => None,
                Some("all") => Some(ryueda::output::Layers::All),
                Some("first") => Some(ryueda::output::Layers::First),
                Some(l) => {
                    return Err(Error::new(
                        Code::Usage,
                        format!("{l:?} isn't a choice of layers: all or first"),
                    ));
                }
            },
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
        Command::Tag {
            target,
            block,
            add,
            remove,
            expect,
        } => Request::Tag {
            target: target.clone(),
            block: block.clone(),
            add: add.clone(),
            remove: remove.clone(),
            expected_revision: expect.expect.clone(),
        },
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
        Command::Version
        | Command::Init { .. }
        | Command::Watch { .. }
        | Command::Daemon(_)
        | Command::Parts(PartsCmd::Login { .. } | PartsCmd::Logout { .. }) => {
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
        // Keys go straight to the user's config folder: never through a
        // daemon, a request or a log.
        Command::Parts(PartsCmd::Login { distributor }) => {
            let dir = ryu_session::prefs::config_dir::<ryueda::Eda>()?;
            let mut values = Vec::new();
            for line in std::io::stdin().lines() {
                let line = line.map_err(|e| Error::io(std::path::Path::new("<stdin>"), &e))?;
                if !line.trim().is_empty() {
                    values.push(line.trim().to_owned());
                }
            }
            ryueda::distributors::login(&dir, distributor, &values)
        }
        Command::Parts(PartsCmd::Logout { distributor }) => {
            let dir = ryu_session::prefs::config_dir::<ryueda::Eda>()?;
            ryueda::distributors::logout(&dir, distributor)
        }
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
            let mut req = request(cmd, &here)?.expect("every other command is a request");
            if let Request::RequestCancel { name, .. } = &mut req {
                *name = participant(cli);
            }
            let root = find_root(&here)?;
            let long = long_ask(cmd);
            let print = long.as_ref().is_some_and(|a| a.progress);
            if !no_daemon(cli) {
                let sent = match &long {
                    Some(ask) => client::request_noting(
                        &root,
                        &req,
                        participant(cli).as_deref(),
                        ask,
                        &mut |p| progress_line(print, p),
                    ),
                    None => client::request(&root, &req, participant(cli).as_deref()),
                };
                match sent {
                    Ok(answer) => return answer.and_then(|a: Answer| deliver(cmd, &here, a)),
                    // Never delivered, so running it here is safe.
                    Err(e) => warnings.push(format!("{}; ran in process", e.message)),
                }
            }
            let me = participant(cli);
            // In process a long request reports its progress and can't be
            // cancelled: nothing else can reach it.
            let job = match &long {
                Some(ask) => ryu_session::job::Job::new(
                    ask.token.as_deref().unwrap_or("cli"),
                    me.as_deref().unwrap_or("agent"),
                    &req.method(),
                    Default::default(),
                )
                .with_line(Box::new(move |p: &Value| progress_line(print, p))),
                None => ryu_session::job::Job::default(),
            };
            server::handle_job(&mut None, None, me.as_deref(), &root, req, &job)
                .and_then(|a| deliver(cmd, &here, a))
        }
    }
}

/// A token for one CLI request: its process and the time, unique enough
/// among one participant's requests (ryucad's `cancel::token`).
fn cli_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("cli-{}-{nanos:08x}", std::process::id())
}

/// What a long request's command asks of its job: always a token.
fn long_ask(cmd: &Command) -> Option<ryu_session::job::Ask> {
    let long = match cmd {
        Command::Parts(
            PartsCmd::Sync { long, .. }
            | PartsCmd::Refresh { long, .. }
            | PartsCmd::Search { long, .. }
            | PartsCmd::Show { long, .. },
        ) => long,
        _ => return None,
    };
    Some(ryu_session::job::Ask {
        progress: long.progress,
        token: Some(long.token.clone().unwrap_or_else(cli_token)),
    })
}

/// A progress line on stderr, when asked for: `fetch: 3/40 files
/// (components-res-001.jsonl.gz)`.
fn progress_line(print: bool, p: &Value) {
    if print && p.get("end").is_none() {
        eprintln!(
            "{}: {}/{} {} ({})",
            p["phase"].as_str().unwrap_or(""),
            p["done"],
            p["total"],
            p["unit"].as_str().unwrap_or(""),
            p["item"].as_str().unwrap_or("")
        );
    }
}

/// A picture's format: as given, else from the file's extension.
fn picture_format(given: Option<&str>, out: &std::path::Path) -> Result<ryueda::schemcmd::Format> {
    Ok(match given {
        Some("svg") => ryueda::schemcmd::Format::Svg,
        Some("png") => ryueda::schemcmd::Format::Png,
        Some(f) => {
            return Err(Error::new(
                Code::Usage,
                format!("{f:?} isn't a format: svg or png"),
            ));
        }
        None => match out.extension().and_then(|e| e.to_str()) {
            Some(e) if e.eq_ignore_ascii_case("png") => ryueda::schemcmd::Format::Png,
            _ => ryueda::schemcmd::Format::Svg,
        },
    })
}

/// The result to print; bytes the answer carries go to `--out` (relative
/// to where the CLI runs as if started), atomically.
fn deliver(cmd: &Command, here: &std::path::Path, a: Answer) -> Result<Value> {
    let Some(bytes) = a.binary else {
        return Ok(a.result);
    };
    let (Command::Render { out, .. }
    | Command::Output { out, .. }
    | Command::Place {
        render: Some(out), ..
    }
    | Command::Layout {
        render: Some(out), ..
    }) = cmd
    else {
        return Err(Error::new(
            Code::Internal,
            "bytes with nowhere to write them",
        ));
    };
    ryueda::source::write_atomic(&here.join(out), &bytes)?;
    let mut result = a.result;
    result["out"] = json!(out);
    Ok(result)
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
