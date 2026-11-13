//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code.

use clap::{Args, Parser, Subcommand};
use ryu_session::job::Job;
use ryucad::doc::project::{find_project, find_root, plain};
use ryucad::error::{Code, Error, Result};
use ryucad::live::{SelectMode, Style};
use ryucad::proto::{
    Answer, FeatureFields, PlacementArg, Position, Request, Transfer, VersionInfo,
};
use ryucad::units::Unit;
use ryucad::view::{CameraArg, ProjectionArg};
use ryucad::{cancel, client, daemon, server};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
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
    /// Print a long request's progress to stderr, a line a second
    /// (`rebuild: 12/40 features (p/fillet1)`); stdout keeps the one JSON
    /// document.
    #[arg(long, global = true)]
    progress: bool,
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
    /// List, set, delete or rename a part's parameters (with --project, the project's,
    /// which every part reads): named values, each an expression such as "3 mm" or
    /// "wall * 2", that features' quantities can read.
    #[command(subcommand)]
    Param(ParamCmd),
    /// Every parameter of the project and of a part (else of every part), and every
    /// expression its features hold (sketch dimensions among them): text, value and what
    /// each reads. With --all, feature fields that are plain values too.
    Equations {
        part: Option<String>,
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        rev: RevArg,
    },
    /// Other ryucad projects whose released parts this one places: add, fetch and list them.
    /// Only `deps add` and `deps fetch` reach the network.
    #[command(subcommand)]
    Deps(DepsCmd),
    /// Inserts of other parts' solids: move one's pin.
    #[command(subcommand)]
    Insert(InsertCmd),
    /// Hole tools that need no model.
    #[command(subcommand)]
    Hole(HoleCmd),
    /// Every hole of a part, patterns and mirrors expanded: its ref, spec, type,
    /// position, axis, sizes, depths, thread, faces and feature.
    Holes {
        part: String,
        /// Only this solid's holes.
        solid: Option<String>,
        /// One row per hole as CSV.
        #[arg(long)]
        csv: bool,
        /// Holes of one spec together, with their count.
        #[arg(long, conflicts_with = "csv")]
        group: bool,
        /// One hole by its reference (p/h/p1, p/row/h/p1/1).
        #[arg(long = "ref", value_name = "REF")]
        reference: Option<String>,
        #[command(flatten)]
        rev: RevArg,
    },
    /// What the hole standards hold, built in and the project's, each value with its
    /// source: ryucad standards threads|fasteners|drills [STANDARD] [SIZE].
    Standards {
        #[arg(value_parser = ["threads", "fasteners", "drills"])]
        table: String,
        /// A standard's key (iso_metric, unc, npt, ...), or a drill series
        /// (metric, fractional, number, letter).
        standard: Option<String>,
        size: Option<String>,
    },
    /// The project's material library: densities (and colours) that mass properties read,
    /// each part's named by its PDM `material` property.
    #[command(subcommand)]
    Materials(MaterialsCmd),
    /// Named views: cameras saved with a part or an assembly, in Git, that
    /// `camera --view` and `render --view` restore.
    #[command(subcommand)]
    View(ViewCmd),
    /// Annotations: requests to the agent, pinned to the model, some
    /// carrying a proposed change.
    #[command(subcommand, long_about = ryucad::annotation::ETIQUETTE)]
    Annotations(AnnotationsCmd),
    /// An assembly's exploded views: list, save, delete, or show one in a
    /// participant's window, part of the way through with --fraction.
    #[command(subcommand)]
    Explode(ExplodeCmd),
    /// An assembly's motions: list, save or delete one. Sweep one with
    /// `assembly sweep`.
    #[command(subcommand)]
    Motion(MotionCmd),
    /// A part's 2D drawings: sheets on a template with a title block, and
    /// views projected with exact hidden lines, written as PDF, SVG and
    /// PNG. Every value a sheet shows comes from the model.
    #[command(subcommand)]
    Drawing(DrawingCmd),
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
    /// Read a path or helix as the app draws it.
    #[command(subcommand)]
    Path(PathCmd),
    /// Read what a sweep or a loft is built from, as the app draws it.
    #[command(subcommand)]
    Sweep(SweepCmd),
    /// Move an assembly's instances, or show its tree of instances.
    #[command(subcommand)]
    Assembly(AssemblyCmd),
    /// Import a STEP file: store it in the project (imports/) and make a part of each
    /// product's bodies and an assembly of each product's children, add one product's
    /// bodies to a part with --into, or replace a file imported before with --update.
    /// `import convert` turns a reference into a healed solid, or back.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Import {
        #[command(subcommand)]
        cmd: Option<ImportCmd>,
        #[command(flatten)]
        args: ImportArgs,
    },
    /// Write a part or an assembly as a file: as the working files have it, as a Git
    /// revision has it (--rev), or as a release (--rev label:<L>). The daemon writes
    /// it, atomically, in a folder that must exist.
    Export {
        part: String,
        /// The file to write; its name goes in a STEP file's header.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// step, stl, 3mf, usda, usdc or usdz; by default the one --out's extension
        /// names (.step, .stp, .stl, .3mf, .usda, .usdc or .usd, .usdz).
        #[arg(long)]
        format: Option<String>,
        /// STEP's schema: ap242 (the default) or ap214.
        #[arg(long)]
        schema: Option<String>,
        /// The length unit: mm (the default), or m; a mesh format or USD also cm; DXF mm
        /// or in.
        #[arg(long)]
        unit: Option<String>,
        /// A mesh format's chord, the most a triangle may stray from the faces: a
        /// literal length (default 0.01 mm).
        #[arg(long, value_name = "LENGTH")]
        chord: Option<String>,
        /// A mesh format's angle, the most a face's normal may turn across a
        /// triangle: a literal angle in (0, 90 deg] (default 15 deg).
        #[arg(long, value_name = "ANGLE")]
        angle: Option<String>,
        /// A mesh format's longest triangle edge: a literal length.
        #[arg(long, value_name = "LENGTH")]
        max_edge: Option<String>,
        /// STL as text, a solid block per placed solid, rather than binary.
        #[arg(long)]
        ascii: bool,
        /// USD without each solid's B-rep (an omniSolid BrepArray): meshes only.
        #[arg(long)]
        no_brep: bool,
        /// USD without each solid's mesh: B-rep only.
        #[arg(long)]
        no_mesh: bool,
        /// USD's tolerance for pipes and sweeps written as NURBS: a literal length
        /// (default 0.0001 mm).
        #[arg(long, value_name = "LENGTH")]
        pipe_tolerance: Option<String>,
        /// Write what built when a feature of what the part places fails (a body
        /// that didn't heal), listing what was left out, rather than refuse.
        #[arg(long)]
        skip_failing: bool,
        /// STEP: write no colours or transparency (by default the
        /// appearance renders show is written as STEP styles).
        #[arg(long)]
        no_appearance: bool,
        /// STEP: write no materials or properties (by default the part's
        /// material, with its density, and every set property is written).
        #[arg(long)]
        no_properties: bool,
        /// DXF: the sheet metal solid (the part's one unless named).
        #[arg(long)]
        solid: Option<String>,
        /// DXF: leave out the bend notes.
        #[arg(long)]
        no_notes: bool,
        /// DXF of a folded solid: the side face held still.
        #[arg(long, value_name = "FACE")]
        fixed: Option<String>,
        /// DXF of a folded solid: a bend's allowance, REF=k_factor:N,
        /// REF=deduction:LENGTH or REF=allowance:LENGTH.
        #[arg(long = "bend", value_name = "REF=FORM:VALUE", value_parser = bend_override)]
        bends: Vec<Value>,
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
    /// 1 when anything fails. Interference only with --interference.
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
        /// Also find which instances of each assembly interfere (slower:
        /// on reference imports, minutes). Without it interference isn't
        /// run, and each assembly says so.
        #[arg(long)]
        interference: bool,
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
        /// A part, or another project's as <dependency>:<part>.
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
        /// Also search the ryucad projects in this folder (a project, or a
        /// folder holding projects) that depend on this one; repeat for
        /// more. Without it, the pdm.dependents preference's folders.
        #[arg(long = "in", value_name = "FOLDER")]
        within: Vec<std::path::PathBuf>,
        /// Search no other projects' folders.
        #[arg(long, conflicts_with = "within")]
        no_dependents: bool,
        /// Ask the review service instead: every repository of the
        /// organization the token can read (review.url and its token).
        #[arg(long, conflicts_with_all = ["within", "no_dependents", "working"])]
        org: bool,
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
    /// A solid read as sheet metal: its thickness, flanges and bend table,
    /// each bend's developed length (the part's one sheet metal solid unless
    /// named).
    Sheet {
        part: String,
        solid: Option<String>,
        #[command(flatten)]
        rev: RevArg,
    },
    /// The faces of the hole, boss or pocket a face belongs to, as a delete_face
    /// --seed takes them: <part>/<solid>/<name>. It is the smallest set a cap
    /// closes, so a counterbored hole's bore is the bore alone; seed from the
    /// counterbore's wall to take it all.
    FeatureFaces {
        face: String,
        #[command(flatten)]
        rev: RevArg,
    },
    /// A solid's fillets, chamfers and ball corners as delete_blend and resize_blend
    /// read them (the part's one solid unless named): each blend's faces, kind and size,
    /// the faces it rolls on and its chain. --below LEN lists only what `feature add
    /// <part> delete_blend --below LEN` deletes.
    Blends {
        part: String,
        solid: Option<String>,
        /// Recognition's tolerance: how far a face may be from a fillet's or chamfer's
        /// form and be read as one (0.01 mm by default; up to 0.1 mm).
        #[arg(long, value_name = "LENGTH")]
        tolerance: Option<String>,
        /// Only the blends delete_blend --below takes at this size.
        #[arg(long, value_name = "LENGTH")]
        below: Option<String>,
        #[command(flatten)]
        rev: RevArg,
    },
    /// A solid's flat pattern (the part's one sheet metal solid unless
    /// named): its outline, holes and bend lines with their notes, in
    /// millimetres. A folded solid is unfolded in memory, about --fixed
    /// (a side face) with each --bend REF=k_factor:N|deduction:LEN|allowance:LEN
    /// overriding the solid's allowance.
    Flat {
        part: String,
        solid: Option<String>,
        #[arg(long, value_name = "FACE")]
        fixed: Option<String>,
        #[arg(long = "bend", value_name = "REF=FORM:VALUE", value_parser = bend_override)]
        bends: Vec<Value>,
        #[command(flatten)]
        rev: RevArg,
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
    /// The colour and opacity of every solid a part or assembly draws (else
    /// the participant's window's, with --of, or the project's model), and
    /// the faces drawn otherwise.
    Appearance {
        part: Option<String>,
        /// The participant whose window's part to give.
        #[arg(long = "of", value_name = "PARTICIPANT", conflicts_with = "part")]
        of: Option<String>,
        #[command(flatten)]
        rev: RevArg,
    },
    /// Colour a part (<part>), its solids (<part>/<solid>) and faces
    /// (<part>/<solid>/<name>), or an assembly's occurrences
    /// (<assembly>/<instance>/…) and their solids: in Git, all in one part.
    /// An assembly alone takes only --no-file-styles and --file-styles.
    Colour {
        #[arg(value_name = "REF", required = true)]
        refs: Vec<String>,
        /// The colour, #rrggbb.
        #[arg(long, value_name = "#RRGGBB", conflicts_with_all = ["material", "clear"])]
        colour: Option<String>,
        /// The part's material's colour (in materials.json).
        #[arg(long, conflicts_with = "clear")]
        material: bool,
        /// The opacity, above 0 and at most 1.
        #[arg(long, value_name = "X", conflicts_with = "clear")]
        opacity: Option<f64>,
        /// Take the colour out, so it comes from what is coloured around it.
        #[arg(long, conflicts_with_all = ["colour", "material", "clear"])]
        no_colour: bool,
        /// Take the opacity out likewise.
        #[arg(long, conflicts_with_all = ["opacity", "clear"])]
        no_opacity: bool,
        /// Draw the part without the colours its imported STEP files give
        /// it (appearance.json's file_styles: false).
        #[arg(long, conflicts_with_all = ["file_styles", "clear"])]
        no_file_styles: bool,
        /// Draw them again.
        #[arg(long, conflicts_with = "clear")]
        file_styles: bool,
        /// Take every entry named out, whether or not the model still has
        /// what it names.
        #[arg(long)]
        clear: bool,
        #[command(flatten)]
        expect: Expect,
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
        #[arg(value_name = "REV", conflicts_with_all = ["off", "worktree"])]
        rev: Option<String>,
        /// Compare with another worktree's working files instead (its
        /// folder): what an agent there has done, committed or not.
        #[arg(long, value_name = "PATH", conflicts_with = "off")]
        worktree: Option<PathBuf>,
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
    /// Cut a participant's window with a section plane (the one human's,
    /// else the caller's; up to three, with --add), take the planes away
    /// with --off, or with no plane say what it cuts with.
    Section {
        /// xy, yz or zx (through the origin, cutting away +z, +x or +y), or
        /// a planar face's reference (cutting away what lies outside it).
        #[arg(value_name = "PLANE", conflicts_with_all = ["origin", "off"])]
        plane: Option<String>,
        /// A plane through this point (three quantities: 0,0,5mm), with
        /// --normal pointing to the side cut away.
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true, requires = "normal", conflicts_with = "off")]
        origin: Option<[String; 3]>,
        #[arg(long, value_name = "X,Y,Z", value_parser = vector, allow_hyphen_values = true, requires = "origin")]
        normal: Option<[f64; 3]>,
        /// Move the plane this far along its normal (before --flip).
        #[arg(long, value_name = "LENGTH", allow_hyphen_values = true)]
        offset: Option<String>,
        /// Cut away the other side.
        #[arg(long)]
        flip: bool,
        /// Add the plane to those the window cuts with, not replace them.
        #[arg(long)]
        add: bool,
        /// Cut with no plane.
        #[arg(long, conflicts_with_all = ["offset", "flip", "add"])]
        off: bool,
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
    /// Set this participant's mode: view, annotate or edit. A person's
    /// window chooses its own; an agent's is shown to others, never
    /// enforced.
    Mode {
        #[arg(value_enum)]
        mode: ModeArg,
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
        /// model, project, participants, activity, merge, pdm, progress or annotations (all by default).
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
        /// An occurrence of an assembly (its path) to mesh; repeat for
        /// several, meshed side by side.
        #[arg(long = "occurrence", value_name = "PATH")]
        occurrences: Vec<String>,
        /// coarse, normal or fine (the user's display quality by default).
        #[arg(long)]
        quality: Option<String>,
        /// Write the meshes to this file as ryucad.mesh.1 instead.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// With --out, write ryucad.mesh.2: about half the size.
        #[arg(long, requires = "out")]
        compact: bool,
        /// Mesh a sheet metal solid's flat body (the part's one unless
        /// named) instead of the model.
        #[arg(long, value_name = "SOLID", num_args = 0..=1, default_missing_value = "")]
        flat: Option<String>,
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
        /// Render this annotation: its view, its pins and its marks.
        #[arg(long, value_name = "ID", conflicts_with_all = ["view", "eye"])]
        annotation: Option<String>,
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
        /// Leave out datum planes, axes and points.
        #[arg(long)]
        no_datums: bool,
        /// Leave out tapped holes' cosmetic thread rings.
        #[arg(long)]
        no_threads: bool,
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
        /// Draw open annotations' pins, numbered.
        #[arg(long)]
        annotations: bool,
        /// Draw every face in the fixed theme, not its appearance.
        #[arg(long)]
        no_appearance: bool,
        /// Cut with a section plane (repeatable, up to three):
        /// PLANE[,offset=LENGTH][,flip], PLANE xy, yz, zx or a planar
        /// face's reference; by default what `--of`'s window cuts with,
        /// as --hidden.
        #[arg(long = "section", value_name = "PLANE", value_parser = ryucad::section::parse_spec, conflicts_with = "no_section")]
        section: Vec<ryucad::section::PlaneArg>,
        /// Cut with no plane, whatever a window cuts with.
        #[arg(long)]
        no_section: bool,
        /// Draw the assembly exploded by this explode; by default what
        /// `--of`'s window shows, as --hidden.
        #[arg(long, value_name = "EXPLODE", conflicts_with = "no_explode")]
        explode: Option<String>,
        /// How far through the explode (1 by default) or the motion (0 by
        /// default), from 0 to 1.
        #[arg(long)]
        fraction: Option<f64>,
        /// Draw no explode, whatever a window shows.
        #[arg(long)]
        no_explode: bool,
        /// Draw the assembly where this motion puts it at --fraction (0
        /// by default); by default what `--of`'s window shows, as
        /// --hidden. Never with --explode.
        #[arg(long, value_name = "MOTION", conflicts_with_all = ["no_motion", "explode"])]
        motion: Option<String>,
        /// Draw no motion, whatever a window shows.
        #[arg(long)]
        no_motion: bool,
        /// Paint the faces that collide at the motion's fraction, and
        /// don't at rest (intersects pairs).
        #[arg(long)]
        interference: bool,
        /// Draw a sheet metal solid's flat pattern (the part's one unless
        /// named) instead of the model.
        #[arg(long, value_name = "SOLID", num_args = 0..=1, default_missing_value = "")]
        flat: Option<String>,
    },
    /// What the merge (or cherry-pick, revert or rebase step) Git is in the
    /// middle of leaves to settle.
    #[command(subcommand)]
    Merge(MergeCmd),
    /// The project's worktrees: one per agent's branch, each a project
    /// folder with its own daemon, all sharing this machine's cache.
    #[command(subcommand)]
    Worktree(WorktreeCmd),
    /// The repository's branches: local ones, and remote-tracking ones
    /// with no local branch of their name, with their upstream, how far
    /// they are ahead of and behind this worktree's head, and the
    /// worktree each is checked out in. Read-only.
    #[command(subcommand)]
    Branch(BranchCmd),
    /// Check out another branch in this project's folder (`git switch`;
    /// a remote's branch gets a local one tracking it): refused while
    /// project files differ from HEAD (commit or stash them in Git first:
    /// ryucad never commits) or a merge is in progress. Everyone attached
    /// sees the files change; the activity log records the switch, which
    /// undo can't take back.
    Switch {
        branch: String,
        /// Fail with `conflict` unless the project is still at this
        /// revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
    /// The project's Git commits (or one part's), and what is
    /// uncommitted. Read-only.
    History {
        part: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Cancel this agent's running long request by its token (what
    /// Ctrl+C sends): it stops before its next feature, body or pair.
    Cancel { token: String },
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
    /// What merging a revision, or another worktree's working files, into this worktree's
    /// working files would leave to settle, as `merge status` would say after the merge:
    /// conflicts, what it would break, sketches and assemblies left stale, PDM faults.
    /// Nothing is merged or written.
    Preview {
        #[arg(
            value_name = "REV",
            required_unless_present = "worktree",
            conflicts_with = "worktree"
        )]
        rev: Option<String>,
        /// Another worktree's folder: its working files, committed or not.
        #[arg(long, value_name = "PATH")]
        worktree: Option<PathBuf>,
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
enum WorktreeCmd {
    /// Make a worktree of the project's repository in a new folder (`git
    /// worktree add`), check out a branch or revision there, and warm its
    /// cache from this machine's: prints the new project folder. With
    /// --as, the participant name the agent working there will use, shown
    /// in the Branches tab until its daemon says who is attached.
    Add {
        /// The new worktree's folder.
        path: PathBuf,
        /// Create this branch there (from --from).
        #[arg(long, short = 'b')]
        branch: Option<String>,
        /// What to check out or start the branch from: HEAD by default.
        #[arg(long, value_name = "REV")]
        from: Option<String>,
        /// Don't rebuild the new worktree to warm its cache.
        #[arg(long)]
        no_warm: bool,
    },
    /// Every worktree holding the project: branch, head, ahead and
    /// behind, uncommitted changes, and who is attached to its daemon (only
    /// if one is running: none is started).
    List,
    /// Stop a worktree's daemon and remove it (`git worktree remove`);
    /// refused while it has uncommitted changes, unless --force.
    Remove {
        path: PathBuf,
        /// Remove it even with uncommitted changes, which are lost.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum BranchCmd {
    /// Every branch, the one checked out here marked `current`.
    List,
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
    /// How each release the pull request proposes stands against its
    /// approval rules, as the service judged it: who approved, who is
    /// still needed, what blocks it. Approve with `gh pr review --approve`.
    Approvals { pr: Option<u64> },
    /// A release's package on the service (design 436): get it (download and verify it
    /// against this clone), have it built, or share it.
    #[command(subcommand)]
    Package(ReviewPackageCmd),
    /// The repository's approval and release events, from the service's
    /// audit log: JSON, or CSV (`--format csv`, or `--out FILE`).
    Audit {
        #[arg(long)]
        part: Option<String>,
        /// From this date or time (2026-11-01, or 2026-11-01T09:00:00Z).
        #[arg(long)]
        since: Option<String>,
        /// Up to this date or time.
        #[arg(long)]
        until: Option<String>,
        #[arg(long, value_parser = ["json", "csv"], default_value = "json")]
        format: String,
        /// Write the CSV to this file.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ReviewPackageCmd {
    /// Download a release's package and verify it against this clone, offline.
    Get {
        part: String,
        label: String,
        /// The zip to write; <part>-<label>.zip here if left out.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Have the service build a release's package that has none, or retry one that hit a
    /// limit (repo admin).
    Build { part: String, label: String },
    /// An expiring link to a release's package for someone without access (repo write).
    Share {
        part: String,
        label: String,
        /// How long it lasts, 1 to 30 days (7 if left out).
        #[arg(long)]
        days: Option<u64>,
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
    /// Store the token sent to the shared cache (`cache.remote`), readable
    /// by you alone; read from standard input without --token.
    /// RYUCAD_CACHE_TOKEN, when set, is sent instead.
    Login {
        #[arg(long)]
        token: Option<String>,
    },
    /// Remove the stored token.
    Logout,
    /// Rebuild a part (with what it places), or every part, through the
    /// disk cache and the shared cache: what the team built comes from the
    /// shared cache, the rest is built here and sent behind.
    Warm { part: Option<String> },
    /// Send every entry this release built that the shared cache lacks:
    /// what a CI job runs after building, so others never build it. Needs
    /// no project.
    Push {
        /// Only this kind: features, tools or meshes.
        #[arg(long, value_name = "KIND", value_parser = cache_kind)]
        kind: Option<ryucad::cache::Kind>,
    },
}

fn cache_kind(s: &str) -> std::result::Result<ryucad::cache::Kind, String> {
    ryucad::cache::Kind::parse(s).ok_or_else(|| format!("{s:?} isn't features, tools or meshes"))
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
    /// through --about (else its origin), in the assembly's frame.
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
        /// The point the turn's axis runs through (with --axis): 10mm,0,0.
        #[arg(long, value_name = "VECTOR", value_parser = triple, allow_hyphen_values = true)]
        about: Option<[String; 3]>,
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
        /// The point the turn's axis runs through (with --axis): 10mm,0,0.
        #[arg(long, value_name = "VECTOR", value_parser = triple, allow_hyphen_values = true)]
        about: Option<[String; 3]>,
        /// Solve only what the instance's mates reach, from the poses the
        /// assembly's rebuild has, as a drag's preview does.
        #[arg(long, requires = "instance")]
        component: bool,
        /// Solve over this revision rather than the working files: a Git
        /// revision, worktree:<path>, annotation:<id> or draft:<name>.
        #[arg(long, value_name = "REV")]
        rev: Option<String>,
        /// A mate to solve with, writing nothing, as JSON of the fields
        /// feature add takes for a mate: {"form": "revolute", "a": REF, "b": REF}.
        #[arg(long = "mate", value_name = "JSON", value_parser = json_object)]
        mates: Vec<Value>,
    },
    /// Sweep a motion, writing nothing: step its drives from start to
    /// end, each step solved from the last, and report where it is
    /// blocked, where anything collides that doesn't at rest
    /// (--interference, the first contact refined), and how close the
    /// moving parts come (--clearance). A saved motion, one mate's angle
    /// (a slider's distance) with --mate --from --to, or --drives.
    Sweep {
        part: String,
        /// A saved motion of the assembly.
        #[arg(conflicts_with_all = ["mate", "drives"], required_unless_present_any = ["mate", "drives"])]
        motion: Option<String>,
        /// Drive this mate's angle (a slider's distance) from --from to --to.
        #[arg(long, requires_all = ["from", "to"], conflicts_with = "drives")]
        mate: Option<String>,
        #[arg(
            long,
            value_name = "QUANTITY",
            allow_hyphen_values = true,
            requires = "mate"
        )]
        from: Option<String>,
        #[arg(
            long,
            value_name = "QUANTITY",
            allow_hyphen_values = true,
            requires = "mate"
        )]
        to: Option<String>,
        /// Drives, as a motion file writes them.
        #[arg(long, value_name = "JSON", value_parser = json_array)]
        drives: Option<Value>,
        /// Steps from start to end, 2 to 1,000 (11 when left out; a saved
        /// motion has its own).
        #[arg(long, conflicts_with = "motion")]
        steps: Option<u32>,
        /// Check each step for instances that collide and don't at rest.
        #[arg(long)]
        interference: bool,
        /// Report each step's closest moving pair.
        #[arg(long)]
        clearance: bool,
        /// How far clearance looks (a length; by default a hundredth of
        /// the assembly's diagonal).
        #[arg(long, value_name = "LENGTH", requires = "clearance")]
        below: Option<String>,
        /// Give each step's moved instances' poses.
        #[arg(long)]
        poses: bool,
        /// Bisections of the first collision and the first stop (10; 0
        /// for none).
        #[arg(long, value_name = "N")]
        refine: Option<u32>,
    },
    /// The instances as a tree: parts, revisions, poses, freedom,
    /// subassemblies.
    Tree {
        part: String,
        /// Start at the subassembly these instances reach: m3/s31.
        #[arg(long)]
        path: Option<String>,
        /// List this many levels of instances (all when left out).
        #[arg(long)]
        depth: Option<u32>,
    },
}

#[derive(Subcommand)]
enum FeatureCmd {
    /// Add a feature: a primitive, its sizes given as --param NAME=QUANTITY
    /// (a cylinder's, cone's or sphere's radius as --radius too): box (x, y,
    /// z), cylinder (radius, height), cone (radius, height, top_radius),
    /// sphere (radius), torus (major_radius, minor_radius); an empty sketch (--plane or a
    /// placement; xy by default), an extrude (--sketch, --length) or a
    /// revolve (--sketch, --axis, --angle), each a solid of its own or,
    /// with --operation, joined to, cut from or intersected with others
    /// (--target); a boolean (--op, --target, --tool) of whole solids; a
    /// fillet (--radius, --edge) or chamfer (--form, --d, --d1, --d2, --angle,
    /// --face, --edge) of a solid's edges, or several sizes (--groups); a
    /// shell (--target, --thickness, --open, --face-thickness, --outward) or
    /// an offset (--target, --distance, --face-distance) of a solid; a
    /// face tweak: move_face (--by translate with --direction and --distance,
    /// rotate with --axis and --angle, or offset with --distance and
    /// --face-distance; --face repeated, --no-tangent) or replace_face (--with,
    /// then the --face values it replaces) or draft (--neutral, or --parting with
    /// --pull; --angle; --face repeated, --face-angle, --flip, --reverse, --no-tangent)
    /// of a solid, keeping every name (offset
    /// moves every face, filling where they part; move_face --by offset moves
    /// only the faces named, keeping the topology); each face tweak takes
    /// --topology change, letting it change the solid's topology, and --blends
    /// rebuild, making again the blends it breaks; a delete_face (--face repeated,
    /// --seed repeated, --heal auto, cap or grow) taking faces out of a solid and
    /// healing each gap (`ryucad feature-faces` shows what a seed takes); a
    /// delete_blend (--below LEN, --face repeated, --tolerance) deleting a solid's
    /// fillets and chamfers by size or by face, their supports growing back (`ryucad
    /// blends` lists them); a resize_blend (--face repeated, with --radius, or --form,
    /// --d, --d1, --d2, --angle and --chamfer-face; or --groups) giving blends a new
    /// size and keeping their names; a
    /// mirror (--solid, --plane, --operation) of a whole solid, joined to
    /// it unless told otherwise; a split (--target, --plane, or --tool
    /// BODY, or --tool-face FACE...; --flip, --keep): the target keeps the
    /// side behind, `--keep behind` trims it, `--keep both` founds what is
    /// in front; a base flange (--sketch, --thickness,
    /// --radius, --k-factor or --deduction, --relief, a bent section's
    /// --depth) founding a sheet metal solid; an edge flange (--edge,
    /// --side, --angle, --length, --measured, --position, --radius,
    /// --relief, --setback, --end-setback, --end-relief) on its straight
    /// edges; an unfold (--target, --fixed-face, --only repeated,
    /// --k-factor, --deduction or --bend-allowance, --bend FACE=FORM:VALUE
    /// repeated) laying a sheet metal solid flat; a refold (--target,
    /// --unfold, --only) folding it back; a fill (--edge, --curve in
    /// order, --tangent after an edge, --tolerance, --angle) spanning
    /// edges and curves; an extract (--face, repeated) of a body's faces as
    /// a sheet; a sew (--sheet, repeated, --tolerance, --no-solid) of
    /// sheets into one body; a thicken (--sheet, or --face repeated;
    /// --front, --back, --face-sides, --operation and --target) of a sheet
    /// or a body's faces into a solid; or a hole (--sketch, --point, --diameter, --depth, --form counterbore or
    /// countersink, --bottom; or --spec, --thread with --pitch, --tpi, --class,
    /// --left-hand, --starts, --thread-depth and --tap-drill, --fastener with
    /// --fastener-fit, --fit) cut from solids (--target); or a pattern of
    /// joins, cuts and holes (--seed): --form linear (--direction,
    /// --spacing, --count, and --direction2 ...), circular (--axis, --count,
    /// --angle) or mirror (--plane), skipping instances (--skip); an insert of
    /// another part's solids (--of <part>, --rev working|label:L, --origin,
    /// --z, --x); a delete_body (--solid, repeated) taking solids out of the
    /// part's result; or, in an assembly, an instance (--of, --rev, --origin, --z,
    /// --x, --fixed), a mate
    /// (--form fastened, revolute, slider, cylindrical, planar or ball, --a, --b,
    /// --a-offset, --b-offset, --angle, --distance, and stops --angle-min, --angle-max,
    /// --distance-min, --distance-max) or an instance pattern (--instance,
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
        /// Check and derive the feature, and write nothing.
        #[arg(long)]
        dry_run: bool,
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
        /// Set an expression field by its JSON pointer, as `ryucad equations` lists it
        /// (repeatable; alone): --expr /params/x="w * 2".
        #[arg(long = "expr", value_name = "POINTER=EXPRESSION", value_parser = param)]
        expressions: Vec<(String, String)>,
        #[command(flatten)]
        expect: Expect,
        /// Check and derive the feature, and write nothing.
        #[arg(long)]
        dry_run: bool,
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
        /// Read the project as this revision has it (`annotation:<id>`,
        /// `draft:<name>`, a Git revision).
        #[arg(long)]
        rev: Option<String>,
    },
    /// The feature types `feature add` takes, or one: the request fields
    /// each takes, the body it needs (solid, sheet, or null for either or
    /// none) and whether it founds one (always, when_new, when_both_kept
    /// or never).
    Types {
        /// One type; every type when left out.
        #[arg(value_name = "TYPE")]
        ty: Option<String>,
    },
}

#[derive(Subcommand)]
enum ParamCmd {
    /// Every parameter: as written, its value, and what reads it.
    List {
        #[arg(required_unless_present = "project")]
        part: Option<String>,
        /// The project's parameters, which every part may read.
        #[arg(long, conflicts_with = "part")]
        project: bool,
        /// Read the project as this revision has it (`annotation:<id>`,
        /// `draft:<name>`, a Git revision).
        #[arg(long)]
        rev: Option<String>,
    },
    /// Set or add parameters: wall="3 mm" hole="wall * 2" n=4 (with --project, the
    /// project's: `param set --project wall="3 mm"`).
    Set {
        #[arg(required_unless_present = "project")]
        part: Option<String>,
        #[arg(value_name = "NAME=EXPRESSION", value_parser = param)]
        values: Vec<(String, String)>,
        /// The project's parameters, which every part may read.
        #[arg(long)]
        project: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete a parameter that nothing reads (with --project, the project's:
    /// `param delete --project wall`).
    Delete {
        #[arg(required_unless_present = "project")]
        part: Option<String>,
        #[arg(required_unless_present = "project")]
        name: Option<String>,
        /// The project's parameters, which every part may read.
        #[arg(long)]
        project: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// Rename a parameter, and every expression that reads it (with --project, the
    /// project's, in every part: `param rename --project wall skin`).
    Rename {
        #[arg(required_unless_present = "project")]
        part: Option<String>,
        #[arg(required_unless_present = "project")]
        from: Option<String>,
        #[arg(required_unless_present = "project")]
        to: Option<String>,
        /// The project's parameters, which every part may read.
        #[arg(long)]
        project: bool,
        #[command(flatten)]
        expect: Expect,
    },
}

/// `--project`'s scope.
fn param_scope(project: bool) -> Option<ryucad::param::Scope> {
    project.then_some(ryucad::param::Scope::Project)
}

/// A param command's positional words with `--project`, which names no
/// part: the first word is the command's next argument. A word left over
/// is sent as the part, so the daemon says to give one or the other.
fn shifted<'a>(
    project: bool,
    part: &'a Option<String>,
    rest: &[&'a Option<String>],
) -> Vec<Option<String>> {
    let words: Vec<Option<String>> = std::iter::once(part)
        .chain(rest.iter().copied())
        .cloned()
        .collect();
    if !project {
        return words;
    }
    let given: Vec<String> = words.iter().flatten().cloned().collect();
    let n = words.len();
    // As many words as the command takes after its part, then any extra
    // as the part.
    let mut out: Vec<Option<String>> = vec![None; n];
    let (extra, args) = if given.len() >= n {
        (Some(given[0].clone()), &given[1..])
    } else {
        (None, &given[..])
    };
    out[0] = extra;
    for (i, w) in args.iter().enumerate() {
        out[i + 1] = Some(w.clone());
    }
    out
}

#[derive(Subcommand)]
// Configure's flags, parsed once per run.
#[allow(clippy::large_enum_variant)]
enum PdmCmd {
    /// Give the project its ID and the default settings (keeps any it has).
    Init,
    /// Give a copied project an ID of its own (a fork): its records under the old ID are read
    /// as the upstream's history, and its own labels continue past them.
    NewId {
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
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
        /// Who must approve releases, as ryupdm.json holds it:
        /// '{"rules":[{"count":1,"from":["alice","bob"],"parts":["*"]}]}'.
        #[arg(long, value_name = "JSON", value_parser = json_object, conflicts_with = "no_approvals")]
        approvals: Option<Value>,
        /// Remove the approval rules.
        #[arg(long)]
        no_approvals: bool,
        /// What the hosted review service does about newer releases of other projects'
        /// parts this project places: none (show a notice) or pr (also open a pull request
        /// moving to them).
        #[arg(long, value_name = "none|pr")]
        bump: Option<String>,
        /// What a release's package holds, comma-separated: bom, dxf, pdf, render, step
        /// (default: every one but dxf).
        #[arg(
            long,
            value_delimiter = ',',
            value_name = "FORMATS",
            conflicts_with = "default_package_formats"
        )]
        package_formats: Option<Vec<String>>,
        /// Back to the default package formats.
        #[arg(long)]
        default_package_formats: bool,
        /// Write even if parts' properties stop fitting the settings.
        #[arg(long)]
        force: bool,
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
}

#[derive(Subcommand)]
enum InsertCmd {
    /// Move an insert's pin to its source's newest official label (or --to working,
    /// --to label:L), saying first what it changes: the inserted solids added, removed
    /// and changed, and the features that would fail.
    Update {
        /// The part holding the insert.
        part: String,
        /// The insert's ID.
        insert: String,
        /// working, label:L (or L), or newest (the default).
        #[arg(long, value_name = "working|label:L|newest")]
        to: Option<String>,
        /// Say what would change, writing nothing.
        #[arg(long)]
        dry_run: bool,
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
}

#[derive(Subcommand)]
enum HoleCmd {
    /// Every size, limit and source a spec derives, and its canonical spec: what
    /// `feature add <part> hole --spec` would build, without a model.
    Derive {
        /// The hole's callout, such as "M6x1.0-6H THRU" or "CBORE ISO4762 M5".
        #[arg(long, value_name = "SPEC")]
        spec: String,
        /// Hole fields as JSON, laid over the spec's ('{"diameter": "6.9 mm"}').
        #[arg(long, value_name = "JSON", value_parser = json_object)]
        fields: Option<Value>,
        /// Read this part's parameters.
        #[arg(long)]
        part: Option<String>,
    },
}

#[derive(Subcommand)]
enum DepsCmd {
    /// Fetch a Git repository and list the ryucad project in it as a dependency.
    Add {
        /// The repository: https://, ssh://, git@host:path, file:// or a path.
        url: String,
        /// The project's folder in the repository (its root when left out).
        #[arg(long, default_value = "")]
        path: String,
        /// The dependency's name (else its folder's or repository's).
        #[arg(long)]
        name: Option<String>,
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
    /// Fetch every dependency's release branch and tags (or the named ones'), and what the lock
    /// reaches through them. Writes nothing in the project.
    Fetch { names: Vec<String> },
    /// Each dependency and transitive project: where it is, whether it is fetched, the labels
    /// the lock holds and which instances place them. Offline.
    List,
    /// Fetch, then move every instance of the dependencies' parts (or the named ones', or one
    /// part's) to its latest official release, and write the lock: what moved and what would
    /// fail after. `--offline` fetches nothing (and with nothing to move, writes the lock again).
    Update {
        names: Vec<String>,
        /// Only instances of this part.
        #[arg(long)]
        part: Option<String>,
        /// To this label rather than the latest.
        #[arg(long, value_name = "LABEL")]
        to: Option<String>,
        /// Use what was fetched; fetch nothing.
        #[arg(long)]
        offline: bool,
        /// Say what would move and what would fail, writing nothing.
        #[arg(long)]
        dry_run: bool,
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
    /// Every instance at a label older than its part's latest official one, this project's
    /// parts and other projects' (as fetched). Offline.
    Outdated,
    /// Everything the assemblies place from other projects (each project, part and label, with
    /// the assemblies, paths and counts placing it), now and in every official release.
    /// Offline; the review service indexes it.
    Uses {
        /// A Git revision rather than the working files.
        #[arg(long)]
        rev: Option<String>,
    },
    /// Take a dependency out of the manifest and the lock, once nothing places its parts.
    Remove {
        /// The dependency's name or project ID.
        name: String,
        /// Fail with `conflict` unless the project is still at this revision.
        #[arg(long = "expect", value_name = "REVISION")]
        expect: Option<String>,
    },
    /// What a dependency has released, as fetched: each part's number, name, kind and labels,
    /// latest first. Offline.
    Parts {
        /// The dependency's name or project ID.
        name: String,
        /// Only this part.
        #[arg(long)]
        part: Option<String>,
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

#[derive(Clone, Copy, clap::ValueEnum)]
enum ModeArg {
    View,
    Annotate,
    Edit,
}

/// Redline marks (design 423): whole as JSON, or by shorthand in the
/// view's plane in the default unit (sent arrows, lines, ellipses,
/// rectangles, then texts, after the JSON's).
#[derive(Args, Clone, Debug, Default)]
struct MarkArgs {
    /// Marks as the file has them, {"part", "list"} (touches optional), or @FILE.
    #[arg(long, value_name = "JSON|@FILE")]
    marks: Option<String>,
    /// The part or assembly the shorthand marks are drawn on.
    #[arg(long, value_name = "PART")]
    model: Option<String>,
    #[arg(long, value_name = "X1,Y1:X2,Y2", allow_hyphen_values = true)]
    arrow: Vec<String>,
    #[arg(long, value_name = "X1,Y1:X2,Y2", allow_hyphen_values = true)]
    line: Vec<String>,
    #[arg(long, value_name = "X1,Y1:X2,Y2", allow_hyphen_values = true)]
    ellipse: Vec<String>,
    #[arg(long, value_name = "X1,Y1:X2,Y2", allow_hyphen_values = true)]
    rectangle: Vec<String>,
    /// A text mark: its anchor and one line of text.
    #[arg(long, value_name = "X,Y=TEXT", allow_hyphen_values = true)]
    text: Vec<String>,
}

impl MarkArgs {
    fn shorthands(&self) -> Vec<ryucad::proto::ShortMark> {
        let mut out = Vec::new();
        for (kind, list) in [
            ("arrow", &self.arrow),
            ("line", &self.line),
            ("ellipse", &self.ellipse),
            ("rectangle", &self.rectangle),
            ("text", &self.text),
        ] {
            out.extend(list.iter().map(|at| ryucad::proto::ShortMark {
                kind: kind.into(),
                at: at.clone(),
            }));
        }
        out
    }

    /// The marks these flags give, if any: `--marks` read (a file after
    /// `@`), the shorthands after them, `--model` naming the part.
    fn read(&self) -> Result<Option<ryucad::proto::MarksArg>> {
        let short = self.shorthands();
        let mut m = match &self.marks {
            None if short.is_empty() => return Ok(None),
            None => ryucad::proto::MarksArg::default(),
            Some(text) => {
                let text = match text.strip_prefix('@') {
                    Some(path) => std::fs::read_to_string(path)
                        .map_err(|e| Error::new(Code::Usage, format!("--marks @{path}: {e}")))?,
                    None => text.clone(),
                };
                serde_json::from_str(&text).map_err(|e| {
                    Error::new(
                        Code::Usage,
                        format!("--marks isn't {{\"part\", \"list\"}} JSON: {e}"),
                    )
                })?
            }
        };
        if let Some(p) = &self.model {
            if !m.part.is_empty() && m.part != *p {
                return Err(Error::new(
                    Code::Usage,
                    "--model and --marks name different parts",
                ));
            }
            m.part = p.clone();
        }
        if m.part.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "marks need --model, or a part in --marks",
            ));
        }
        m.short = short;
        Ok(Some(m))
    }
}

#[derive(Subcommand)]
enum AnnotationsCmd {
    /// Pin a comment to references (or, with --part and --point, to a bare point), with
    /// redline marks if given.
    Add {
        refs: Vec<String>,
        #[arg(long)]
        comment: String,
        /// The clicked point, three quantities: on the one reference, or with --part.
        #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true)]
        point: Option<[String; 3]>,
        /// A bare point's part or assembly.
        #[arg(long, requires = "point")]
        part: Option<String>,
        /// Keep this participant's camera with it.
        #[arg(long)]
        view: bool,
        /// Keep this named view (of the marks' part, else the first reference's) with it.
        #[arg(long = "of", value_name = "VIEW", conflicts_with = "view")]
        of: Option<String>,
        #[command(flatten)]
        marks: MarkArgs,
    },
    /// Propose a change: a request run over your draft, kept, not written.
    Propose {
        /// {"method": …, "params": …}
        #[arg(long, value_name = "JSON")]
        request: String,
        #[arg(long)]
        comment: Option<String>,
        #[arg(long = "pin", value_name = "REF")]
        pins: Vec<String>,
        /// Where it should go roughly, not exactly (a freehand drag).
        #[arg(long)]
        rough: bool,
    },
    /// Every annotation, newest first, with whether it is outdated, stale or applied.
    List {
        #[arg(long = "status", value_name = "STATUS")]
        status: Vec<String>,
        #[arg(long)]
        part: Option<String>,
        #[arg(long, value_name = "NAME")]
        by: Option<String>,
        /// Open and in progress only.
        #[arg(long)]
        open: bool,
    },
    /// One annotation, its pins resolved and its change's files.
    Show { id: String },
    Reply {
        id: String,
        text: String,
        #[arg(long, value_name = "BLAKE3")]
        expect: Option<String>,
    },
    /// Mark it in progress, yours.
    Claim {
        id: String,
        #[arg(long, value_name = "BLAKE3")]
        expect: Option<String>,
    },
    /// Write its proposed change (after those it stands on) and mark it done.
    Apply {
        id: String,
        #[arg(long)]
        keep_open: bool,
        #[arg(long, value_name = "BLAKE3")]
        expect: Option<String>,
    },
    Resolve {
        id: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, value_name = "BLAKE3")]
        expect: Option<String>,
    },
    Reject {
        id: String,
        #[arg(long)]
        reason: String,
        #[arg(long, value_name = "BLAKE3")]
        expect: Option<String>,
    },
    Reopen {
        id: String,
        #[arg(long, value_name = "BLAKE3")]
        expect: Option<String>,
    },
    /// Its author changes the comment or pins while it is open and unclaimed.
    Edit {
        id: String,
        #[arg(long)]
        comment: Option<String>,
        #[arg(long = "pin", value_name = "REF")]
        pins: Vec<String>,
        #[command(flatten)]
        marks: MarkArgs,
        /// Take its marks away.
        #[arg(long, conflicts_with_all = ["marks", "arrow", "line", "ellipse", "rectangle", "text"])]
        no_marks: bool,
        #[arg(long, value_name = "BLAKE3")]
        expect: Option<String>,
    },
    Delete {
        id: String,
        #[arg(long, value_name = "BLAKE3")]
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
enum ExplodeCmd {
    /// The exploded views of an assembly, or of every assembly: each one's
    /// steps as written and as they resolve, with each assembly's revision.
    List { part: Option<String> },
    /// Save an exploded view of an assembly, replacing one of that name.
    Save {
        part: String,
        explode: String,
        /// The steps, as the file writes them:
        /// '[{"instances": ["lid"], "translate": [0, 0, "40 mm"]},
        /// {"instances": ["p1"], "rotate": {"axis": [0, 0, 1], "angle": "90 deg"}}]'.
        #[arg(long, value_name = "JSON", value_parser = json_array)]
        steps: Value,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete an exploded view.
    Delete {
        part: String,
        explode: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Show an exploded view in a participant's window (the one human's,
    /// else the caller's), all the way or --fraction of it; with only
    /// --fraction, move the one shown; --off shows none.
    Show {
        #[arg(conflicts_with = "off", required_unless_present_any = ["off", "fraction"])]
        explode: Option<String>,
        /// The assembly whose explode it is (the window's, else the
        /// project's model).
        #[arg(long, requires = "explode")]
        part: Option<String>,
        /// From 0 (assembled) to 1 (exploded); 1 by default.
        #[arg(long, conflicts_with = "off")]
        fraction: Option<f64>,
        #[arg(long)]
        off: bool,
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
}

#[derive(Subcommand)]
enum MotionCmd {
    /// The motions of an assembly, or of every assembly: each one's steps
    /// and drives as written and as they resolve, with each assembly's
    /// revision.
    List { part: Option<String> },
    /// Save a motion of an assembly, replacing one of that name.
    Save {
        part: String,
        motion: String,
        /// The drives, as the file writes them:
        /// '[{"mate": "hinge", "angle": {"from": "-6 deg", "to": "6 deg"}}]' or
        /// '[{"instances": ["p1"], "rotate": {"axis": {"origin": [0, 0, 0],
        /// "direction": "z"}, "from": "0 deg", "to": "90 deg"}}]'.
        #[arg(long, value_name = "JSON", value_parser = json_array)]
        drives: Value,
        /// Steps from start to end, 2 to 1,000 (11 when left out).
        #[arg(long)]
        steps: Option<u32>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Delete a motion.
    Delete {
        part: String,
        motion: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Show a motion in a participant's window (the one human's, else the
    /// caller's), from its start or --fraction of the way, paused or
    /// with --play; with only --fraction, --play or --pause, change the
    /// one shown (a fraction alone pauses it); --off shows none. A motion
    /// shown takes an explode away.
    Show {
        #[arg(conflicts_with = "off", required_unless_present_any = ["off", "fraction", "play", "pause"])]
        motion: Option<String>,
        /// The assembly whose motion it is (the window's, else the
        /// project's model).
        #[arg(long, requires = "motion")]
        part: Option<String>,
        /// From 0 (its start) to 1 (its end); 0 for a new one.
        #[arg(long, conflicts_with = "off")]
        fraction: Option<f64>,
        /// Play it, back and forth, in the window.
        #[arg(long, conflicts_with_all = ["off", "pause"])]
        play: bool,
        /// Stop playing it where it is.
        #[arg(long, conflicts_with = "off")]
        pause: bool,
        #[arg(long)]
        off: bool,
        #[arg(long = "of", value_name = "PARTICIPANT")]
        of: Option<String>,
    },
    /// Where a motion puts each moved body at even fractions: each
    /// frame's state, drive values, moved instances' poses and offsets.
    /// It only solves; nothing is intersected.
    Frames {
        part: String,
        #[arg(conflicts_with = "drives", required_unless_present = "drives")]
        motion: Option<String>,
        /// Inline drives, as `save --drives` takes them.
        #[arg(long, value_name = "JSON", value_parser = json_array)]
        drives: Option<Value>,
        /// Steps of the inline drives (11 when left out).
        #[arg(long, requires = "drives")]
        steps: Option<u32>,
        /// How many frames, 2 to 1,000 (120 when left out).
        #[arg(long)]
        frames: Option<u32>,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum DrawingCmd {
    /// A part's drawings, or every part's, with each listed part's revision.
    List { part: Option<String> },
    /// A drawing with one sheet; --views standard adds a base front view,
    /// its top and right (first-angle's when the standard says) and an
    /// isometric, laid out.
    Create {
        part: String,
        drawing: String,
        #[arg(long)]
        name: Option<String>,
        /// A built-in template (asme-a to asme-e, iso-a4 to iso-a0) or a
        /// project template's ID; the size's built-in one by default.
        #[arg(long)]
        template: Option<String>,
        /// iso_a0 to iso_a4, ansi_a to ansi_e (ansi_b for ASME, iso_a3 for
        /// ISO by default).
        #[arg(long)]
        size: Option<String>,
        #[arg(long, value_parser = ["standard"])]
        views: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// A batch of ops on a drawing's sheets and views, all or nothing:
    /// '[{"op": "add", "kind": "view", "value": {"type": "projected",
    /// "sheet": "s1", "from": "v1", "side": "left"}}, {"op": "set",
    /// "id": "v1", "fields": {"hidden": "dashed"}}, {"op": "move", "id":
    /// "v1", "at": ["120 mm", "150 mm"]}, {"op": "remove", "id": "v4"}]'.
    Edit {
        part: String,
        drawing: String,
        #[arg(long, value_name = "JSON", value_parser = json_array)]
        ops: Value,
        #[command(flatten)]
        expect: Expect,
    },
    /// Add a view: the same op `edit` sends.
    #[command(subcommand)]
    View(DrawingViewCmd),
    /// Move a view to a centre on its sheet, or a projected or auxiliary
    /// view along its row or column.
    Move {
        part: String,
        drawing: String,
        view: String,
        #[arg(
            long,
            value_name = "X,Y",
            conflicts_with = "offset",
            required_unless_present = "offset"
        )]
        at: Option<String>,
        #[arg(long, value_name = "LENGTH")]
        offset: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Remove a sheet or a view.
    Remove {
        part: String,
        drawing: String,
        id: String,
        #[command(flatten)]
        expect: Expect,
    },
    Delete {
        part: String,
        drawing: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// Give a drawing a new ID.
    Rename {
        part: String,
        drawing: String,
        to: String,
        #[command(flatten)]
        expect: Expect,
    },
    /// The built drawing: sheets, views with their boxes and frames, and
    /// findings; --items adds every item's exact geometry.
    Get {
        part: String,
        drawing: String,
        #[arg(long)]
        rev: Option<String>,
        #[arg(long)]
        sheet: Option<String>,
        #[arg(long)]
        items: bool,
    },
    /// Each sheet in words: its views, what they show, what lies near what.
    Describe {
        part: String,
        drawing: String,
        #[arg(long)]
        rev: Option<String>,
        #[arg(long)]
        sheet: Option<String>,
    },
    /// A sheet as PNG (4 pixels a millimetre unless --scale) or SVG.
    Render {
        part: String,
        drawing: String,
        #[arg(long)]
        out: std::path::PathBuf,
        #[arg(long)]
        sheet: Option<String>,
        #[arg(long, value_parser = ["png", "svg"])]
        format: Option<String>,
        /// Pixels to the millimetre, 0.5 to 40.
        #[arg(long, value_name = "PX_PER_MM")]
        scale: Option<f32>,
        #[arg(long)]
        rev: Option<String>,
    },
    /// Every sheet as one PDF, or one sheet as SVG or DXF, by --out's
    /// extension unless --format; the daemon writes it (never over a
    /// project file).
    Export {
        part: String,
        drawing: String,
        #[arg(long)]
        out: std::path::PathBuf,
        #[arg(long)]
        sheet: Option<String>,
        #[arg(long, value_parser = ["pdf", "svg", "dxf"])]
        format: Option<String>,
        /// A DXF's unit.
        #[arg(long, value_parser = ["mm", "in"])]
        unit: Option<String>,
        /// A Git revision, `label:<L>`, `annotation:<id>` or `draft:<name>`.
        #[arg(long)]
        rev: Option<String>,
    },
    /// The drawing lint: its findings with their fixes and each sheet's
    /// score; --explain CODE gives a rule's entry.
    Lint {
        part: Option<String>,
        drawing: Option<String>,
        #[arg(long)]
        rev: Option<String>,
        #[arg(long)]
        sheet: Option<String>,
        #[arg(long, value_name = "CODE")]
        explain: Option<String>,
    },
    /// Sets the drawing lint's rules' weights and severities
    /// (`default` puts one back), in drawing_lint.json.
    LintConfigure {
        #[arg(long, value_name = "CODE=N")]
        weight: Vec<String>,
        #[arg(long, value_name = "CODE=off|warning|error|default")]
        severity: Vec<String>,
        #[command(flatten)]
        expect: Expect,
    },
    /// Add a dimension or a hole callout: the op `edit` sends.
    #[command(subcommand)]
    Dim(DrawingDimCmd),
    /// Add a note: the op `edit` sends.
    #[command(subcommand)]
    Note(DrawingNoteCmd),
    /// Add a feature control frame or a datum feature: the op `edit`
    /// sends.
    #[command(subcommand)]
    Gdt(DrawingGdtCmd),
    /// Lay the views out again from nothing.
    Layout {
        part: String,
        drawing: String,
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum DrawingViewCmd {
    /// A base view (--orient), one projected from another (--from with
    /// --side), or an auxiliary one (--from with --normal-to).
    Add {
        part: String,
        drawing: String,
        /// Its key (v1, v2, …); the next free one by default.
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value = "s1")]
        sheet: String,
        /// A standard view's name, a named view of the part, or with
        /// --normal-to unset a face's reference to look normal to.
        #[arg(long, conflicts_with = "from")]
        orient: Option<String>,
        /// The part or assembly a base, flat or exploded view draws (the
        /// drawing's own).
        #[arg(long = "of")]
        of: Option<String>,
        #[arg(long, requires = "orient")]
        label_rev: Option<String>,
        #[arg(long, required_unless_present_any = ["orient", "flat"])]
        from: Option<String>,
        /// A section (with --from) on a plane: xy, yz, zx, a planar face's
        /// or a datum plane's reference.
        #[arg(long, requires = "from", conflicts_with_all = ["through", "side", "normal_to", "centre"])]
        cut: Option<String>,
        /// Moves --cut's plane along its normal.
        #[arg(long, requires = "cut", allow_hyphen_values = true)]
        cut_offset: Option<String>,
        /// An offset section (with --from) through these points in order:
        /// X,Y,Z lengths, a vertex's or datum point's reference, or origin.
        #[arg(long, requires = "from", conflicts_with_all = ["side", "normal_to", "centre"])]
        through: Vec<String>,
        /// Sees a section from the other side.
        #[arg(long)]
        flip: bool,
        /// An instance path an assembly's section leaves whole.
        #[arg(long)]
        unsectioned: Vec<String>,
        /// A detail (with --from) about this point, X,Y,Z or a reference.
        #[arg(long, requires_all = ["from", "radius"], conflicts_with_all = ["side", "normal_to"])]
        centre: Option<String>,
        /// A detail's radius in the model.
        #[arg(long, requires = "centre")]
        radius: Option<String>,
        /// A sheet metal solid's flat pattern.
        #[arg(long, conflicts_with_all = ["orient", "from"])]
        flat: bool,
        /// The flat view's solid (the part's one sheet metal solid).
        #[arg(long, requires = "flat")]
        solid: Option<String>,
        /// A flat view without its bend notes.
        #[arg(long, requires = "flat")]
        no_notes: bool,
        /// An exploded view (with --orient) of this explode.
        #[arg(long, requires = "orient")]
        explode: Option<String>,
        /// How far through the explode, 0 to 1 (all of it).
        #[arg(long, requires = "explode")]
        fraction: Option<f64>,
        /// Draws the explode's trails.
        #[arg(long, requires = "explode")]
        trails: bool,
        /// top, bottom, left, right, back, or iso and the cube's corners.
        #[arg(long, requires = "from", conflicts_with = "normal_to")]
        side: Option<String>,
        /// A straight edge or planar face the auxiliary view looks along.
        #[arg(long, requires = "from")]
        normal_to: Option<String>,
        #[arg(long, value_name = "X,Y")]
        at: Option<String>,
        #[arg(long, value_name = "SHEET:MODEL")]
        scale: Option<String>,
        #[arg(long, value_parser = ["off", "dashed"])]
        hidden: Option<String>,
        #[arg(long, value_parser = ["thin", "off", "phantom"])]
        tangent: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum DrawingDimCmd {
    /// A dimension on a view: linear (--a, --b, --along), angular (--a,
    /// --b), radius or diameter (--of), ordinate (--datum, --of,
    /// --along), chamfer or hole_callout (--of). Entities are
    /// `<solid>/<name>` (in an assembly `<instance>/…/<solid>/<name>`) or
    /// a datum's ID.
    Add {
        part: String,
        drawing: String,
        #[arg(long)]
        view: String,
        #[arg(long = "type", value_parser = ["linear", "angular", "radius", "diameter", "ordinate", "chamfer", "hole_callout"])]
        kind: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        a: Option<String>,
        #[arg(long)]
        b: Option<String>,
        #[arg(long)]
        of: Option<String>,
        #[arg(long)]
        datum: Option<String>,
        #[arg(long)]
        vertex: Option<String>,
        #[arg(long, value_parser = ["horizontal", "vertical", "aligned"])]
        along: Option<String>,
        /// Its line's distance from its feature on the sheet (signed).
        #[arg(long, allow_hyphen_values = true)]
        offset: Option<String>,
        /// Its text's place, relative to the view's centre.
        #[arg(long, value_name = "X,Y", allow_hyphen_values = true)]
        at: Option<String>,
        /// Text with <> for the value.
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        suffix: Option<String>,
        /// ± this.
        #[arg(long, value_name = "Q", conflicts_with_all = ["deviation", "limits", "fit", "basic", "reference"])]
        tol: Option<String>,
        /// Upper and lower deviations, stacked.
        #[arg(long, value_name = "UPPER,LOWER", allow_hyphen_values = true)]
        deviation: Option<String>,
        /// Upper and lower deviations, shown as the limits.
        #[arg(long, value_name = "UPPER,LOWER", allow_hyphen_values = true)]
        limits: Option<String>,
        /// An ISO 286 class or fit: H7, g6, H7/g6.
        #[arg(long)]
        fit: Option<String>,
        #[arg(long)]
        basic: bool,
        #[arg(long)]
        reference: bool,
        #[arg(long)]
        precision: Option<u32>,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum DrawingNoteCmd {
    /// A note on a sheet (--sheet) or a view (--view, with an optional
    /// --leader to an entity); fields `{name}` filled as the title
    /// block's.
    Add {
        part: String,
        drawing: String,
        #[arg(long)]
        text: String,
        #[arg(long, conflicts_with = "view")]
        sheet: Option<String>,
        #[arg(long)]
        view: Option<String>,
        #[arg(long, requires = "view")]
        leader: Option<String>,
        #[arg(long, value_name = "X,Y", allow_hyphen_values = true)]
        at: Option<String>,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
}

#[derive(Subcommand)]
enum DrawingGdtCmd {
    /// A feature control frame (--characteristic, --tol, its datums,
    /// --on an entity or --to a dimension), or a datum feature (--letter
    /// --on an entity).
    Add {
        part: String,
        drawing: String,
        #[arg(long)]
        view: String,
        #[arg(long, conflicts_with = "letter", required_unless_present = "letter")]
        characteristic: Option<String>,
        #[arg(long, requires = "characteristic")]
        tol: Option<String>,
        #[arg(long)]
        diameter: bool,
        /// m, l, e, p, f or t.
        #[arg(long)]
        condition: Option<String>,
        /// A datum letter, repeated in order of precedence (A or A:m).
        #[arg(long = "datum")]
        datums: Vec<String>,
        #[arg(long)]
        on: Option<String>,
        #[arg(long, conflicts_with = "on")]
        to: Option<String>,
        /// A datum feature's letter.
        #[arg(long, requires = "on")]
        letter: Option<String>,
        #[arg(long, value_name = "X,Y", allow_hyphen_values = true)]
        at: Option<String>,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        expect: Expect,
    },
}

/// A pair of lengths `A,B` as two quantities.
fn pair_of(text: &str) -> core::result::Result<Value, Error> {
    match text
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [a, b] => Ok(json!([a, b])),
        _ => Err(Error::new(
            Code::Usage,
            format!("{text:?}: expected UPPER,LOWER"),
        )),
    }
}

/// A geometric reference as a drawing writes it: a word (`xy`, `x`,
/// `origin`), `<part>/<feature>` (a datum) or `<part>/<feature>/<name>`.
fn drawing_geom(text: &str) -> core::result::Result<Value, Error> {
    let parts: Vec<&str> = text.split('/').collect();
    match parts.as_slice() {
        [w] => Ok(json!(w)),
        [_, f] => Ok(json!({"feature": f})),
        _ => drawing_ref(text),
    }
}

/// A model point as a drawing writes it: `X,Y,Z` lengths, else a
/// geometric reference.
fn drawing_point(text: &str) -> core::result::Result<Value, Error> {
    let xyz: Vec<&str> = text.split(',').map(str::trim).collect();
    match xyz.as_slice() {
        [x, y, z] => Ok(json!([x, y, z])),
        _ => drawing_geom(text),
    }
}

/// A reference `<part>/<feature>/<name>` as a drawing writes it.
fn drawing_ref(text: &str) -> core::result::Result<Value, Error> {
    let parts: Vec<&str> = text.split('/').collect();
    match parts.as_slice() {
        [_, f, n] => Ok(json!({"feature": f, "name": n})),
        _ => Err(Error::new(
            Code::InvalidParam,
            format!("{text:?} is no reference: <part>/<solid>/<name>"),
        )),
    }
}

/// `X,Y` as two sheet lengths (bare numbers millimetres).
fn sheet_point(text: &str) -> core::result::Result<Value, Error> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    match parts.as_slice() {
        [x, y] => {
            let q = |s: &str| -> Value {
                s.parse::<f64>()
                    .map_or_else(|_| json!(s), |n| json!(format!("{n} mm")))
            };
            Ok(json!([q(x), q(y)]))
        }
        _ => Err(Error::new(
            Code::Usage,
            format!("{text:?}: a point on the sheet is X,Y"),
        )),
    }
}

fn json_array(s: &str) -> core::result::Result<Value, String> {
    match serde_json::from_str::<Value>(s) {
        Ok(v @ Value::Array(_)) => Ok(v),
        _ => Err("expected a JSON array".to_owned()),
    }
}

#[derive(Subcommand)]
enum PropsCmd {
    /// Every property of a part, set or not, with its declaration and faults.
    Show {
        part: String,
        /// Read the project as this revision has it (`annotation:<id>`,
        /// `draft:<name>`, a Git revision).
        #[arg(long)]
        rev: Option<String>,
    },
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
    /// Build an official release's package (design 436) from its official commit alone: the
    /// part's STEP, its drawings, an assembly's BOM, an isometric render and the record, with a
    /// manifest of every file's hash, as one deterministic zip. `release package verify`
    /// checks one someone was sent.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Package {
        #[command(subcommand)]
        cmd: Option<PackageCmd>,
        #[command(flatten)]
        args: PackageArgs,
    },
}

#[derive(clap::Args)]
struct PackageArgs {
    /// The part or assembly.
    #[arg(required = true)]
    part: Option<String>,
    /// The release; the part's latest official one if left out.
    #[arg(long)]
    label: Option<String>,
    /// The zip to write, or a folder (one that exists, or a path ending in /) to write the
    /// package's files into; <number>-<label>.zip here if left out.
    #[arg(long, value_name = "FILE.zip|DIR")]
    out: Option<PathBuf>,
    /// The formats, comma-separated (bom, dxf, pdf, render, step), instead of the project's
    /// packages.formats.
    #[arg(long, value_delimiter = ',')]
    formats: Option<Vec<String>>,
}

#[derive(Subcommand)]
enum PackageCmd {
    /// Check a package someone was sent against this clone, offline: every file against the
    /// manifest, the manifest against the release, and the package tag
    /// ryupdm/<part>/<label>.package when fetched. Fails (exit 1) when it doesn't verify.
    Verify {
        file: PathBuf,
        /// Also build it again and compare the bytes (needs the ryucad that built it).
        #[arg(long)]
        rebuild: bool,
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
        /// Read the project as this revision has it (`annotation:<id>`,
        /// `draft:<name>`, a Git revision).
        #[arg(long)]
        rev: Option<String>,
    },
}

#[derive(Subcommand)]
enum PathCmd {
    /// A path's or helix's polyline in model space (meters), cut as renders cut it, with
    /// whether it closes and its part's revision.
    Get {
        part: String,
        path: String,
        /// How finely it is cut: coarse, normal or fine (the user's display quality by
        /// default).
        #[arg(long)]
        quality: Option<String>,
        /// Read the project as this revision has it (`annotation:<id>`,
        /// `draft:<name>`, a Git revision).
        #[arg(long)]
        rev: Option<String>,
    },
}

#[derive(Subcommand)]
enum SweepCmd {
    /// A sweep's profile and path, or a loft's sections in order (each turned as it built,
    /// each loop from its start, with where that start is), as polylines in model space
    /// (meters), with its part's revision.
    Get {
        part: String,
        /// The sweep or loft.
        feature: String,
        /// How finely it is cut: coarse, normal or fine (the user's display quality by
        /// default).
        #[arg(long)]
        quality: Option<String>,
        /// Read the project as this revision has it (`annotation:<id>`,
        /// `draft:<name>`, a Git revision).
        #[arg(long)]
        rev: Option<String>,
    },
}

/// Sketch ops, parsed from one argument.
#[derive(Clone)]
struct Ops(Vec<Value>);

/// A fillet's or chamfer's groups, parsed from one argument.
#[derive(Clone)]
struct Groups(Value);

#[derive(Args)]
struct ImportArgs {
    /// The STEP file (.step or .stp).
    #[arg(required = true)]
    file: Option<std::path::PathBuf>,
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
    /// Heal each body into a checked solid you can model on, rather than keep it as a
    /// reference (shown, measured and mated to, never changed).
    #[arg(long)]
    solid: bool,
    /// Make one part of every body of the file, each placed where its assemblies put it,
    /// rather than a part per product and an assembly per assembly: a vendor PCBA as one
    /// part of references.
    #[arg(long)]
    flatten: bool,
    /// A folder to look in for the files an assembly's parts are kept in, after the
    /// folder of the file naming each (repeatable, in order).
    #[arg(long, value_name = "DIR")]
    search: Vec<std::path::PathBuf>,
    /// Write nothing: say what it would store and make, and what each body would be
    /// (a reference, or with --solid how it heals).
    #[arg(long)]
    dry_run: bool,
    #[command(flatten)]
    expect: Expect,
}

#[derive(Subcommand)]
enum ImportCmd {
    /// Turn a reference import into a healed solid you can model on, or back with
    /// --reference: one import feature (<part>/<feature>), or every one of a part. Face
    /// names don't change.
    Convert {
        /// <part> or <part>/<feature>.
        of: String,
        /// Make it a reference again (nothing may act on it).
        #[arg(long)]
        reference: bool,
        #[command(flatten)]
        expect: Expect,
    },
    /// What a STEP file the project has imported holds, and what the project made
    /// of it: interpreted, retained, unreferenced and failed instances by category
    /// (design 448). Reads only.
    Report {
        /// The imported file: imports/<name>.step.
        file: String,
        /// Every entity name with its first 64 instances, not only the 8 largest.
        #[arg(long)]
        detail: bool,
    },
}

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
    /// may swap), each part by its ID, or another project's as
    /// <dependency>:<part>=N; --compact numbers its parts 1 to n in their
    /// current order.
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
    /// <part>/<feature>/<name>. A mirror's or split's: any plane: xy, yz, zx, a datum
    /// plane <part>/<datum>, a sketch <part>/<sketch>, or a planar face
    /// <part>/<solid>/<name>.
    #[arg(long)]
    plane: Option<String>,
    /// A planar face to sketch on, the face a chamfer's first distance runs along, or a
    /// planar face whose loops a sweep carries: <part>/<feature>/<name>, as inspect
    /// gives it. Repeat for an extract's faces, a thicken's or a move_face's; a
    /// replace_face's, after the --with they take; or a draft's (repeat for each).
    #[arg(long = "face", value_name = "REFERENCE")]
    face: Vec<String>,
    /// A sketch on a face: a vertex for its origin ("" for the part origin, projected).
    #[arg(long, value_name = "REFERENCE")]
    face_origin: Option<String>,
    /// A sketch on a face: a straight edge for its x ("" for the part's x, projected).
    #[arg(long, value_name = "REFERENCE")]
    face_x: Option<String>,
    /// The sketch an extrude, revolve or sweep sweeps, or a hole drills at.
    #[arg(long)]
    sketch: Option<String>,
    /// An entity on a loop to sweep (a sweep of a face: an edge or vertex of the face,
    /// by name); repeat for several. Every closed loop when left out.
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
    /// (or --reverse false); a draft's pull, turned round.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    reverse: Option<bool>,
    /// Sweep the profile's loops and open chains into a sheet of its own: an
    /// extrude, revolve, sweep or loft (or --surface false).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    surface: Option<bool>,
    /// A revolve's axis: a line of its sketch, or a straight edge in the sketch's plane
    /// (<part>/<feature>/<name>). A circular pattern's: x, y, z, or a straight or
    /// circular edge or a cylindrical or conical face.
    #[arg(long, value_name = "AXIS")]
    axis: Option<String>,
    /// A revolve's angle: 90, "90 deg", or full; a chamfer's angle to its face
    /// (distance_angle). A circular pattern's span, which it needs: 360 spaces --count
    /// evenly round a whole turn, less spans the first instance to the last. A
    /// move_face's turn about its --axis (right-handed; a minus sign turns it back). A
    /// draft's angle from its pull (a positive one tapers a boss inward along it and
    /// opens a hole).
    #[arg(long, allow_hyphen_values = true)]
    angle: Option<String>,
    /// Revolve half the angle each side of the sketch's plane (or --symmetric false).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    symmetric: Option<bool>,
    /// What a primitive, extrude or revolve does with its body: new (a solid of its
    /// own, the default), join, cut or intersect. A mirror's copy's: join (the
    /// default), new, cut or intersect.
    #[arg(long)]
    operation: Option<String>,
    /// A solid a join, cut or intersect acts on, by the feature that founded it; repeat
    /// for several. A boolean's target, a shell's, offset's or split's. The part's one
    /// solid when left out.
    #[arg(long = "target", value_name = "SOLID")]
    targets: Vec<String>,
    /// How a boolean combines its target with its tools: unite, subtract or intersect.
    #[arg(long)]
    op: Option<String>,
    /// A solid a boolean uses as a tool, and consumes; repeat for several.
    /// A split's tool: a sheet or solid it cuts with, never changed.
    #[arg(long = "tool", value_name = "SOLID")]
    tools: Vec<String>,
    /// A face a split cuts with (its faces are taken off as a sheet):
    /// <part>/<solid>/<name>; repeat for several. A split's tool body is
    /// --tool.
    #[arg(long = "tool-face", value_name = "FACE")]
    tool_faces: Vec<String>,
    /// A fillet's radius, or a cylinder's, cone's or sphere's (its radius parameter).
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
    /// --propagate false. A path's: right after a --segment, grow that edge to its
    /// tangent chain.
    #[arg(
        long,
        value_name = "BOOL",
        num_args = 0..=1,
        default_missing_value = "true",
        overrides_with = "propagate"
    )]
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
    /// A hole's spec: "M6x1.0-6H THRU", "1/4-20 UNC-2B .5 DEEP, DRILL .625 DEEP",
    /// "CBORE ISO4762 M5", "DIA 6 H7 THRU".
    #[arg(long, value_name = "SPEC")]
    spec: Option<String>,
    /// A tapped hole's thread, STANDARD:SIZE: iso_metric:M6, unc:1/4, npt:1/8, bspp:G1/4,
    /// or a project standard's; "" takes it away.
    #[arg(long, value_name = "STANDARD:SIZE")]
    thread: Option<String>,
    /// A tapped hole's threads per inch, when not the series' own.
    #[arg(long, value_name = "NUMBER")]
    tpi: Option<String>,
    /// A tapped hole's class: 6H, 5G, 2B.
    #[arg(long = "class", value_name = "CLASS")]
    thread_class: Option<String>,
    /// A left-hand thread (--right-hand takes it back).
    #[arg(long, conflicts_with = "right_hand")]
    left_hand: bool,
    #[arg(long)]
    right_hand: bool,
    /// A multi-start thread's starts.
    #[arg(long, value_name = "N")]
    starts: Option<u32>,
    /// How deep the thread runs from the mouth: a length, or "full".
    #[arg(long, value_name = "QUANTITY")]
    thread_depth: Option<String>,
    /// The tap drill: table (ryucad's rule, or the project's tap drill), roll-form, or
    /// engagement:N (percent).
    #[arg(long, value_name = "table|roll-form|engagement:N")]
    tap_drill: Option<String>,
    /// A clearance hole's screw, STANDARD:SIZE from the project's hole_standards.json;
    /// "" takes it away.
    #[arg(long, value_name = "STANDARD:SIZE")]
    fastener: Option<String>,
    /// The clearance's fit: close, normal or loose.
    #[arg(long, value_name = "FIT")]
    fastener_fit: Option<String>,
    /// The bore's ISO 286 class: H7; "" takes it away.
    #[arg(long, value_name = "CLASS")]
    fit: Option<String>,
    /// A join, cut or hole a pattern repeats, or a delete_face's seed: one face of a
    /// hole, boss or pocket, deleted whole. Repeat for several.
    #[arg(long = "seed", value_name = "FEATURE")]
    seeds: Vec<String>,
    /// A linear pattern's direction: x, y, z, or a straight edge or planar face
    /// <part>/<feature>/<name>. A binormal sweep's: any axis, or a plane's normal.
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
    /// The part an instance places or an insert inserts (another project's as
    /// <dependency>:<part>).
    #[arg(long, value_name = "PART")]
    of: Option<String>,
    /// The revision an instance places or an insert pins: working, or a release
    /// label (L or label:L). An instance defaults to working; an insert to its
    /// source's newest official label, else working.
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
    /// A mate's stop below its free angle, an angle: -6deg ("" to clear it).
    #[arg(long, value_name = "QUANTITY", allow_hyphen_values = true)]
    angle_min: Option<String>,
    /// A mate's stop above its free angle ("" to clear it).
    #[arg(long, value_name = "QUANTITY", allow_hyphen_values = true)]
    angle_max: Option<String>,
    /// A slider's or cylindrical mate's stop below its free distance ("" to clear it).
    #[arg(long, value_name = "QUANTITY", allow_hyphen_values = true)]
    distance_min: Option<String>,
    /// A slider's or cylindrical mate's stop above its free distance ("" to clear it).
    #[arg(long, value_name = "QUANTITY", allow_hyphen_values = true)]
    distance_max: Option<String>,
    /// An instance an instance pattern copies; repeat for several.
    #[arg(long = "instance", value_name = "INSTANCE")]
    instances: Vec<String>,
    /// How a datum is built. A plane: offset, angle, three_points, parallel,
    /// axis_point, normal_to or midplane; an axis: of, two_points, planes or normal; a
    /// point: of, coordinates, along, intersection, projection or midpoint.
    #[arg(long, value_name = "CONSTRUCTION")]
    by: Option<String>,
    /// A datum plane's base plane (offset, angle, parallel), or the end a point along a
    /// curve is measured from (start, end). Geometry is xy, yz, zx, x, y, z, origin,
    /// <part>/<feature>, <part>/<feature>/<name> or <part>/<sketch>/<entity>; a bare
    /// feature ID is the part's.
    #[arg(long, value_name = "REFERENCE")]
    from: Option<String>,
    /// The axis an angle plane turns about.
    #[arg(long, value_name = "AXIS")]
    about: Option<String>,
    /// The point a parallel plane passes through.
    #[arg(long, value_name = "POINT")]
    through: Option<String>,
    /// A datum's curve (the curve a normal_to plane or an along point is
    /// on); a fill's side, in order with --edge.
    #[arg(long = "curve", value_name = "CURVE")]
    curve: Vec<String>,
    /// A fill's or a sew's tolerance (how far its sides, or the sheets'
    /// edges, may be apart), or a blend edit's recognition tolerance (10 µm
    /// when not given).
    #[arg(long, value_name = "LENGTH")]
    tolerance: Option<String>,
    /// A fill's sides as written: --edge, --curve and --tangent in order.
    #[arg(skip)]
    fill_sides: Vec<Value>,
    /// A sheet a sew sews, by its founder; repeat for each. A thicken's one
    /// sheet.
    #[arg(long = "sheet", value_name = "SHEET")]
    sheets: Vec<String>,
    /// A sew that closes up stays a closed sheet, not a solid.
    #[arg(long, conflicts_with = "make_solid")]
    no_solid: bool,
    /// Whether a sew that closes up makes a solid (true by default).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    make_solid: Option<bool>,
    /// A thicken's thickness along its faces' normals.
    #[arg(long, value_name = "LENGTH")]
    front: Option<String>,
    /// A thicken's thickness against its faces' normals.
    #[arg(long, value_name = "LENGTH")]
    back: Option<String>,
    /// A face of a thicken with its own front and back: REFERENCE=FRONT,BACK;
    /// repeat for several. Faces of one pair are one group. Given, they
    /// replace a thicken's groups.
    #[arg(long = "face-sides", value_name = "REFERENCE=FRONT,BACK", value_parser = param)]
    face_sides: Vec<(String, String)>,
    /// Give no face of a thicken a front and back of its own.
    #[arg(long, conflicts_with = "face_sides")]
    no_face_sides: bool,
    /// Where on its curve a normal_to plane is: start, end or a point.
    #[arg(long, value_name = "POINT")]
    at: Option<String>,
    /// A plane a midplane or a planes axis is between, or what an intersection point
    /// meets; repeat for the second.
    #[arg(long = "between", value_name = "REFERENCE")]
    between: Vec<String>,
    /// A coordinates point's position: three quantities, 1mm,2mm,0.
    #[arg(long, value_name = "X,Y,Z", value_parser = triple, allow_hyphen_values = true)]
    coordinates: Option<[String; 3]>,
    /// The plane a coordinates point's coordinates are in ("" for the part's).
    #[arg(long = "in", value_name = "PLANE")]
    in_plane: Option<String>,
    /// How far along its curve an along point is, from 0 to 1.
    #[arg(long, value_name = "NUMBER")]
    fraction: Option<String>,
    /// What a projection point is projected onto: a plane or an axis.
    #[arg(long, value_name = "REFERENCE")]
    onto: Option<String>,
    /// Turn a datum plane's normal, or a datum axis's direction (or --flip false); a
    /// split's plane, turned: the target keeps the side behind it. A draft's: a face it
    /// drafts against its pull, <part>/<solid>/<name>; repeat for each (given, they
    /// replace a draft's flipped faces).
    #[arg(long, value_name = "BOOL|FACE", num_args = 0..=1, default_missing_value = "true")]
    flip: Vec<String>,
    /// Flip none of a draft's faces.
    #[arg(long, conflicts_with = "flip")]
    no_flip: bool,
    /// A draft's neutral plane: xy, yz, zx, a datum plane <part>/<datum>, a sketch, or a
    /// planar face <part>/<solid>/<name>; its normal is the pull (--reverse turns it
    /// round). The faces turn about where the plane crosses them.
    #[arg(long, value_name = "PLANE", conflicts_with = "parting")]
    neutral: Option<String>,
    /// An edge of a draft's parting line, which the faces it bounds turn about:
    /// <part>/<solid>/<name>; repeat for each. Needs --pull.
    #[arg(long = "parting", value_name = "EDGE")]
    parting: Vec<String>,
    /// A parting-line draft's pull: x, y, z, a datum axis, a straight edge, or a plane's
    /// normal (--reverse turns it round).
    #[arg(long, value_name = "DIRECTION")]
    pull: Option<String>,
    /// A face a draft drafts at its own angle: REFERENCE=ANGLE, of either sign; repeat for
    /// several. Faces of one angle are one group. Given, they replace a draft's groups.
    #[arg(
        long = "face-angle",
        value_name = "REFERENCE=ANGLE",
        value_parser = param,
        allow_hyphen_values = true,
        conflicts_with_all = ["face_thickness", "face_distance"]
    )]
    face_angle: Vec<(String, String)>,
    /// A path's segment, in order; repeat for each: an edge <part>/<solid>/<name>
    /// (--propagate right after it grows it to its tangent chain), a sketch's line, arc
    /// or circle <part>/<sketch>/<entity>, or a path or helix (<part>/<feature> or its
    /// ID). Given, they replace a path's segments whole.
    #[arg(long = "segment", value_name = "CURVE")]
    segments: Vec<String>,
    /// Which --segments a --propagate follows, from the command line as written.
    #[arg(skip)]
    grown: Vec<bool>,
    /// Where a helix starts: a point, its distance from --axis the radius (in place of
    /// --radius).
    #[arg(long, value_name = "POINT")]
    start: Option<String>,
    /// A helix's rise per turn, or a tapped hole's pitch.
    #[arg(long, value_name = "QUANTITY")]
    pitch: Option<String>,
    /// A helix's number of turns.
    #[arg(long, value_name = "NUMBER")]
    turns: Option<String>,
    /// A helix's height along its axis.
    #[arg(long, value_name = "QUANTITY")]
    height: Option<String>,
    /// A helix's hand: right (the default) or left.
    #[arg(long, value_name = "HAND")]
    hand: Option<String>,
    /// A helix's taper, an angle: positive widens along its axis ("" for none).
    #[arg(long, value_name = "ANGLE", allow_hyphen_values = true)]
    taper: Option<String>,
    /// What a sweep follows: a path or helix (<part>/<feature> or its ID), an edge
    /// <part>/<solid>/<name>, or a sketch's line, arc or circle <part>/<sketch>/<entity>.
    /// Put the profile across the path: a sketch on a datum plane normal_to the path at
    /// its start is the usual way.
    #[arg(long, value_name = "CURVE")]
    path: Option<String>,
    /// How a sweep carries its profile: minimal (rotation-minimizing) or binormal (keeping
    /// --direction in the profile's plane). Left out of a new sweep: minimal, or along
    /// exactly one helix binormal about its axis.
    #[arg(long, value_name = "LAW")]
    law: Option<String>,
    /// How far a sweep turns its profile about the path, start to end: 90, "720 deg"
    /// ("" for none). Round a closed path, whole turns only.
    #[arg(long, value_name = "ANGLE", allow_hyphen_values = true)]
    twist: Option<String>,
    /// A sweep's end size over its start size: 0.5 ("" for 1).
    #[arg(long, value_name = "NUMBER")]
    scale: Option<String>,
    /// A loft's section, in order; repeat for each: a sketch (its ID or <part>/<sketch>)
    /// or a planar face <part>/<feature>/<name>. A --loop right after a section names one
    /// of its loops and where it starts (a point or vertex, or a curve or edge at its
    /// first point); repeat for each loop, in the order sections match them. Given, they
    /// replace a loft's sections whole. Sketches on datum planes are the usual sections.
    #[arg(long = "section", value_name = "SECTION")]
    sections: Vec<String>,
    /// Which --loops follow each --section, from the command line as written.
    #[arg(skip)]
    section_loops: Vec<Vec<String>>,
    /// Loft round from the last section back to the first (--closed false to open it).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    closed: Option<bool>,
    /// Which way a loft leaves its first section: normal (its own normal), or an axis or
    /// a plane's normal ("" for no tangent).
    #[arg(long, value_name = "DIRECTION")]
    start_tangent: Option<String>,
    /// A loft's start tangent's weight, times the distance between its end sections:
    /// 1.5 ("" for 1).
    #[arg(long, value_name = "NUMBER")]
    start_weight: Option<String>,
    /// Which way a loft enters its last section, as --start-tangent.
    #[arg(long, value_name = "DIRECTION")]
    end_tangent: Option<String>,
    /// A loft's end tangent's weight, as --start-weight.
    #[arg(long, value_name = "NUMBER")]
    end_weight: Option<String>,
    /// A shell's wall thickness.
    #[arg(long, value_name = "QUANTITY")]
    thickness: Option<String>,
    /// Shell outwards: the wall grows out of the solid (--direction inward goes back).
    #[arg(long, conflicts_with = "direction")]
    outward: bool,
    /// A face a shell leaves open: <part>/<solid>/<name>; repeat for several. Given, they
    /// replace a shell's open faces.
    #[arg(long = "open", value_name = "REFERENCE")]
    open: Vec<String>,
    /// Leave no face of a shell open: a closed shell (a cavity, inwards).
    #[arg(long, conflicts_with = "open")]
    no_open: bool,
    /// A face of a shell with a wall of its own: REFERENCE=QUANTITY; repeat for several.
    /// Faces of one thickness are one group. Given, they replace a shell's groups.
    #[arg(long = "face-thickness", value_name = "REFERENCE=QUANTITY", value_parser = param)]
    face_thickness: Vec<(String, String)>,
    /// A face an offset moves by its own distance: REFERENCE=QUANTITY, of either sign;
    /// repeat for several. Faces of one distance are one group. Given, they replace an
    /// offset's groups; a move_face's (--by offset) likewise.
    #[arg(
        long = "face-distance",
        value_name = "REFERENCE=QUANTITY",
        value_parser = param,
        allow_hyphen_values = true,
        conflicts_with = "face_thickness"
    )]
    face_distance: Vec<(String, String)>,
    /// Give no face of a shell or offset a thickness or distance of its own, nor of a
    /// draft an angle of its own.
    #[arg(long, conflicts_with_all = ["face_thickness", "face_distance", "face_angle"])]
    no_faces: bool,
    /// Move or draft a face tweak's tangent neighbours with the faces it names (the default;
    /// --tangent false or --no-tangent keeps the kernel's tangency_broken). In a
    /// fill, the --edge just before meets its face tangentially (the edge must have
    /// one face).
    #[arg(
        long,
        value_name = "BOOL",
        num_args = 0..=1,
        default_missing_value = "true",
        conflicts_with = "no_tangent"
    )]
    tangent: Option<bool>,
    /// Don't move a face tweak's tangent neighbours: a fillet beside a moved face is
    /// refused (tangency_broken).
    #[arg(long)]
    no_tangent: bool,
    /// Let a face tweak change the solid's topology: keep (the default) or change, which
    /// lets faces grow over their neighbours, shrink away or split, and edges and vertices
    /// come and go.
    #[arg(long, value_name = "KEEP|CHANGE")]
    topology: Option<String>,
    /// Make again the fillets and chamfers a face tweak breaks: keep (the default) or
    /// rebuild.
    #[arg(long, value_name = "KEEP|REBUILD")]
    blends: Option<String>,
    /// How a delete_face heals each gap: auto (a cap where one closes it, else its
    /// neighbours grow; the default), cap or grow.
    #[arg(long, value_name = "HOW")]
    heal: Option<String>,
    /// A delete_blend's size: every fillet and chamfer at most this length goes (a
    /// fillet's radius, a chamfer's larger distance), closed over the chains and corners
    /// it reaches (`ryucad blends <part> --below LEN` lists them).
    #[arg(long, value_name = "LENGTH")]
    below: Option<String>,
    /// A resize_blend's chamfer group: the face its --d1 (or its --d with --angle) runs
    /// along, one of the chamfer's two supports. --face names the blends it resizes.
    #[arg(long, value_name = "REFERENCE")]
    chamfer_face: Option<String>,
    /// The surface a replace_face gives the --face values after it, until the next
    /// --with: a face <part>/<solid>/<name> (its surface whole), or a plane (xy, a datum
    /// plane <part>/<datum>, a sketch).
    #[arg(long = "with", value_name = "REFERENCE")]
    with: Vec<String>,
    /// Which --face values follow each --with, from the command line as written.
    #[arg(skip)]
    with_groups: Vec<(String, Vec<String>)>,
    /// The solid a mirror copies, by the feature that founded it (the part's one solid
    /// when left out). Left without --operation, its copy joins this solid. A
    /// delete_body's solids, repeated: a founder, <insert>/<solid>, or an insert for
    /// every solid it inserted.
    #[arg(long = "solid", value_name = "SOLID")]
    solid: Vec<String>,
    /// Which sides of its plane a split keeps: both (the side in front founds a solid
    /// named by the split, the default) or behind (a trim).
    #[arg(long, value_name = "SIDES")]
    keep: Option<String>,
    /// A base flange's or unfold's K-factor, 0 to 1: its bend allowance.
    #[arg(long, value_name = "NUMBER", conflicts_with = "deduction")]
    k_factor: Option<String>,
    /// A base flange's or unfold's bend deduction instead: how much shorter the flat
    /// is than its flanges to their outer mould line.
    #[arg(long, value_name = "LENGTH")]
    deduction: Option<String>,
    /// A sheet metal relief: none, rectangular, obround or tear, sized by
    /// --relief-width and --relief-depth. A flange's "" takes its founder's.
    #[arg(long, value_name = "KIND")]
    relief: Option<String>,
    #[arg(long, value_name = "LENGTH")]
    relief_width: Option<String>,
    #[arg(long, value_name = "LENGTH")]
    relief_depth: Option<String>,
    /// Where a flange's length is measured from: outer_mould (the default),
    /// inner_mould or tangent.
    #[arg(long, value_name = "FROM")]
    measured: Option<String>,
    /// Where a flange's bend sits against its edge: material_inside (the
    /// default), material_outside or bend_outside.
    #[arg(long, value_name = "POSITION")]
    position: Option<String>,
    /// A flange's setback at both ends of every edge.
    #[arg(long, value_name = "LENGTH")]
    setback: Option<String>,
    /// The side face an edge of a flange folds towards: EDGE=FACE,
    /// repeated (with --edge, which lists the edges).
    #[arg(long = "side", value_name = "EDGE=FACE", value_parser = param)]
    sides: Vec<(String, String)>,
    /// An end vertex's own setback: VERTEX=LENGTH, repeated.
    #[arg(long = "end-setback", value_name = "VERTEX=LENGTH", value_parser = param)]
    end_setbacks: Vec<(String, String)>,
    /// An end vertex's own relief: VERTEX=KIND[,WIDTH[,DEPTH]], repeated.
    #[arg(long = "end-relief", value_name = "VERTEX=RELIEF", value_parser = param)]
    end_reliefs: Vec<(String, String)>,
    /// Give no end vertex a setback or relief of its own.
    #[arg(long, conflicts_with_all = ["end_setbacks", "end_reliefs"])]
    no_ends: bool,
    /// The side face an unfold holds still (`<part>/<solid>/<name>`); ""
    /// for the kernel's default flange.
    #[arg(long, value_name = "FACE")]
    fixed_face: Option<String>,
    /// A bend an unfold or refold acts on, by one of its faces, repeated;
    /// every bend when left out.
    #[arg(long, value_name = "FACE")]
    only: Vec<String>,
    /// Act on every bend again.
    #[arg(long, conflicts_with = "only")]
    no_only: bool,
    /// An unfold's developed length for every bend it unfolds, instead of
    /// a K-factor or a deduction.
    #[arg(long, value_name = "LENGTH", conflicts_with_all = ["k_factor", "deduction"])]
    bend_allowance: Option<String>,
    /// One bend's allowance in an unfold: FACE=k_factor:0.4,
    /// FACE=deduction:2mm or FACE=allowance:2.1mm, repeated.
    #[arg(long = "bend", value_name = "FACE=FORM:VALUE", value_parser = bend_override)]
    bends: Vec<Value>,
    /// Give no bend of an unfold its own allowance.
    #[arg(long, conflicts_with = "bends")]
    no_bends: bool,
    /// The unfold a refold folds back by.
    #[arg(long, value_name = "UNFOLD")]
    unfold: Option<String>,
}

/// Which `--segment`s each `--propagate` follows, in order, read from the
/// command line as written (clap keeps only the last `--propagate`): one
/// flag per `--segment`. A `--propagate` before the first `--segment` of
/// a command that has some is a usage error.
fn grown(args: &[String]) -> Result<Vec<bool>> {
    let (mut out, mut early) = (Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        if a == "--segment" {
            out.push(false);
            i += 2;
            continue;
        }
        if a.starts_with("--segment=") {
            out.push(false);
        } else if a == "--propagate" || a.starts_with("--propagate=") {
            let v = match a.strip_prefix("--propagate=") {
                Some(x) => x != "false",
                None => match args.get(i + 1).map(String::as_str) {
                    Some(x @ ("true" | "false")) => {
                        i += 1;
                        x == "true"
                    }
                    _ => true,
                },
            };
            match out.last_mut() {
                Some(last) => *last = v,
                None => early = true,
            }
        }
        i += 1;
    }
    if early && !out.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "--propagate goes right after the --segment it grows",
        ));
    }
    Ok(out)
}

/// Which `--loop`s follow each `--section`, in order, read from the
/// command line as written: a loft's loops narrow the section before
/// them. A `--loop` before the first `--section` of a command that has
/// some is a usage error.
/// A fill's sides from the command line as written: each --edge or --curve
/// a side in order, --tangent qualifying the --edge just before it.
fn fill_sides(args: &[String]) -> Result<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        let value = |flag: &str| -> Option<(String, usize)> {
            if a == flag {
                args.get(i + 1).map(|v| (v.clone(), 2))
            } else {
                a.strip_prefix(&format!("{flag}="))
                    .map(|v| (v.to_owned(), 1))
            }
        };
        if let Some((v, step)) = value("--edge") {
            out.push(json!({"edge": v}));
            i += step;
            continue;
        }
        if let Some((v, step)) = value("--curve") {
            out.push(json!({"curve": v}));
            i += step;
            continue;
        }
        // --tangent [<BOOL>] or --tangent=<BOOL>: a fill side's after its
        // --edge, a face tweak's (clap reads it) when no --edge is given
        // (#674).
        let tangent = if a == "--tangent" {
            match args.get(i + 1).map(String::as_str) {
                Some(v @ ("true" | "false")) => Some((v == "true", 2)),
                _ => Some((true, 1)),
            }
        } else {
            a.strip_prefix("--tangent=").map(|v| (v == "true", 1))
        };
        if let Some((on, step)) = tangent {
            let edge_later = || {
                args[i + step..]
                    .iter()
                    .take_while(|x| *x != "--")
                    .any(|x| x == "--edge" || x.starts_with("--edge="))
            };
            match out.last_mut() {
                Some(last) if last.get("edge").is_some() => {
                    if on {
                        last["tangent"] = json!(true);
                    }
                }
                None if !edge_later() => {}
                _ => {
                    return Err(Error::new(
                        Code::Usage,
                        "--tangent goes right after the --edge it qualifies",
                    ));
                }
            }
            i += step;
            continue;
        }
        i += 1;
    }
    Ok(out)
}

/// The passes over a feature write's command line as written, which clap
/// can't make: grown edges, a loft's loops, a fill's sides, --with groups.
fn read_in_order(args: &[String], fields: &mut FieldArgs) -> Result<()> {
    fields.grown = grown(args)?;
    fields.section_loops = section_loops(args)?;
    fields.fill_sides = fill_sides(args)?;
    // A fill's --tangent is its sides', not a face tweak's.
    if !fields.fill_sides.is_empty() {
        fields.tangent = None;
    }
    fields.with_groups = with_groups(args)?;
    Ok(())
}

fn section_loops(args: &[String]) -> Result<Vec<Vec<String>>> {
    let (mut out, mut early): (Vec<Vec<String>>, bool) = (Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        let value = |flag: &str| -> Option<(String, usize)> {
            if a == flag {
                args.get(i + 1).map(|v| (v.clone(), 2))
            } else {
                a.strip_prefix(&format!("{flag}="))
                    .map(|v| (v.to_owned(), 1))
            }
        };
        if let Some((_, step)) = value("--section") {
            out.push(Vec::new());
            i += step;
            continue;
        }
        if let Some((v, step)) = value("--loop") {
            match out.last_mut() {
                Some(last) => last.push(v),
                None => early = true,
            }
            i += step;
            continue;
        }
        i += 1;
    }
    if early && !out.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "--loop goes right after the --section it narrows",
        ));
    }
    Ok(out)
}

