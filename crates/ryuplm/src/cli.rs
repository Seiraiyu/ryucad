//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code, as ryucad's, ryueda's and ryudfm's are.
//! `git textconv` and `git merge-driver` are Git's, not an agent's: they
//! write what Git reads.

use clap::{Args, Parser, Subcommand};
use ryu_session::live::SelectMode;
use ryuplm::error::{Code, Error, Result};
use ryuplm::proto::{Request, VersionInfo};
use ryuplm::units::Unit;
use ryuplm::{client, daemon, server};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ryuplm",
    about = "Product lifecycle management: a catalogue of items in Git, for agents",
    disable_version_flag = true
)]
struct Cli {
    /// Run as if started in this folder.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    dir: Option<PathBuf>,
    /// Indent the JSON output.
    #[arg(long, global = true)]
    pretty: bool,
    /// Run in this process instead of through the catalogue's daemon (also
    /// RYUPLM_NO_DAEMON=1).
    #[arg(long, global = true)]
    no_daemon: bool,
    /// Act as the agent participant with this name (also
    /// RYUPLM_PARTICIPANT); `agent` by default.
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

/// The catalogue at or above `start` (`ryuplm.json`), or
/// `project_not_found`.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    let start = ryuplm::source::plain(start.canonicalize().map_err(|e| Error::io(start, &e))?);
    start
        .ancestors()
        .find(|d| d.join(ryuplm::catalogue::MANIFEST).is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            Error::new(Code::ProjectNotFound, format!(
                "no ryuplm.json in {} or any folder above it; run `ryuplm init` or pass -C <dir>", start.display()))
            .with("path", &start)
        })
}

#[derive(Subcommand)]
enum Command {
    /// This build's version, commit, target and protocol.
    Version,
    /// Open a catalogue (this folder by default) in the desktop app,
    /// ryuplm-app; RYUPLM_APP names another.
    Open {
        /// The folder to open: this one (or -C's) by default.
        #[arg(id = "folder", value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Every error code with its exit code, title and the detail keys it
    /// declares, or one code's: answered by this build, with no catalogue
    /// or daemon.
    Errors {
        /// One code, such as item_not_found.
        code: Option<String>,
    },
    /// Make this folder a catalogue.
    Init {
        #[arg(long, value_parser = unit)]
        unit: Option<Unit>,
    },
    /// The catalogue: its project ID, classes, item count and revision.
    Info,
    #[command(subcommand)]
    Item(ItemCmd),
    /// Controlled documents (specifications, datasheets, drawing PDFs,
    /// certificates): their records, bytes and items.
    #[command(subcommand)]
    Document(DocumentCmd),
    #[command(subcommand)]
    Settings(SettingsCmd),
    /// Every finding: file faults, AML, classes, properties, numbers, releases.
    Check {
        /// Also compare numbers with the catalogues under this folder.
        #[arg(long = "in", value_name = "DIR")]
        within: Vec<String>,
        /// Compare with this Git revision: faults say whether they are
        /// new since then, and the report lists what changed.
        #[arg(long, value_name = "REV")]
        against: Option<String>,
    },
    #[command(subcommand)]
    Release(ReleaseCmd),
    /// An item's BOM: its own lines plus its representations' (show),
    /// its own lines edited (add, edit, remove), and what changed (diff).
    #[command(subcommand)]
    Bom(BomCmd),
    /// Items from other systems' files (CSV, XLSX), through a committed
    /// mapping (imports/<name>.json) the user or agent writes.
    #[command(subcommand)]
    Import(ImportCmd),
    /// The organization's ledger of part numbers (design 936), for a
    /// catalogue whose ledger is on: reserve numbers, reserve by hand, who
    /// holds a number, the live reservations.
    #[command(subcommand)]
    Number(NumberCmd),
    /// Every item whose own lines or representations' lines use ITEM:
    /// here, in other catalogues' checkouts, and in their releases.
    WhereUsed {
        item: String,
        /// Search the catalogues under this folder (else pdm.dependents).
        #[arg(long = "in", value_name = "DIR")]
        within: Vec<String>,
        /// Don't search the pdm.dependents folders.
        #[arg(long)]
        no_dependents: bool,
        /// Working files only: no releases.
        #[arg(long)]
        working: bool,
        /// Ask the hosted service instead: every repository of the
        /// organization the token can read (review.url and its token).
        #[arg(long, conflicts_with_all = ["within", "no_dependents", "working"])]
        org: bool,
    },
    /// The projects this catalogue reaches, without reading them: each
    /// ryucad or ryueda project its links name, each other catalogue its
    /// lines name, and the folder each was found in.
    Reps {
        /// A Git revision of the catalogue: the working files by default.
        #[arg(long, value_name = "REV")]
        rev: Option<String>,
        /// Also look under this folder.
        #[arg(long = "in", value_name = "DIR")]
        within: Vec<String>,
    },
    /// The whole catalogue as JSON lines in a file: a header, one line
    /// per item (fields, links, joined BOM lines, state, labels, faults
    /// counted), then the faults that name no item.
    Index {
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// A Git revision of the catalogue: the working files by default.
        #[arg(long, value_name = "REV")]
        rev: Option<String>,
        /// Also look for representations under this folder.
        #[arg(long = "in", value_name = "DIR")]
        within: Vec<String>,
    },
    /// Items added, removed and changed between two revisions.
    Diff {
        from: Option<String>,
        to: Option<String>,
    },
    /// Whether references (item/<id>, item/<id>/line/<line>) still stand
    /// at a revision: here, removed from their item, or gone.
    Resolve {
        #[arg(value_name = "REF", required = true)]
        refs: Vec<String>,
        /// A Git revision of the catalogue: the working files by default.
        #[arg(long, value_name = "REV")]
        rev: Option<String>,
    },
    #[command(subcommand)]
    Git(GitCmd),
    #[command(subcommand)]
    Merge(MergeCmd),
    /// The hosted service for this clone's GitHub pull requests: status,
    /// threads, comments, approvals. It needs a GitHub token
    /// (RYUPLM_REVIEW_TOKEN, GH_TOKEN or gh).
    #[command(subcommand)]
    Review(ReviewCmd),
    History {
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        before: Option<u64>,
    },
    Participants,
    Undo {
        entry: Option<u64>,
    },
    Redo {
        entry: Option<u64>,
    },
    /// Select references (item/<id or number>, item/<id>/line/<line>,
    /// import/<name>), replacing your selection, or with --add or
    /// --remove changing it; --clear empties it. Through the daemon.
    Select {
        refs: Vec<String>,
        #[arg(long, conflicts_with_all = ["remove", "clear"])]
        add: bool,
        #[arg(long, conflicts_with = "clear")]
        remove: bool,
        #[arg(long)]
        clear: bool,
    },
    /// What a participant has selected (the one person attached by
    /// default), each reference with what it names. Through the daemon.
    Selection {
        /// A participant's ID (`participants` lists them).
        #[arg(long)]
        of: Option<String>,
    },
    /// Highlight references in every window watching, in your colour:
    /// pending (about to change), changed, or note (the default), under a
    /// --label; --clear takes yours away (only --style's, if given).
    /// Through the daemon.
    Highlight {
        refs: Vec<String>,
        #[arg(long)]
        style: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long, conflicts_with = "label")]
        clear: bool,
    },
    /// Say what you are doing, in one line under every window's People
    /// strip, with --ref for what you are working on; "" clears it.
    /// Through the daemon.
    Status {
        text: String,
        #[arg(long = "ref", value_name = "REF")]
        refs: Vec<String>,
    },
    /// What a window shows (the one person attached by default); `view
    /// set` sets yours. Through the daemon.
    View {
        #[command(subcommand)]
        cmd: Option<ViewCmd>,
        /// A participant's ID (`participants` lists them).
        #[arg(long)]
        of: Option<String>,
    },
    /// Show a reference to a participant (every person attached by
    /// default): an item opens, a line opens its item's BOM, an import
    /// the Imports tab, and it is selected. Through the daemon.
    Show {
        #[arg(value_name = "REF")]
        reference: String,
        /// A participant's ID (`participants` lists them).
        #[arg(long = "for", value_name = "PARTICIPANT")]
        participant: Option<String>,
    },
    /// Cancel a running long request by its token (through the daemon).
    Cancel {
        token: String,
    },
    Watch {
        #[arg(long = "topic")]
        topics: Vec<String>,
        #[arg(long, default_value_t = 1)]
        count: usize,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    #[command(subcommand)]
    Daemon(DaemonCmd),
}

