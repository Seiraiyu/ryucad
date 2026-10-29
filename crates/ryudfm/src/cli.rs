//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code, as ryucad's and ryueda's are.
//!
//! `inspect`, `render` and `check` take a board of the project they run in
//! or, in path mode, a job's path: a name that isn't one of the project's
//! boards (or any name outside a project) is read in this process,
//! read-only, with no daemon.

use clap::{Args, Parser, Subcommand, ValueEnum};
use ryudfm::error::{Code, Error, Result};
use ryudfm::id::Id;
use ryudfm::project::{BOARD_FILE, BOARDS, find_root};
use ryudfm::proto::{Answer, Request, VersionInfo};
use ryudfm::units::Unit;
use ryudfm::{client, daemon, server};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ryudfm",
    about = "Design for manufacturing of printed circuit boards, for agents",
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
    /// RYUDFM_NO_DAEMON=1).
    #[arg(long, global = true)]
    no_daemon: bool,
    /// Act as the agent participant with this name (also
    /// RYUDFM_PARTICIPANT); `agent` by default.
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

#[derive(Clone, Copy, ValueEnum)]
enum SideArg {
    Top,
    Bottom,
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Png,
    Svg,
}

#[derive(Subcommand)]
enum Command {
    /// This build's version.
    Version,
    /// Create a project in this folder: ryudfm.json and .gitattributes.
    Init {
        /// The unit of lengths typed as bare numbers (mm by default).
        #[arg(long, value_parser = unit)]
        unit: Option<Unit>,
    },
    /// The project, its boards and their revisions.
    Info,
    /// Read an ODB++ job (a folder, .tgz, .tar or .zip) and commit it as
    /// a board, byte for byte; with --update, the next revision of a
    /// board.
    Import {
        path: PathBuf,
        /// The new board's ID (from the file's name by default).
        #[arg(long)]
        board: Option<String>,
        /// The board step, when the job's steps leave it open.
        #[arg(long)]
        step: Option<String>,
        /// The board this job is the next revision of.
        #[arg(long, value_name = "BOARD")]
        update: Option<String>,
        /// Read and check the job, write nothing.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// List, show, edit or delete boards.
    #[command(subcommand)]
    Board(BoardCmd),
    /// Summarise a board, or list a layer's features, its components or
    /// its nets a page at a time, or show one feature.
    Inspect {
        /// A board of the project, or a job's path.
        board: String,
        #[arg(long)]
        step: Option<String>,
        #[arg(long)]
        layer: Option<String>,
        /// A feature's reference, as inspect gives it.
        #[arg(long)]
        feature: Option<String>,
        #[arg(long)]
        components: bool,
        #[arg(long)]
        nets: bool,
        #[arg(long)]
        limit: Option<usize>,
        /// The page after this reference.
        #[arg(long)]
        after: Option<String>,
    },
    /// Draw layers of a board as PNG or SVG.
    Render {
        /// A board of the project, or a job's path.
        board: String,
        #[arg(long)]
        out: PathBuf,
        /// The layers, bottom first (comma separated or repeated); the
        /// viewed side's copper, mask, silk and the drills by default.
        #[arg(long, value_delimiter = ',')]
        layers: Vec<String>,
        #[arg(long, value_enum)]
        side: Option<SideArg>,
        /// png or svg; from --out's extension by default.
        #[arg(long, value_enum)]
        format: Option<FormatArg>,
        /// The long side in pixels, or WxH (1600 by default).
        #[arg(long)]
        size: Option<String>,
        /// X0,Y0,X1,Y1: lengths, bare numbers in the default unit.
        #[arg(long)]
        window: Option<String>,
        #[arg(long)]
        step: Option<String>,
    },
    /// Load the project (or a board, or a job's path) from nothing and
    /// report every load fault; fails (check_failed) on any error.
    Check {
        board: Option<String>,
        /// The board step of a job read by path.
        #[arg(long)]
        step: Option<String>,
    },
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
enum BoardCmd {
    List,
    Show {
        board: String,
    },
    /// Rename a board, or choose another of its job's steps as the board.
    Edit {
        board: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        step: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete a board and its input.
    Delete {
        board: String,
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
        /// Every ryudfm daemon of this user.
        #[arg(long)]
        all: bool,
    },
}

fn unit(s: &str) -> core::result::Result<Unit, String> {
    Unit::parse(s).ok_or_else(|| format!("unknown unit {s:?}: m, cm, mm, um, in or ft"))
}

/// `p` as an absolute path, relative to `here` (where the CLI runs as if
/// started), for the daemon, which has no folder of the caller's.
fn absolute(here: &Path, p: &Path) -> String {
    let joined = here.join(p);
    let p = joined.canonicalize().unwrap_or(joined);
    ryudfm::source::plain(p).to_string_lossy().into_owned()
}

/// What a `<board or path>` argument names.
enum Target {
    /// A board of the project at the root.
    Board(PathBuf, String),
    /// A job read in this process.
    Path(PathBuf),
}

/// A board ID of the project `here` is in wins; else a path that exists.
fn target(here: &Path, text: &str) -> Result<Target> {
    let root = find_root(here).ok();
    if let Some(root) = &root
        && Id::parse(text, "board").is_ok()
        && root.join(BOARDS).join(text).join(BOARD_FILE).is_file()
    {
        return Ok(Target::Board(root.clone(), text.to_owned()));
    }
    let p = here.join(text);
    if p.exists() {
        return Ok(Target::Path(p));
    }
    let mut e = Error::new(
        Code::NotFound,
        match &root {
            Some(_) => format!("no board {text} in the project, and no file or folder there"),
            None => format!("no file or folder {text}, and no project here"),
        },
    )
    .with("board", text);
    if let Some(r) = root {
        e = e.with("root", r);
    }
    Err(e)
}

/// A board a write names: never a path.
fn board_arg(here: &Path, text: &str) -> Result<String> {
    if Id::parse(text, "board").is_err() && here.join(text).exists() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{text} is a path; writes name a board of the project by its ID"),
        )
        .with("board", text));
    }
    Ok(text.to_owned())
}