fn json_object(s: &str) -> core::result::Result<Value, String> {
    match serde_json::from_str::<Value>(s) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err("expected a JSON object".to_owned()),
    }
}

impl FieldArgs {
    fn to_fields(&self) -> FeatureFields {
        // A fill's sides given in order (--tangent after an --edge, several
        // --curve, or --curve beside --edge) go as sides; a lone --curve is
        // a datum's, --edge alone a blend's edges.
        let ordered = (self.tangent == Some(true) && !self.edges.is_empty())
            || self.curve.len() > 1
            || (!self.curve.is_empty() && !self.edges.is_empty());
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
            spec: self.spec.clone(),
            thread: self.thread.clone().map(Value::String),
            fastener: self.fastener.clone().map(Value::String),
            fastener_fit: self.fastener_fit.clone(),
            fit: self.fit.clone(),
            tpi: self.tpi.clone().map(Value::String),
            class: self.thread_class.clone(),
            starts: self.starts,
            thread_depth: self.thread_depth.clone().map(Value::String),
            tap_drill: self.tap_drill.clone(),
            plane: self.plane.clone(),
            neutral: self.neutral.clone(),
            parting: (!self.parting.is_empty()).then(|| self.parting.clone()),
            pull: self.pull.clone(),
            // One --face is a chamfer's, a sketch's or a sweep's `face`;
            // several are a move's `faces`, beside any --face-distance
            // groups; with --with they are a replacement's groups.
            face: (self.with_groups.is_empty() && self.face.len() == 1)
                .then(|| self.face[0].clone()),
            face_list: None,
            tolerance: self.tolerance.clone().map(Value::String),
            sheets: (!self.sheets.is_empty()).then(|| self.sheets.clone()),
            sheet: None,
            make_solid: if self.no_solid {
                Some(false)
            } else {
                self.make_solid
            },
            front: self.front.clone().map(Value::String),
            back: self.back.clone().map(Value::String),
            from_faces: None,
            face_sides: if self.no_face_sides {
                Some(json!([]))
            } else if self.face_sides.is_empty() {
                None
            } else {
                // Faces of one pair are one group, in the order first named.
                let mut groups: Vec<(String, String, Vec<String>)> = Vec::new();
                for (face, pair) in &self.face_sides {
                    let (front, back) = pair.split_once(',').unwrap_or((pair.as_str(), "0"));
                    match groups.iter_mut().find(|(f, b, _)| f == front && b == back) {
                        Some((_, _, fs)) => fs.push(face.clone()),
                        None => {
                            groups.push((front.to_owned(), back.to_owned(), vec![face.clone()]))
                        }
                    }
                }
                Some(Value::Array(
                    groups
                        .into_iter()
                        .map(|(f, b, fs)| json!({"front": f, "back": b, "faces": fs}))
                        .collect(),
                ))
            },
            face_origin: self.face_origin.clone(),
            face_x: self.face_x.clone(),
            sketch: self.sketch.clone(),
            // A loft's loops are its sections'.
            loops: if !self.sections.is_empty() {
                None
            } else if self.all_loops {
                Some(Vec::new())
            } else {
                (!self.loops.is_empty()).then(|| self.loops.clone())
            },
            extent: (!extent.is_empty()).then_some(Value::Object(extent)),
            reverse: self.reverse,
            surface: self.surface,
            axis: self.axis.clone(),
            angle: self.angle.clone().map(Value::String),
            symmetric: self.symmetric,
            operation: self.operation.clone(),
            targets: (!self.targets.is_empty()).then(|| self.targets.clone()),
            op: self.op.clone(),
            target: None,
            tools: (!self.tools.is_empty()).then(|| self.tools.clone()),
            tool: None,
            tool_faces: (!self.tool_faces.is_empty()).then(|| self.tool_faces.clone()),
            groups: if self.with_groups.is_empty() {
                self.groups.clone().map(|g| g.0)
            } else {
                Some(Value::Array(
                    self.with_groups
                        .iter()
                        .map(|(w, fs)| json!({"with": w, "faces": fs}))
                        .collect(),
                ))
            },
            // A path's --propagate is its segments'.
            propagate: self.propagate.filter(|_| self.segments.is_empty()),
            tangent: if self.no_tangent {
                Some(false)
            } else {
                self.tangent
            },
            topology: self.topology.clone(),
            blends: self.blends.clone(),
            heal: self.heal.clone(),
            below: self.below.clone().map(Value::String),
            chamfer_face: self.chamfer_face.clone(),
            seeds: None,
            radius: self.radius.clone().map(Value::String),
            edges: (!self.edges.is_empty() && !ordered).then(|| self.edges.clone()),
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
            direction: if self.outward {
                Some("outward".to_owned())
            } else {
                self.direction.clone()
            },
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
            rev: self
                .rev
                .as_deref()
                .map(|r| r.strip_prefix("label:").unwrap_or(r).to_owned()),
            fixed: self.fixed,
            a: self.a.clone(),
            b: self.b.clone(),
            a_offset: self.a_offset.clone(),
            b_offset: self.b_offset.clone(),
            distance: self.distance.clone().map(Value::String),
            limits: {
                let mut l = Map::new();
                for (value, lo, hi) in [
                    ("angle", &self.angle_min, &self.angle_max),
                    ("distance", &self.distance_min, &self.distance_max),
                ] {
                    let mut r = Map::new();
                    if let Some(x) = lo {
                        r.insert("min".into(), json!(x));
                    }
                    if let Some(x) = hi {
                        r.insert("max".into(), json!(x));
                    }
                    if !r.is_empty() {
                        l.insert(value.into(), Value::Object(r));
                    }
                }
                (!l.is_empty()).then_some(Value::Object(l))
            },
            instances: (!self.instances.is_empty()).then(|| self.instances.clone()),
            point: None,
            by: self.by.clone(),
            from: self.from.clone(),
            about: self.about.clone(),
            through: self.through.clone(),
            curve: (self.curve.len() == 1 && !ordered).then(|| self.curve[0].clone()),
            at: self.at.clone(),
            between: (!self.between.is_empty()).then(|| self.between.clone()),
            coordinates: self
                .coordinates
                .clone()
                .map(|c| c.into_iter().map(Value::String).collect()),
            in_plane: self.in_plane.clone(),
            fraction: self.fraction.clone().map(Value::String),
            onto: self.onto.clone(),
            origin: None,
            x: None,
            flip: flip_of(&self.flip, self.no_flip),
            segments: (!self.segments.is_empty()).then(|| {
                self.segments
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        if self.grown.get(i).copied().unwrap_or(false) {
                            json!({"curve": t, "propagate": true})
                        } else {
                            json!(t)
                        }
                    })
                    .collect()
            }),
            start: self.start.clone(),
            pitch: self.pitch.clone().map(Value::String),
            turns: self.turns.clone().map(Value::String),
            height: self.height.clone().map(Value::String),
            // A helix's --hand, or a tapped hole's --left-hand/--right-hand.
            hand: if self.left_hand {
                Some("left".into())
            } else if self.right_hand {
                Some("right".into())
            } else {
                self.hand.clone()
            },
            taper: self.taper.clone().map(Value::String),
            path: self.path.clone(),
            law: self.law.clone(),
            twist: self.twist.clone().map(Value::String),
            scale: self.scale.clone().map(Value::String),
            sections: (!self.sections.is_empty()).then(|| {
                self.sections
                    .iter()
                    .enumerate()
                    .map(|(i, t)| match self.section_loops.get(i) {
                        Some(l) if !l.is_empty() => json!({"section": t, "loops": l}),
                        _ => json!(t),
                    })
                    .collect()
            }),
            closed: self.closed,
            start_tangent: self.start_tangent.clone(),
            start_weight: self.start_weight.clone().map(Value::String),
            end_tangent: self.end_tangent.clone(),
            end_weight: self.end_weight.clone().map(Value::String),
            thickness: self.thickness.clone().map(Value::String),
            open: if self.no_open {
                Some(Vec::new())
            } else {
                (!self.open.is_empty()).then(|| self.open.clone())
            },
            faces: if self.no_faces {
                Some(json!([]))
            } else {
                let mut list: Vec<Value> = if self.with_groups.is_empty() && self.face.len() > 1 {
                    self.face.iter().map(|f| json!(f)).collect()
                } else {
                    Vec::new()
                };
                let groups = if !self.face_thickness.is_empty() {
                    face_groups(&self.face_thickness, "thickness")
                } else if !self.face_distance.is_empty() {
                    face_groups(&self.face_distance, "distance")
                } else if !self.face_angle.is_empty() {
                    face_groups(&self.face_angle, "angle")
                } else {
                    json!([])
                };
                list.extend(groups.as_array().into_iter().flatten().cloned());
                (!list.is_empty()).then_some(Value::Array(list))
            },
            solid: (self.solid.len() == 1).then(|| self.solid[0].clone()),
            // Two or more are a delete_body's; one goes as `solid`, which a
            // delete takes as its one.
            solids: (self.solid.len() > 1).then(|| self.solid.clone()),
            keep: self.keep.clone(),
            allowance: self
                .k_factor
                .clone()
                .map(|k| json!({"k_factor": k}))
                .or_else(|| self.deduction.clone().map(|d| json!({"deduction": d})))
                .or_else(|| self.bend_allowance.clone().map(|a| json!({"allowance": a}))),
            relief: self.relief.as_deref().map(|k| match k {
                "" => json!(""),
                k => relief_json(
                    k,
                    self.relief_width.as_deref(),
                    self.relief_depth.as_deref(),
                ),
            }),
            length: None,
            measured: self.measured.clone(),
            position: self.position.clone(),
            setback: self.setback.clone().map(Value::String),
            sides: if !self.sides.is_empty() {
                Some(json!(
                    self.sides
                        .iter()
                        .map(|(e, s)| json!({"edge": e, "side": s}))
                        .collect::<Vec<_>>()
                ))
            } else if ordered {
                Some(json!(self.fill_sides))
            } else {
                None
            },
            ends: if self.no_ends {
                Some(json!([]))
            } else {
                ends_json(&self.end_setbacks, &self.end_reliefs)
            },
            fixed_face: self.fixed_face.clone(),
            only: if self.no_only {
                Some(Vec::new())
            } else {
                (!self.only.is_empty()).then(|| self.only.clone())
            },
            bends: if self.no_bends {
                Some(json!([]))
            } else {
                (!self.bends.is_empty()).then(|| Value::Array(self.bends.clone()))
            },
            unfold: self.unfold.clone(),
        }
    }
}

