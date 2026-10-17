//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code.

use clap::{Args, Parser, Subcommand};
use ryucad::doc::project::{find_project, find_root};
use ryucad::error::{Code, Error, Result};
use ryucad::live::{SelectMode, Style};
use ryucad::proto::{Answer, PlacementArg, Position, Request, Transfer, VersionInfo};
use ryucad::units::Unit;
use ryucad::view::{CameraArg, ProjectionArg};
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
    /// Act as the agent participant with this name (also
    /// RYUCAD_PARTICIPANT); `agent` by default.
    #[arg(long = "as", global = true, value_name = "NAME")]
    as_name: Option<String>,
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
    /// Diagnose the project folder: a lost manifest, unlisted or missing
    /// feature files, conflict markers, sync tools' copies and more. With
    /// --repair, apply repairs it offered.
    Doctor {
        /// A repair ID from a doctor report.
        #[arg(long = "repair", value_name = "ID")]
        repairs: Vec<String>,
        /// A repair parameter: unit=in, order=a,b,c.
        #[arg(long = "param", value_name = "NAME=VALUE", value_parser = param, requires = "repairs")]
        params: Vec<(String, String)>,
    },
    /// List the humans and agents attached to the project's session.
    Participants,
    /// Set this agent's selection (entity references
    /// <part>/<feature>/<name>), or add to it, remove from it, or clear it.
    Select {
        #[arg(value_name = "REF")]
        entities: Vec<String>,
        #[arg(long, conflicts_with_all = ["remove", "clear"])]
        add: bool,
        #[arg(long, conflicts_with = "clear")]
        remove: bool,
        #[arg(long, conflicts_with = "entities")]
        clear: bool,
    },
    /// What a participant has selected: the one human, unless --of names
    /// another.
    Selection {
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
    /// Show a participant's camera (the one human's, unless --of names
    /// another), or set this agent's with --eye, --target and --up.
    Camera {
        #[arg(long = "of", value_name = "PARTICIPANT", conflicts_with_all = ["eye", "view", "fit"])]
        of: Option<String>,
        /// Look from a standard view: front, back, left, right, top,
        /// bottom or iso, framing the model (or the --fit entities).
        #[arg(long, conflicts_with = "eye")]
        view: Option<String>,
        /// Frame these entities, keeping the view direction.
        #[arg(long, value_name = "REF", conflicts_with = "eye")]
        fit: Vec<String>,
        /// The eye point, three quantities: 0,-100mm,50mm.
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, requires_all = ["target", "up"])]
        eye: Option<[String; 3]>,
        /// The point looked at.
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, requires = "eye")]
        target: Option<[String; 3]>,
        /// The up direction: 0,0,1.
        #[arg(long, value_name = "X,Y,Z", value_parser = vector, requires = "eye")]
        up: Option<[f64; 3]>,
        /// Perspective with this vertical field of view (45 by default).
        #[arg(long, value_name = "DEGREES", conflicts_with = "ortho")]
        fov: Option<f64>,
        /// Orthographic, this tall at the target.
        #[arg(long, value_name = "QUANTITY")]
        ortho: Option<String>,
    },
    /// Highlight entities for everyone to see, or clear highlights.
    Highlight {
        #[arg(value_name = "REF", required_unless_present = "clear")]
        entities: Vec<String>,
        /// pending (about to change), changed, or note (the default).
        #[arg(long, value_parser = style)]
        style: Option<Style>,
        #[arg(long)]
        label: Option<String>,
        /// Remove this agent's highlights (of --style only, if given).
        #[arg(long, conflicts_with_all = ["entities", "label"])]
        clear: bool,
    },
    /// Wait for notifications from the project's daemon: what humans
    /// select, the model changing, and so on.
    Watch {
        /// model, project, participants or activity (all by default).
        #[arg(long = "topic", value_name = "TOPIC")]
        topics: Vec<String>,
        /// Stop after this many notifications.
        #[arg(long, default_value_t = 1)]
        count: usize,
        /// Stop after this many seconds.
        #[arg(long, value_name = "SECONDS", default_value_t = 30)]
        timeout: u64,
    },
    /// The session's activity log: every change, who made it, and the
    /// files it touched, oldest first.
    Activity {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Only entries older than this sequence number.
        #[arg(long, value_name = "SEQ")]
        before: Option<u64>,
    },
    /// Read or change settings: project settings (in ryucad.json) and
    /// user preferences.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Display meshes of a part's bodies, by stable name.
    Mesh {
        part: String,
        feature: Option<String>,
        /// coarse, normal or fine (the user's display quality by default).
        #[arg(long)]
        quality: Option<String>,
        /// Write the meshes to this file as ryucad.mesh.1 instead.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Draw a part (or the whole project) to a PNG: iso framing the model
    /// unless a view, a participant's camera or a camera is given.
    Render {
        part: Option<String>,
        /// The PNG to write.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// front, back, left, right, top, bottom or iso.
        #[arg(long, conflicts_with_all = ["of", "eye"])]
        view: Option<String>,
        /// Use this participant's camera.
        #[arg(long = "of", value_name = "PARTICIPANT", conflicts_with = "eye")]
        of: Option<String>,
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, requires_all = ["target", "up"])]
        eye: Option<[String; 3]>,
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, requires = "eye")]
        target: Option<[String; 3]>,
        #[arg(long, value_name = "X,Y,Z", value_parser = vector, requires = "eye")]
        up: Option<[f64; 3]>,
        #[arg(
            long,
            value_name = "DEGREES",
            conflicts_with = "ortho",
            requires = "eye"
        )]
        fov: Option<f64>,
        #[arg(long, value_name = "QUANTITY", requires = "eye")]
        ortho: Option<String>,
        /// The image size, WIDTHxHEIGHT.
        #[arg(long, value_name = "WxH", value_parser = size, default_value = "800x600")]
        size: (u32, u32),
        /// Point out an entity.
        #[arg(long = "highlight", value_name = "REF")]
        highlight: Vec<String>,
        /// Label highlighted entities.
        #[arg(long)]
        labels: bool,
        /// Leave out edges and silhouettes.
        #[arg(long)]
        no_edges: bool,
    },
    /// The project's Git commits (or one part's), and what is
    /// uncommitted. Read-only.
    History {
        part: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Undo this agent's latest change, or the activity entry --entry.
    Undo {
        #[arg(long, value_name = "SEQ")]
        entry: Option<u64>,
    },
    /// Redo this agent's latest undo, or the undo entry --entry.
    Redo {
        #[arg(long, value_name = "SEQ")]
        entry: Option<u64>,
    },
    /// Say what this agent is doing; "" clears it.
    Status {
        text: String,
        /// An entity the work is on.
        #[arg(long = "entity", value_name = "REF")]
        entities: Vec<String>,
    },
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
enum ConfigCmd {
    /// One setting, or every setting.
    Get {
        key: Option<String>,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Set a setting. The value is JSON, or else a string: dark, false,
    /// 500, '["/a/project"]'.
    Set {
        key: String,
        value: String,
        #[command(flatten)]
        scope: ScopeArgs,
        /// For project settings: fail with `conflict` unless the project is
        /// still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
}

#[derive(Args)]
struct ScopeArgs {
    /// Only project settings.
    #[arg(long, conflicts_with = "user")]
    project: bool,
    /// Only user preferences.
    #[arg(long)]
    user: bool,
}

impl ScopeArgs {
    fn get(&self) -> Option<String> {
        match (self.project, self.user) {
            (true, _) => Some("project".into()),
            (_, true) => Some("user".into()),
            _ => None,
        }
    }
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

fn style(s: &str) -> core::result::Result<Style, String> {
    serde_json::from_value(json!(s)).map_err(|_| "use pending, changed or note".to_owned())
}

fn size(s: &str) -> core::result::Result<(u32, u32), String> {
    let bad = || "expected WIDTHxHEIGHT, such as 800x600".to_owned();
    let (w, h) = s.split_once('x').ok_or_else(bad)?;
    Ok((w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?))
}

/// A camera from command-line flags, when the eye, target and up are
/// all given.
fn camera_arg(
    eye: &Option<[String; 3]>,
    target: &Option<[String; 3]>,
    up: &Option<[f64; 3]>,
    fov: &Option<f64>,
    ortho: &Option<String>,
) -> Option<CameraArg> {
    let (eye, target, up) = (eye.as_ref()?, target.as_ref()?, (*up)?);
    Some(CameraArg {
        eye: eye.clone().map(Value::String),
        target: target.clone().map(Value::String),
        up,
        projection: match ortho {
            Some(h) => ProjectionArg::Orthographic {
                height: Value::String(h.clone()),
            },
            None => ProjectionArg::Perspective {
                fov_deg: fov.unwrap_or(45.0),
            },
        },
    })
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
        Command::Init { .. } | Command::Version | Command::Daemon(_) | Command::Watch { .. } => {
            return None;
        }
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
        Command::Doctor { repairs, params } if repairs.is_empty() => {
            let _ = params;
            Request::Doctor {}
        }
        Command::Doctor { repairs, params } => Request::DoctorRepair {
            repairs: repairs.clone(),
            params: params.iter().map(|(k, v)| (k.clone(), json!(v))).collect(),
        },
        Command::Participants => Request::ParticipantList {},
        Command::Select {
            entities,
            add,
            remove,
            clear: _,
        } => Request::SelectionSet {
            entities: entities.clone(),
            mode: if *add {
                SelectMode::Add
            } else if *remove {
                SelectMode::Remove
            } else {
                SelectMode::Replace
            },
        },
        Command::Selection { of } => Request::SelectionGet {
            participant: of.clone(),
        },
        Command::Camera {
            of,
            view,
            fit,
            eye,
            target,
            up,
            fov,
            ortho,
        } => match camera_arg(eye, target, up, fov, ortho) {
            _ if view.is_some() || !fit.is_empty() => Request::CameraSet {
                camera: None,
                view: view.clone(),
                fit: fit.clone(),
            },
            Some(camera) => Request::CameraSet {
                view: None,
                fit: Vec::new(),
                camera: Some(camera),
            },
            None => Request::CameraGet {
                participant: of.clone(),
            },
        },
        Command::Render {
            part,
            out: _,
            view,
            of,
            eye,
            target,
            up,
            fov,
            ortho,
            size,
            highlight,
            labels,
            no_edges,
        } => Request::Render {
            part: part.clone(),
            camera: camera_arg(eye, target, up, fov, ortho),
            participant: of.clone(),
            view: view.clone(),
            width: Some(size.0),
            height: Some(size.1),
            highlight: highlight.clone(),
            edges: Some(!no_edges),
            labels: Some(*labels),
        },
        Command::Highlight {
            entities,
            style,
            label,
            clear,
        } => {
            if *clear {
                Request::HighlightClear { style: *style }
            } else {
                Request::HighlightSet {
                    entities: entities.clone(),
                    style: style.unwrap_or_default(),
                    label: label.clone(),
                }
            }
        }
        Command::Activity { limit, before } => Request::ActivityLog {
            limit: Some(*limit),
            before: *before,
        },
        Command::Config(ConfigCmd::Get { key, scope }) => Request::ConfigGet {
            key: key.clone(),
            scope: scope.get(),
        },
        Command::Config(ConfigCmd::Set {
            key,
            value,
            scope,
            expect,
        }) => Request::ConfigSet {
            key: key.clone(),
            value: serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.clone())),
            scope: scope.get(),
            expected_revision: expect.clone(),
        },
        Command::Mesh {
            part,
            feature,
            quality,
            out,
        } => Request::MeshGet {
            part: part.clone(),
            feature: feature.clone(),
            quality: quality.clone(),
            transfer: out.as_ref().map(|_| Transfer::Binary),
        },
        Command::History { part, limit } => Request::HistoryLog {
            part: part.clone(),
            limit: Some(*limit),
        },
        Command::Undo { entry } => Request::Undo { entry: *entry },
        Command::Redo { entry } => Request::Redo { entry: *entry },
        Command::Status { text, entities } => Request::ActivitySet {
            status: text.clone(),
            entities: entities.clone(),
        },
    })
}