fn inspect_args(cmd: &Command) -> ryudfm::inspect::Args {
    let Command::Inspect {
        step,
        layer,
        feature,
        components,
        nets,
        limit,
        after,
        ..
    } = cmd
    else {
        unreachable!("inspect only")
    };
    ryudfm::inspect::Args {
        step: step.clone(),
        layer: layer.clone(),
        feature: feature.clone(),
        components: *components,
        nets: *nets,
        limit: *limit,
        after: after.clone(),
    }
}

fn render_args(cmd: &Command) -> ryudfm::render::Args {
    let Command::Render {
        out,
        layers,
        side,
        format,
        size,
        window,
        step,
        ..
    } = cmd
    else {
        unreachable!("render only")
    };
    let svg = out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    ryudfm::render::Args {
        step: step.clone(),
        layers: layers.clone(),
        side: side.map(|s| match s {
            SideArg::Top => ryudfm::render::Side::Top,
            SideArg::Bottom => ryudfm::render::Side::Bottom,
        }),
        format: match format {
            Some(FormatArg::Svg) => Some(ryudfm::render::Format::Svg),
            Some(FormatArg::Png) => Some(ryudfm::render::Format::Png),
            None if svg => Some(ryudfm::render::Format::Svg),
            None => None,
        },
        size: size.clone(),
        window: window.clone(),
    }
}

/// The request a command sends to the project, for those that are
/// requests, with the project's root.
fn request(cmd: &Command, here: &Path) -> Result<Option<(PathBuf, Request)>> {
    let root = || find_root(here);
    Ok(Some(match cmd {
        Command::Info => (root()?, Request::ProjectInfo {}),
        Command::Import {
            path,
            board,
            step,
            update,
            dry_run,
            expect,
        } => (
            root()?,
            Request::Import {
                path: absolute(here, path),
                board: board.clone(),
                step: step.clone(),
                update: update.as_deref().map(|b| board_arg(here, b)).transpose()?,
                dry_run: *dry_run,
                expected_revision: expect.expect.clone(),
            },
        ),
        Command::Board(BoardCmd::List) => (root()?, Request::BoardList {}),
        Command::Board(BoardCmd::Show { board }) => (
            root()?,
            Request::BoardShow {
                board: board.clone(),
            },
        ),
        Command::Board(BoardCmd::Edit {
            board,
            name,
            step,
            expect,
        }) => (
            root()?,
            Request::BoardEdit {
                board: board_arg(here, board)?,
                name: name.clone(),
                step: step.clone(),
                expected_revision: expect.expect.clone(),
            },
        ),
        Command::Board(BoardCmd::Delete { board, expect }) => (
            root()?,
            Request::BoardDelete {
                board: board_arg(here, board)?,
                expected_revision: expect.expect.clone(),
            },
        ),
        Command::Inspect { board, .. } => match target(here, board)? {
            Target::Board(root, id) => (
                root,
                Request::Inspect {
                    board: id,
                    args: inspect_args(cmd),
                },
            ),
            Target::Path(_) => return Ok(None),
        },
        Command::Render { board, .. } => match target(here, board)? {
            Target::Board(root, id) => (
                root,
                Request::Render {
                    board: id,
                    args: render_args(cmd),
                },
            ),
            Target::Path(_) => return Ok(None),
        },
        Command::Check { board: None, .. } => (root()?, Request::Check { board: None }),
        Command::Check { board: Some(b), .. } => match target(here, b)? {
            Target::Board(root, id) => (root, Request::Check { board: Some(id) }),
            Target::Path(_) => return Ok(None),
        },
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
        Command::Version | Command::Init { .. } | Command::Watch { .. } | Command::Daemon(_) => {
            return Ok(None);
        }
    }))
}