// Vectors may start with a minus (`--x -0,1,0`), so they take values that
// look like flags.
#[derive(Args)]
struct PlacementArgs {
    /// The local origin, three quantities: 0,0,10mm. A datum plane's frame origin
    /// instead, as a point it projects: <part>/<feature>/<name>, <part>/<datum> (""
    /// for its own).
    #[arg(long, value_name = "X,Y,Z", value_parser = triple_or_ref, allow_hyphen_values = true)]
    origin: Option<OrRef<[String; 3]>>,
    /// The local z axis: 0,0,1.
    #[arg(long, value_name = "X,Y,Z", value_parser = vector, allow_hyphen_values = true)]
    z: Option<[f64; 3]>,
    /// The local x direction: 1,0,0. A datum plane's frame x instead, as an axis it
    /// projects: x, y, z, <part>/<feature>/<name>, <part>/<datum> ("" for its own).
    #[arg(long, value_name = "X,Y,Z", value_parser = vector_or_ref, allow_hyphen_values = true)]
    x: Option<OrRef<[f64; 3]>>,
}

/// A placement's three numbers, or (with no comma) a datum plane's frame
/// reference.
#[derive(Clone, Debug)]
enum OrRef<T> {
    Value(T),
    Ref(String),
}

fn triple_or_ref(s: &str) -> core::result::Result<OrRef<[String; 3]>, String> {
    if s.contains(',') {
        triple(s).map(OrRef::Value)
    } else {
        Ok(OrRef::Ref(s.to_owned()))
    }
}

