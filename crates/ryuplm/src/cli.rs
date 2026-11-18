//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code, as ryucad's, ryueda's and ryudfm's are.
//! `git textconv` and `git merge-driver` are Git's, not an agent's: they
//! write what Git reads.

use clap::{Args, Parser, Subcommand};
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
    /// Make this folder a catalogue.
    Init {
        #[arg(long, value_parser = unit)]
        unit: Option<Unit>,
    },
    /// The catalogue: its project ID, classes, item count and revision.
    Info,
    #[command(subcommand)]
    Item(ItemCmd),
    #[command(subcommand)]
    Settings(SettingsCmd),
    /// Every finding: file faults, AML, classes, properties, numbers, releases.
    Check {
        /// Also compare numbers with the catalogues under this folder.
        #[arg(long = "in", value_name = "DIR")]
        within: Vec<String>,
    },
    #[command(subcommand)]
    Release(ReleaseCmd),
    /// An item's BOM: its own lines plus its representations' (show),
    /// its own lines edited (add, edit, remove), and what changed (diff).
    #[command(subcommand)]
    Bom(BomCmd),
    /// Items added, removed and changed between two revisions.
    Diff {
        from: Option<String>,
        to: Option<String>,
    },
    #[command(subcommand)]
    Git(GitCmd),
    #[command(subcommand)]
    Merge(MergeCmd),
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
        /// Take the next number of SCHEME (or of the one that applies).
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
    List {
        #[arg(long)]
        number: Option<String>,
        #[arg(long)]
        class: Option<String>,
        #[arg(long)]
        text: Option<String>,
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
        #[arg(long)]
        next_template: Option<String>,
        #[arg(long)]
        property: Option<String>,
        /// The property's declaration, as JSON.
        #[arg(long)]
        declaration: Option<String>,
        #[arg(long)]
        remove: bool,
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

fn unit(s: &str) -> core::result::Result<Unit, String> {
    Unit::parse(s).ok_or_else(|| format!("unknown unit {s:?}: m, cm, mm, um, in or ft"))
}

fn request(cmd: &Command, here: &Path) -> Result<Option<(PathBuf, Request)>> {
    let root = || find_root(here);
    let exp = |e: &Expect| e.expect.clone();
    Ok(Some(match cmd {
        Command::Info => (root()?, Request::ProjectInfo {}),
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
            limit,
        }) => (
            root()?,
            Request::ItemList {
                number: number.clone(),
                class: class.clone(),
                text: text.clone(),
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
            next_template,
            property,
            declaration,
            remove,
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
                        next_template: next_template.clone(),
                        property: property.clone(),
                        declaration,
                        remove: *remove,
                    },
                    expected_revision: exp(expect),
                },
            )
        }
        Command::Check { within } => (
            root()?,
            Request::Check {
                within: within.clone(),
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
        // Version, Init, Watch, Daemon, Git and Merge are not requests.
        Command::Version
        | Command::Init { .. }
        | Command::Watch { .. }
        | Command::Daemon(_)
        | Command::Git(_)
        | Command::Merge(_) => return Ok(None),
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

/// Runs the command, collecting warnings for the output.
fn run(cli: &Cli, warnings: &mut Vec<String>) -> Result<Value> {
    let here = match &cli.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::io(Path::new("."), &e))?,
    };
    match &cli.command {
        Command::Version => Ok(json!(VersionInfo::current())),
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
        Command::Git(GitCmd::Textconv { .. } | GitCmd::MergeDriver { .. })
        | Command::Merge(MergeCmd::Status { hook: true }) => unreachable!("answered in main"),
        Command::Merge(MergeCmd::Status { hook: false }) => {
            let roots = ryu_git::merging_projects::<ryuplm::Plm>(&here)?;
            Ok(json!({"merging": roots
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
        cmd => {
            let Some((root, req)) = request(cmd, &here)? else {
                return Err(Error::new(Code::Internal, "a command that is no request"));
            };
            if !no_daemon(cli) {
                match client::request(&root, &req, participant(cli).as_deref()) {
                    Ok(answer) => return answer.map(|a| a.result),
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
