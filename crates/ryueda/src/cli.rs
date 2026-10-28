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

fn unit(s: &str) -> core::result::Result<Unit, String> {
    Unit::parse(s).ok_or_else(|| format!("unknown unit {s:?}: m, cm, mm, um, in or ft"))
}

/// The request a command sends, for those that are requests.
fn request(cmd: &Command) -> Option<Request> {
    Some(match cmd {
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
        Command::Version | Command::Init { .. } | Command::Watch { .. } | Command::Daemon(_) => {
            return None;
        }
    })
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
            let req = request(cmd).expect("every other command is a request");
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