fn vector_or_ref(s: &str) -> core::result::Result<OrRef<[f64; 3]>, String> {
    if s.contains(',') {
        vector(s).map(OrRef::Value)
    } else {
        Ok(OrRef::Ref(s.to_owned()))
    }
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

/// The `--face` values after each `--with`, from the command line as
/// written: a replacement's groups. With no `--with`, none. A `--face`
/// before the first `--with` of a command that has one is a usage error.
fn with_groups(args: &[String]) -> Result<Vec<(String, Vec<String>)>> {
    let (mut out, mut early): (Vec<(String, Vec<String>)>, bool) = (Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        let value = |flag: &str| -> Option<(String, usize)> {
            if a == flag {
                args.get(i + 1).map(|v| (v.clone(), 2))
            } else {
                a.strip_prefix(&format!("{flag}="))
                    .map(|v| (v.to_owned(), 1))
            }
        };
        if let Some((w, step)) = value("--with") {
            out.push((w, Vec::new()));
            i += step;
            continue;
        }
        if let Some((f, step)) = value("--face") {
            match out.last_mut() {
                Some((_, faces)) => faces.push(f),
                None => early = true,
            }
            i += step;
            continue;
        }
        i += 1;
    }
    if early && !out.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "give --with before the faces it replaces: each --face takes the surface of the --with before it",
        ));
    }
    Ok(out)
}