#[derive(Subcommand)]
enum ViewCmd {
    /// Set your window's view, whole: the open item, its tab and the
    /// list's filter.
    Set {
        /// An item's ID or number.
        #[arg(long)]
        item: Option<String>,
        /// bom, where_used, findings, changes, releases, imports,
        /// documents or activity.
        #[arg(long)]
        tab: Option<String>,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        class: Option<String>,
        #[arg(long)]
        state: Option<String>,
        /// errors or warnings.
        #[arg(long)]
        faults: Option<String>,
        /// An import mapping's name.
        #[arg(long)]
        source: Option<String>,
        /// id, number, name or class.
        #[arg(long)]
        sort: Option<String>,
    },
}

/// A highlight style by name.
fn style(s: &str) -> Result<ryu_session::live::Style> {
    serde_json::from_value(json!(s)).map_err(|_| {
        Error::new(
            Code::Usage,
            format!("--style is pending, changed or note, not {s:?}"),
        )
    })
}

/// What `item create` and `item edit` set.
#[derive(Args, Default)]
struct FieldArgs {
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    class: Option<String>,
    /// each, m, m2, m3, kg or l.
    #[arg(long)]
    unit: Option<String>,
    /// make or buy.
    #[arg(long)]
    make_buy: Option<String>,
    /// A declared property: NAME=JSON (NAME=null removes it).
    #[arg(long = "property", value_name = "NAME=JSON")]
    properties: Vec<String>,
    /// The whole AML, as JSON.
    #[arg(long)]
    aml: Option<String>,
}

impl FieldArgs {
    fn fields(&self) -> Result<ryuplm::items::Fields> {
        let mut props = std::collections::BTreeMap::new();
        for p in &self.properties {
            let (k, v) = p.split_once('=').ok_or_else(|| {
                Error::new(Code::Usage, format!("--property {p:?}: use NAME=JSON"))
            })?;
            let v = ryuplm::canonical::parse(v)
                .or_else(|_| Ok::<_, Error>(serde_json::Value::from(v)))?;
            props.insert(k.to_owned(), v);
        }
        let aml = self
            .aml
            .as_deref()
            .map(|a| -> Result<Vec<serde_json::Value>> {
                serde_json::from_str(a)
                    .map_err(|e| Error::new(Code::InvalidJson, format!("--aml: {e}")))
            })
            .transpose()?;
        Ok(ryuplm::items::Fields {
            name: self.name.clone(),
            description: self.description.clone(),
            class: self.class.clone(),
            unit: self.unit.clone(),
            make_buy: self.make_buy.clone(),
            properties: (!props.is_empty()).then_some(props),
            aml,
        })
    }
}

#[derive(Subcommand)]
enum ImportCmd {
    /// Propose a mapping for FILE from its header and first rows.
    Guess {
        file: PathBuf,
        #[arg(long)]
        sheet: Option<String>,
        /// One character, or `tab`.
        #[arg(long)]
        delimiter: Option<String>,
        /// utf-8, utf-16le, utf-16be, latin1 or windows-1252.
        #[arg(long)]
        encoding: Option<String>,
        /// JSON and XML: where the records are (a JSON Pointer such as
        /// /data/items, or an element path such as /Export/Items/Item),
        /// instead of the proposed one.
        #[arg(long)]
        records: Option<String>,
        /// Save the proposal as imports/NAME.json.
        #[arg(long, value_name = "NAME")]
        save: Option<String>,
    },
    /// What mapping NAME reads from FILE, row by row; every refusal at its
    /// row and column. Writes nothing.
    Check {
        name: String,
        file: PathBuf,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Import FILE through mapping NAME: new keys become items, changed
    /// rows are settled field by field, nothing is ever deleted.
    Run {
        name: String,
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
        /// Settle conflicts: source or catalogue.
        #[arg(long)]
        prefer: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[command(flatten)]
        expect: Expect,
    },
    /// The mapping files, and how many items each has made.
    List,
}

#[derive(Subcommand)]
enum NumberCmd {
    /// Reserve the next COUNT numbers of SCHEME (or of the one that
    /// applies) without creating items.
    Next {
        scheme: Option<String>,
        #[arg(long)]
        class: Option<String>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=100_000))]
        count: u64,
        /// Days each stays reserved, 1 to 365 (30 by default).
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=365))]
        days: Option<u64>,
    },
    /// Reserve numbers typed by hand to this catalogue, all or none.
    Reserve {
        #[arg(required = true)]
        numbers: Vec<String>,
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=365))]
        days: Option<u64>,
    },
    /// Who holds NUMBER in the organization.
    Who { number: String },
    /// This catalogue's live reservations, 1,000 a page.
    List {
        /// Only the ones you reserved.
        #[arg(long)]
        mine: bool,
        /// The whole organization's, not just this catalogue's.
        #[arg(long)]
        all: bool,
        /// The page after this number (the last page's `after`).
        #[arg(long)]
        after: Option<String>,
    },
}

