//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code.

use clap::{Args, Parser, Subcommand};
use ryucad::doc::project::{find_project, find_root, plain};
use ryucad::error::{Code, Error, Result};
use ryucad::live::{SelectMode, Style};
use ryucad::proto::{
    Answer, FeatureFields, PlacementArg, Position, Request, Transfer, VersionInfo,
};
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
    /// List, set, delete or rename a part's parameters: named values, each an expression
    /// such as "3 mm" or "wall * 2", that features' quantities can read.
    #[command(subcommand)]
    Param(ParamCmd),
    /// The project's material library: densities (and colours) that mass properties read,
    /// each part's named by its PDM `material` property.
    #[command(subcommand)]
    Materials(MaterialsCmd),
    /// Named views: cameras saved with a part or an assembly, in Git, that
    /// `camera --view` and `render --view` restore.
    #[command(subcommand)]
    View(ViewCmd),
    /// ryupdm's settings for this project: its ID, how revisions are labelled, the release
    /// branch, the part number pattern and the custom properties parts may have.
    #[command(subcommand)]
    Pdm(PdmCmd),
    /// Show or set a part's properties: its number, name, description, material, unit of
    /// measure, make or buy, and the project's own.
    #[command(subcommand)]
    Props(PropsCmd),
    /// Propose a part's next revision, or make its latest one obsolete. Records are files
    /// under releases/, official once the release branch holds them.
    #[command(subcommand)]
    Release(ReleaseCmd),
    /// Edit, solve or read a sketch: points, lines, arcs and circles, and the constraints and
    /// dimensions between them.
    #[command(subcommand)]
    Sketch(SketchCmd),
    /// Move an assembly's instances, or show its tree of instances.
    #[command(subcommand)]
    Assembly(AssemblyCmd),
    /// Import a STEP file: store it in the project (imports/) and make a part of each
    /// product's bodies and an assembly of each product's children, add one product's
    /// bodies to a part with --into, or replace a file imported before with --update.
    Import {
        /// The STEP file (.step or .stp).
        file: std::path::PathBuf,
        /// The part or assembly to make of the file's one root.
        #[arg(long)]
        part: Option<String>,
        /// Replace this imported file (imports/<name>.step) with the file's bytes, and
        /// update what was made of it.
        #[arg(long, value_name = "IMPORTED")]
        update: Option<String>,
        /// Add the bodies to this part instead of making parts.
        #[arg(long, value_name = "PART")]
        into: Option<String>,
        /// With --into, the product whose bodies to add: its id, else its name.
        #[arg(long)]
        product: Option<String>,
        /// Write nothing: say what it would store and make, and how each body heals.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// Write a part or an assembly as a file: as the working files have it, as a Git
    /// revision has it (--rev), or as a release (--rev label:<L>). The daemon writes
    /// it, atomically, in a folder that must exist.
    Export {
        part: String,
        /// The file to write; its name goes in a STEP file's header.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// step; by default the one --out's extension names (.step, .stp).
        #[arg(long)]
        format: Option<String>,
        /// ap242 (the default) or ap214.
        #[arg(long)]
        schema: Option<String>,
        /// The length unit: mm (the default) or m.
        #[arg(long)]
        unit: Option<String>,
        /// The header's time stamp; by default the commit's time when exporting a
        /// commit, else now.
        #[arg(long, value_name = "T")]
        time_stamp: Option<String>,
        /// Read the project as this Git revision has it (main, HEAD~2, a hash), or
        /// label:<L> for the part's release L.
        #[arg(long, value_name = "REV")]
        rev: Option<String>,
    },
    /// Rebuild one part, or all of them.
    Rebuild {
        part: Option<String>,
        #[command(flatten)]
        rev: RevArg,
    },
    /// List a part's bodies, faces, edges and vertices by stable name.
    Inspect {
        part: String,
        feature: Option<String>,
        /// Only this entity: its name, or a full reference <part>/<feature>/<name>.
        #[arg(long)]
        entity: Option<String>,
        #[command(flatten)]
        rev: RevArg,
    },
    /// Report files that aren't canonical, broken, unlisted or unknown.
    CheckFormat,
    /// Rebuild the project from its files alone, with nothing reused, and
    /// verify it: doctor's issues, every feature builds, sketches are
    /// current, bodies don't intersect themselves and mesh cleanly. Exits
    /// 1 when anything fails.
    Check {
        /// Only this part (and the project's own issues).
        part: Option<String>,
        /// Also report what changed from this revision (main, HEAD~1),
        /// bodies included, as `diff --model` does.
        #[arg(long, value_name = "REV")]
        against: Option<String>,
        /// Also rebuild official releases and compare their bodies with their records
        /// (slower).
        #[arg(long)]
        results: bool,
    },
    /// An assembly's bill of materials, from its files alone: indented (each
    /// assembly's children with how many it places), or flat (every part
    /// with its total), as JSON or CSV; `bom diff` compares two revisions.
    #[command(args_conflicts_with_subcommands = true)]
    Bom {
        #[command(subcommand)]
        cmd: Option<BomCmd>,
        #[command(flatten)]
        show: BomShow,
    },
    /// Every assembly that uses a part: in the working files, and in every
    /// official release of an assembly.
    WhereUsed {
        part: String,
        /// Only uses of this release of the part.
        #[arg(long)]
        label: Option<String>,
        /// Only uses of the part as it is (working).
        #[arg(long, conflicts_with = "label")]
        working: bool,
        /// Only assemblies that place it themselves.
        #[arg(long)]
        direct: bool,
    },
    /// Which instances of an assembly overlap: each pair of solids that
    /// share a volume, with its box, its volume (estimated from a mesh)
    /// and the faces it touches. Faces that only touch are clear.
    Interference {
        part: String,
        /// Only pairs with a side under this instance path (s1/b2,
        /// holes:2/bolt); repeat for more.
        #[arg(long = "instance", value_name = "PATH")]
        instances: Vec<String>,
    },
    /// Measure one entity (its point, length or area and shape; a solid's or occurrence's
    /// volume and area) or two (their distance and nearest points, and their angle where
    /// both have directions): <part>/<solid>/<name>, <part>/<solid>,
    /// <assembly>/<instance>/…/<solid>/<name> or <assembly>/<instance>/….
    Measure {
        #[arg(num_args = 1..=2, required = true, value_name = "ENTITY")]
        entities: Vec<String>,
        #[command(flatten)]
        rev: RevArg,
    },
    /// How far apart an assembly's instances are: the minimum distance between each pair
    /// of solids whose boxes come within --below (a hundredth of the assembly's diagonal
    /// by default), closest first, with the nearest points. Pairs at 0 touch or interfere.
    Clearance {
        part: String,
        /// Only pairs with a side under this instance path (s1/b2, holes:2/bolt); repeat
        /// for more.
        #[arg(long = "instance", value_name = "PATH")]
        instances: Vec<String>,
        /// Pairs whose boxes come within this length (5 mm).
        #[arg(long, value_name = "LENGTH")]
        below: Option<String>,
    },
    /// Mass properties of a part or assembly, a solid (<part>/<solid>) or an occurrence
    /// (<assembly>/<instance>/…): volume, area, centre of mass, and where every part has a
    /// material in materials.json, mass and inertia.
    Mass {
        of: String,
        /// The relative agreement the integration reaches (1e-9).
        #[arg(long, value_name = "REL")]
        tolerance: Option<f64>,
        #[command(flatten)]
        rev: RevArg,
    },
    /// What changed between two revisions: features, parameters and sketch
    /// entities, and with --model what that did to the bodies.
    Diff {
        /// The revision to compare from: HEAD by default.
        #[arg(long, value_name = "REV")]
        from: Option<String>,
        /// The revision to compare to: the working files (working) by
        /// default.
        #[arg(long, value_name = "REV")]
        to: Option<String>,
        /// Only this part.
        #[arg(long)]
        part: Option<String>,
        /// Rebuild both and say what changed in the bodies: faces, edges and
        /// vertices gone, new and changed, and failures.
        #[arg(long)]
        model: bool,
        /// List every sketch position that moved.
        #[arg(long)]
        positions: bool,
        /// Include what is the same.
        #[arg(long)]
        all: bool,
    },
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
    /// Open a project (this folder by default) in the desktop app, or
    /// bring its open windows to the front.
    Open {
        /// The folder to open: this one (or `-C`'s) by default.
        #[arg(id = "folder", value_name = "DIR")]
        dir: Option<PathBuf>,
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
        #[arg(long = "of", value_name = "PARTICIPANT", conflicts_with_all = ["eye", "view", "fit", "normal_to"])]
        of: Option<String>,
        /// Look from a standard view (front, back, left, right, top, bottom, an edge or corner
        /// of the cube such as front_top_right, iso, dimetric or trimetric), framing the model
        /// (or the --fit entities); or restore a named view as saved.
        #[arg(long, conflicts_with_all = ["eye", "normal_to"])]
        view: Option<String>,
        /// Look along this face's normal (a planar face) or axis (a face of revolution), its
        /// longest straight edge level, framing the model (or the --fit entities).
        #[arg(long, value_name = "REF", conflicts_with = "eye")]
        normal_to: Option<String>,
        /// The part whose named view --view is (this agent's focus, else the project's model
        /// when it is one part).
        #[arg(long, requires = "view")]
        part: Option<String>,
        /// Frame these entities, keeping the view direction.
        #[arg(long, value_name = "REF", conflicts_with = "eye")]
        fit: Vec<String>,
        /// The eye point, three quantities: 0,-100mm,50mm.
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true, requires_all = ["target", "up"])]
        eye: Option<[String; 3]>,
        /// The point looked at.
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true, requires = "eye")]
        target: Option<[String; 3]>,
        /// The up direction: 0,0,1.
        #[arg(long, value_name = "X,Y,Z", value_parser = vector, allow_hyphen_values = true, requires = "eye")]
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
    /// Show what a participant compares the working files against (the one
    /// human, unless --of names another), or put them into a compare with
    /// a Git revision (HEAD, a branch, a commit), or take them out with
    /// --off. An app window then ghosts that revision and colours what
    /// changed.
    Compare {
        #[arg(value_name = "REV", conflicts_with = "off")]
        rev: Option<String>,
        /// Stop comparing.
        #[arg(long)]
        off: bool,
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
    /// Hide occurrences or solids in a participant's window (the one
    /// human's, else the caller's): parts, instances at any depth, or a
    /// part's solids.
    Hide {
        #[arg(value_name = "REF", required = true)]
        refs: Vec<String>,
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
    /// Show hidden occurrences or solids again, or everything with --all.
    Show {
        #[arg(value_name = "REF", required_unless_present = "all")]
        refs: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
    /// Hide everything else in the same parts and assemblies.
    Isolate {
        #[arg(value_name = "REF", required = true)]
        refs: Vec<String>,
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
    /// Show a part or assembly in a participant's window (the one human's,
    /// else the caller's), or the project's model with --project; with
    /// neither, say what it shows.
    Focus {
        #[arg(value_name = "PART", conflicts_with = "project")]
        part: Option<String>,
        /// Show the project's model: its one root assembly, else every part.
        #[arg(long)]
        project: bool,
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
    /// Wait for notifications from the project's daemon: what humans
    /// select, the model changing, and so on.
    Watch {
        /// model, project, participants, activity, merge or pdm (all by default).
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
    /// The disk cache of results and meshes, shared by every project on
    /// this machine.
    #[command(subcommand)]
    Cache(CacheCmd),
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
        #[command(flatten)]
        rev: RevArg,
    },
    /// Draw a part (or the whole project) to a PNG: iso framing the model
    /// unless a view, a participant's camera or a camera is given.
    Render {
        part: Option<String>,
        /// The PNG to write.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// A standard view (front, back, left, right, top, bottom, an edge or corner of the
        /// cube such as front_top_right, iso, dimetric or trimetric), or a named view of the
        /// part rendered.
        #[arg(long, conflicts_with_all = ["of", "eye"])]
        view: Option<String>,
        /// Use this participant's camera.
        #[arg(long = "of", value_name = "PARTICIPANT", conflicts_with = "eye")]
        of: Option<String>,
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true, requires_all = ["target", "up"])]
        eye: Option<[String; 3]>,
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true, requires = "eye")]
        target: Option<[String; 3]>,
        #[arg(long, value_name = "X,Y,Z", value_parser = vector, allow_hyphen_values = true, requires = "eye")]
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
        /// Label highlighted entities and sketches' dimensions.
        #[arg(long)]
        labels: bool,
        /// Leave out edges and silhouettes.
        #[arg(long)]
        no_edges: bool,
        /// Leave out sketches.
        #[arg(long)]
        no_sketches: bool,
        #[command(flatten)]
        rev: RevArg,
        /// Frame this revision's model too (repeatable; `working` for the
        /// working files), so renders of each revision share a camera.
        #[arg(long = "frame", value_name = "REV", conflicts_with_all = ["of", "eye"])]
        frame: Vec<String>,
        /// Paint faces new and changed since this revision, and ghost its
        /// model with the faces it loses in red.
        #[arg(long, value_name = "REV")]
        compare: Option<String>,
        /// Leave out a part, an occurrence or a solid (repeatable); by
        /// default what `--of`'s window hides, else the one human's
        /// (where `hide` goes), else the caller's.
        #[arg(long = "hidden", value_name = "REF")]
        hidden: Vec<String>,
        /// Draw the origin's axes and planes.
        #[arg(long)]
        origin: bool,
    },
    /// What the merge (or cherry-pick, revert or rebase step) Git is in the
    /// middle of leaves to settle.
    #[command(subcommand)]
    Merge(MergeCmd),
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
    /// Register ryucad with Git, and what Git runs: the diff driver's text
    /// conversion and the merge driver.
    #[command(subcommand)]
    Git(GitCmd),
    /// The hosted review service for this clone's GitHub pull requests:
    /// checks, renders, comments pinned to faces and edges, and merging the
    /// base in. Needs a GitHub token (RYUCAD_REVIEW_TOKEN, GH_TOKEN or gh).
    #[command(subcommand)]
    Review(ReviewCmd),
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
enum MergeCmd {
    /// Every conflict the merge leaves: in files' conflict records, paths Git left unmerged,
    /// and what the merge broke that built on either side, each with the side that caused it
    /// and its choices.
    Status {
        /// As Git's hooks run it: check every ryucad project in the repository, print what
        /// is unresolved for people, and exit 1 if anything is. Outside a merge only the
        /// release records and parts staged are checked.
        #[arg(long)]
        hook: bool,
    },
    /// Settle a conflict with one side's value, a value of your own, or one of its planned
    /// repairs, as one logged, undoable change. Marking the file resolved (git add) and
    /// committing are yours.
    #[command(group = clap::ArgGroup::new("how").required(true))]
    Resolve {
        /// The conflict's ID, as `merge status` gives it.
        conflict: String,
        #[arg(long, group = "how")]
        ours: bool,
        #[arg(long, group = "how")]
        theirs: bool,
        /// A JSON value to put in place.
        #[arg(long, group = "how", value_name = "JSON")]
        value: Option<String>,
        /// A planned repair's ID.
        #[arg(long, group = "how", value_name = "ID")]
        repair: Option<String>,
        /// Fail with `conflict` unless the conflict's part is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
}

#[derive(Subcommand)]
enum ReviewCmd {
    /// The pull request's checks, findings, changes, merge state and
    /// renders (the current branch's by default).
    Status { pr: Option<u64> },
    /// Comment threads, with each one's entity and whether it is outdated.
    Threads {
        pr: Option<u64>,
        /// Only threads still open.
        #[arg(long)]
        open: bool,
    },
    /// Open a thread on an entity (<part>/<solid>/<name>).
    Comment {
        reference: String,
        text: String,
        #[arg(long)]
        pr: Option<u64>,
        /// The commit it is about (the PR's head by default).
        #[arg(long, value_name = "SHA")]
        commit: Option<String>,
    },
    /// Reply to a thread.
    Reply { thread: String, text: String },
    /// Mark a thread resolved.
    Resolve { thread: String },
    /// Open a resolved thread again.
    Reopen { thread: String },
    /// Push the merge of the base into the PR's branch, as the service
    /// validated it (when GitHub can't merge but ryucad can).
    Update { pr: Option<u64> },
    /// Download the pull request's before and after renders.
    Renders {
        pr: Option<u64>,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum GitCmd {
    /// Print a ryucad file as one line per value, for `git diff` (Git's
    /// textconv): plain text, not JSON.
    Textconv { file: PathBuf },
    /// Merge three versions of a ryucad file value by value, for Git's merge
    /// (`merge.ryucad.driver`): writes the result over OURS and exits 0, or 1
    /// when conflicts are left in it. Prints nothing.
    MergeDriver {
        /// The common ancestor's version (empty when both sides added it).
        base: PathBuf,
        /// Ours, which the result replaces.
        ours: PathBuf,
        theirs: PathBuf,
        /// The file's path in the repository, as Git names it.
        path: Option<String>,
    },
    /// Tell Git to read and merge ryucad's files through ryucad: the diff and
    /// merge drivers, in this repository's config with its pre-merge-commit
    /// hook, or with --global the drivers in yours (the installers do that).
    Install {
        #[arg(long)]
        global: bool,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// What the cache holds: entries and bytes, all and by kind.
    Stats,
    /// Remove every entry. What they held is rebuilt when next asked for.
    Clear,
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
    /// Create an empty part, or with --assembly an empty assembly.
    Create {
        part: String,
        /// An assembly: its features place other parts.
        #[arg(long)]
        assembly: bool,
    },
    Delete {
        part: String,
        #[command(flatten)]
        expect: Expect,
    },
    List,
}

// Parsed once per run; the size difference costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum AssemblyCmd {
    /// Move an instance: to a pose (--origin, --z, --x; what is left out
    /// stays), then by --translate and a turn of --angle about --axis
    /// through its origin, in the assembly's frame.
    Move {
        part: String,
        instance: String,
        #[command(flatten)]
        placement: PlacementArgs,
        /// Three quantities to move it by: 0,0,5mm.
        #[arg(long, value_name = "VECTOR", value_parser = triple, allow_hyphen_values = true)]
        translate: Option<[String; 3]>,
        /// The direction to turn it about (with --angle): 0,0,1.
        #[arg(long, value_name = "VECTOR", value_parser = vector, allow_hyphen_values = true)]
        axis: Option<[f64; 3]>,
        /// How far to turn it (with --axis): 90deg.
        #[arg(long, value_name = "QUANTITY", allow_hyphen_values = true)]
        angle: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Solve the assembly's mates and report, writing nothing: as it is,
    /// or with an instance moved (as assembly move moves it) and held
    /// where it is put for a first solve.
    Solve {
        part: String,
        /// An instance to move first.
        instance: Option<String>,
        #[command(flatten)]
        placement: PlacementArgs,
        /// Three quantities to move it by: 0,0,5mm.
        #[arg(long, value_name = "VECTOR", value_parser = triple, allow_hyphen_values = true)]
        translate: Option<[String; 3]>,
        /// The direction to turn it about (with --angle): 0,0,1.
        #[arg(long, value_name = "VECTOR", value_parser = vector, allow_hyphen_values = true)]
        axis: Option<[f64; 3]>,
        /// How far to turn it (with --axis): 90deg.
        #[arg(long, value_name = "QUANTITY", allow_hyphen_values = true)]
        angle: Option<String>,
        /// Solve only what the instance's mates reach, from the poses the
        /// assembly's rebuild has, as a drag's preview does.
        #[arg(long, requires = "instance")]
        component: bool,
    },
    /// The instances as a tree: parts, revisions, poses, freedom,
    /// subassemblies.
    Tree { part: String },
}

#[derive(Subcommand)]
enum FeatureCmd {
    /// Add a feature: box (x, y, z), cylinder (radius, height), cone
    /// (radius, height, top_radius), sphere (radius), torus
    /// (major_radius, minor_radius), an empty sketch (--plane or a
    /// placement; xy by default), an extrude (--sketch, --length) or a
    /// revolve (--sketch, --axis, --angle), each a solid of its own or,
    /// with --operation, joined to, cut from or intersected with others
    /// (--target); a boolean (--op, --target, --tool) of whole solids; a
    /// fillet (--radius, --edge) or chamfer (--form, --d, --d1, --d2, --angle,
    /// --face, --edge) of a solid's edges, or several sizes (--groups); or a
    /// hole (--sketch, --point, --diameter, --depth, --form counterbore or
    /// countersink, --bottom) cut from solids (--target); or a pattern of
    /// joins, cuts and holes (--seed): --form linear (--direction,
    /// --spacing, --count, and --direction2 ...), circular (--axis, --count,
    /// --angle) or mirror (--plane), skipping instances (--skip); or, in an
    /// assembly, an instance (--of, --rev, --origin, --z, --x, --fixed), a mate
    /// (--form fastened, revolute, slider, cylindrical, planar or ball, --a, --b,
    /// --a-offset, --b-offset, --angle, --distance) or an instance pattern (--instance,
    /// --form linear or circular, as a pattern's, its directions and axis x, y, z or a
    /// connector).
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
        fields: FieldArgs,
        #[command(flatten)]
        expect: Expect,
    },
    /// Change some of a feature's parameters or placement, a sketch's
    /// plane, what an extrude or revolve sweeps and how far, a fillet's or
    /// chamfer's sizes and edges, a hole's points, form, sizes and depth,
    /// a pattern's seeds, form, directions, counts and skips, an instance's
    /// revision, pose and fixing, or a mate's form, connectors, offsets and
    /// driven values.
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
        fields: FieldArgs,
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

#[derive(Subcommand)]
enum ParamCmd {
    /// Every parameter: as written, its value, and what reads it.
    List { part: String },
    /// Set or add parameters: wall="3 mm" hole="wall * 2" n=4.
    Set {
        part: String,
        #[arg(value_name = "NAME=EXPRESSION", value_parser = param, required = true)]
        values: Vec<(String, String)>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete a parameter that nothing reads.
    Delete {
        part: String,
        name: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Rename a parameter, and every expression that reads it.
    Rename {
        part: String,
        from: String,
        to: String,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum PdmCmd {
    /// Give the project its ID and the default settings (keeps any it has).
    Init,
    /// The settings, with their defaults filled in, and the project revision.
    Config,
    /// Change the settings; what isn't given stays.
    Configure {
        /// How revisions are labelled: letters (A, B, ...), numeric (1, 2, ...) or numeric2
        /// (01, 02, ...). Fixed once any part is released.
        #[arg(long)]
        revisions: Option<String>,
        /// The branch whose history makes releases official.
        #[arg(long, value_name = "BRANCH")]
        release_branch: Option<String>,
        /// The pattern every part number must match whole (Rust regex syntax), or "" for
        /// none.
        #[arg(long, value_name = "REGEX")]
        pattern: Option<String>,
        /// Declare a custom property: finish='{"type":"enum","values":["bare","anodized"]}'.
        /// Types: string, text, integer, number, quantity (with "dim"), boolean, enum (with
        /// "values"); "required" (false) and "revising" (true) are optional.
        #[arg(long = "property", value_name = "NAME=JSON", value_parser = declaration)]
        properties: Vec<(String, Value)>,
        /// Remove a custom property's declaration.
        #[arg(long = "remove-property", value_name = "NAME")]
        remove: Vec<String>,
        /// Write even if parts' properties stop fitting the settings.
        #[arg(long)]
        force: bool,
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
}

#[derive(Subcommand)]
enum MaterialsCmd {
    /// Every material, the parts that name it, the default and the project revision.
    List,
    /// Add or replace a material.
    Set {
        name: String,
        /// Its density: 7850 kg/m^3 or 7.85 g/cm^3.
        #[arg(long, value_name = "DENSITY")]
        density: String,
        /// Its colour, #rrggbb (left out, it keeps its own).
        #[arg(long, value_name = "#RRGGBB")]
        colour: Option<String>,
        /// Make it the material of parts that name none.
        #[arg(long)]
        default: bool,
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
    /// Remove a material no part names.
    Delete {
        name: String,
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
}

// Parsed once per run; the size difference costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum ViewCmd {
    /// The named views of a part, or of every part, with each part's revision.
    List { part: Option<String> },
    /// Save a camera as a named view of a part: the one --of's window has (else the one
    /// human's, else this agent's), or one given by --eye, --target and --up. A view of
    /// that name is replaced.
    Save {
        part: String,
        view: String,
        #[arg(long = "of", value_name = "PARTICIPANT", conflicts_with = "eye")]
        of: Option<String>,
        /// The eye point, three quantities: 0,-100mm,50mm.
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true, requires_all = ["target", "up"])]
        eye: Option<[String; 3]>,
        /// The point looked at.
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true, requires = "eye")]
        target: Option<[String; 3]>,
        /// The up direction: 0,0,1.
        #[arg(long, value_name = "X,Y,Z", value_parser = vector, allow_hyphen_values = true, requires = "eye")]
        up: Option<[f64; 3]>,
        /// Perspective with this vertical field of view (45 by default).
        #[arg(
            long,
            value_name = "DEGREES",
            conflicts_with = "ortho",
            requires = "eye"
        )]
        fov: Option<f64>,
        /// Orthographic, this tall at the target.
        #[arg(long, value_name = "QUANTITY", requires = "eye")]
        ortho: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete a named view.
    Delete {
        part: String,
        view: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Rename a named view.
    Rename {
        part: String,
        from: String,
        to: String,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum PropsCmd {
    /// Every property of a part, set or not, with its declaration and faults.
    Show { part: String },
    /// Set properties: number=100-0042 material=6061-T6 count=4; NAME= clears one. Values are
    /// read as each property's type.
    Set {
        part: String,
        #[arg(value_name = "NAME=VALUE", value_parser = param, required = true)]
        values: Vec<(String, String)>,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// Write the record of a part's next revision, with its content hash, revising properties
    /// and rebuilt bodies. It is a proposal until the release branch holds it; nothing is
    /// committed.
    Propose {
        part: String,
        /// Why this revision, for the record.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// Write nothing: say the label it would take and every reason it would be refused.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// Every release record (or one part's): official at which commit or still proposed,
    /// its tag, and each part's state.
    List { part: Option<String> },
    /// Check every record against the parts, the release branch and the tags; fails
    /// (exit 1) on any error.
    Check {
        /// Also rebuild official releases and compare their bodies (slower).
        #[arg(long)]
        results: bool,
    },
    /// Create the local tag ryupdm/<part>/<label> of every official release that has none,
    /// and move those that point elsewhere. Nothing is pushed or deleted.
    Sync,
    /// Write an obsolete record for a part's latest official revision (the part may be
    /// deleted).
    Obsolete {
        part: String,
        label: String,
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum SketchCmd {
    /// Apply a batch of edits with one solve, or refuse them all.
    ///
    /// OPS is a JSON array (or @FILE holding one) of ops. With no ops, re-solves and writes the
    /// positions. Quantities are strings with units ("10 mm"), expressions over parameters
    /// ("width / 2"), or bare numbers in the project's default unit.
    ///
    /// Ops:
    ///   {"op": "add", "type": T, ...}        add an entity or constraint (fields below); "id"
    ///                                        is optional (p3, l2, a1, ci4, c7, d4 if left out)
    ///   {"op": "remove", "id": ID}           remove it, and every constraint naming it
    ///   {"op": "set", "id": ID, "value": Q}  change a dimension's value
    ///   {"op": "set", "id": ID, "construction": true|false}
    ///   {"op": "move", "id": ID, "at": [X, Y]}
    ///                                        move a point (held there for the solve)
    ///   {"op": "move", "id": ID, "r": Q}     resize a circle
    ///
    /// Entities ("construction": true and "projected": "<part>/<feature>/<name>" optional,
    /// except on points, which take only "projected"):
    ///   point   "at": [X, Y]
    ///   line    "start": P, "end": P
    ///   arc     "center": P, "start": P, "end": P   (counterclockwise from start to end)
    ///   circle  "center": P, "r": Q    (the file and `sketch get` say r_m: meters, solved)
    ///
    /// Constraints name entities in "a", then "b", then "about":
    ///   one     horizontal, vertical, fix
    ///   two     coincident, parallel, perpendicular, tangent, equal, midpoint, on, concentric
    ///   three   symmetric ("a" and "b" about "about")
    ///
    /// Dimensions add a "value" (Q; angles in deg or rad, a bare number is degrees):
    ///   one     length, radius, diameter
    ///   two     distance, distance_x, distance_y, angle
    ///
    /// Example: [{"op": "add", "type": "point", "at": ["0", "10 mm"]},
    ///           {"op": "add", "type": "line", "start": "p1", "end": "p2"},
    ///           {"op": "add", "type": "length", "a": "l1", "value": "width"}]
    #[command(verbatim_doc_comment)]
    Edit {
        part: String,
        sketch: String,
        #[arg(value_name = "OPS", value_parser = ops)]
        ops: Option<Ops>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Solve without writing: the report for the sketch as it is, or with OPS applied.
    Solve {
        part: String,
        sketch: String,
        #[arg(value_name = "OPS", value_parser = ops)]
        ops: Option<Ops>,
    },
    /// The solved geometry, in the sketch's frame and the part's, with each entity's and
    /// constraint's state, curves as polylines and dimensions' labels.
    Get {
        part: String,
        sketch: String,
        /// How finely arcs and circles are drawn: coarse, normal or fine (the
        /// user's display quality by default).
        #[arg(long)]
        quality: Option<String>,
    },
}

/// Sketch ops, parsed from one argument.
#[derive(Clone)]
struct Ops(Vec<Value>);

/// A fillet's or chamfer's groups, parsed from one argument.
#[derive(Clone)]
struct Groups(Value);

#[derive(Args)]
struct BomShow {
    /// The part or assembly: the project's one root assembly if left out.
    part: Option<String>,
    /// A Git revision (main, HEAD~2, a hash), or label:<L> for the part's
    /// release L: the working files by default.
    #[arg(long, value_name = "REV")]
    rev: Option<String>,
    /// Every part with its total, instead of the indented tree.
    #[arg(long)]
    flat: bool,
    /// List this many levels below the part (the indented BOM).
    #[arg(long, value_name = "N")]
    depth: Option<usize>,
    /// json (the default) or csv.
    #[arg(long)]
    format: Option<String>,
    /// Write the CSV to this file (implies --format csv).
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
}

#[derive(Subcommand)]
enum BomCmd {
    /// What changed in a BOM between two revisions: parts added, removed
    /// and changed (quantity, revision, properties), and where.
    Diff {
        part: Option<String>,
        /// The revision to compare from: HEAD by default.
        #[arg(long, value_name = "REV")]
        from: Option<String>,
        /// The revision to compare to: working by default.
        #[arg(long, value_name = "REV")]
        to: Option<String>,
    },
    /// Set an assembly's item numbers: bolt=3 nut=4 (several at once, so two
    /// may swap); --compact numbers its parts 1 to n in their current order.
    Renumber {
        part: String,
        #[arg(value_name = "PART=N", value_parser = param, required_unless_present = "compact")]
        items: Vec<(String, String)>,
        /// Number every part 1 to n, by its current number.
        #[arg(long, conflicts_with = "items")]
        compact: bool,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Args)]
struct RevArg {
    /// Read the project as this Git revision has it (main, HEAD~2, a hash)
    /// instead of the working files.
    #[arg(long, value_name = "REV")]
    rev: Option<String>,
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
struct FieldArgs {
    /// A sketch's plane: xy, yz or zx. A mirror pattern's: those, or a planar face
    /// <part>/<feature>/<name>.
    #[arg(long)]
    plane: Option<String>,
    /// A planar face to sketch on, or the face a chamfer's first distance runs along:
    /// <part>/<feature>/<name>, as inspect gives it.
    #[arg(long, value_name = "REFERENCE")]
    face: Option<String>,
    /// A sketch on a face: a vertex for its origin ("" for the part origin, projected).
    #[arg(long, value_name = "REFERENCE")]
    face_origin: Option<String>,
    /// A sketch on a face: a straight edge for its x ("" for the part's x, projected).
    #[arg(long, value_name = "REFERENCE")]
    face_x: Option<String>,
    /// The sketch an extrude or revolve sweeps, or a hole drills at.
    #[arg(long)]
    sketch: Option<String>,
    /// An entity on a loop to sweep; repeat for several. Every closed loop when left out.
    #[arg(long = "loop", value_name = "ENTITY")]
    loops: Vec<String>,
    /// Sweep every closed loop again, when editing.
    #[arg(long, conflicts_with = "loops")]
    all_loops: bool,
    /// An extrude's length.
    #[arg(long, value_name = "QUANTITY")]
    length: Option<String>,
    /// An extrude's extent: blind (from the sketch's plane, the default), symmetric (half
    /// each side), up_to (--up-to) or through_all (past everything it cuts or
    /// intersects; --both for each way).
    #[arg(long, value_name = "TYPE")]
    extent: Option<String>,
    /// Through all, each way from the sketch's plane (or --both false).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    both: Option<bool>,
    /// Extrude (or, with --depth up_to, drill) up to a planar face parallel to the sketch,
    /// or a vertex: <part>/<feature>/<name>.
    #[arg(long, value_name = "REFERENCE")]
    up_to: Option<String>,
    /// How much further than --up-to's target to go (negative stops short).
    #[arg(long, value_name = "QUANTITY", allow_hyphen_values = true)]
    offset: Option<String>,
    /// Extrude against the sketch's normal, or a linear pattern against its direction
    /// (or --reverse false).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    reverse: Option<bool>,
    /// A revolve's axis: a line of its sketch, or a straight edge in the sketch's plane
    /// (<part>/<feature>/<name>). A circular pattern's: x, y, z, or a straight or
    /// circular edge or a cylindrical or conical face.
    #[arg(long, value_name = "AXIS")]
    axis: Option<String>,
    /// A revolve's angle: 90, "90 deg", or full; a chamfer's angle to its face
    /// (distance_angle). A circular pattern's span, which it needs: 360 spaces --count
    /// evenly round a whole turn, less spans the first instance to the last.
    #[arg(long)]
    angle: Option<String>,
    /// Revolve half the angle each side of the sketch's plane (or --symmetric false).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    symmetric: Option<bool>,
    /// What a primitive, extrude or revolve does with its body: new (a solid of its
    /// own, the default), join, cut or intersect.
    #[arg(long)]
    operation: Option<String>,
    /// A solid a join, cut or intersect acts on, by the feature that founded it; repeat
    /// for several. A boolean's target. The part's one solid when left out.
    #[arg(long = "target", value_name = "SOLID")]
    targets: Vec<String>,
    /// How a boolean combines its target with its tools: unite, subtract or intersect.
    #[arg(long)]
    op: Option<String>,
    /// A solid a boolean uses as a tool, and consumes; repeat for several.
    #[arg(long = "tool", value_name = "SOLID")]
    tools: Vec<String>,
    /// A fillet's radius.
    #[arg(long, value_name = "QUANTITY")]
    radius: Option<String>,
    /// An edge a fillet or chamfer rounds or bevels: <part>/<solid>/<name>, as inspect
    /// gives it; repeat for several.
    #[arg(long = "edge", value_name = "REFERENCE")]
    edges: Vec<String>,
    /// A chamfer's form: equal (--d, the default), distances (--d1 along --face, --d2
    /// along the other face) or distance_angle (--d along --face, at --angle to it). A
    /// hole's: simple (the default), counterbore or countersink.
    #[arg(long)]
    form: Option<String>,
    /// A chamfer's distance: along both faces, or along --face.
    #[arg(long, value_name = "QUANTITY")]
    d: Option<String>,
    /// A chamfer's distance along --face.
    #[arg(long, value_name = "QUANTITY")]
    d1: Option<String>,
    /// A chamfer's distance along the other face.
    #[arg(long, value_name = "QUANTITY")]
    d2: Option<String>,
    /// Grow each edge to the chain of edges meeting it tangentially (the default), or
    /// --propagate false.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    propagate: Option<bool>,
    /// A fillet's or chamfer's groups as JSON, or @FILE holding them, for more than one
    /// size: [{"radius": "2 mm", "edges": ["<part>/<solid>/<name>"]}, ...].
    #[arg(long, value_name = "JSON", value_parser = groups)]
    groups: Option<Groups>,
    /// A point of its sketch a hole drills at; repeat for several. Every point no curve
    /// uses when left out.
    #[arg(long = "point", value_name = "POINT")]
    points: Vec<String>,
    /// Drill at every point no curve uses again, when editing.
    #[arg(long, conflicts_with = "points")]
    all_points: bool,
    /// A hole's diameter.
    #[arg(long, value_name = "QUANTITY")]
    diameter: Option<String>,
    /// A counterbore's diameter.
    #[arg(long, value_name = "QUANTITY")]
    counterbore_diameter: Option<String>,
    /// A counterbore's depth.
    #[arg(long, value_name = "QUANTITY")]
    counterbore_depth: Option<String>,
    /// A countersink's diameter at the hole's mouth.
    #[arg(long, value_name = "QUANTITY")]
    countersink_diameter: Option<String>,
    /// A countersink's included angle: 90, "82 deg".
    #[arg(long, value_name = "ANGLE")]
    countersink_angle: Option<String>,
    /// A hole's depth: a length (blind, to the drill point's shoulder), through_all, or
    /// up_to (--up-to, --offset).
    #[arg(long, value_name = "DEPTH")]
    depth: Option<String>,
    /// A hole's bottom: flat (the default), or a drill point's included angle: 118.
    #[arg(long, value_name = "BOTTOM")]
    bottom: Option<String>,
    /// A join, cut or hole a pattern repeats; repeat for several.
    #[arg(long = "seed", value_name = "FEATURE")]
    seeds: Vec<String>,
    /// A linear pattern's direction: x, y, z, or a straight edge or planar face
    /// <part>/<feature>/<name>.
    #[arg(long, value_name = "DIRECTION")]
    direction: Option<String>,
    /// A linear pattern's spacing.
    #[arg(long, value_name = "QUANTITY")]
    spacing: Option<String>,
    /// A pattern's count along its direction or round its axis, the seed's included.
    #[arg(long, value_name = "EXPRESSION")]
    count: Option<String>,
    /// A linear pattern's second direction, as --direction ("" for none).
    #[arg(long, value_name = "DIRECTION")]
    direction2: Option<String>,
    /// A linear pattern's spacing along its second direction.
    #[arg(long, value_name = "QUANTITY")]
    spacing2: Option<String>,
    /// A linear pattern's count along its second direction.
    #[arg(long, value_name = "EXPRESSION")]
    count2: Option<String>,
    /// A linear pattern against its second direction (or --reverse2 false).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    reverse2: Option<bool>,
    /// An instance a pattern leaves out: i, or i,j with two directions; repeat for
    /// several.
    #[arg(long = "skip", value_name = "INDEX", value_parser = instance)]
    skip: Vec<Value>,
    /// Leave no instance out again, when editing.
    #[arg(long, conflicts_with = "skip")]
    no_skip: bool,
    /// The part an instance places.
    #[arg(long, value_name = "PART")]
    of: Option<String>,
    /// The revision an instance places: working (the default) or a release label.
    #[arg(long, value_name = "REV")]
    rev: Option<String>,
    /// Hold an instance where it is when mates are solved (--fixed false to free it).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    fixed: Option<bool>,
    /// A mate's first connector: an entity of an occurrence,
    /// <assembly>/<instance>/.../<solid>/<name> as inspect gives it, or an occurrence's
    /// own frame, <assembly>/<instance>/...; a pattern's copy is <pattern>:<i>/<seed>.
    #[arg(long, value_name = "CONNECTOR")]
    a: Option<String>,
    /// A mate's second connector, as --a: its frame follows --a's.
    #[arg(long, value_name = "CONNECTOR")]
    b: Option<String>,
    /// --a's offset in its own axes as JSON: {"origin": ["0", "0", "2 mm"], "rotate":
    /// "90 deg", "flip": true}, any part; {} for none.
    #[arg(long, value_name = "JSON", value_parser = json_object)]
    a_offset: Option<Value>,
    /// --b's offset, as --a-offset.
    #[arg(long, value_name = "JSON", value_parser = json_object)]
    b_offset: Option<Value>,
    /// A slider's, cylindrical or planar mate's distance along --a's z ("" to let it go).
    #[arg(long, value_name = "QUANTITY", allow_hyphen_values = true)]
    distance: Option<String>,
    /// An instance an instance pattern copies; repeat for several.
    #[arg(long = "instance", value_name = "INSTANCE")]
    instances: Vec<String>,
}

fn json_object(s: &str) -> core::result::Result<Value, String> {
    match serde_json::from_str::<Value>(s) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err("expected a JSON object".to_owned()),
    }
}

impl FieldArgs {
    fn to_fields(&self) -> FeatureFields {
        let mut extent = Map::new();
        if let Some(t) = &self.extent {
            extent.insert("type".into(), json!(t));
        }
        if let Some(l) = &self.length {
            extent.insert("length".into(), json!(l));
        }
        // An up-to target and offset are a hole's when its depth says so.
        let drills = self.depth.as_deref() == Some("up_to");
        if let Some(t) = self.up_to.as_ref().filter(|_| !drills) {
            extent.insert("type".into(), json!("up_to"));
            extent.insert("target".into(), json!(t));
        }
        if let Some(o) = self.offset.as_ref().filter(|_| !drills) {
            extent.insert("offset".into(), json!(o));
        }
        let depth = self.depth.as_deref().map(|d| match d {
            "through_all" => json!("through_all"),
            "up_to" => {
                let mut up = Map::new();
                if let Some(t) = &self.up_to {
                    up.insert("target".into(), json!(t));
                }
                if let Some(o) = &self.offset {
                    up.insert("offset".into(), json!(o));
                }
                json!({ "up_to": up })
            }
            d => json!({ "blind": d }),
        });
        let part = |a: (&str, &Option<String>), b: (&str, &Option<String>)| {
            let m: Map<String, Value> = [a, b]
                .into_iter()
                .filter_map(|(k, v)| Some((k.to_owned(), json!(v.as_ref()?))))
                .collect();
            (!m.is_empty()).then_some(Value::Object(m))
        };
        if let Some(b) = self.both {
            extent.insert("both".into(), json!(b));
        }
        FeatureFields {
            plane: self.plane.clone(),
            face: self.face.clone(),
            face_origin: self.face_origin.clone(),
            face_x: self.face_x.clone(),
            sketch: self.sketch.clone(),
            loops: if self.all_loops {
                Some(Vec::new())
            } else {
                (!self.loops.is_empty()).then(|| self.loops.clone())
            },
            extent: (!extent.is_empty()).then_some(Value::Object(extent)),
            reverse: self.reverse,
            axis: self.axis.clone(),
            angle: self.angle.clone().map(Value::String),
            symmetric: self.symmetric,
            operation: self.operation.clone(),
            targets: (!self.targets.is_empty()).then(|| self.targets.clone()),
            op: self.op.clone(),
            target: None,
            tools: (!self.tools.is_empty()).then(|| self.tools.clone()),
            groups: self.groups.clone().map(|g| g.0),
            propagate: self.propagate,
            radius: self.radius.clone().map(Value::String),
            edges: (!self.edges.is_empty()).then(|| self.edges.clone()),
            form: self.form.clone(),
            d: self.d.clone().map(Value::String),
            d1: self.d1.clone().map(Value::String),
            d2: self.d2.clone().map(Value::String),
            points: if self.all_points {
                Some(Vec::new())
            } else {
                (!self.points.is_empty()).then(|| self.points.clone())
            },
            diameter: self.diameter.clone().map(Value::String),
            counterbore: part(
                ("diameter", &self.counterbore_diameter),
                ("depth", &self.counterbore_depth),
            ),
            countersink: part(
                ("diameter", &self.countersink_diameter),
                ("angle", &self.countersink_angle),
            ),
            depth,
            bottom: self.bottom.as_deref().map(|b| match b {
                "flat" => json!("flat"),
                b => json!({ "drill": b }),
            }),
            features: (!self.seeds.is_empty()).then(|| self.seeds.clone()),
            direction: self.direction.clone(),
            spacing: self.spacing.clone().map(Value::String),
            count: self.count.clone().map(Value::String),
            direction2: self.direction2.clone(),
            spacing2: self.spacing2.clone().map(Value::String),
            count2: self.count2.clone().map(Value::String),
            reverse2: self.reverse2,
            skip: if self.no_skip {
                Some(json!([]))
            } else {
                (!self.skip.is_empty()).then(|| json!(self.skip))
            },
            of: self.of.clone(),
            rev: self.rev.clone(),
            fixed: self.fixed,
            a: self.a.clone(),
            b: self.b.clone(),
            a_offset: self.a_offset.clone(),
            b_offset: self.b_offset.clone(),
            distance: self.distance.clone().map(Value::String),
            instances: (!self.instances.is_empty()).then(|| self.instances.clone()),
        }
    }
}

// Vectors may start with a minus (`--x -0,1,0`), so they take values that
// look like flags.
#[derive(Args)]
struct PlacementArgs {
    /// The local origin, three quantities: 0,0,10mm.
    #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true)]
    origin: Option<[String; 3]>,
    /// The local z axis: 0,0,1.
    #[arg(long, value_name = "X,Y,Z", value_parser = vector, allow_hyphen_values = true)]
    z: Option<[f64; 3]>,
    /// The local x direction: 1,0,0.
    #[arg(long, value_name = "X,Y,Z", value_parser = vector, allow_hyphen_values = true)]
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

/// A JSON array of sketch ops, or `@FILE` holding one.
fn ops(s: &str) -> core::result::Result<Ops, String> {
    let text = match s.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => s.to_owned(),
    };
    serde_json::from_str(&text)
        .map(Ops)
        .map_err(|e| format!("OPS must be a JSON array of ops: {e}"))
}

/// A JSON array of a fillet's or chamfer's groups, or `@FILE` holding
/// one.
fn groups(s: &str) -> core::result::Result<Groups, String> {
    let text = match s.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => s.to_owned(),
    };
    match serde_json::from_str(&text) {
        Ok(v @ Value::Array(_)) => Ok(Groups(v)),
        Ok(_) => Err("JSON must be an array of groups".to_owned()),
        Err(e) => Err(format!("JSON must be an array of groups: {e}")),
    }
}

fn param(s: &str) -> core::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .ok_or_else(|| "expected NAME=VALUE".to_owned())
}

/// `NAME=JSON`, the JSON a property's declaration.
fn declaration(s: &str) -> core::result::Result<(String, Value), String> {
    let (k, v) = param(s)?;
    let v = serde_json::from_str(&v).map_err(|e| format!("not JSON: {e}"))?;
    Ok((k, v))
}

/// Three comma-separated values; commas inside parentheses belong to an
/// expression (`max(a, b),0,0`).
fn triple(s: &str) -> core::result::Result<[String; 3], String> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].trim().to_owned());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].trim().to_owned());
    parts
        .try_into()
        .map_err(|_| "expected three values: X,Y,Z".to_owned())
}

/// A pattern instance's index: `i`, or `i,j` along two directions.
fn instance(s: &str) -> core::result::Result<Value, String> {
    let n = |x: &str| {
        x.trim()
            .parse::<u32>()
            .map_err(|_| format!("{x:?} is not an instance's index"))
    };
    match s.split_once(',') {
        Some((i, j)) => Ok(json!([n(i)?, n(j)?])),
        None => Ok(json!(n(s)?)),
    }
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
        Command::Init { .. }
        | Command::Version
        | Command::Daemon(_)
        | Command::Git(_)
        | Command::Review(_)
        | Command::Watch { .. }
        | Command::Open { .. } => {
            return None;
        }
        Command::Part(PartCmd::Create { part, assembly }) => Request::PartCreate {
            part: part.clone(),
            kind: assembly.then(|| "assembly".to_owned()),
        },
        Command::Part(PartCmd::Delete { part, expect }) => Request::PartDelete {
            part: part.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Part(PartCmd::List) => Request::PartList {},
        Command::Assembly(AssemblyCmd::Move {
            part,
            instance,
            placement,
            translate,
            axis,
            angle,
            expect,
        }) => Request::AssemblyMove {
            part: part.clone(),
            instance: instance.clone(),
            placement: placement.to_arg(),
            translate: translate.clone().map(|t| t.map(Value::String)),
            axis: *axis,
            angle: angle.clone().map(Value::String),
            expected_revision: expect.revision.clone(),
        },
        Command::Assembly(AssemblyCmd::Tree { part }) => {
            Request::AssemblyTree { part: part.clone() }
        }
        Command::Assembly(AssemblyCmd::Solve {
            part,
            instance,
            placement,
            translate,
            axis,
            angle,
            component,
        }) => Request::AssemblySolve {
            part: part.clone(),
            component: *component,
            moves: instance
                .iter()
                .map(|i| ryucad::proto::InstanceMove {
                    instance: i.clone(),
                    placement: placement.to_arg(),
                    translate: translate.clone().map(|t| t.map(Value::String)),
                    axis: *axis,
                    angle: angle.clone().map(Value::String),
                })
                .collect(),
        },
        Command::Feature(FeatureCmd::Add {
            part,
            ty,
            id,
            position,
            params,
            placement,
            fields,
            expect,
        }) => Request::FeatureAdd {
            part: part.clone(),
            ty: ty.clone(),
            params: params.to_map(),
            placement: placement.to_arg(),
            fields: fields.to_fields(),
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
            fields,
            expect,
        }) => {
            let mut params = params.to_map();
            params.extend(unset.iter().map(|k| (k.clone(), Value::Null)));
            Request::FeatureEdit {
                part: part.clone(),
                id: id.clone(),
                params,
                placement: placement.to_arg(),
                fields: fields.to_fields(),
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
        Command::Param(ParamCmd::List { part }) => Request::ParamList { part: part.clone() },
        Command::Param(ParamCmd::Set {
            part,
            values,
            expect,
        }) => Request::ParamSet {
            part: part.clone(),
            parameters: values.iter().map(|(k, v)| (k.clone(), json!(v))).collect(),
            expected_revision: expect.revision.clone(),
        },
        Command::Param(ParamCmd::Delete { part, name, expect }) => Request::ParamDelete {
            part: part.clone(),
            name: name.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Param(ParamCmd::Rename {
            part,
            from,
            to,
            expect,
        }) => Request::ParamRename {
            part: part.clone(),
            from: from.clone(),
            to: to.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Materials(MaterialsCmd::List) => Request::MaterialsList {},
        Command::View(ViewCmd::List { part }) => Request::ViewList { part: part.clone() },
        Command::View(ViewCmd::Save {
            part,
            view,
            of,
            eye,
            target,
            up,
            fov,
            ortho,
            expect,
        }) => Request::ViewSave {
            part: part.clone(),
            view: view.clone(),
            camera: camera_arg(eye, target, up, fov, ortho),
            participant: of.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::View(ViewCmd::Delete { part, view, expect }) => Request::ViewDelete {
            part: part.clone(),
            view: view.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::View(ViewCmd::Rename {
            part,
            from,
            to,
            expect,
        }) => Request::ViewRename {
            part: part.clone(),
            from: from.clone(),
            to: to.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Materials(MaterialsCmd::Set {
            name,
            density,
            colour,
            default,
            expect,
        }) => Request::MaterialsSet {
            name: name.clone(),
            density: density.clone(),
            colour: colour.clone(),
            default: *default,
            expected_revision: expect.clone(),
        },
        Command::Materials(MaterialsCmd::Delete { name, expect }) => Request::MaterialsDelete {
            name: name.clone(),
            expected_revision: expect.clone(),
        },
        Command::Pdm(PdmCmd::Init) => Request::PdmInit {},
        Command::Pdm(PdmCmd::Config) => Request::PdmConfig {},
        Command::Pdm(PdmCmd::Configure {
            revisions,
            release_branch,
            pattern,
            properties,
            remove,
            force,
            expect,
        }) => Request::PdmConfigure {
            revisions: revisions.clone(),
            release_branch: release_branch.clone(),
            pattern: pattern.clone(),
            properties: properties
                .iter()
                .cloned()
                .chain(remove.iter().map(|n| (n.clone(), Value::Null)))
                .collect(),
            force: *force,
            expected_revision: expect.clone(),
        },
        Command::Props(PropsCmd::Show { part }) => Request::PropsGet { part: part.clone() },
        Command::Props(PropsCmd::Set {
            part,
            values,
            expect,
        }) => Request::PropsSet {
            part: part.clone(),
            properties: values
                .iter()
                .map(|(k, v)| {
                    let v = if v.is_empty() { Value::Null } else { json!(v) };
                    (k.clone(), v)
                })
                .collect(),
            expected_revision: expect.revision.clone(),
        },
        Command::Release(ReleaseCmd::List { part }) => Request::ReleaseList { part: part.clone() },
        Command::Release(ReleaseCmd::Sync) => Request::ReleaseSync {},
        Command::Release(ReleaseCmd::Check { results }) => {
            Request::ReleaseCheck { results: *results }
        }
        Command::Release(ReleaseCmd::Propose {
            part,
            note,
            dry_run,
            expect,
        }) => Request::ReleasePropose {
            part: part.clone(),
            note: note.clone(),
            expected_revision: expect.revision.clone(),
            dry_run: *dry_run,
        },
        Command::Release(ReleaseCmd::Obsolete {
            part,
            label,
            note,
            expect,
        }) => Request::ReleaseObsolete {
            part: part.clone(),
            label: label.clone(),
            note: note.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Sketch(SketchCmd::Edit {
            part,
            sketch,
            ops,
            expect,
        }) => Request::SketchEdit {
            part: part.clone(),
            sketch: sketch.clone(),
            ops: ops.clone().map_or_else(Vec::new, |o| o.0),
            expected_revision: expect.revision.clone(),
        },
        Command::Sketch(SketchCmd::Solve { part, sketch, ops }) => Request::SketchSolve {
            part: part.clone(),
            sketch: sketch.clone(),
            ops: ops.clone().map_or_else(Vec::new, |o| o.0),
        },
        Command::Sketch(SketchCmd::Get {
            part,
            sketch,
            quality,
        }) => Request::SketchGet {
            part: part.clone(),
            sketch: sketch.clone(),
            quality: quality.clone(),
        },
        Command::Import {
            file,
            part,
            update,
            into,
            product,
            dry_run,
            expect,
        } => Request::Import {
            // The daemon's folder isn't the caller's.
            path: std::path::absolute(file)
                .unwrap_or_else(|_| file.clone())
                .to_string_lossy()
                .into_owned(),
            part: part.clone(),
            into: into.clone(),
            product: product.clone(),
            dry_run: *dry_run,
            expected_revision: expect.revision.clone(),
            update: update.clone(),
        },
        Command::Export {
            part,
            out,
            format,
            schema,
            unit,
            time_stamp,
            rev,
        } => Request::Export {
            part: part.clone(),
            rev: rev.clone(),
            format: format.clone(),
            // The daemon's folder isn't the caller's.
            out: Some(
                std::path::absolute(out)
                    .unwrap_or_else(|_| out.clone())
                    .to_string_lossy()
                    .into_owned(),
            ),
            schema: schema.clone(),
            unit: unit.clone(),
            time_stamp: time_stamp.clone(),
            file_name: out.file_name().map(|n| n.to_string_lossy().into_owned()),
        },
        Command::Rebuild { part, rev } => Request::Rebuild {
            part: part.clone(),
            rev: rev.rev.clone(),
        },
        Command::Inspect {
            part,
            feature,
            entity,
            rev,
        } => Request::Inspect {
            part: part.clone(),
            feature: feature.clone(),
            entity: entity.clone(),
            rev: rev.rev.clone(),
        },
        Command::CheckFormat => Request::CheckFormat {},
        Command::Bom {
            cmd: Some(BomCmd::Diff { part, from, to }),
            ..
        } => Request::BomDiff {
            part: part.clone(),
            from: from.clone(),
            to: to.clone(),
        },
        Command::Bom {
            cmd:
                Some(BomCmd::Renumber {
                    part,
                    items,
                    compact,
                    expect,
                }),
            ..
        } => Request::BomRenumber {
            part: part.clone(),
            items: items
                .iter()
                .map(|(k, v)| {
                    let n = v.parse::<u64>().map_or_else(|_| json!(v), |n| json!(n));
                    (k.clone(), n)
                })
                .collect(),
            compact: *compact,
            expected_revision: expect.revision.clone(),
        },
        Command::Bom { cmd: None, show } => Request::BomGet {
            part: show.part.clone(),
            rev: show.rev.clone(),
            flat: show.flat,
            depth: show.depth,
            format: show
                .format
                .clone()
                .or_else(|| show.out.as_ref().map(|_| "csv".to_owned())),
        },
        Command::WhereUsed {
            part,
            label,
            working,
            direct,
        } => Request::BomWhereUsed {
            part: part.clone(),
            label: label.clone(),
            working: *working,
            direct: *direct,
        },
        Command::Clearance {
            part,
            instances,
            below,
        } => Request::Clearance {
            part: part.clone(),
            instances: instances.clone(),
            below: below.clone(),
        },
        Command::Mass { of, tolerance, rev } => Request::Mass {
            of: of.clone(),
            tolerance: *tolerance,
            rev: rev.rev.clone(),
        },
        Command::Measure { entities, rev } => Request::Measure {
            entities: entities.clone(),
            rev: rev.rev.clone(),
        },
        Command::Interference { part, instances } => Request::Interference {
            part: part.clone(),
            instances: instances.clone(),
        },
        Command::Check {
            part,
            against,
            results,
        } => Request::Check {
            part: part.clone(),
            against: against.clone(),
            results: *results,
        },
        Command::Diff {
            from,
            to,
            part,
            model,
            positions,
            all,
        } => Request::Diff {
            from: from.clone(),
            to: to.clone(),
            part: part.clone(),
            model: *model,
            positions: *positions,
            all: *all,
        },
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
            normal_to,
            part,
            fit,
            eye,
            target,
            up,
            fov,
            ortho,
        } => match camera_arg(eye, target, up, fov, ortho) {
            _ if view.is_some() || normal_to.is_some() || !fit.is_empty() => Request::CameraSet {
                camera: None,
                view: view.clone(),
                fit: fit.clone(),
                normal_to: normal_to.clone(),
                part: part.clone(),
            },
            Some(camera) => Request::CameraSet {
                view: None,
                fit: Vec::new(),
                camera: Some(camera),
                normal_to: None,
                part: None,
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
            no_sketches,
            rev,
            frame,
            compare,
            hidden,
            origin,
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
            sketches: Some(!no_sketches),
            rev: rev.rev.clone(),
            frame: frame.clone(),
            compare: compare.clone(),
            hidden: (!hidden.is_empty()).then(|| hidden.clone()),
            origin: *origin,
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
        Command::Compare { rev, off, of } if rev.is_none() && !off => Request::CompareGet {
            participant: of.clone(),
        },
        Command::Compare { rev, off, of } => Request::CompareSet {
            rev: rev.clone(),
            off: *off,
            participant: of.clone(),
        },
        Command::Hide { refs, of } => Request::VisibilitySet {
            hide: refs.clone(),
            show: Vec::new(),
            isolate: Vec::new(),
            all: false,
            participant: of.clone(),
        },
        Command::Show { refs, all, of } => Request::VisibilitySet {
            hide: Vec::new(),
            show: refs.clone(),
            isolate: Vec::new(),
            all: *all,
            participant: of.clone(),
        },
        Command::Isolate { refs, of } => Request::VisibilitySet {
            hide: Vec::new(),
            show: Vec::new(),
            isolate: refs.clone(),
            all: false,
            participant: of.clone(),
        },
        Command::Focus { part, project, of } => Request::ViewFocus {
            part: part.clone(),
            project: *project,
            participant: of.clone(),
        },
        Command::Activity { limit, before } => Request::ActivityLog {
            limit: Some(*limit),
            before: *before,
        },
        Command::Cache(CacheCmd::Stats) => Request::CacheStats {},
        Command::Cache(CacheCmd::Clear) => Request::CacheClear {},
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
            rev,
        } => Request::MeshGet {
            part: part.clone(),
            feature: feature.clone(),
            quality: quality.clone(),
            transfer: out.as_ref().map(|_| Transfer::Binary),
            rev: rev.rev.clone(),
        },
        Command::History { part, limit } => Request::HistoryLog {
            part: part.clone(),
            limit: Some(*limit),
        },
        Command::Merge(MergeCmd::Status { .. }) => Request::MergeStatus {},
        Command::Merge(MergeCmd::Resolve {
            conflict,
            ours,
            theirs,
            value,
            repair,
            expect,
        }) => {
            // Clap requires exactly one.
            let choice = match (ours, theirs, value) {
                (true, _, _) => "ours",
                (_, true, _) => "theirs",
                (_, _, Some(_)) => "value",
                _ => "repair",
            };
            // A value that isn't JSON is the text it says.
            let value = value
                .as_ref()
                .map(|v| serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.clone())));
            let part = conflict
                .strip_prefix("parts/")
                .and_then(|r| r.split('/').next())
                .map(str::to_owned);
            let expected_revisions = match (part, expect) {
                (Some(p), Some(e)) => [(p, e.clone())].into_iter().collect(),
                _ => Default::default(),
            };
            Request::MergeResolve {
                resolutions: vec![ryucad::proto::Resolution {
                    conflict: conflict.clone(),
                    choice: choice.to_owned(),
                    value,
                    repair: repair.clone(),
                }],
                expected_revisions,
            }
        }
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
        Command::Init { unit } => {
            let mut v = server::init(&here, *unit)?;
            // In a repository, the clone is set up for ryucad too; the
            // project is made either way, and `doctor` says what's missing.
            if ryucad::git::Repo::open(&here)?.is_some() {
                v["git"] = match ryucad::drivers::install(&here, true) {
                    Ok(g) => g,
                    Err(e) => json!({ "error": e }),
                };
            }
            Ok(v)
        }
        Command::Daemon(DaemonCmd::Run { idle_timeout }) => {
            let idle = idle_timeout.map_or(daemon::DEFAULT_IDLE, Duration::from_secs);
            daemon::run(&find_root(&here)?, idle).map(|()| Value::Null)
        }
        Command::Daemon(DaemonCmd::Status) => client::status(&find_root(&here)?),
        Command::Git(GitCmd::Install { global }) => ryucad::drivers::install(&here, !global),
        Command::Review(cmd) => {
            use ryucad::review::Action;
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
                ReviewCmd::Renders { pr, out } => Action::Renders {
                    pr: *pr,
                    out: here.join(out),
                },
            };
            ryucad::review::run(&here, action)
        }
        Command::Open { dir } => {
            let dir = dir.as_ref().map_or_else(|| here.clone(), |d| here.join(d));
            let dir = plain(dir.canonicalize().map_err(|e| Error::io(&dir, &e))?);
            // A damaged project, or a folder that isn't one, opens in the
            // app too: it offers to repair or create one.
            let root = match find_project(&dir) {
                Ok((root, false)) if !no_daemon(cli) => Some(root),
                _ => None,
            };
            client::open(&dir, root.as_deref())
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
                    Ok(answer) => return deliver(cmd, answer?).map(|r| lift(cmd, r, warnings)),
                    // Never delivered, so running it here is safe.
                    Err(e) => warnings.push(format!("{}; ran in process", e.message)),
                }
            }
            // As the participant asked for, as through the daemon.
            let me = participant(cli);
            deliver(
                cmd,
                server::handle_safely(&mut None, None, me.as_deref(), &root, req)?,
            )
            .map(|r| lift(cmd, r, warnings))
        }
    }
}

/// The CLI's warnings are all in its envelope: the warnings a request
/// returns in its result (the protocol has no envelope) move there, as
/// strings.
fn lift(cmd: &Command, mut result: Value, warnings: &mut Vec<String>) -> Value {
    if !matches!(
        cmd,
        Command::History { .. }
            | Command::Config(ConfigCmd::Get { .. })
            | Command::Rebuild { .. }
            | Command::Import { .. }
            | Command::Mass { .. }
    ) {
        return result;
    }
    if let Some(Value::Array(list)) = result.as_object_mut().and_then(|o| o.remove("warnings")) {
        warnings.extend(list.into_iter().map(|w| {
            match w {
                Value::String(s) => s,
                w => w["message"]
                    .as_str()
                    .map_or_else(|| w.to_string(), str::to_owned),
            }
        }));
    }
    result
}

/// The result to print; bytes the answer carries go to the command's
/// `--out` file.
fn deliver(cmd: &Command, a: Answer) -> Result<Value> {
    let Some(bytes) = a.binary else {
        return written(cmd, a.result);
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

/// The result to print, once text a command writes to its `--out` file
/// (a BOM's CSV) is written there, atomically.
fn written(cmd: &Command, mut result: Value) -> Result<Value> {
    if let Command::Bom {
        cmd: None,
        show: BomShow { out: Some(out), .. },
    } = cmd
    {
        let csv = result
            .as_object_mut()
            .and_then(|o| o.remove("csv"))
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| {
                Error::new(
                    Code::InvalidParam,
                    "--out writes CSV: leave out --format or give csv",
                )
                .with("param", "format")
            })?;
        ryucad::doc::project::write_atomic(out, csv)?;
        result["out"] = json!(out);
    }
    Ok(result)
}

/// Whether the arguments ask only for the version the way most tools take
/// it (`--version`, `-V`), beside `--pretty`: answered as `ryucad version`.
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
            // `ryucad` alone, like --help, is a person asking: but a command
            // missing its subcommand (`ryucad config`) is a usage error, in
            // JSON, like any other.
            let bare = std::env::args_os().len() <= 1;
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion)
                || (e.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand && bare)
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
    // Git reads a textconv's output as the file's text, not JSON.
    if let Command::Git(GitCmd::Textconv { file }) = &cli.command {
        return match std::fs::read(file) {
            Ok(bytes) => {
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                match out.write_all(&ryucad::diff::textconv(&bytes)) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(_) => ExitCode::from(Code::Io.exit_code()),
                }
            }
            Err(e) => finish(Err(Error::io(file, &e)), Vec::new(), cli.pretty),
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

/// `ryucad merge status --hook`: every project a merge in progress
/// concerns, checked in this process, then on every commit what the
/// index would commit wrong about releases. Exits 0 when nothing is
/// unresolved, 1 otherwise, saying what to settle on stderr.
fn hook_check(here: &std::path::Path) -> ExitCode {
    let projects = match ryucad::git::merging_projects(here) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ryucad: checking the merge: {}", e.message);
            return ExitCode::from(1);
        }
    };
    let mut failed = false;
    match ryucad::git::release_guard(here) {
        Ok(found) => {
            for (root, list) in found {
                failed = true;
                eprintln!(
                    "ryucad: this commit would leave {} release problem(s) in {}:",
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
            eprintln!("ryucad: checking the release records staged: {}", e.message);
        }
    }
    for root in projects {
        let at = root.display();
        match server::handle_safely(&mut None, None, None, &root, Request::MergeStatus {}) {
            Ok(a) if a.result["clean"] == true => {}
            Ok(a) => {
                failed = true;
                let list = a.result["conflicts"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                eprintln!(
                    "ryucad: the merge leaves {} conflict(s) in {at}:",
                    list.len()
                );
                for c in &list {
                    eprintln!(
                        "  {} ({}, {}): {}",
                        c["id"].as_str().unwrap_or_default(),
                        c["kind"].as_str().unwrap_or_default(),
                        c["cause"].as_str().unwrap_or_default(),
                        c["message"].as_str().unwrap_or_default()
                    );
                }
            }
            Err(e) if e.code == Code::NotMerging => {}
            Err(e) => {
                failed = true;
                eprintln!("ryucad: checking the merge in {at}: {}", e.message);
            }
        }
        // The check read the work tree; the commit takes the index.
        let unstaged = ryucad::git::Repo::open(&root).and_then(|repo| match repo {
            Some(repo) => repo.unstaged(&ryucad::activity::snapshot(&root)?),
            None => Ok(Vec::new()),
        });
        match unstaged {
            Ok(paths) if paths.is_empty() => {}
            Ok(paths) => {
                failed = true;
                eprintln!(
                    "ryucad: {} file(s) in {at} differ from what would be committed; `git add` (or restore) them first:",
                    paths.len()
                );
                for p in &paths {
                    eprintln!("  {p}");
                }
            }
            Err(e) => {
                failed = true;
                eprintln!("ryucad: reading the index in {at}: {}", e.message);
            }
        }
    }
    if failed {
        eprintln!(
            "ryucad: see `ryucad merge status` and `ryucad release check`, and settle them with `ryucad merge resolve` or by editing the model; `git commit --no-verify` skips this check"
        );
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// `ryucad git merge-driver`: merges, writes over `ours`, and exits 0 when
/// nothing is left to resolve, 1 when conflicts are (in the file's record,
/// or as Git's markers when it fell back to merging lines), 4 when a file
/// can't be read or written. Failures go to stderr: Git shows them.
fn merge_driver(
    base: &std::path::Path,
    ours: &std::path::Path,
    theirs: &std::path::Path,
    path: Option<&str>,
) -> ExitCode {
    let read = |p: &std::path::Path| std::fs::read(p).map_err(|e| Error::io(p, &e));
    let merged = (|| -> Result<ryucad::merge::Outcome> {
        let (out, outcome) = ryucad::merge::bytes(&read(base)?, &read(ours)?, &read(theirs)?);
        std::fs::write(ours, out).map_err(|e| Error::io(ours, &e))?;
        Ok(outcome)
    })();
    match merged {
        Ok(ryucad::merge::Outcome::Clean) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(e) => {
            eprintln!(
                "ryucad: merging {}: {}",
                path.unwrap_or(&ours.to_string_lossy()),
                e.message
            );
            ExitCode::from(Code::Io.exit_code())
        }
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
    // A reader that stops early (`| head`) has what it wanted: a closed
    // pipe isn't worth a panic, and the exit code still says what happened.
    use std::io::Write;
    let _ = writeln!(
        std::io::stdout().lock(),
        "{}",
        s.expect("JSON values always serialize")
    );
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_keeps_c_apart_from_its_folder() {
        // #139: both were clap's argument `dir`, so each filled the other.
        let cli = Cli::try_parse_from(["ryucad", "-C", "project", "open"]).unwrap();
        assert_eq!(cli.dir, Some(PathBuf::from("project")));
        assert!(matches!(&cli.command, Command::Open { dir: None }));
        let cli = Cli::try_parse_from(["ryucad", "-C", "project", "open", "."]).unwrap();
        assert_eq!(cli.dir, Some(PathBuf::from("project")));
        assert!(matches!(&cli.command, Command::Open { dir: Some(d) } if d == &PathBuf::from(".")));
    }
}