/// `REFERENCE=QUANTITY` pairs as face groups `[{<key>: Q, faces: [REFERENCE, ...]}]`:
/// the faces of one quantity, as written, are one group, in the order first given.
/// A relief as the protocol takes it.
fn relief_json(kind: &str, width: Option<&str>, depth: Option<&str>) -> Value {
    let mut v = json!({"kind": kind});
    if let Some(w) = width {
        v["width"] = json!(w);
    }
    if let Some(d) = depth {
        v["depth"] = json!(d);
    }
    v
}

/// `--end-setback` and `--end-relief` as a flange's ends, one per vertex
/// in the order first named; none given is `None` (the ends stay).
fn ends_json(setbacks: &[(String, String)], reliefs: &[(String, String)]) -> Option<Value> {
    if setbacks.is_empty() && reliefs.is_empty() {
        return None;
    }
    let mut order: Vec<String> = Vec::new();
    let mut ends: BTreeMap<String, Value> = BTreeMap::new();
    let mut end = |v: &str, key: &str, x: Value| {
        if !order.iter().any(|o| o == v) {
            order.push(v.to_owned());
        }
        ends.entry(v.to_owned())
            .or_insert_with(|| json!({"vertex": v}))[key] = x;
    };
    for (v, q) in setbacks {
        end(v, "setback", json!(q));
    }
    for (v, r) in reliefs {
        let mut parts = r.split(',');
        let kind = parts.next().unwrap_or_default();
        end(v, "relief", relief_json(kind, parts.next(), parts.next()));
    }
    Some(Value::Array(
        order.iter().map(|v| ends[v].clone()).collect(),
    ))
}