fn no_daemon(cli: &Cli) -> bool {
    cli.no_daemon || std::env::var("RYUCAD_NO_DAEMON").is_ok_and(|v| v == "1")
}

/// The agent participant to act as.
fn participant(cli: &Cli) -> Option<String> {
    cli.as_name
        .clone()
        .or_else(|| std::env::var("RYUCAD_PARTICIPANT").ok())
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
        Command::Watch {
            topics,
            count,
            timeout,
        } => {
            let root = find_root(&here)?;
            if no_daemon(cli) {
                return Err(ryucad::live::no_live_session("subscribe"));
            }
            client::watch(
                &root,
                participant(cli).as_deref(),
                topics,
                *count,
                Duration::from_secs(*timeout),
            )
        }
        // A damaged project has no daemon and no session: doctor works on
        // the folder.
        Command::Doctor { repairs, params } if find_project(&here)?.1 => {
            let root = find_project(&here)?.0;
            if repairs.is_empty() {
                return ryucad::doctor::report(&root);
            }
            let params = params.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            ryucad::doctor::repair(&root, repairs, &ryucad::doctor::Params::from_json(&params)?)
        }
        cmd => {
            let req = request(cmd).expect("every other command is a request");
            let root = find_root(&here)?;
            if !no_daemon(cli) {
                match client::request(&root, &req, participant(cli).as_deref()) {
                    Ok(answer) => return deliver(cmd, answer?),
                    // Never delivered, so running it here is safe.
                    Err(e) => warnings.push(format!("{}; ran in process", e.message)),
                }
            }
            deliver(
                cmd,
                server::handle_safely(&mut None, None, None, &root, req)?,
            )
        }
    }
}

/// The result to print; bytes the answer carries go to the command's
/// `--out` file.
fn deliver(cmd: &Command, a: Answer) -> Result<Value> {
    let Some(bytes) = a.binary else {
        return Ok(a.result);
    };
    let out = match cmd {
        Command::Mesh { out: Some(o), .. } | Command::Render { out: o, .. } => o,
        _ => {
            return Err(Error::new(
                Code::Internal,
                "bytes with nowhere to write them",
            ));
        }
    };
    std::fs::write(out, bytes).map_err(|e| Error::io(out, &e))?;
    let mut result = a.result;
    result["out"] = json!(out);
    Ok(result)
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