#[derive(Subcommand)]
enum BomCmd {
    /// The item's BOM: own lines, then its representations' lines, each
    /// child resolved to an item; --depth follows child items deeper.
    Show {
        item: String,
        /// A Git revision of the catalogue: the working files by default.
        #[arg(long, value_name = "REV")]
        rev: Option<String>,
        #[arg(long)]
        depth: Option<usize>,
        /// Also look for representations under this folder.
        #[arg(long = "in", value_name = "DIR")]
        within: Vec<String>,
    },
    /// What changed in BOMs between two revisions: HEAD and the working
    /// files by default; every item, or ITEM's.
    Diff {
        item: Option<String>,
        #[arg(long, value_name = "REV")]
        from: Option<String>,
        #[arg(long, value_name = "REV")]
        to: Option<String>,
        #[arg(long = "in", value_name = "DIR")]
        within: Vec<String>,
    },
    /// A new own line: CHILD (an ID, a number, or <catalogue>:<ID>) and
    /// its QUANTITY in the child's unit.
    Add {
        item: String,
        child: String,
        quantity: String,
        #[arg(long)]
        unit: Option<String>,
        /// The find number a balloon cites.
        #[arg(long)]
        find: Option<String>,
        #[arg(long)]
        refdes: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Change an own line; --unset clears unit, find, refdes or notes.
    Edit {
        item: String,
        line: String,
        #[arg(long)]
        child: Option<String>,
        #[arg(long)]
        quantity: Option<String>,
        #[arg(long)]
        unit: Option<String>,
        #[arg(long)]
        find: Option<String>,
        #[arg(long)]
        refdes: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        unset: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Take an own line off the item.
    Remove {
        item: String,
        line: String,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum ItemCmd {
    /// A new item, with a number or the next of a scheme.
    Create {
        number: Option<String>,
        /// Take the next number of SCHEME (or of the one that applies): from
        /// the organization's ledger when the catalogue's is on.
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        next: Option<String>,
        #[command(flatten)]
        fields: FieldArgs,
        #[command(flatten)]
        expect: Expect,
    },
    Show {
        item: String,
    },
    /// Items, a window at a time: --offset and --limit cut the rows from
    /// the matches, whose number is the answer's `total`.
    List {
        #[arg(long)]
        number: Option<String>,
        #[arg(long)]
        class: Option<String>,
        /// Found in the number, a previous number, the name or description.
        #[arg(long)]
        text: Option<String>,
        /// in_work, released, modified or obsolete.
        #[arg(long)]
        state: Option<String>,
        /// make or buy.
        #[arg(long)]
        make_buy: Option<String>,
        /// errors or warnings: items with at least one.
        #[arg(long)]
        faults: Option<String>,
        /// An import mapping's name: items it made or updated.
        #[arg(long)]
        source: Option<String>,
        /// Also count every item by class, state, findings and import.
        #[arg(long)]
        counts: bool,
        /// id (the default), number, name or class.
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        offset: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
    },
    Edit {
        item: String,
        #[command(flatten)]
        fields: FieldArgs,
        #[arg(long)]
        unset: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// A new number; the old one is kept in previous_numbers.
    Renumber {
        item: String,
        number: Option<String>,
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        next: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Delete {
        item: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Link the item to its ryucad or ryueda representation (one per
    /// product): PROJECT is a project ID or the project's folder.
    Link {
        item: String,
        product: String,
        project: String,
        part: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Remove the item's link of PRODUCT.
    Unlink {
        item: String,
        product: String,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Args)]
struct DocArgs {
    #[arg(long)]
    title: Option<String>,
    /// specification, datasheet, drawing, certificate, procedure or other.
    #[arg(long = "type")]
    doc_type: Option<String>,
    #[arg(long)]
    description: Option<String>,
}

impl DocArgs {
    fn fields(&self, media_type: Option<String>) -> ryuplm::documents::New {
        ryuplm::documents::New {
            title: self.title.clone(),
            doc_type: self.doc_type.clone(),
            description: self.description.clone(),
            media_type,
        }
    }
}

#[derive(Subcommand)]
enum DocumentCmd {
    /// A new document: FILE's bytes in Git beside its record, or on the
    /// service (--storage service), or a Google Drive file's (--drive).
    Add {
        #[arg(required_unless_present = "drive")]
        file: Option<PathBuf>,
        /// A Google Drive file the organization's connection can read,
        /// instead of FILE: its head revision pinned by hash.
        #[arg(long, conflicts_with = "file")]
        drive: Option<String>,
        /// git or service; the catalogue's default (documents.storage) when
        /// absent.
        #[arg(long)]
        storage: Option<String>,
        #[arg(long)]
        number: Option<String>,
        /// Take the next number of SCHEME (or of the document scheme there is).
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        next: Option<String>,
        #[command(flatten)]
        fields: DocArgs,
        /// The media type, else the one FILE's extension gives.
        #[arg(long)]
        media_type: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Show {
        document: String,
    },
    List {
        #[arg(long = "type")]
        doc_type: Option<String>,
        #[arg(long)]
        number: Option<String>,
        /// Number, title or description containing TEXT (any case).
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Change the title, type or description; --unset description clears it.
    Edit {
        document: String,
        #[command(flatten)]
        fields: DocArgs,
        #[arg(long)]
        unset: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Renumber {
        document: String,
        number: Option<String>,
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        next: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// New bytes for the document, from FILE.
    Replace {
        document: String,
        file: PathBuf,
        #[arg(long)]
        media_type: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Pin each document's Drive file again and take a newer revision.
    Sync {
        documents: Vec<String>,
        /// Every document with a Drive pin.
        #[arg(long)]
        all: bool,
        /// Say what would change; write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Upload service-stored documents' bytes the service lacks, from
    /// this machine's cache (all of them when none are named).
    Upload {
        documents: Vec<String>,
    },
    /// Move the document's bytes between Git and the service.
    Move {
        document: String,
        /// git or service.
        #[arg(long)]
        storage: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Remove a document with no releases; --detach takes it off its items too.
    Remove {
        document: String,
        #[arg(long)]
        detach: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// Write the document's bytes to --out, checked against its record.
    Get {
        document: String,
        /// A Git revision, or label:<L> for an official release.
        #[arg(long, value_name = "REV")]
        rev: Option<String>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Name DOCUMENT (this catalogue's, or <project>:<id>) on each ITEM.
    Attach {
        document: String,
        #[arg(required = true)]
        items: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Detach {
        document: String,
        #[arg(required = true)]
        items: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Every item naming DOCUMENT: here, in other catalogues' checkouts,
    /// and in their releases.
    WhereUsed {
        document: String,
        #[arg(long = "in", value_name = "DIR")]
        within: Vec<String>,
        #[arg(long)]
        no_dependents: bool,
        #[arg(long)]
        working: bool,
    },
}

/// Parsed once per run: the size of a variant doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum SettingsCmd {
    Show,
    /// Set or remove one class, numbering scheme or declared property.
    Edit {
        #[arg(long)]
        class: Option<String>,
        #[arg(long, value_delimiter = ',')]
        class_properties: Option<Vec<String>>,
        #[arg(long)]
        scheme: Option<String>,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long)]
        scheme_class: Option<String>,
        /// With --scheme: what it numbers, item (the default) or document.
        #[arg(long)]
        scheme_kind: Option<String>,
        /// New documents' storage: git (the default) or service.
        #[arg(long)]
        document_storage: Option<String>,
        #[arg(long)]
        next_template: Option<String>,
        #[arg(long)]
        property: Option<String>,
        /// The property's declaration, as JSON.
        #[arg(long)]
        declaration: Option<String>,
        #[arg(long)]
        remove: bool,
        /// The organization's ledger hands out and holds this catalogue's
        /// numbers: on or off.
        #[arg(long, value_parser = on_off)]
        ledger: Option<bool>,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum ReleaseCmd {
    Propose {
        items: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        note: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    Obsolete {
        item: String,
        label: String,
        #[arg(long)]
        note: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    List {
        item: Option<String>,
    },
    Check,
    Sync,
}

#[derive(Subcommand)]
enum GitCmd {
    /// Register ryuplm's merge driver, textconv and hook with Git.
    Install {
        #[arg(long)]
        global: bool,
    },
    /// The .gitattributes lines ryuplm's drivers need, relative to a
    /// catalogue's folder: for a tool that sets up a clone itself (the
    /// hosted service).
    Attributes,
    /// Git's textconv: a file's lines (stdout, not JSON).
    Textconv { path: PathBuf },
    /// Git's merge driver: %O %A %B %P.
    MergeDriver {
        base: PathBuf,
        ours: PathBuf,
        theirs: PathBuf,
        path: Option<String>,
    },
}

#[derive(Subcommand)]
enum ReviewCmd {
    /// The pull request's checks and threads (the branch's own pull
    /// request unless named).
    Status { pr: Option<u64> },
    /// Its comment threads; --open for those not resolved.
    Threads {
        pr: Option<u64>,
        #[arg(long)]
        open: bool,
    },
    /// Start a thread pinned to a reference (item/<id>, or
    /// item/<id>/line/<line>).
    Comment {
        #[arg(value_name = "REF")]
        reference: String,
        text: String,
        #[arg(long)]
        pr: Option<u64>,
        /// The commit the reference was read at (the pull request's head
        /// by default).
        #[arg(long)]
        commit: Option<String>,
    },
    /// Reply in a thread.
    Reply { thread: String, text: String },
    /// Resolve a thread.
    Resolve { thread: String },
    /// Reopen a thread.
    Reopen { thread: String },
    /// Merge the base into the pull request's head with ryuplm's driver
    /// (Merge in), when GitHub's line merge conflicts and ryuplm's doesn't.
    Update { pr: Option<u64> },
    /// What the release records the pull request proposes still need,
    /// and what blocks them.
    Approvals { pr: Option<u64> },
}

#[derive(Subcommand)]
enum MergeCmd {
    /// What a merge in progress left unresolved; `--hook` for Git's hook.
    Status {
        #[arg(long)]
        hook: bool,
    },
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Serve the catalogue (the CLI starts this itself).
    Run {
        /// Seconds idle before it exits.
        #[arg(long)]
        idle_timeout: Option<u64>,
    },
    Status,
    Stop {
        /// Every ryuplm daemon of this user.
        #[arg(long)]
        all: bool,
    },
}

/// Folders an agent named, absolute: the daemon runs elsewhere.
fn absolute(here: &Path, dirs: &[String]) -> Vec<String> {
    dirs.iter()
        .map(|d| {
            let p = here.join(d);
            p.canonicalize().unwrap_or(p).display().to_string()
        })
        .collect()
}

/// `file` from `here`, absolute, so a daemon reads the same file.
fn abs_file(here: &Path, file: &Path) -> String {
    let p = here.join(file);
    p.canonicalize().unwrap_or(p).display().to_string()
}

fn on_off(s: &str) -> core::result::Result<bool, String> {
    match s {
        "on" | "true" => Ok(true),
        "off" | "false" => Ok(false),
        _ => Err(format!("{s:?}: on or off")),
    }
}

fn unit(s: &str) -> core::result::Result<Unit, String> {
    Unit::parse(s).ok_or_else(|| format!("unknown unit {s:?}: m, cm, mm, um, in or ft"))
}

fn request(cmd: &Command, here: &Path) -> Result<Option<(PathBuf, Request)>> {
    let root = || find_root(here);
    let exp = |e: &Expect| e.expect.clone();
    Ok(Some(match cmd {
        Command::Info => (root()?, Request::ProjectInfo {}),
        Command::Import(ImportCmd::Guess {
            file,
            sheet,
            delimiter,
            encoding,
            records,
            save,
        }) => (
            root()?,
            Request::ImportGuess {
                file: abs_file(here, file),
                sheet: sheet.clone(),
                delimiter: delimiter.clone(),
                encoding: encoding.clone(),
                records: records.clone(),
                save: save.clone(),
            },
        ),
        Command::Import(ImportCmd::Check { name, file, limit }) => (
            root()?,
            Request::ImportCheck {
                name: name.clone(),
                file: abs_file(here, file),
                limit: *limit,
            },
        ),
        Command::Import(ImportCmd::Run {
            name,
            file,
            dry_run,
            prefer,
            limit,
            expect,
        }) => (
            root()?,
            Request::ImportRun {
                name: name.clone(),
                file: abs_file(here, file),
                dry_run: *dry_run,
                prefer: prefer.clone(),
                limit: *limit,
                numbers: Default::default(),
                expected_revision: exp(expect),
            },
        ),
        Command::Import(ImportCmd::List) => (root()?, Request::ImportList {}),
        Command::Document(DocumentCmd::Add {
            file,
            drive: _,
            storage,
            number,
            next,
            fields,
            media_type,
            expect,
        }) => (
            root()?,
            Request::DocumentAdd {
                // A Drive file goes as a pin, once the CLI has pinned it
                // (`run`); the request never names the Drive ID itself.
                file: file.as_ref().map(|f| abs_file(here, f)),
                number: number.clone(),
                next: next.clone(),
                fields: fields.fields(media_type.clone()),
                storage: storage.clone(),
                drive: None,
                expected_revision: exp(expect),
            },
        ),
        Command::Document(DocumentCmd::Show { document }) => (
            root()?,
            Request::DocumentShow {
                document: document.clone(),
            },
        ),
        Command::Document(DocumentCmd::List {
            doc_type,
            number,
            text,
            limit,
        }) => (
            root()?,
            Request::DocumentList {
                doc_type: doc_type.clone(),
                number: number.clone(),
                text: text.clone(),
                limit: *limit,
            },
        ),
        Command::Document(DocumentCmd::Edit {
            document,
            fields,
            unset,
            expect,
        }) => (
            root()?,
            Request::DocumentEdit {
                document: document.clone(),
                fields: fields.fields(None),
                unset: unset.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Document(DocumentCmd::Renumber {
            document,
            number,
            next,
            expect,
        }) => (
            root()?,
            Request::DocumentRenumber {
                document: document.clone(),
                number: number.clone(),
                next: next.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Document(DocumentCmd::Replace {
            document,
            file,
            media_type,
            expect,
        }) => (
            root()?,
            Request::DocumentReplace {
                document: document.clone(),
                file: abs_file(here, file),
                media_type: media_type.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Document(DocumentCmd::Move {
            document,
            storage,
            expect,
        }) => (
            root()?,
            Request::DocumentMove {
                document: document.clone(),
                storage: storage.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Document(DocumentCmd::Remove {
            document,
            detach,
            expect,
        }) => (
            root()?,
            Request::DocumentRemove {
                document: document.clone(),
                detach: *detach,
                expected_revision: exp(expect),
            },
        ),
        Command::Document(DocumentCmd::Get { document, rev, out }) => (
            root()?,
            Request::DocumentGet {
                document: document.clone(),
                rev: rev.clone(),
                out: abs_file(here, out),
            },
        ),
        Command::Document(DocumentCmd::Attach {
            document,
            items,
            expect,
        }) => (
            root()?,
            Request::DocumentAttach {
                document: document.clone(),
                items: items.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Document(DocumentCmd::Detach {
            document,
            items,
            expect,
        }) => (
            root()?,
            Request::DocumentDetach {
                document: document.clone(),
                items: items.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Document(DocumentCmd::WhereUsed {
            document,
            within,
            no_dependents,
            working,
        }) => (
            root()?,
            Request::DocumentWhereUsed {
                document: document.clone(),
                within: absolute(here, within),
                no_dependents: *no_dependents,
                working: *working,
            },
        ),
        Command::Item(ItemCmd::Create {
            number,
            next,
            fields,
            expect,
        }) => (
            root()?,
            Request::ItemCreate {
                fields: fields.fields()?,
                number: number.clone(),
                next: next.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Item(ItemCmd::Show { item }) => {
            (root()?, Request::ItemShow { item: item.clone() })
        }
        Command::Item(ItemCmd::List {
            number,
            class,
            text,
            state,
            make_buy,
            faults,
            source,
            counts,
            sort,
            offset,
            limit,
        }) => (
            root()?,
            Request::ItemList {
                number: number.clone(),
                class: class.clone(),
                text: text.clone(),
                state: state.clone(),
                make_buy: make_buy.clone(),
                faults: faults.clone(),
                source: source.clone(),
                counts: *counts,
                sort: sort.clone(),
                offset: *offset,
                limit: *limit,
            },
        ),
        Command::Item(ItemCmd::Edit {
            item,
            fields,
            unset,
            expect,
        }) => (
            root()?,
            Request::ItemEdit {
                item: item.clone(),
                fields: fields.fields()?,
                unset: unset.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Item(ItemCmd::Renumber {
            item,
            number,
            next,
            expect,
        }) => (
            root()?,
            Request::ItemRenumber {
                item: item.clone(),
                number: number.clone(),
                next: next.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Item(ItemCmd::Delete { item, expect }) => (
            root()?,
            Request::ItemDelete {
                item: item.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Item(ItemCmd::Link {
            item,
            product,
            project,
            part,
            expect,
        }) => (
            root()?,
            Request::ItemLink {
                item: item.clone(),
                product: product.clone(),
                // A folder is sent absolute: the daemon runs elsewhere.
                project: match here.join(project) {
                    d if d.is_dir() => d.canonicalize().unwrap_or(d).display().to_string(),
                    _ => project.clone(),
                },
                part: part.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Item(ItemCmd::Unlink {
            item,
            product,
            expect,
        }) => (
            root()?,
            Request::ItemUnlink {
                item: item.clone(),
                product: product.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::WhereUsed {
            item,
            within,
            no_dependents,
            working,
            org: false,
        } => (
            root()?,
            Request::BomWhereUsed {
                item: item.clone(),
                within: absolute(here, within),
                no_dependents: *no_dependents,
                working: *working,
            },
        ),
        Command::Reps { rev, within } => (
            root()?,
            Request::RepsList {
                rev: rev.clone(),
                within: absolute(here, within),
            },
        ),
        Command::Index { out, rev, within } => (
            root()?,
            Request::Index {
                out: abs_file(here, out),
                rev: rev.clone(),
                within: absolute(here, within),
            },
        ),
        Command::Bom(BomCmd::Show {
            item,
            rev,
            depth,
            within,
        }) => (
            root()?,
            Request::BomGet {
                item: item.clone(),
                rev: rev.clone(),
                depth: *depth,
                within: absolute(here, within),
            },
        ),
        Command::Bom(BomCmd::Diff {
            item,
            from,
            to,
            within,
        }) => (
            root()?,
            Request::BomDiff {
                item: item.clone(),
                from: from.clone(),
                to: to.clone(),
                within: absolute(here, within),
            },
        ),
        Command::Bom(BomCmd::Add {
            item,
            child,
            quantity,
            unit,
            find,
            refdes,
            notes,
            expect,
        }) => (
            root()?,
            Request::BomAdd {
                item: item.clone(),
                fields: ryuplm::bom::LineFields {
                    child: Some(child.clone()),
                    quantity: Some(quantity.clone()),
                    unit: unit.clone(),
                    find: find.clone(),
                    refdes: refdes.clone(),
                    notes: notes.clone(),
                },
                expected_revision: exp(expect),
            },
        ),
        Command::Bom(BomCmd::Edit {
            item,
            line,
            child,
            quantity,
            unit,
            find,
            refdes,
            notes,
            unset,
            expect,
        }) => (
            root()?,
            Request::BomEdit {
                item: item.clone(),
                line: line.clone(),
                fields: ryuplm::bom::LineFields {
                    child: child.clone(),
                    quantity: quantity.clone(),
                    unit: unit.clone(),
                    find: find.clone(),
                    refdes: refdes.clone(),
                    notes: notes.clone(),
                },
                unset: unset.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Bom(BomCmd::Remove { item, line, expect }) => (
            root()?,
            Request::BomRemove {
                item: item.clone(),
                line: line.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Settings(SettingsCmd::Show) => (root()?, Request::SettingsShow {}),
        Command::Settings(SettingsCmd::Edit {
            class,
            class_properties,
            scheme,
            pattern,
            scheme_class,
            scheme_kind,
            document_storage,
            next_template,
            property,
            declaration,
            remove,
            ledger,
            expect,
        }) => {
            let declaration = declaration
                .as_deref()
                .map(ryuplm::canonical::parse)
                .transpose()?;
            (
                root()?,
                Request::SettingsEdit {
                    change: ryuplm::settings::Change {
                        class: class.clone(),
                        class_properties: class_properties.clone(),
                        scheme: scheme.clone(),
                        pattern: pattern.clone(),
                        scheme_class: scheme_class.clone(),
                        scheme_kind: scheme_kind.clone(),
                        document_storage: document_storage.clone(),
                        next_template: next_template.clone(),
                        property: property.clone(),
                        declaration,
                        remove: *remove,
                        ledger: *ledger,
                    },
                    expected_revision: exp(expect),
                },
            )
        }
        Command::Check { within, against } => (
            root()?,
            Request::Check {
                within: absolute(here, within),
                against: against.clone(),
            },
        ),
        Command::Release(ReleaseCmd::Propose {
            items,
            all,
            dry_run,
            note,
            expect,
        }) => (
            root()?,
            Request::ReleasePropose {
                items: items.clone(),
                all: *all,
                dry_run: *dry_run,
                note: note.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Release(ReleaseCmd::Obsolete {
            item,
            label,
            note,
            expect,
        }) => (
            root()?,
            Request::ReleaseObsolete {
                item: item.clone(),
                label: label.clone(),
                note: note.clone(),
                expected_revision: exp(expect),
            },
        ),
        Command::Release(ReleaseCmd::List { item }) => {
            (root()?, Request::ReleaseList { item: item.clone() })
        }
        Command::Release(ReleaseCmd::Check) => (root()?, Request::ReleaseCheck {}),
        Command::Release(ReleaseCmd::Sync) => (root()?, Request::ReleaseSync {}),
        Command::Diff { from, to } => (
            root()?,
            Request::Diff {
                from: from.clone(),
                to: to.clone(),
            },
        ),
        Command::Resolve { refs, rev } => (
            root()?,
            Request::Resolve {
                refs: refs.clone(),
                rev: rev.clone(),
            },
        ),
        Command::History { limit, before } => (
            root()?,
            Request::ActivityLog {
                limit: *limit,
                before: *before,
            },
        ),
        Command::Participants => (root()?, Request::ParticipantList {}),
        Command::Undo { entry } => (root()?, Request::Undo { entry: *entry }),
        Command::Redo { entry } => (root()?, Request::Redo { entry: *entry }),
        Command::Select {
            refs,
            add,
            remove,
            clear,
        } => {
            if refs.is_empty() != *clear {
                return Err(Error::new(
                    Code::Usage,
                    "name references to select, or --clear, not both",
                ));
            }
            let mode = if *add {
                SelectMode::Add
            } else if *remove {
                SelectMode::Remove
            } else {
                SelectMode::Replace
            };
            (
                root()?,
                Request::SelectionSet {
                    refs: refs.clone(),
                    mode,
                },
            )
        }
        Command::Selection { of } => (
            root()?,
            Request::SelectionGet {
                participant: of.clone(),
            },
        ),
        Command::Highlight {
            refs,
            style: named,
            label,
            clear,
        } => {
            let named = named.as_deref().map(style).transpose()?;
            let req = match (*clear, refs.is_empty()) {
                (true, true) => Request::HighlightClear { style: named },
                (false, false) => Request::HighlightSet {
                    refs: refs.clone(),
                    style: named.unwrap_or_default(),
                    label: label.clone(),
                },
                _ => {
                    return Err(Error::new(
                        Code::Usage,
                        "name references to highlight, or --clear, not both",
                    ));
                }
            };
            (root()?, req)
        }
        Command::Status { text, refs } => (
            root()?,
            Request::ActivitySet {
                status: text.clone(),
                refs: refs.clone(),
            },
        ),
        Command::View { cmd: None, of } => (
            root()?,
            Request::ViewGet {
                participant: of.clone(),
            },
        ),
        Command::View {
            cmd:
                Some(ViewCmd::Set {
                    item,
                    tab,
                    text,
                    class,
                    state,
                    faults,
                    source,
                    sort,
                }),
            ..
        } => {
            let list = ryuplm::view::List {
                text: text.clone(),
                class: class.clone(),
                state: state.clone(),
                faults: faults.clone(),
                source: source.clone(),
                sort: sort.clone(),
            };
            let list = (list != ryuplm::view::List::default()).then_some(list);
            (
                root()?,
                Request::ViewSet {
                    item: item.clone(),
                    tab: tab.clone(),
                    list,
                },
            )
        }
        Command::Show {
            reference,
            participant,
        } => (
            root()?,
            Request::ViewShow {
                reference: reference.clone(),
                participant: participant.clone(),
            },
        ),
        Command::Cancel { token } => (
            root()?,
            Request::RequestCancel {
                token: token.clone(),
                name: None,
            },
        ),
        // Version, Init, Watch, Daemon, Git, Merge, Review, Number and
        // where-used --org are not requests.
        Command::Version
        | Command::Open { .. }
        | Command::Errors { .. }
        | Command::Init { .. }
        | Command::Watch { .. }
        | Command::Daemon(_)
        | Command::Git(_)
        | Command::Merge(_)
        | Command::Review(_)
        | Command::Number(_)
        | Command::Document(DocumentCmd::Upload { .. })
        | Command::Document(DocumentCmd::Sync { .. })
        | Command::WhereUsed { org: true, .. } => return Ok(None),
    }))
}

fn no_daemon(cli: &Cli) -> bool {
    cli.no_daemon || std::env::var("RYUPLM_NO_DAEMON").is_ok_and(|v| v == "1")
}

/// The agent participant to act as.
fn participant(cli: &Cli) -> Option<String> {
    cli.as_name
        .clone()
        .or_else(|| std::env::var("RYUPLM_PARTICIPANT").ok())
}

/// Sends `req` to the catalogue's daemon, or answers it in process when
/// none can be reached (or with --no-daemon), as every request goes.
fn send(cli: &Cli, root: &Path, req: Request, warnings: &mut Vec<String>) -> Result<Value> {
    if !no_daemon(cli) {
        match client::request(root, &req, participant(cli).as_deref()) {
            Ok(answer) => return answer.map(|a| a.result),
            // Never delivered, so running it here is safe.
            Err(e) => warnings.push(format!("{}; ran in process", e.message)),
        }
    }
    let me = participant(cli);
    server::handle_safely(&mut None, None, me.as_deref(), root, req).map(|a| a.result)
}

/// Runs the command, collecting warnings for the output.
fn run(cli: &Cli, warnings: &mut Vec<String>) -> Result<Value> {
    let here = match &cli.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::io(Path::new("."), &e))?,
    };
    match &cli.command {
        Command::Version => Ok(json!(VersionInfo::current())),
        Command::Open { dir } => {
            let dir = dir.as_ref().map_or_else(|| here.clone(), |d| here.join(d));
            let dir = ryuplm::source::plain(dir.canonicalize().map_err(|e| Error::io(&dir, &e))?);
            client::open(&find_root(&dir)?)
        }
        Command::Errors { code } => ryuplm::error::details::listing(code.as_deref()),
        Command::Init { unit } => {
            let root =
                ryuplm::source::plain(here.canonicalize().map_err(|e| Error::io(&here, &e))?);
            server::init(&root, *unit)
        }
        Command::Daemon(DaemonCmd::Run { idle_timeout }) => {
            let idle = idle_timeout.map_or(daemon::DEFAULT_IDLE, Duration::from_secs);
            daemon::run(&find_root(&here)?, idle).map(|()| Value::Null)
        }
        Command::Daemon(DaemonCmd::Status) => client::status(&find_root(&here)?),
        Command::Daemon(DaemonCmd::Stop { all: true }) => client::stop_all(),
        Command::Daemon(DaemonCmd::Stop { all: false }) => client::stop(&find_root(&here)?),
        Command::Git(GitCmd::Install { global }) => ryuplm::drivers::install(&here, !global),
        Command::Git(GitCmd::Attributes) => Ok(json!({
            "schema": ryuplm::OUTPUT_SCHEMA,
            "lines": ryuplm::drivers::LINES,
        })),
        Command::Git(GitCmd::Textconv { .. } | GitCmd::MergeDriver { .. })
        | Command::Merge(MergeCmd::Status { hook: true }) => unreachable!("answered in main"),
        Command::Merge(MergeCmd::Status { hook: false }) => {
            let roots = ryu_git::merging_projects::<ryuplm::Plm>(&here)?;
            Ok(json!({"schema": ryuplm::OUTPUT_SCHEMA, "merging": roots
                .iter()
                .map(|r| json!({"root": r, "unresolved": ryuplm::merge::unresolved(r)}))
                .collect::<Vec<_>>()}))
        }
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
        Command::WhereUsed {
            item, org: true, ..
        } => ryuplm::review::where_used_org(&find_root(&here)?, item),
        Command::Review(cmd) => {
            use ryu_review::Action;
            let action = match cmd {
                ReviewCmd::Status { pr } => Action::Status { pr: *pr },
                ReviewCmd::Threads { pr, open } => Action::Threads {
                    pr: *pr,
                    open: *open,
                },
                ReviewCmd::Comment {
                    reference,
                    text,
                    pr,
                    commit,
                } => Action::Comment {
                    reference: reference.clone(),
                    text: text.clone(),
                    pr: *pr,
                    commit: commit.clone(),
                },
                ReviewCmd::Reply { thread, text } => Action::Reply {
                    thread: thread.clone(),
                    text: text.clone(),
                },
                ReviewCmd::Resolve { thread } => Action::Resolve {
                    thread: thread.clone(),
                },
                ReviewCmd::Reopen { thread } => Action::Reopen {
                    thread: thread.clone(),
                },
                ReviewCmd::Update { pr } => Action::Update { pr: *pr },
                ReviewCmd::Approvals { pr } => Action::Approvals { pr: *pr },
            };
            ryuplm::review::run(&here, action)
        }
        Command::Document(DocumentCmd::Sync {
            documents,
            all,
            dry_run,
        }) => {
            let root = find_root(&here)?;
            let mut sent = Vec::new();
            let out = {
                let mut send_one = |r: Request| send(cli, &root, r, &mut sent);
                let open = || ryuplm::docstore::DocStore::open(&root);
                ryuplm::docstore::sync_all(
                    &mut send_one,
                    &open,
                    documents,
                    *all,
                    *dry_run,
                    warnings,
                )
            };
            warnings.extend(sent);
            out
        }
        Command::Document(DocumentCmd::Add {
            drive: Some(id), ..
        }) => {
            let Some((root, req)) = request(&cli.command, &here)? else {
                return Err(Error::new(Code::Internal, "document add is a request"));
            };
            let mut sent = Vec::new();
            let out = {
                let mut send_one = |r: Request| send(cli, &root, r, &mut sent);
                let open = || ryuplm::docstore::DocStore::open(&root);
                ryuplm::docstore::drive_add(&mut send_one, &open, id, req, warnings)
            };
            warnings.extend(sent);
            out
        }
        Command::Document(DocumentCmd::Upload { documents }) => {
            let root = find_root(&here)?;
            let mut sent = Vec::new();
            let out = {
                let mut send_one = |r: Request| send(cli, &root, r, &mut sent);
                let open = || ryuplm::docstore::DocStore::open(&root);
                ryuplm::docstore::upload_all(&mut send_one, &open, documents)
            };
            warnings.extend(sent);
            out
        }
        Command::Number(cmd) => {
            let root = find_root(&here)?;
            let ask = match cmd {
                NumberCmd::Next {
                    scheme,
                    class,
                    count,
                    days,
                } => ryuplm::ledger::Ask::Next {
                    scheme: scheme.clone(),
                    class: class.clone(),
                    count: *count,
                    days: *days,
                },
                NumberCmd::Reserve { numbers, days } => ryuplm::ledger::Ask::Reserve {
                    numbers: numbers.clone(),
                    days: *days,
                },
                NumberCmd::Who { number } => ryuplm::ledger::Ask::Who {
                    number: number.clone(),
                },
                NumberCmd::List { mine, all, after } => ryuplm::ledger::Ask::List {
                    mine: *mine,
                    all: *all,
                    after: after.clone(),
                },
            };
            let mut sent = Vec::new();
            let out = {
                let mut send_one = |r: Request| send(cli, &root, r, &mut sent);
                let open = || ryuplm::ledger::Ledger::open(&root);
                ryuplm::ledger::number(&mut send_one, &open, ask)
            };
            warnings.extend(sent);
            out
        }
        cmd => {
            let Some((root, req)) = request(cmd, &here)? else {
                return Err(Error::new(Code::Internal, "a command that is no request"));
            };
            // The ledger is asked here, in the CLI's process, never by
            // the daemon (design 936); what the daemon gets is an
            // ordinary request carrying the numbers.
            let mut sent = Vec::new();
            let out = {
                let mut send_one = |r: Request| send(cli, &root, r, &mut sent);
                let open = || ryuplm::ledger::Ledger::open(&root);
                match &req {
                    Request::ItemCreate { .. } | Request::ItemRenumber { .. } => {
                        ryuplm::ledger::numbered(&mut send_one, &open, req, warnings)
                    }
                    Request::ImportRun { .. } => {
                        ryuplm::ledger::import_run(&mut send_one, &open, req, warnings)
                    }
                    // Service-stored bytes move here, in the CLI's process,
                    // never in the daemon (ryuplm 3.B).
                    Request::DocumentAdd { .. }
                    | Request::DocumentReplace { .. }
                    | Request::DocumentMove { .. }
                    | Request::DocumentGet { .. } => {
                        let open = || ryuplm::docstore::DocStore::open(&root);
                        ryuplm::docstore::around(&mut send_one, &open, req, warnings)
                    }
                    _ => send_one(req),
                }
            };
            sent.dedup();
            warnings.extend(sent);
            out
        }
    }
}

/// Whether the arguments ask only for the version the way most tools take
/// it (`--version`, `-V`), beside `--pretty`: answered as `ryuplm version`.
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
            // `ryuplm` alone, like --help, is a person asking: but a command
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
    // Git reads a textconv's output as the file's text, not JSON.
    if let Command::Git(GitCmd::Textconv { path }) = &cli.command {
        return match std::fs::read(path) {
            Ok(bytes) => {
                use std::io::Write;
                let text = ryuplm::textconv::textconv(&bytes);
                match std::io::stdout().lock().write_all(&text) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(_) => ExitCode::from(Code::Io.exit_code()),
                }
            }
            Err(e) => finish(Err(Error::io(path, &e)), Vec::new(), cli.pretty),
        };
    }
    // Git calls the merge driver: its answer is the file and the exit code.
    if let Command::Git(GitCmd::MergeDriver {
        base,
        ours,
        theirs,
        path,
    }) = &cli.command
    {
        return merge_driver(base, ours, theirs, path.as_deref());
    }
    // Git's hooks read the exit code; people read what it prints.
    if let Command::Merge(MergeCmd::Status { hook: true }) = &cli.command {
        let here = match &cli.dir {
            Some(d) => d.clone(),
            None => std::env::current_dir().unwrap_or_default(),
        };
        return hook_check(&here);
    }
    let mut warnings = Vec::new();
    let outcome = run(&cli, &mut warnings);
    finish(outcome, warnings, cli.pretty)
}

/// `ryuplm git merge-driver`: merges, writes over `ours`, and exits 0 when
/// nothing is left to resolve, 1 when conflicts are (in the file's record,
/// or as Git's markers when it fell back to merging lines), 4 when a file
/// can't be read or written. Failures go to stderr: Git shows them.
fn merge_driver(base: &Path, ours: &Path, theirs: &Path, path: Option<&str>) -> ExitCode {
    let read = |p: &Path| std::fs::read(p).map_err(|e| Error::io(p, &e));
    let merged = (|| -> Result<ryuplm::merge::Outcome> {
        let (out, outcome) = ryuplm::merge::bytes(&read(base)?, &read(ours)?, &read(theirs)?);
        std::fs::write(ours, out).map_err(|e| Error::io(ours, &e))?;
        Ok(outcome)
    })();
    match merged {
        Ok(ryuplm::merge::Outcome::Clean) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(e) => {
            eprintln!(
                "ryuplm: merging {}: {}",
                path.unwrap_or(&ours.to_string_lossy()),
                e.message
            );
            ExitCode::from(Code::Io.exit_code())
        }
    }
}

/// `ryuplm merge status --hook`: the commit guard, then every catalogue a
/// merge in progress concerns checked for files the merge left
/// unresolved. Exits 0 when nothing is, 1 otherwise, saying what to
/// settle on stderr.
fn hook_check(here: &Path) -> ExitCode {
    let catalogues = match ryu_git::merging_projects::<ryuplm::Plm>(here) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ryuplm: checking the merge: {}", e.message);
            return ExitCode::from(1);
        }
    };
    let mut failed = false;
    // The commit guard (design 629): a commit that would leave a proposal
    // stale or change an official record, merge or not.
    match ryupdm::release::guard_for::<ryuplm::Plm>(here) {
        Ok(found) => {
            for (root, list) in found {
                failed = true;
                eprintln!(
                    "ryuplm: this commit would leave {} release problem(s) in {}:",
                    list.len(),
                    root.display()
                );
                for f in &list {
                    eprintln!("  {}: {}", f.code, f.message);
                }
            }
        }
        Err(e) => {
            failed = true;
            eprintln!("ryuplm: checking the release records staged: {}", e.message);
        }
    }
    for root in catalogues {
        let left = ryuplm::merge::unresolved(&root);
        if !left.is_empty() {
            failed = true;
            eprintln!(
                "ryuplm: the merge leaves {} file(s) unresolved in {}:",
                left.len(),
                root.display()
            );
            for p in &left {
                eprintln!("  {p}");
            }
        }
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn finish(outcome: Result<Value>, warnings: Vec<String>, pretty: bool) -> ExitCode {
    let (doc, code) = match outcome {
        Ok(result) => (
            json!({"ok": true, "result": result, "warnings": warnings}),
            0,
        ),
        Err(e) => {
            ryuplm::error::details::check(&e);
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