/// `--flip` as a request takes it: a bool where every value is `true` or
/// `false` (the last one counting: a datum's or split's), else a draft's
/// faces; `--no-flip` none.
fn flip_of(values: &[String], none: bool) -> Option<ryucad::proto::Flip> {
    use ryucad::proto::Flip;
    if none {
        return Some(Flip::Faces(Vec::new()));
    }
    let last = values.last()?;
    if values.iter().all(|v| v == "true" || v == "false") {
        return Some(Flip::On(last == "true"));
    }
    Some(Flip::Faces(values.to_vec()))
}

fn face_groups(pairs: &[(String, String)], key: &str) -> Value {
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for (face, q) in pairs {
        match groups.iter_mut().find(|(x, _)| x == q) {
            Some((_, faces)) => faces.push(face),
            None => groups.push((q, vec![face])),
        }
    }
    Value::Array(
        groups
            .into_iter()
            .map(|(q, faces)| {
                let mut g = json!({"faces": faces});
                g[key] = json!(q);
                g
            })
            .collect(),
    )
}

/// One `--bend FACE=FORM:VALUE` as an unfold's override,
/// `{"bend": FACE, FORM: VALUE}`, FORM `k_factor`, `deduction` or
/// `allowance`. `flat` and `export` read theirs through it too.
fn bend_override(s: &str) -> core::result::Result<Value, String> {
    let (face, v) = param(s)?;
    match v.split_once(':') {
        Some((form @ ("k_factor" | "deduction" | "allowance"), q)) => {
            Ok(json!({"bend": face, form: q.trim()}))
        }
        _ => Err(format!(
            "{face}={v}: give FACE=FORM:VALUE, FORM k_factor, deduction or allowance"
        )),
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
            origin: match &self.origin {
                Some(OrRef::Value(o)) => Some(o.clone().map(Value::String)),
                _ => None,
            },
            z: self.z,
            x: match &self.x {
                Some(OrRef::Value(x)) => Some(*x),
                _ => None,
            },
        };
        (arg != PlacementArg::default()).then_some(arg)
    }

    /// `fields` with a datum plane's frame origin and x, where they were
    /// given as references.
    fn frame(&self, mut fields: FeatureFields) -> FeatureFields {
        if let Some(OrRef::Ref(r)) = &self.origin {
            fields.origin = Some(r.clone());
        }
        if let Some(OrRef::Ref(r)) = &self.x {
            fields.x = Some(r.clone());
        }
        fields
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
        | Command::Cache(CacheCmd::Push { .. })
        | Command::Worktree(WorktreeCmd::Add { .. } | WorktreeCmd::Remove { .. })
        | Command::Watch { .. }
        | Command::Cancel { .. }
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
            about,
            expect,
        }) => Request::AssemblyMove {
            part: part.clone(),
            instance: instance.clone(),
            placement: placement.to_arg(),
            translate: translate.clone().map(|t| t.map(Value::String)),
            axis: *axis,
            angle: angle.clone().map(Value::String),
            about: about.clone().map(|t| t.map(Value::String)),
            expected_revision: expect.revision.clone(),
        },
        Command::Assembly(AssemblyCmd::Tree { part, path, depth }) => Request::AssemblyTree {
            part: part.clone(),
            path: path.clone(),
            depth: *depth,
        },
        Command::Assembly(AssemblyCmd::Solve {
            part,
            instance,
            placement,
            translate,
            axis,
            angle,
            about,
            component,
            rev,
            mates,
        }) => Request::AssemblySolve {
            part: part.clone(),
            component: *component,
            rev: rev.clone(),
            mates: mates
                .iter()
                .filter_map(|m| m.as_object().cloned())
                .collect(),
            moves: instance
                .iter()
                .map(|i| ryucad::proto::InstanceMove {
                    instance: i.clone(),
                    placement: placement.to_arg(),
                    translate: translate.clone().map(|t| t.map(Value::String)),
                    axis: *axis,
                    angle: angle.clone().map(Value::String),
                    about: about.clone().map(|t| t.map(Value::String)),
                })
                .collect(),
        },
        Command::Assembly(AssemblyCmd::Sweep {
            part,
            motion,
            mate,
            from,
            to,
            drives,
            steps,
            interference,
            clearance,
            below,
            poses,
            refine,
        }) => Request::AssemblySweep {
            part: part.clone(),
            motion: motion.clone(),
            drives: drives.clone(),
            mate: mate.clone(),
            from: from.clone(),
            to: to.clone(),
            steps: *steps,
            interference: *interference,
            clearance: *clearance,
            below: below.clone(),
            poses: *poses,
            refine: *refine,
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
            dry_run,
        }) => Request::FeatureAdd {
            part: part.clone(),
            ty: ty.clone(),
            params: params.to_map(),
            placement: placement.to_arg(),
            fields: placement.frame(fields.to_fields()),
            id: id.clone(),
            position: position.to_position(),
            expected_revision: expect.revision.clone(),
            dry_run: *dry_run,
        },
        Command::Feature(FeatureCmd::Edit {
            part,
            id,
            params,
            unset,
            placement,
            fields,
            expressions,
            expect,
            dry_run,
        }) => {
            let mut params = params.to_map();
            params.extend(unset.iter().map(|k| (k.clone(), Value::Null)));
            Request::FeatureEdit {
                part: part.clone(),
                id: id.clone(),
                params,
                placement: placement.to_arg(),
                fields: placement.frame(fields.to_fields()),
                expressions: expressions
                    .iter()
                    .map(|(k, v)| (k.clone(), json!(v)))
                    .collect(),
                expected_revision: expect.revision.clone(),
                dry_run: *dry_run,
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
        Command::Feature(FeatureCmd::Show { part, id, rev }) => Request::FeatureShow {
            part: part.clone(),
            id: id.clone(),
            rev: rev.clone(),
        },
        Command::Feature(FeatureCmd::Types { ty }) => Request::FeatureTypes { ty: ty.clone() },
        Command::Param(ParamCmd::List { part, project, rev }) => Request::ParamList {
            part: part.clone(),
            scope: param_scope(*project),
            rev: rev.clone(),
        },
        Command::Param(ParamCmd::Set {
            part,
            values,
            project,
            expect,
        }) => {
            // With --project the first word is a value too.
            let (part, first) = match part {
                Some(p) if *project => match param(p) {
                    Ok(v) => (None, Some(v)),
                    Err(_) => (Some(p.clone()), None),
                },
                p => (p.clone(), None),
            };
            Request::ParamSet {
                part,
                scope: param_scope(*project),
                parameters: first
                    .iter()
                    .chain(values)
                    .map(|(k, v)| (k.clone(), json!(v)))
                    .collect(),
                expected_revision: expect.revision.clone(),
            }
        }
        Command::Param(ParamCmd::Delete {
            part,
            name,
            project,
            expect,
        }) => {
            let w = shifted(*project, part, &[name]);
            Request::ParamDelete {
                part: w[0].clone(),
                scope: param_scope(*project),
                name: w[1].clone().unwrap_or_default(),
                expected_revision: expect.revision.clone(),
            }
        }
        Command::Param(ParamCmd::Rename {
            part,
            from,
            to,
            project,
            expect,
        }) => {
            let w = shifted(*project, part, &[from, to]);
            Request::ParamRename {
                part: w[0].clone(),
                scope: param_scope(*project),
                from: w[1].clone().unwrap_or_default(),
                to: w[2].clone().unwrap_or_default(),
                expected_revision: expect.revision.clone(),
            }
        }
        Command::Deps(DepsCmd::Add {
            url,
            path,
            name,
            expect,
        }) => Request::DepsAdd {
            url: url.clone(),
            path: path.clone(),
            name: name.clone(),
            expected_revision: expect.clone(),
        },
        Command::Deps(DepsCmd::Fetch { names }) => Request::DepsFetch {
            names: names.clone(),
        },
        Command::Deps(DepsCmd::List) => Request::DepsList {},
        Command::Deps(DepsCmd::Outdated) => Request::DepsOutdated {},
        Command::Deps(DepsCmd::Uses { rev }) => Request::DepsUses { rev: rev.clone() },
        Command::Deps(DepsCmd::Remove { name, expect }) => Request::DepsRemove {
            name: name.clone(),
            expected_revision: expect.clone(),
        },
        Command::Deps(DepsCmd::Update {
            names,
            part,
            to,
            offline,
            dry_run,
            expect,
        }) => Request::DepsUpdate {
            names: names.clone(),
            part: part.clone(),
            to: to.clone(),
            offline: *offline,
            dry_run: *dry_run,
            expected_revision: expect.clone(),
        },
        Command::Holes {
            part,
            solid,
            csv,
            group,
            reference,
            rev,
        } => Request::HolesList {
            part: part.clone(),
            solid: solid.clone(),
            csv: *csv,
            group: *group,
            reference: reference.clone(),
            rev: rev.rev.clone(),
        },
        Command::Standards {
            table,
            standard,
            size,
        } => Request::StandardsList {
            table: table.clone(),
            standard: standard.clone(),
            size: size.clone(),
        },
        Command::Hole(HoleCmd::Derive { spec, fields, part }) => Request::HoleDerive {
            spec: Some(spec.clone()),
            fields: fields.clone(),
            part: part.clone(),
        },
        Command::Insert(InsertCmd::Update {
            part,
            insert,
            to,
            dry_run,
            expect,
        }) => Request::InsertUpdate {
            part: part.clone(),
            feature: insert.clone(),
            to: to.clone(),
            dry_run: *dry_run,
            expected_revision: expect.clone(),
        },
        Command::Deps(DepsCmd::Parts { name, part }) => Request::DepsParts {
            name: name.clone(),
            part: part.clone(),
        },
        Command::Materials(MaterialsCmd::List) => Request::MaterialsList {},
        Command::Annotations(c) => {
            use ryucad::proto::PinArg;
            let refs = |r: &[String]| {
                r.iter()
                    .map(|r| PinArg {
                        reference: Some(r.clone()),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>()
            };
            let author = None;
            match c {
                AnnotationsCmd::Add {
                    refs: r,
                    comment,
                    point,
                    part,
                    view,
                    of,
                    marks,
                } => {
                    let mut pins = refs(r);
                    let point = point.clone().map(|p| p.map(Value::String));
                    match (point, part, pins.len()) {
                        (Some(p), Some(part), 0) => pins.push(PinArg {
                            part: Some(part.clone()),
                            point: Some(p),
                            ..Default::default()
                        }),
                        (Some(p), None, 1) => pins[0].point = Some(p),
                        (Some(_), _, _) => unreachable!("checked by annotations_usage"),
                        (None, _, _) => {}
                    }
                    Request::AnnotationCreate {
                        pins,
                        comment: comment.clone(),
                        camera: None,
                        with_view: *view,
                        marks: marks.read().expect("checked by annotations_usage"),
                        view: of.clone(),
                        author,
                    }
                }
                AnnotationsCmd::Propose {
                    request,
                    comment,
                    pins,
                    rough,
                } => {
                    let request = serde_json::from_str(request).unwrap_or(Value::Null);
                    Request::AnnotationPropose {
                        request,
                        comment: comment.clone(),
                        pins: refs(pins),
                        rough: *rough,
                        author,
                    }
                }
                AnnotationsCmd::List {
                    status,
                    part,
                    by,
                    open,
                } => Request::AnnotationList {
                    status: status.clone(),
                    part: part.clone(),
                    by: by.clone(),
                    open: *open,
                },
                AnnotationsCmd::Show { id } => Request::AnnotationShow { id: id.clone() },
                AnnotationsCmd::Reply { id, text, expect } => Request::AnnotationReply {
                    id: id.clone(),
                    text: text.clone(),
                    expect: expect.clone(),
                    author,
                },
                AnnotationsCmd::Claim { id, expect } => Request::AnnotationClaim {
                    id: id.clone(),
                    expect: expect.clone(),
                    author,
                },
                AnnotationsCmd::Apply {
                    id,
                    keep_open,
                    expect,
                } => Request::AnnotationApply {
                    id: id.clone(),
                    keep_open: *keep_open,
                    expect: expect.clone(),
                    author,
                },
                AnnotationsCmd::Resolve { id, note, expect } => Request::AnnotationResolve {
                    id: id.clone(),
                    note: note.clone(),
                    expect: expect.clone(),
                    author,
                },
                AnnotationsCmd::Reject { id, reason, expect } => Request::AnnotationReject {
                    id: id.clone(),
                    reason: reason.clone(),
                    expect: expect.clone(),
                    author,
                },
                AnnotationsCmd::Reopen { id, expect } => Request::AnnotationReopen {
                    id: id.clone(),
                    expect: expect.clone(),
                    author,
                },
                AnnotationsCmd::Edit {
                    id,
                    comment,
                    pins,
                    marks,
                    no_marks,
                    expect,
                } => Request::AnnotationEdit {
                    id: id.clone(),
                    comment: comment.clone(),
                    pins: (!pins.is_empty()).then(|| refs(pins)),
                    marks: marks.read().expect("checked by annotations_usage"),
                    no_marks: *no_marks,
                    expect: expect.clone(),
                    author,
                },
                AnnotationsCmd::Delete { id, expect } => Request::AnnotationDelete {
                    id: id.clone(),
                    expect: expect.clone(),
                    author,
                },
            }
        }
        Command::View(ViewCmd::List { part }) => Request::ViewList { part: part.clone() },
        // `main` checks it first (`drawing_request`'s refusals).
        Command::Drawing(d) => drawing_request(d).expect("checked before the request is made"),
        Command::Explode(ExplodeCmd::List { part }) => Request::ExplodeList { part: part.clone() },
        Command::Explode(ExplodeCmd::Save {
            part,
            explode,
            steps,
            expect,
        }) => Request::ExplodeSave {
            part: part.clone(),
            explode: explode.clone(),
            steps: steps.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Explode(ExplodeCmd::Delete {
            part,
            explode,
            expect,
        }) => Request::ExplodeDelete {
            part: part.clone(),
            explode: explode.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Explode(ExplodeCmd::Show {
            explode,
            part,
            fraction,
            off,
            of,
        }) => Request::ExplodeShow {
            part: part.clone(),
            explode: explode.clone(),
            fraction: *fraction,
            off: *off,
            participant: of.clone(),
        },
        Command::Motion(MotionCmd::List { part }) => Request::MotionList { part: part.clone() },
        Command::Motion(MotionCmd::Save {
            part,
            motion,
            drives,
            steps,
            expect,
        }) => Request::MotionSave {
            part: part.clone(),
            motion: motion.clone(),
            drives: drives.clone(),
            steps: *steps,
            expected_revision: expect.revision.clone(),
        },
        Command::Motion(MotionCmd::Delete {
            part,
            motion,
            expect,
        }) => Request::MotionDelete {
            part: part.clone(),
            motion: motion.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Motion(MotionCmd::Show {
            motion,
            part,
            fraction,
            play,
            pause,
            off,
            of,
        }) => Request::MotionShow {
            part: part.clone(),
            motion: motion.clone(),
            fraction: *fraction,
            playing: if *play {
                Some(true)
            } else if *pause {
                Some(false)
            } else {
                None
            },
            off: *off,
            participant: of.clone(),
        },
        Command::Motion(MotionCmd::Frames {
            part,
            motion,
            drives,
            steps,
            frames,
        }) => Request::MotionFrames {
            part: part.clone(),
            motion: motion.clone(),
            drives: drives.clone(),
            steps: *steps,
            frames: *frames,
        },
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
        Command::Pdm(PdmCmd::NewId { expect }) => Request::PdmNewId {
            expected_revision: expect.clone(),
        },
        Command::Pdm(PdmCmd::Config) => Request::PdmConfig {},
        Command::Pdm(PdmCmd::Configure {
            revisions,
            release_branch,
            pattern,
            properties,
            remove,
            approvals,
            no_approvals,
            bump,
            package_formats,
            default_package_formats,
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
            approvals: if *no_approvals {
                Some(Value::Bool(false))
            } else {
                approvals.clone()
            },
            bump: bump.clone(),
            package_formats: if *default_package_formats {
                Some(Vec::new())
            } else {
                package_formats.clone()
            },
            force: *force,
            expected_revision: expect.clone(),
        },
        Command::Props(PropsCmd::Show { part, rev }) => Request::PropsGet {
            part: part.clone(),
            rev: rev.clone(),
        },
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
        Command::Release(ReleaseCmd::Package {
            cmd: Some(PackageCmd::Verify { file, rebuild }),
            ..
        }) => Request::ReleasePackageVerify {
            file: file.to_string_lossy().into_owned(),
            rebuild: *rebuild,
        },
        Command::Release(ReleaseCmd::Package {
            cmd: None,
            args:
                PackageArgs {
                    part,
                    label,
                    out,
                    formats,
                },
        }) => {
            // Made absolute with the request's other paths
            // (`client::absolute`), a trailing separator kept.
            let text = |p: &PathBuf| p.to_string_lossy().into_owned();
            Request::ReleasePackage {
                part: part.clone().unwrap_or_default(),
                label: label.clone(),
                into: match out {
                    None => Some(".".to_owned()),
                    Some(_) => None,
                },
                out: out.as_ref().map(text),
                formats: formats.clone(),
            }
        }
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
            rev,
        }) => Request::SketchGet {
            part: part.clone(),
            sketch: sketch.clone(),
            quality: quality.clone(),
            rev: rev.clone(),
        },
        Command::Path(PathCmd::Get {
            part,
            path,
            quality,
            rev,
        }) => Request::PathGet {
            part: part.clone(),
            path: path.clone(),
            quality: quality.clone(),
            rev: rev.clone(),
        },
        Command::Sweep(SweepCmd::Get {
            part,
            feature,
            quality,
            rev,
        }) => Request::SweepGet {
            part: part.clone(),
            feature: feature.clone(),
            quality: quality.clone(),
            rev: rev.clone(),
        },
        Command::Import {
            cmd:
                Some(ImportCmd::Convert {
                    of,
                    reference,
                    expect,
                }),
            ..
        } => {
            let (part, feature) = match of.split_once('/') {
                Some((p, f)) => (p.to_owned(), Some(f.to_owned())),
                None => (of.clone(), None),
            };
            Request::ImportConvert {
                part,
                feature,
                reference: *reference,
                expected_revision: expect.revision.clone(),
            }
        }
        Command::Import {
            cmd: Some(ImportCmd::Report { file, detail }),
            ..
        } => Request::ImportReport {
            file: file.clone(),
            detail: *detail,
        },
        Command::Import {
            cmd: None,
            args:
                ImportArgs {
                    file,
                    part,
                    update,
                    into,
                    product,
                    solid,
                    flatten,
                    search,
                    dry_run,
                    expect,
                },
        } => {
            let file = file.as_ref().expect("clap requires it");
            Request::Import {
                path: file.to_string_lossy().into_owned(),
                part: part.clone(),
                into: into.clone(),
                product: product.clone(),
                dry_run: *dry_run,
                expected_revision: expect.revision.clone(),
                update: update.clone(),
                solid: *solid,
                flatten: *flatten,
                search: search
                    .iter()
                    .map(|d| d.to_string_lossy().into_owned())
                    .collect(),
            }
        }
        Command::Export {
            part,
            out,
            format,
            schema,
            unit,
            chord,
            angle,
            max_edge,
            ascii,
            no_brep,
            no_mesh,
            pipe_tolerance,
            skip_failing,
            no_appearance,
            no_properties,
            solid,
            no_notes,
            fixed,
            bends,
            time_stamp,
            rev,
        } => Request::Export {
            part: part.clone(),
            rev: rev.clone(),
            format: format.clone(),
            out: Some(out.to_string_lossy().into_owned()),
            schema: schema.clone(),
            unit: unit.clone(),
            time_stamp: time_stamp.clone(),
            file_name: out.file_name().map(|n| n.to_string_lossy().into_owned()),
            chord: chord.clone(),
            angle: angle.clone(),
            max_edge: max_edge.clone(),
            ascii: *ascii,
            brep: no_brep.then_some(false),
            mesh: no_mesh.then_some(false),
            pipe_tolerance: pipe_tolerance.clone(),
            skip_failing: *skip_failing,
            appearance: no_appearance.then_some(false),
            properties: no_properties.then_some(false),
            solid: solid.clone(),
            notes: no_notes.then_some(false),
            fixed: fixed.clone(),
            bends: bends.clone(),
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
        Command::WhereUsed { org: true, .. } => return None,
        Command::WhereUsed {
            part,
            label,
            working,
            direct,
            within,
            no_dependents,
            org: false,
        } => Request::BomWhereUsed {
            part: part.clone(),
            label: label.clone(),
            working: *working,
            direct: *direct,
            within: within
                .iter()
                .map(|f| f.to_string_lossy().into_owned())
                .collect(),
            no_dependents: *no_dependents,
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
        Command::Appearance { part, of, rev } => Request::AppearanceGet {
            part: part.clone(),
            participant: of.clone(),
            rev: rev.rev.clone(),
        },
        Command::Colour {
            refs,
            clear: true,
            expect,
            ..
        } => Request::AppearanceClear {
            entities: refs.clone(),
            expected_revision: expect.revision.clone(),
        },
        Command::Colour {
            refs,
            colour,
            material,
            opacity,
            no_colour,
            no_opacity,
            no_file_styles,
            file_styles,
            clear: false,
            expect,
        } => Request::AppearanceSet {
            entities: refs.clone(),
            colour: colour.clone(),
            material: *material,
            opacity: *opacity,
            unset: [(*no_colour, "colour"), (*no_opacity, "opacity")]
                .into_iter()
                .filter(|(on, _)| *on)
                .map(|(_, f)| f.to_owned())
                .collect(),
            file_styles: if *no_file_styles {
                Some(false)
            } else if *file_styles {
                Some(true)
            } else {
                None
            },
            expected_revision: expect.revision.clone(),
        },
        Command::Equations { part, all, rev } => Request::EquationsList {
            part: part.clone(),
            all: *all,
            rev: rev.rev.clone(),
        },
        Command::Sheet { part, solid, rev } => Request::SheetGet {
            part: part.clone(),
            solid: solid.clone(),
            rev: rev.rev.clone(),
        },
        Command::FeatureFaces { face, rev } => Request::FeatureFacesGet {
            face: face.clone(),
            rev: rev.rev.clone(),
        },
        Command::Blends {
            part,
            solid,
            tolerance,
            below,
            rev,
        } => Request::BlendsGet {
            part: part.clone(),
            solid: solid.clone(),
            tolerance: tolerance.clone(),
            below: below.clone(),
            rev: rev.rev.clone(),
        },
        Command::Flat {
            part,
            solid,
            fixed,
            bends,
            rev,
        } => Request::FlatGet {
            part: part.clone(),
            solid: solid.clone(),
            rev: rev.rev.clone(),
            fixed: fixed.clone(),
            bends: bends.clone(),
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
            interference,
        } => Request::Check {
            part: part.clone(),
            against: against.clone(),
            results: *results,
            interference: *interference,
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
            annotation,
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
            no_datums,
            no_threads,
            rev,
            frame,
            compare,
            hidden,
            origin,
            annotations,
            no_appearance,
            section,
            no_section,
            explode,
            fraction,
            no_explode,
            motion,
            no_motion,
            interference,
            flat,
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
            datums: Some(!no_datums),
            threads: Some(!no_threads),
            rev: rev.rev.clone(),
            frame: frame.clone(),
            compare: compare.clone(),
            hidden: (!hidden.is_empty()).then(|| hidden.clone()),
            origin: *origin,
            appearance: Some(!no_appearance),
            section: (!section.is_empty() || *no_section).then(|| section.clone()),
            explode: if *no_explode {
                Some(String::new())
            } else {
                explode.clone()
            },
            fraction: *fraction,
            motion: if *no_motion {
                Some(String::new())
            } else {
                motion.clone()
            },
            interference: *interference,
            annotations: *annotations,
            annotation: annotation.clone(),
            flat: flat.clone(),
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
        Command::Compare {
            rev,
            off,
            of,
            worktree: None,
        } if rev.is_none() && !off => Request::CompareGet {
            participant: of.clone(),
        },
        Command::Compare {
            rev,
            off,
            of,
            worktree,
        } => Request::CompareSet {
            rev: match worktree {
                Some(w) => Some(format!("{}{}", ryucad::session::WORKTREE, w.display())),
                None => rev.clone(),
            },
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
        Command::Section {
            plane,
            origin,
            normal,
            offset,
            flip,
            add,
            off,
            of,
        } => {
            let named = plane.is_some() || origin.is_some();
            if *off {
                Request::SectionSet {
                    planes: Vec::new(),
                    add: false,
                    off: true,
                    participant: of.clone(),
                }
            } else if named {
                Request::SectionSet {
                    planes: vec![ryucad::section::PlaneArg {
                        plane: plane.clone(),
                        origin: origin.clone().map(|o| o.map(Value::String)),
                        normal: *normal,
                        offset: offset.clone().map(Value::String),
                        flip: *flip,
                    }],
                    add: *add,
                    off: false,
                    participant: of.clone(),
                }
            } else {
                Request::SectionGet {
                    participant: of.clone(),
                }
            }
        }
        Command::Mode { mode } => Request::ParticipantMode {
            mode: match mode {
                ModeArg::View => ryucad::live::Mode::View,
                ModeArg::Annotate => ryucad::live::Mode::Annotate,
                ModeArg::Edit => ryucad::live::Mode::Edit,
            },
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
        Command::Cache(CacheCmd::Login { token }) => Request::CacheLogin {
            token: token.clone().unwrap_or_else(|| {
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                line.trim().to_owned()
            }),
        },
        Command::Cache(CacheCmd::Logout) => Request::CacheLogout {},
        Command::Cache(CacheCmd::Warm { part }) => Request::CacheWarm { part: part.clone() },
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
            occurrences,
            quality,
            out,
            compact,
            flat,
            rev,
        } => Request::MeshGet {
            part: part.clone(),
            feature: feature.clone(),
            features: occurrences.clone(),
            quality: quality.clone(),
            transfer: out.as_ref().map(|_| match compact {
                true => Transfer::Compact,
                false => Transfer::Binary,
            }),
            rev: rev.rev.clone(),
            flat: flat.clone(),
            before: None,
        },
        Command::History { part, limit } => Request::HistoryLog {
            part: part.clone(),
            limit: Some(*limit),
        },
        Command::Merge(MergeCmd::Status { .. }) => Request::MergeStatus {},
        Command::Merge(MergeCmd::Preview { rev, worktree }) => Request::MergePreview {
            rev: match worktree {
                Some(w) => format!("{}{}", ryucad::session::WORKTREE, w.display()),
                None => rev.clone().unwrap_or_default(),
            },
        },
        Command::Worktree(WorktreeCmd::List) => Request::WorktreeList {},
        Command::Branch(BranchCmd::List) => Request::BranchList {},
        Command::Switch { branch, expect } => Request::BranchSwitch {
            branch: branch.clone(),
            expected_revision: expect.clone(),
        },
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
/// Refusals of `ryucad annotations` the parser can't make: a point with
/// no place, a request that isn't JSON.
fn annotations_usage(c: &AnnotationsCmd) -> Result<()> {
    match c {
        AnnotationsCmd::Add {
            refs,
            point: Some(_),
            part,
            ..
        } if !matches!((part.is_some(), refs.len()), (true, 0) | (false, 1)) => Err(Error::new(
            Code::Usage,
            "--point goes with one reference, or with --part and none",
        )),
        AnnotationsCmd::Propose { request, .. } => serde_json::from_str::<Value>(request)
            .map(drop)
            .map_err(|e| Error::new(Code::Usage, format!("--request isn't JSON: {e}"))),
        AnnotationsCmd::Add { marks, .. } | AnnotationsCmd::Edit { marks, .. } => {
            marks.read().map(drop)
        }
        _ => Ok(()),
    }
}

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
        Command::Cache(CacheCmd::Push { kind }) => {
            use ryucad::cache::{Disk, Kind};
            let disk = Disk::for_user().ok_or_else(|| {
                Error::new(Code::Io, "there is no disk cache on this machine to push")
            })?;
            let shared: Vec<Kind> = Kind::ALL.into_iter().filter(|k| k.shared()).collect();
            let kinds = kind.map_or(shared, |k| vec![k]);
            ryucad::cache::push(&disk, &kinds)
        }
        Command::WhereUsed {
            part,
            label,
            direct,
            org: true,
            ..
        } => ryucad::review::where_used_org(&find_root(&here)?, part, label.as_deref(), *direct),
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
                ReviewCmd::Approvals { pr } => Action::Approvals { pr: *pr },
                ReviewCmd::Package(ReviewPackageCmd::Get { part, label, out }) => {
                    let out = here.join(
                        out.clone()
                            .unwrap_or_else(|| PathBuf::from(format!("{part}-{label}.zip"))),
                    );
                    let mut got = ryucad::review::run(
                        &here,
                        Action::PackageGet {
                            part: part.clone(),
                            label: label.clone(),
                            out: out.clone(),
                        },
                    )?;
                    // Verified here, against this clone, whatever the service said.
                    let root = find_root(&here)?;
                    let req = Request::ReleasePackageVerify {
                        file: out.to_string_lossy().into_owned(),
                        rebuild: false,
                    };
                    let mut slot = None;
                    let v = server::handle_job(&mut slot, None, None, &root, req, &Job::default())?
                        .result;
                    got["verify"] = v;
                    return Ok(got);
                }
                ReviewCmd::Package(ReviewPackageCmd::Build { part, label }) => {
                    Action::PackageBuild {
                        part: part.clone(),
                        label: label.clone(),
                    }
                }
                ReviewCmd::Package(ReviewPackageCmd::Share { part, label, days }) => {
                    Action::PackageShare {
                        part: part.clone(),
                        label: label.clone(),
                        days: *days,
                    }
                }
                ReviewCmd::Audit {
                    part,
                    since,
                    until,
                    format,
                    out,
                } => Action::Audit {
                    part: part.clone(),
                    since: since.clone(),
                    until: until.clone(),
                    csv: format == "csv",
                    out: out.as_ref().map(|o| here.join(o)),
                },

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
        Command::Worktree(WorktreeCmd::Add {
            path,
            branch,
            from,
            no_warm,
        }) => {
            let root = find_root(&here)?;
            let ask = ryucad::worktree::Add {
                path,
                branch: branch.as_deref(),
                from: from.as_deref(),
                name: cli.as_name.as_deref(),
                warm: !no_warm,
            };
            let mut v = ryucad::worktree::add(&root, &here, &ask)?;
            if let Some(Value::Array(list)) = v.as_object_mut().and_then(|o| o.remove("warnings")) {
                warnings.extend(list.into_iter().map(|w| {
                    w["message"]
                        .as_str()
                        .map_or_else(|| w.to_string(), str::to_owned)
                }));
            }
            Ok(v)
        }
        Command::Worktree(WorktreeCmd::Remove { path, force }) => {
            let root = find_root(&here)?;
            ryucad::worktree::remove(&root, &here.join(path), *force)
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
        Command::Cancel { token } => {
            let root = find_root(&here)?;
            // Nothing runs in this process to cancel.
            if no_daemon(cli) {
                return Ok(json!({"cancelled": false, "token": token}));
            }
            client::cancel(&root, participant(cli).as_deref(), token)
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
            if let Command::Annotations(c) = cmd {
                annotations_usage(c)?;
            }
            if let Command::Drawing(d) = cmd {
                drawing_request(d)?;
            }
            // The daemon's folder isn't the caller's: every path the
            // request names is made absolute against `here` (`-C`'s).
            let req = client::absolute(
                request(cmd).expect("every other command is a request"),
                &here,
            )?;
            let root = find_root(&here)?;
            let me = participant(cli);
            let req = client::with_author(req, me.as_deref());
            let method = req.method();
            // A long request has a token, which Ctrl+C cancels, and its
            // progress on stderr with --progress.
            let long = daemon::LONG.contains(&method.as_str());
            let ask = cancel::ask(cli.progress && long);
            let printer = cancel::Printer::new(&method);
            if !no_daemon(cli) {
                if long {
                    let (root, me, token) = (root.clone(), me.clone(), ask.token.clone());
                    cancel::Interrupt::install(
                        Box::new(move || {
                            // Whether the daemon says it has begun writing.
                            token.is_some_and(|t| {
                                client::cancel(&root, me.as_deref(), &t)
                                    .is_ok_and(|v| v["writing"] == true)
                            })
                        }),
                        cli.pretty,
                    );
                }
                let ask = if long {
                    ask.clone()
                } else {
                    Default::default()
                };
                let note = &mut |p: &Value| printer.print(p);
                match client::request_noting(&root, &req, me.as_deref(), &ask, note) {
                    Ok(answer) => return deliver(cmd, answer?).map(|r| lift(cmd, r, warnings)),
                    // Never delivered, so running it here is safe.
                    Err(e) => warnings.push(format!("{}; ran in process", e.message)),
                }
            }
            // As the participant asked for, as through the daemon.
            let job = match long {
                false => Job::default(),
                true => {
                    let flag = cancel::local_flag();
                    let writing = cancel::local_flag();
                    let (set, wrote) = (
                        std::sync::Arc::clone(&flag),
                        std::sync::Arc::clone(&writing),
                    );
                    cancel::Interrupt::install(
                        Box::new(move || {
                            set.store(true, std::sync::atomic::Ordering::SeqCst);
                            wrote.load(std::sync::atomic::Ordering::SeqCst)
                        }),
                        cli.pretty,
                    );
                    let token = ask.token.clone().unwrap_or_default();
                    let who = me.clone().unwrap_or_else(|| "agent".into());
                    let job = Job::new(&token, &who, &method, flag).with_writing(writing);
                    match ask.progress {
                        true => job.with_line(Box::new(move |p| printer.print(p))),
                        false => job,
                    }
                }
            };
            let mut slot = None;
            let answer = server::handle_job(&mut slot, None, me.as_deref(), &root, req, &job);
            // Its last progress line, as a daemon sends it.
            job.finish();
            // A daemon sends what it built to the shared cache behind its
            // requests; this process has only until it exits.
            if let Some(r) = slot.as_ref().and_then(|s| s.disk()?.remote()) {
                r.flush(REMOTE_FLUSH);
            }
            deliver(cmd, answer?).map(|r| lift(cmd, r, warnings))
        }
    }
}

/// How long a request run in this process waits, before it exits, for
/// what it built to reach the shared cache.
const REMOTE_FLUSH: Duration = Duration::from_secs(30);

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
            | Command::Feature(FeatureCmd::Edit { .. })
            | Command::Assembly(AssemblyCmd::Tree { .. })
            | Command::Assembly(AssemblyCmd::Sweep { .. })
            | Command::Interference { .. }
            | Command::Appearance { .. }
            | Command::Render { .. }
            | Command::Export { .. }
            | Command::Diff { .. }
            | Command::Inspect { .. }
            | Command::Mesh { .. }
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
        Command::Mesh { out: Some(o), .. }
        | Command::Render { out: o, .. }
        | Command::Drawing(DrawingCmd::Render { out: o, .. })
        | Command::Drawing(DrawingCmd::Export { out: o, .. }) => o,
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

/// The request a `drawing` command sends.
fn drawing_request(d: &DrawingCmd) -> Result<Request> {
    let edit =
        |part: &String, drawing: &String, ops: Value, expect: &Expect| Request::DrawingEdit {
            part: part.clone(),
            drawing: drawing.clone(),
            ops,
            expected_revision: expect.revision.clone(),
        };
    Ok(match d {
        DrawingCmd::List { part } => Request::DrawingList { part: part.clone() },
        DrawingCmd::Create {
            part,
            drawing,
            name,
            template,
            size,
            views,
            expect,
        } => Request::DrawingCreate {
            part: part.clone(),
            drawing: drawing.clone(),
            name: name.clone(),
            template: template.clone(),
            size: size.clone(),
            views: views.clone(),
            expected_revision: expect.revision.clone(),
        },
        DrawingCmd::Edit {
            part,
            drawing,
            ops,
            expect,
        } => edit(part, drawing, ops.clone(), expect),
        DrawingCmd::View(DrawingViewCmd::Add {
            part,
            drawing,
            id,
            sheet,
            orient,
            of,
            label_rev,
            from,
            cut,
            cut_offset,
            through,
            flip,
            unsectioned,
            centre,
            radius,
            flat,
            solid,
            no_notes,
            explode,
            fraction,
            trails,
            side,
            normal_to,
            at,
            scale,
            hidden,
            tangent,
            label,
            expect,
        }) => {
            let mut v = json!({"sheet": sheet});
            match (orient, from) {
                _ if *flat => {
                    v["type"] = json!("flat");
                    if let Some(of) = of {
                        v["of"] = json!(of);
                    }
                    if let Some(s) = solid {
                        v["solid"] = json!(s);
                    }
                    if *no_notes {
                        v["notes"] = json!(false);
                    }
                }
                (Some(o), _) if explode.is_some() => {
                    v["type"] = json!("exploded");
                    v["explode"] = json!(explode);
                    v["orient"] = if o.contains('/') {
                        json!({"normal_to": drawing_ref(o)?})
                    } else {
                        json!(o)
                    };
                    if let Some(of) = of {
                        v["of"] = json!(of);
                    }
                    if let Some(f) = fraction {
                        v["fraction"] = json!(f);
                    }
                    if *trails {
                        v["trails"] = json!(true);
                    }
                }
                (None, Some(f)) if cut.is_some() || !through.is_empty() => {
                    v["type"] = json!("section");
                    v["from"] = json!(f);
                    v["cut"] = match cut {
                        Some(c) => {
                            let mut c = json!({"plane": drawing_geom(c)?});
                            if let Some(d) = cut_offset {
                                c["offset"] = json!(d);
                            }
                            c
                        }
                        None => {
                            json!({"through": through.iter().map(|p| drawing_point(p)).collect::<core::result::Result<Vec<_>, _>>()?})
                        }
                    };
                    if *flip {
                        v["flip"] = json!(true);
                    }
                    if !unsectioned.is_empty() {
                        v["unsectioned"] = json!(unsectioned);
                    }
                }
                (None, Some(f)) if centre.is_some() => {
                    v["type"] = json!("detail");
                    v["from"] = json!(f);
                    v["centre"] = drawing_point(centre.as_deref().unwrap_or_default())?;
                    v["radius"] = json!(radius);
                }
                (Some(o), _) => {
                    v["type"] = json!("base");
                    v["orient"] = if o.contains('/') {
                        json!({"normal_to": drawing_ref(o)?})
                    } else {
                        json!(o)
                    };
                    if let Some(of) = of {
                        v["of"] = json!(of);
                    }
                    if let Some(l) = label_rev {
                        v["rev"] = json!({"label": l});
                    }
                }
                (None, Some(f)) => {
                    v["from"] = json!(f);
                    match (side, normal_to) {
                        (_, Some(n)) => {
                            v["type"] = json!("auxiliary");
                            v["normal_to"] = drawing_ref(n)?;
                        }
                        (Some(sd), None) => {
                            v["type"] = json!("projected");
                            v["side"] = json!(sd);
                        }
                        (None, None) => {
                            return Err(Error::new(
                                Code::Usage,
                                "a view from another takes --side or --normal-to",
                            ));
                        }
                    }
                }
                (None, None) => unreachable!("clap requires one"),
            }
            if let Some(a) = at {
                v["at"] = sheet_point(a)?;
            }
            for (k, x) in [
                ("scale", scale),
                ("hidden", hidden),
                ("tangent", tangent),
                ("label", label),
            ] {
                if let Some(x) = x {
                    v[k] = json!(x);
                }
            }
            let mut op = json!({"op": "add", "kind": "view", "value": v});
            if let Some(id) = id {
                op["id"] = json!(id);
            }
            edit(part, drawing, json!([op]), expect)
        }
        DrawingCmd::Move {
            part,
            drawing,
            view,
            at,
            offset,
            expect,
        } => {
            let mut op = json!({"op": "move", "id": view});
            match (at, offset) {
                (Some(a), _) => op["at"] = sheet_point(a)?,
                (None, Some(o)) => op["offset"] = json!(o),
                (None, None) => unreachable!("clap requires one"),
            }
            edit(part, drawing, json!([op]), expect)
        }
        DrawingCmd::Remove {
            part,
            drawing,
            id,
            expect,
        } => edit(part, drawing, json!([{"op": "remove", "id": id}]), expect),
        DrawingCmd::Delete {
            part,
            drawing,
            expect,
        } => Request::DrawingDelete {
            part: part.clone(),
            drawing: drawing.clone(),
            expected_revision: expect.revision.clone(),
        },
        DrawingCmd::Rename {
            part,
            drawing,
            to,
            expect,
        } => Request::DrawingRename {
            part: part.clone(),
            drawing: drawing.clone(),
            to: to.clone(),
            expected_revision: expect.revision.clone(),
        },
        DrawingCmd::Get {
            part,
            drawing,
            rev,
            sheet,
            items,
        } => Request::DrawingGet {
            part: part.clone(),
            drawing: drawing.clone(),
            rev: rev.clone(),
            sheet: sheet.clone(),
            items: *items,
        },
        DrawingCmd::Describe {
            part,
            drawing,
            rev,
            sheet,
        } => Request::DrawingDescribe {
            part: part.clone(),
            drawing: drawing.clone(),
            rev: rev.clone(),
            sheet: sheet.clone(),
        },
        DrawingCmd::Render {
            part,
            drawing,
            out: _,
            sheet,
            format,
            scale,
            rev,
        } => Request::DrawingRender {
            part: part.clone(),
            drawing: drawing.clone(),
            rev: rev.clone(),
            sheet: sheet.clone(),
            format: format.clone(),
            scale: *scale,
        },
        DrawingCmd::Export {
            part,
            drawing,
            out,
            sheet,
            format,
            unit,
            rev,
        } => Request::DrawingExport {
            part: part.clone(),
            drawing: drawing.clone(),
            rev: rev.clone(),
            sheet: sheet.clone(),
            format: format.clone(),
            unit: unit.clone(),
            out: Some(out.to_string_lossy().into_owned()),
        },
        DrawingCmd::Lint {
            part,
            drawing,
            rev,
            sheet,
            explain,
        } => Request::DrawingLint {
            part: part.clone(),
            drawing: drawing.clone(),
            rev: rev.clone(),
            sheet: sheet.clone(),
            explain: explain.clone(),
        },
        DrawingCmd::LintConfigure {
            weight,
            severity,
            expect,
        } => {
            let split = |t: &String| -> Result<(String, String)> {
                t.split_once('=')
                    .map(|(a, b)| (a.to_owned(), b.to_owned()))
                    .ok_or_else(|| Error::new(Code::Usage, format!("{t:?}: expected CODE=VALUE")))
            };
            let mut weights = std::collections::BTreeMap::new();
            for w in weight {
                let (c, v) = split(w)?;
                weights.insert(c, v.parse::<i64>().map_or_else(|_| json!(v), |n| json!(n)));
            }
            let mut sev = std::collections::BTreeMap::new();
            for x in severity {
                let (c, v) = split(x)?;
                sev.insert(c, v);
            }
            Request::DrawingLintConfigure {
                weights,
                severity: sev,
                expected_revision: expect.revision.clone(),
            }
        }
        DrawingCmd::Dim(DrawingDimCmd::Add {
            part,
            drawing,
            view,
            kind,
            id,
            a,
            b,
            of,
            datum,
            vertex,
            along,
            offset,
            at,
            text,
            prefix,
            suffix,
            tol,
            deviation,
            limits,
            fit,
            basic,
            reference,
            precision,
            expect,
        }) => {
            let mut v = json!({"type": kind, "view": view});
            for (k, x) in [
                ("a", a),
                ("b", b),
                ("of", of),
                ("datum", datum),
                ("vertex", vertex),
                ("along", along),
                ("offset", offset),
                ("text", text),
                ("prefix", prefix),
                ("suffix", suffix),
            ] {
                if let Some(x) = x {
                    v[k] = json!(x);
                }
            }
            if let Some(a) = at {
                v["at"] = sheet_point(a)?;
            }
            if let Some(p) = precision {
                v["precision"] = json!(p);
            }
            let tolerance = match (tol, deviation, limits, fit) {
                (Some(t), ..) => Some(json!({"symmetric": t})),
                (_, Some(d), ..) => Some(json!({"deviation": pair_of(d)?})),
                (_, _, Some(l), _) => Some(json!({"limits": pair_of(l)?})),
                (_, _, _, Some(f)) => Some(json!({"fit": f})),
                _ if *basic => Some(json!({"basic": true})),
                _ if *reference => Some(json!({"reference": true})),
                _ => None,
            };
            if let Some(t) = tolerance {
                v["tolerance"] = t;
            }
            let mut op = json!({"op": "add", "kind": "annotation", "value": v});
            if let Some(id) = id {
                op["id"] = json!(id);
            }
            edit(part, drawing, json!([op]), expect)
        }
        DrawingCmd::Note(DrawingNoteCmd::Add {
            part,
            drawing,
            text,
            sheet,
            view,
            leader,
            at,
            id,
            expect,
        }) => {
            let mut v = json!({"type": "note", "text": text.replace("\\n", "\n")});
            match (sheet, view) {
                (_, Some(w)) => v["view"] = json!(w),
                (Some(sh), None) => v["sheet"] = json!(sh),
                (None, None) => v["sheet"] = json!("s1"),
            }
            if let Some(l) = leader {
                v["leader"] = json!(l);
            }
            if let Some(a) = at {
                v["at"] = sheet_point(a)?;
            }
            let mut op = json!({"op": "add", "kind": "annotation", "value": v});
            if let Some(id) = id {
                op["id"] = json!(id);
            }
            edit(part, drawing, json!([op]), expect)
        }
        DrawingCmd::Gdt(DrawingGdtCmd::Add {
            part,
            drawing,
            view,
            characteristic,
            tol,
            diameter,
            condition,
            datums,
            on,
            to,
            letter,
            at,
            id,
            expect,
        }) => {
            let mut v = json!({"view": view});
            match (characteristic, letter) {
                (Some(c), _) => {
                    v["type"] = json!("feature_control_frame");
                    v["characteristic"] = json!(c);
                    let mut row =
                        json!({"tolerance": tol.clone().unwrap_or_else(|| "0 mm".into())});
                    if *diameter {
                        row["diameter"] = json!(true);
                    }
                    if let Some(c) = condition {
                        row["condition"] = json!(c);
                    }
                    if !datums.is_empty() {
                        row["datums"] = json!(
                            datums
                                .iter()
                                .map(|d| match d.split_once(':') {
                                    Some((l, c)) => json!({"datum": l, "condition": c}),
                                    None => json!(d),
                                })
                                .collect::<Vec<_>>()
                        );
                    }
                    v["rows"] = json!([row]);
                    if let Some(o) = on {
                        v["on"] = json!(o);
                    }
                    if let Some(t) = to {
                        v["to"] = json!(t);
                    }
                }
                (None, Some(l)) => {
                    v["type"] = json!("datum_feature");
                    v["letter"] = json!(l);
                    v["on"] = json!(on);
                }
                (None, None) => unreachable!("clap requires one"),
            }
            if let Some(a) = at {
                v["at"] = sheet_point(a)?;
            }
            let mut op = json!({"op": "add", "kind": "annotation", "value": v});
            if let Some(id) = id {
                op["id"] = json!(id);
            }
            edit(part, drawing, json!([op]), expect)
        }
        DrawingCmd::Layout {
            part,
            drawing,
            dry_run,
            expect,
        } => Request::DrawingLayout {
            part: part.clone(),
            drawing: drawing.clone(),
            dry_run: *dry_run,
            expected_revision: expect.revision.clone(),
        },
    })
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

/// The command line, parsed as `Cli::try_parse` does, with `feature add
/// --help` ending with the feature registry's types.
fn parse_with_types() -> std::result::Result<Cli, clap::Error> {
    use clap::{CommandFactory, FromArgMatches};
    let types = crate::feature::type_list();
    let mut cmd = Cli::command().mut_subcommand("feature", |f| {
        f.mut_subcommand("add", |a| a.after_help(types))
    });
    let mut matches = cmd.try_get_matches_from_mut(std::env::args_os())?;
    Cli::from_arg_matches_mut(&mut matches).map_err(|e| e.format(&mut cmd))
}

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if version_flag(&args) {
        let pretty = args.iter().any(|a| a == "--pretty");
        return finish(Ok(json!(VersionInfo::current())), Vec::new(), pretty);
    }
    let mut cli = match parse_with_types() {
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
    if let Command::Feature(FeatureCmd::Add { fields, .. } | FeatureCmd::Edit { fields, .. }) =
        &mut cli.command
        && let Err(e) = read_in_order(&args, fields)
    {
        return finish(Err(e), Vec::new(), cli.pretty);
    }
    // Git reads a textconv's output as the file's text, not JSON.
    if let Command::Git(GitCmd::Textconv { file }) = &cli.command {
        return match std::fs::read(file) {
            Ok(bytes) => {
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                let names = ryucad::diff::names_near(&std::env::current_dir().unwrap_or_default());
                match out.write_all(&ryucad::diff::textconv_named(&bytes, &names)) {
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

    /// Runs `f` on a stack the size of the CLI's own thread: a debug
    /// build's clap parses in frames past a test thread's 2 MiB.
    fn on_cli_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(crate::process::STACK)
            .spawn(f)
            .expect("a thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    }

    #[test]
    fn an_insert_is_added_from_the_command_line() {
        on_cli_stack(|| {
            let c = Cli::try_parse_from([
                "ryucad",
                "feature",
                "add",
                "enclosure",
                "insert",
                "--id",
                "board",
                "--of",
                "pcb",
                "--rev",
                "label:B",
                "--origin",
                "0,0,board_z",
            ])
            .unwrap();
            let r = request(&c.command).unwrap();
            let v = serde_json::to_value(&r).unwrap();
            assert_eq!(v["params"]["of"], "pcb", "{v}");
            assert_eq!(v["params"]["rev"], "B");
            assert_eq!(
                v["params"]["placement"]["origin"],
                json!(["0", "0", "board_z"])
            );
            assert_eq!(v["params"]["type"], "insert");
        });
    }

    #[test]
    fn a_delete_takes_solid_repeated() {
        on_cli_stack(|| {
            let c = Cli::try_parse_from([
                "ryucad",
                "feature",
                "add",
                "enclosure",
                "delete_body",
                "--id",
                "drop-board",
                "--solid",
                "board",
                "--solid",
                "lid",
            ])
            .unwrap();
            let v = serde_json::to_value(request(&c.command).unwrap()).unwrap();
            assert_eq!(v["params"]["solids"], json!(["board", "lid"]), "{v}");
            assert!(v["params"].get("solid").is_none());
        });
    }

    #[test]
    fn marks_parse_from_json_shorthands_and_a_file() {
        on_cli_stack(|| {
            let req = |args: &[&str]| {
                let cli = Cli::try_parse_from(args).unwrap();
                let Command::Annotations(c) = &cli.command else {
                    panic!()
                };
                annotations_usage(c).unwrap();
                request(&cli.command).unwrap()
            };
            let r = req(&[
                "ryucad",
                "annotations",
                "add",
                "--comment",
                "here",
                "--model",
                "rig",
                "--arrow",
                "-20,11:9,-4",
                "--text",
                "20,15=2 mm",
                "--view",
            ]);
            let Request::AnnotationCreate {
                marks: Some(m),
                with_view: true,
                ..
            } = r
            else {
                panic!("{r:?}")
            };
            assert_eq!(m.part, "rig");
            assert_eq!(
                m.short,
                vec![
                    ryucad::proto::ShortMark {
                        kind: "arrow".into(),
                        at: "-20,11:9,-4".into()
                    },
                    ryucad::proto::ShortMark {
                        kind: "text".into(),
                        at: "20,15=2 mm".into()
                    },
                ]
            );
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("m.json");
            std::fs::write(
                &file,
                r#"{"part": "rig", "list": [{"kind": "line", "in": "view", "xy_m": [[0, 0], [0.01, 0]]}]}"#,
            )
            .unwrap();
            let at = format!("@{}", file.display());
            let r = req(&[
                "ryucad",
                "annotations",
                "add",
                "--comment",
                "here",
                "--marks",
                &at,
                "--of",
                "front_view",
            ]);
            let Request::AnnotationCreate {
                marks: Some(m),
                view: Some(v),
                ..
            } = r
            else {
                panic!()
            };
            assert_eq!(
                (m.part.as_str(), m.list.len(), v.as_str()),
                ("rig", 1, "front_view")
            );
            let r = req(&["ryucad", "annotations", "edit", "aaaaaaaaaa", "--no-marks"]);
            assert!(matches!(
                r,
                Request::AnnotationEdit {
                    no_marks: true,
                    marks: None,
                    ..
                }
            ));
            // A shorthand with no model to draw on is a usage error.
            let cli = Cli::try_parse_from([
                "ryucad",
                "annotations",
                "add",
                "--comment",
                "x",
                "--arrow",
                "0,0:1,1",
            ])
            .unwrap();
            let Command::Annotations(c) = &cli.command else {
                panic!()
            };
            assert_eq!(annotations_usage(c).unwrap_err().code, Code::Usage);
            // `render --annotation` with a view is refused by the parser.
            assert!(
                Cli::try_parse_from([
                    "ryucad",
                    "render",
                    "--annotation",
                    "aaaaaaaaaa",
                    "--view",
                    "iso",
                    "--out",
                    "a.png"
                ])
                .is_err()
            );
        });
    }

    #[test]
    fn open_keeps_c_apart_from_its_folder() {
        on_cli_stack(|| {
            // #139: both were clap's argument `dir`, so each filled the other.
            let cli = Cli::try_parse_from(["ryucad", "-C", "project", "open"]).unwrap();
            assert_eq!(cli.dir, Some(PathBuf::from("project")));
            assert!(matches!(&cli.command, Command::Open { dir: None }));
            let cli = Cli::try_parse_from(["ryucad", "-C", "project", "open", "."]).unwrap();
            assert_eq!(cli.dir, Some(PathBuf::from("project")));
            assert!(
                matches!(&cli.command, Command::Open { dir: Some(d) } if d == &PathBuf::from("."))
            );
        });
    }

    #[test]
    fn fill_sides_are_read_in_order() {
        let args: Vec<String> = [
            "ryucad",
            "feature",
            "add",
            "p",
            "fill",
            "--edge",
            "p/w/aa",
            "--tangent",
            "--curve=p/s/l1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            fill_sides(&args).unwrap(),
            vec![
                json!({"edge": "p/w/aa", "tangent": true}),
                json!({"curve": "p/s/l1"})
            ]
        );
        let early: Vec<String> = ["--tangent", "--edge", "x"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(fill_sides(&early).unwrap_err().code, Code::Usage);
        // #674: --tangent takes a value, after an --edge as on a face tweak.
        let valued: Vec<String> = [
            "--edge",
            "a",
            "--tangent",
            "false",
            "--edge",
            "b",
            "--tangent=true",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            fill_sides(&valued).unwrap(),
            vec![json!({"edge": "a"}), json!({"edge": "b", "tangent": true})]
        );
        let tweak: Vec<String> = ["--face", "p/b/a", "--tangent", "true"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert!(fill_sides(&tweak).unwrap().is_empty());
    }

    #[test]
    fn a_face_tweaks_tangent_takes_a_value() {
        on_cli_stack(|| {
            // #674: `--tangent true|false` on a draft or a move is the
            // tweak's, not a fill side's.
            for (v, want) in [("true", Some(true)), ("false", Some(false))] {
                let f = sent(&["ryucad", "feature", "edit", "p", "d", "--tangent", v])
                    .unwrap_or_else(|e| panic!("--tangent {v}: {e:?}"));
                assert_eq!(f.tangent, want, "--tangent {v}");
            }
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "draft",
                "--neutral",
                "xy",
                "--angle",
                "2deg",
                "--face",
                "p/b/a",
                "--tangent",
            ])
            .unwrap();
            assert_eq!(f.tangent, Some(true));
        });
    }

    #[test]
    fn a_thickens_and_a_sews_flags_reach_their_fields() {
        on_cli_stack(|| {
            let cli = Cli::try_parse_from([
                "ryucad",
                "feature",
                "add",
                "p",
                "thicken",
                "--face",
                "p/b/aa",
                "--face",
                "p/b/bb",
                "--front",
                "2mm",
                "--face-sides",
                "p/b/aa=3mm,0",
                "--operation",
                "join",
                "--target",
                "b",
            ])
            .unwrap();
            let Command::Feature(FeatureCmd::Add { fields, .. }) = &cli.command else {
                panic!()
            };
            let f = fields.to_fields();
            assert_eq!(f.faces, Some(json!(["p/b/aa", "p/b/bb"])));
            assert_eq!(f.front, Some(json!("2mm")));
            assert_eq!(
                f.face_sides,
                Some(json!([{"front": "3mm", "back": "0", "faces": ["p/b/aa"]}]))
            );
            let cli = Cli::try_parse_from([
                "ryucad",
                "feature",
                "add",
                "p",
                "sew",
                "--sheet",
                "a",
                "--sheet",
                "b",
                "--no-solid",
                "--tolerance",
                "0.02mm",
            ])
            .unwrap();
            let Command::Feature(FeatureCmd::Add { fields, .. }) = &cli.command else {
                panic!()
            };
            let f = fields.to_fields();
            assert_eq!(
                f.sheets.as_deref(),
                Some(&["a".to_owned(), "b".to_owned()][..])
            );
            assert_eq!(f.make_solid, Some(false));
            assert_eq!(f.tolerance, Some(json!("0.02mm")));
        });
    }

    #[test]
    fn blend_edits_take_below_and_a_chamfer_face() {
        on_cli_stack(|| {
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "resize_blend",
                "--form",
                "distances",
                "--d1",
                "1mm",
                "--d2",
                "2mm",
                "--chamfer-face",
                "p/b/aa",
                "--face",
                "p/b/bb",
            ])
            .unwrap();
            assert_eq!(f.chamfer_face.as_deref(), Some("p/b/aa"));
            assert_eq!(f.face.as_deref(), Some("p/b/bb"));
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "delete_blend",
                "--below",
                "2mm",
            ])
            .unwrap();
            assert_eq!(f.below, Some(json!("2mm")));
        });
    }

    /// What `ryucad <args>` sends as a feature's fields, after the passes
    /// `main` makes over the command line.
    fn sent(args: &[&str]) -> Result<FeatureFields> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut cli =
            Cli::try_parse_from(&args).map_err(|e| Error::new(Code::Usage, e.to_string()))?;
        if let Command::Feature(FeatureCmd::Add { fields, .. } | FeatureCmd::Edit { fields, .. }) =
            &mut cli.command
        {
            read_in_order(&args, fields)?;
        }
        match request(&cli.command) {
            Some(Request::FeatureAdd { fields, .. } | Request::FeatureEdit { fields, .. }) => {
                Ok(fields)
            }
            _ => panic!("not a feature write"),
        }
    }

    #[test]
    fn drafts_from_the_command_line() {
        on_cli_stack(|| {
            use ryucad::proto::Flip;
            // About a neutral plane: faces, a group at its own angle, a face
            // flipped, the pull reversed, tangency off.
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "draft",
                "--neutral",
                "xy",
                "--angle",
                "2deg",
                "--face",
                "p/b/a",
                "--face",
                "p/b/c",
                "--face-angle",
                "p/b/d=-5deg",
                "--flip",
                "p/b/d",
                "--reverse",
                "--no-tangent",
            ])
            .unwrap();
            assert_eq!(f.neutral.as_deref(), Some("xy"));
            assert_eq!(
                f.faces,
                Some(json!(["p/b/a", "p/b/c", {"angle": "-5deg", "faces": ["p/b/d"]}]))
            );
            assert_eq!(f.flip, Some(Flip::Faces(vec!["p/b/d".to_owned()])));
            assert_eq!((f.reverse, f.tangent), (Some(true), Some(false)));
            assert_eq!(f.angle, Some(json!("2deg")));
            // From a parting line of two edges, with a pull.
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "draft",
                "--parting",
                "p/b/e1",
                "--parting",
                "p/b/e2",
                "--pull",
                "z",
                "--angle",
                "3deg",
                "--face",
                "p/b/a",
            ])
            .unwrap();
            assert_eq!(
                f.parting,
                Some(vec!["p/b/e1".to_owned(), "p/b/e2".to_owned()])
            );
            assert_eq!(f.pull.as_deref(), Some("z"));
            assert_eq!(f.face.as_deref(), Some("p/b/a"));
            // --flip alone, and --flip false, are a datum's or a split's.
            let f = sent(&["ryucad", "feature", "edit", "p", "d1", "--flip"]).unwrap();
            assert_eq!(f.flip, Some(Flip::On(true)));
            let f = sent(&["ryucad", "feature", "edit", "p", "d1", "--flip", "false"]).unwrap();
            assert_eq!(f.flip, Some(Flip::On(false)));
            // --no-flip: no face of a draft flipped.
            let f = sent(&["ryucad", "feature", "edit", "p", "d", "--no-flip"]).unwrap();
            assert_eq!(f.flip, Some(Flip::Faces(Vec::new())));
            // A neutral plane and a parting line together is a usage error.
            let e = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "draft",
                "--neutral",
                "xy",
                "--parting",
                "p/b/e",
            ])
            .unwrap_err();
            assert_eq!(e.code, Code::Usage);
        });
    }

    #[test]
    fn faces_tangents_and_replacements_from_the_command_line() {
        on_cli_stack(|| {
            // One --face is `face`, as a chamfer and a sketch take it.
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "move_face",
                "--by",
                "translate",
                "--direction",
                "x",
                "--distance",
                "5mm",
                "--face",
                "p/b/a",
            ])
            .unwrap();
            assert_eq!((f.face.as_deref(), &f.faces), (Some("p/b/a"), &None));
            assert_eq!(f.tangent, None);
            // Several, with groups of their own and tangency off.
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "move_face",
                "--by",
                "offset",
                "--distance",
                "-0.5mm",
                "--face",
                "p/b/a",
                "--face",
                "p/b/c",
                "--face-distance",
                "p/b/d=1mm",
                "--no-tangent",
            ])
            .unwrap();
            assert_eq!(f.face, None);
            assert_eq!(
                f.faces,
                Some(json!(["p/b/a", "p/b/c", {"distance": "1mm", "faces": ["p/b/d"]}]))
            );
            assert_eq!(f.tangent, Some(false));
            // A turn the other way.
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "move_face",
                "--by",
                "rotate",
                "--axis",
                "x",
                "--angle",
                "-3deg",
                "--face",
                "p/b/a",
            ])
            .unwrap();
            assert_eq!(f.angle, Some(json!("-3deg")));
            // --with takes the faces after it, until the next --with.
            let f = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "replace_face",
                "--with",
                "p/lid/e",
                "--face",
                "p/b/a",
                "--face",
                "p/b/c",
                "--with",
                "xy",
                "--face",
                "p/b/d",
            ])
            .unwrap();
            assert_eq!(
                f.groups,
                Some(json!([{"with": "p/lid/e", "faces": ["p/b/a", "p/b/c"]},
                            {"with": "xy", "faces": ["p/b/d"]}]))
            );
            assert_eq!((f.face, f.faces), (None, None));
            // A face before any --with, when there is one, is a usage error.
            let e = sent(&[
                "ryucad",
                "feature",
                "add",
                "p",
                "replace_face",
                "--face",
                "p/b/a",
                "--with",
                "xy",
            ])
            .unwrap_err();
            assert_eq!(e.code, Code::Usage);
        });
    }

    #[test]
    fn a_tweaks_topology_and_blends_parse() {
        on_cli_stack(|| {
            let cli = Cli::try_parse_from([
                "ryucad",
                "feature",
                "add",
                "p",
                "move_face",
                "--direction",
                "z",
                "--distance",
                "1mm",
                "--face",
                "p/b/x",
                "--topology",
                "change",
                "--blends",
                "rebuild",
            ])
            .unwrap();
            let Command::Feature(FeatureCmd::Add { fields, .. }) = &cli.command else {
                panic!()
            };
            let f = fields.to_fields();
            assert_eq!(
                (f.topology.as_deref(), f.blends.as_deref()),
                (Some("change"), Some("rebuild"))
            );
        });
    }
}
