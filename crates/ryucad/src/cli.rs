//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code.

use clap::{Args, Parser, Subcommand};
use ryucad::doc::project::find_root;
use ryucad::error::{Code, Error, Result};
use ryucad::proto::{PlacementArg, Position, Request, VersionInfo};
use ryucad::units::Unit;
use ryucad::{client, daemon, server};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ryucad",
    about = "Agent-driven CAD",
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
    /// RYUCAD_NO_DAEMON=1).
    #[arg(long, global = true)]
    no_daemon: bool,
    #[command(subcommand)]
    command: Command,
}

// Parsed once per run; the size difference costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// Create a project in this folder.
    Init {
        /// The unit for bare numbers.
        #[arg(long, value_parser = unit)]
        unit: Option<Unit>,
    },
    /// Create, delete or list parts.
    #[command(subcommand)]
    Part(PartCmd),
    /// Add, edit, delete, move, list or show features.
    #[command(subcommand)]
    Feature(FeatureCmd),
    /// Rebuild one part, or all of them.
    Rebuild { part: Option<String> },
    /// List a part's bodies, faces, edges and vertices by stable name.
    Inspect {
        part: String,
        feature: Option<String>,
        /// Only the entity with this name.
        #[arg(long)]
        entity: Option<String>,
    },
    /// Report files that aren't canonical, broken, unlisted or unknown.
    CheckFormat,
    /// Run, query or stop the project's session daemon.
    #[command(subcommand)]
    Daemon(DaemonCmd),
    /// Print the ryucad, ryukei and protocol versions.
    Version,
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Serve the project in the foreground (the CLI starts this itself).
    Run {
        /// Exit after this many seconds without a request.
        #[arg(long, value_name = "SECONDS")]
        idle_timeout: Option<u64>,
    },
    /// Whether the project's daemon is running, and its details.
    Status,
    /// Stop the project's daemon, or with --all every daemon of this user.
    Stop {
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum PartCmd {
    Create {
        part: String,
    },
    Delete {
        part: String,
        #[command(flatten)]
        expect: Expect,
    },
    List,
}

#[derive(Subcommand)]
enum FeatureCmd {
    /// Add a feature: box (x, y, z), cylinder (radius, height), cone
    /// (radius, height, top_radius), sphere (radius) or torus
    /// (major_radius, minor_radius).
    Add {
        part: String,
        #[arg(value_name = "TYPE")]
        ty: String,
        /// The feature's ID; generated as <type>-<n> when left out.
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        position: PositionArgs,
        #[command(flatten)]
        params: ParamArgs,
        #[command(flatten)]
        placement: PlacementArgs,
        #[command(flatten)]
        expect: Expect,
    },
    /// Change some of a feature's parameters or placement.
    Edit {
        part: String,
        id: String,
        #[command(flatten)]
        params: ParamArgs,
        /// Remove an optional parameter.
        #[arg(long, value_name = "NAME")]
        unset: Vec<String>,
        #[command(flatten)]
        placement: PlacementArgs,
        #[command(flatten)]
        expect: Expect,
    },
    Delete {
        part: String,
        id: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Move a feature after another, or to the front.
    Move {
        part: String,
        id: String,
        #[command(flatten)]
        position: PositionArgs,
        #[command(flatten)]
        expect: Expect,
    },
    List {
        part: String,
    },
    Show {
        part: String,
        id: String,
    },
}

#[derive(Args)]
struct Expect {
    /// Fail with `conflict` unless the part is still at this revision.
    #[arg(long = "expect", value_name = "REVISION")]
    revision: Option<String>,
}

#[derive(Args)]
struct PositionArgs {
    /// Put the feature after this one.
    #[arg(long, value_name = "ID", conflicts_with = "first")]
    after: Option<String>,
    /// Put the feature first.
    #[arg(long)]
    first: bool,
}

#[derive(Args)]
struct ParamArgs {
    /// A parameter, such as radius=12.5 or "height=2 in".
    #[arg(long = "param", value_name = "NAME=QUANTITY", value_parser = param)]
    params: Vec<(String, String)>,
}

#[derive(Args)]
struct PlacementArgs {
    /// The local origin, three quantities: 0,0,10mm.
    #[arg(long, value_name = "X,Y,Z", value_parser = triple)]
    origin: Option<[String; 3]>,
    /// The local z axis: 0,0,1.
    #[arg(long, value_name = "X,Y,Z", value_parser = vector)]
    z: Option<[f64; 3]>,
    /// The local x direction: 1,0,0.
    #[arg(long, value_name = "X,Y,Z", value_parser = vector)]
    x: Option<[f64; 3]>,
}

fn unit(s: &str) -> core::result::Result<Unit, String> {
    Unit::parse(s).ok_or_else(|| "use m, cm, mm, um, in or ft".to_owned())
}

fn param(s: &str) -> core::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .ok_or_else(|| "expected NAME=QUANTITY".to_owned())
}

fn triple(s: &str) -> core::result::Result<[String; 3], String> {
    let parts: Vec<String> = s.split(',').map(|p| p.trim().to_owned()).collect();
    parts
        .try_into()
        .map_err(|_| "expected three values: X,Y,Z".to_owned())
}

fn vector(s: &str) -> core::result::Result<[f64; 3], String> {
    let t = triple(s)?;
    let n = |x: &str| {
        x.parse::<f64>()
            .map_err(|_| format!("{x:?} is not a number"))
    };
    Ok([n(&t[0])?, n(&t[1])?, n(&t[2])?])
}

impl ParamArgs {
    fn to_map(&self) -> Map<String, Value> {
        self.params
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect()
    }
}

impl PlacementArgs {
    fn to_arg(&self) -> Option<PlacementArg> {
        let arg = PlacementArg {
            origin: self.origin.clone().map(|o| o.map(Value::String)),
            z: self.z,
            x: self.x,
        };
        (arg != PlacementArg::default()).then_some(arg)
    }
}

impl PositionArgs {
    fn to_position(&self) -> Position {
        Position {
            after: self.after.clone(),
            first: self.first,
        }
    }
}

/// The session request for a command; `None` for commands that need no
/// project.
fn request(cmd: &Command) -> Option<Request> {
    Some(match cmd {
        Command::Init { .. } | Command::Version | Command::Daemon(_) => return None,
        Command::Part(PartCmd::Create { part }) => Request::PartCreate { part: part.clone() },
        Command::Part(PartCmd::Delete { part, expect }) => Request::PartDelete {
            part: part.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Part(PartCmd::List) => Request::PartList {},
        Command::Feature(FeatureCmd::Add {
            part,
            ty,
            id,
            position,
            params,
            placement,
            expect,
        }) => Request::FeatureAdd {
            part: part.clone(),
            ty: ty.clone(),
            params: params.to_map(),
            placement: placement.to_arg(),
            id: id.clone(),
            position: position.to_position(),
            expected_revision: expect.revision.clone(),
        },
        Command::Feature(FeatureCmd::Edit {
            part,
            id,
            params,
            unset,
            placement,
            expect,
        }) => {
            let mut params = params.to_map();
            params.extend(unset.iter().map(|k| (k.clone(), Value::Null)));
            Request::FeatureEdit {
                part: part.clone(),
                id: id.clone(),
                params,
                placement: placement.to_arg(),
                expected_revision: expect.revision.clone(),
            }
        }
        Command::Feature(FeatureCmd::Delete { part, id, expect }) => Request::FeatureDelete {
            part: part.clone(),
            id: id.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Feature(FeatureCmd::Move {
            part,
            id,
            position,
            expect,
        }) => Request::FeatureMove {
            part: part.clone(),
            id: id.clone(),
            position: position.to_position(),
            expected_revision: expect.revision.clone(),
        },
        Command::Feature(FeatureCmd::List { part }) => Request::FeatureList { part: part.clone() },
        Command::Feature(FeatureCmd::Show { part, id }) => Request::FeatureShow {
            part: part.clone(),
            id: id.clone(),
        },
        Command::Rebuild { part } => Request::Rebuild { part: part.clone() },
        Command::Inspect {
            part,
            feature,
            entity,
        } => Request::Inspect {
            part: part.clone(),
            feature: feature.clone(),
            entity: entity.clone(),
        },
        Command::CheckFormat => Request::CheckFormat {},
    })
}

/// Runs the command, collecting warnings for the output.
fn run(cli: &Cli, warnings: &mut Vec<String>) -> Result<Value> {
    let here = match &cli.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::io(std::path::Path::new("."), &e))?,
    };
    match &cli.command {
        Command::Version => Ok(json!(VersionInfo::current())),
        Command::Init { unit } => server::init(&here, *unit),
        Command::Daemon(DaemonCmd::Run { idle_timeout }) => {
            let idle = idle_timeout.map_or(daemon::DEFAULT_IDLE, Duration::from_secs);
            daemon::run(&find_root(&here)?, idle).map(|()| Value::Null)
        }
        Command::Daemon(DaemonCmd::Status) => client::status(&find_root(&here)?),
        Command::Daemon(DaemonCmd::Stop { all: true }) => client::stop_all(),
        Command::Daemon(DaemonCmd::Stop { all: false }) => client::stop(&find_root(&here)?),
        cmd => {
            let req = request(cmd).expect("every other command is a request");
            let root = find_root(&here)?;
            let no_daemon =
                cli.no_daemon || std::env::var("RYUCAD_NO_DAEMON").is_ok_and(|v| v == "1");
            if !no_daemon {
                match client::request(&root, &req) {
                    Ok(answer) => return answer,
                    // Never delivered, so running it here is safe.
                    Err(e) => warnings.push(format!("{}; ran in process", e.message)),
                }
            }
            server::handle_safely(&mut None, &root, req)
        }
    }
}

pub fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind;
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion)
                || e.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            {
                // Help is for people; print it as clap does.
                let _ = e.print();
                return ExitCode::SUCCESS;
            }
            let err = Error::new(Code::Usage, e.render().to_string().trim().to_owned());
            return finish(
                Err(err),
                Vec::new(),
                std::env::args().any(|a| a == "--pretty"),
            );
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
    println!("{}", s.expect("JSON values always serialize"));
    ExitCode::from(code)
}