/// Path mode: `inspect`, `render` and `check` of a job read here.
fn by_path(cmd: &Command, here: &Path) -> Result<Answer> {
    let read = |path: &Path, step: &Option<String>| {
        let options = ryudfm::odb::ReadOptions {
            step: step.clone(),
            ..Default::default()
        };
        ryudfm::odb::read_path(path, &options)
            .map_err(|e| ryudfm::codes::error(&e).with("path", path))
    };
    let at = |v: &mut Value, p: &Path| v["path"] = json!(p);
    match cmd {
        Command::Inspect { board, step, .. } => {
            let Target::Path(p) = target(here, board)? else {
                unreachable!("a path")
            };
            let b = read(&p, step)?;
            let mut v = ryudfm::inspect::inspect(&b, None, &inspect_args(cmd))?;
            at(&mut v, &p);
            Ok(Answer::json(v))
        }
        Command::Render { board, step, .. } => {
            let Target::Path(p) = target(here, board)? else {
                unreachable!("a path")
            };
            let b = read(&p, step)?;
            let mut a = ryudfm::render::render(&b, None, &render_args(cmd), Unit::Mm)?;
            at(&mut a.result, &p);
            Ok(a)
        }
        Command::Check {
            board: Some(b),
            step,
        } => {
            let Target::Path(p) = target(here, b)? else {
                unreachable!("a path")
            };
            ryudfm::check::path(&p, step.as_deref()).map(Answer::json)
        }
        _ => unreachable!("only inspect, render and check read paths"),
    }
}

fn no_daemon(cli: &Cli) -> bool {
    cli.no_daemon || std::env::var("RYUDFM_NO_DAEMON").is_ok_and(|v| v == "1")
}

/// The agent participant to act as.
fn participant(cli: &Cli) -> Option<String> {
    cli.as_name
        .clone()
        .or_else(|| std::env::var("RYUDFM_PARTICIPANT").ok())
}

/// The result to print; bytes the answer carries go to `--out` (relative
/// to where the CLI runs as if started), atomically.
fn deliver(cmd: &Command, here: &Path, a: Answer) -> Result<Value> {
    let Some(bytes) = a.binary else {
        return Ok(a.result);
    };
    let Command::Render { out, .. } = cmd else {
        return Err(Error::new(
            Code::Internal,
            "bytes with nowhere to write them",
        ));
    };
    ryudfm::source::write_atomic(&here.join(out), &bytes)?;
    let mut result = a.result;
    result["out"] = json!(out);
    Ok(result)
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
                ryudfm::source::plain(here.canonicalize().map_err(|e| Error::io(&here, &e))?);
            server::init(&root, *unit)
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
            let Some((root, req)) = request(cmd, &here)? else {
                return deliver(cmd, &here, by_path(cmd, &here)?);
            };
            if !no_daemon(cli) {
                match client::request(&root, &req, participant(cli).as_deref()) {
                    Ok(answer) => return deliver(cmd, &here, answer?),
                    // Never delivered, so running it here is safe.
                    Err(e) => warnings.push(format!("{}; ran in process", e.message)),
                }
            }
            let me = participant(cli);
            let a = server::handle_safely(&mut None, None, me.as_deref(), &root, req)?;
            deliver(cmd, &here, a)
        }
    }
}

/// Whether the arguments ask only for the version the way most tools take
/// it (`--version`, `-V`), beside `--pretty`: answered as `ryudfm version`.
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
            // `ryudfm` alone, like --help, is a person asking: but a command
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
