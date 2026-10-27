//! The request types shared by the CLI, the daemon and every later
//! adapter, and the JSON-RPC 2.0 framing they travel in. One definition,
//! so they never diverge.

use crate::error::{Code, Error, Result};
use crate::live::{Kind, SelectMode, Style};
use crate::units::Unit;
use crate::view::CameraArg;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The protocol version. A CLI and a daemon talk only when theirs match.
pub const PROTOCOL_VERSION: u32 = 2;

/// What `ryucad version` reports. Future cache keys are built from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub ryucad: String,
    pub ryukei: String,
    pub commit: String,
    pub target: String,
    pub protocol: u32,
}

impl VersionInfo {
    /// This binary's version information.
    pub fn current() -> Self {
        Self {
            ryucad: env!("CARGO_PKG_VERSION").to_owned(),
            ryukei: ryukei::VERSION.to_owned(),
            commit: env!("RYUCAD_COMMIT").to_owned(),
            target: env!("RYUCAD_TARGET").to_owned(),
            protocol: PROTOCOL_VERSION,
        }
    }
}

pub use ryu_session::rpc::{Answer, RPC_ERROR, parse_rpc_response, rpc_response};

/// How a request wants bulk data: in the JSON, or as bytes after it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    #[default]
    Json,
    Binary,
}

/// Where a feature goes in a part's list: after `after`, first, or (with
/// neither) at the end.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub first: bool,
}

/// The fields of a feature that only some types take: a sketch's plane,
/// what an extrude or revolve sweeps and how far. Each is left out unless
/// given.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureFields {
    /// A sketch's origin plane: `xy`, `yz` or `zx`; a mirror pattern's,
    /// or a planar face as `<part>/<feature>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plane: Option<String>,
    /// A planar face to sketch on, or the face a chamfer's first distance
    /// runs along: `<part>/<feature>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<String>,
    /// A sketch on a face: a vertex for its origin (`""` for the default,
    /// the part origin projected onto the face).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_origin: Option<String>,
    /// A sketch on a face: a straight edge for its `x` (`""` for the
    /// default, the part's `x` projected onto the face).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_x: Option<String>,
    /// The sketch an extrude or revolve sweeps, or a hole drills at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sketch: Option<String>,
    /// An entity on each loop to sweep; empty means every closed loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loops: Option<Vec<String>>,
    /// An extrude's extent: `{"type": "blind" | "symmetric", "length"}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extent: Option<Value>,
    /// Extrude against the sketch's normal; a linear pattern against its
    /// direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<bool>,
    /// A revolve's axis: a line of its sketch, or a straight edge as
    /// `<part>/<feature>/<name>`. A circular pattern's: `x`, `y`, `z`, or
    /// an edge or face as `<part>/<feature>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    /// A revolve's angle, or `"full"`; a chamfer's angle to its face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<Value>,
    /// Revolve half the angle each side of the sketch's plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symmetric: Option<bool>,
    /// What a primitive, extrude or revolve does with its body: `new`
    /// (the default), `join`, `cut` or `intersect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// The solids a join, cut or intersect acts on, by their founders; a
    /// boolean's one target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<String>>,
    /// How a boolean combines its target with its tools: `unite`,
    /// `subtract` or `intersect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// A boolean's target solid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// A boolean's tool solids, which it consumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// A fillet's or chamfer's groups, each as a file has it but with its
    /// edges and face as `<part>/<feature>/<name>` (or `{feature, name}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Value>,
    /// Whether a fillet or chamfer grows each edge to its tangent chain
    /// (true when left out of a new one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub propagate: Option<bool>,
    /// A fillet's one group: its radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<Value>,
    /// A fillet's or chamfer's one group: its edges, as
    /// `<part>/<feature>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<String>>,
    /// A chamfer's one group: `equal` (the default), `distances` or
    /// `distance_angle`. A hole's: `simple` (the default), `counterbore`
    /// or `countersink`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    /// A chamfer's one group: its distance along both faces (`equal`), or
    /// along `face` (`distance_angle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<Value>,
    /// A chamfer's one group, by `distances`: its distance along `face`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d1: Option<Value>,
    /// A chamfer's one group, by `distances`: its distance along the other
    /// face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d2: Option<Value>,
    /// The points of its sketch a hole drills at; empty means every point
    /// no curve uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<String>>,
    /// A hole's diameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diameter: Option<Value>,
    /// A counterbored hole's counterbore, `{diameter, depth}`, or either.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterbore: Option<Value>,
    /// A countersunk hole's countersink, `{diameter, angle}`, or either.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub countersink: Option<Value>,
    /// A hole's depth: `{"blind": length}`, `"through_all"`, or
    /// `{"up_to": {target, offset}}` with its target as
    /// `<part>/<feature>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<Value>,
    /// A hole's bottom: `"flat"` (the default) or `{"drill": angle}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<Value>,
    /// The joins, cuts and holes a pattern repeats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
    /// A linear pattern's direction: `x`, `y`, `z`, or a straight edge or
    /// planar face as `<part>/<feature>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// A linear pattern's spacing along its direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacing: Option<Value>,
    /// How many instances a pattern has along its direction or round its
    /// axis, the seed's included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<Value>,
    /// A linear pattern's second direction, as `direction`; `""` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction2: Option<String>,
    /// A linear pattern's spacing along its second direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacing2: Option<Value>,
    /// How many instances along a linear pattern's second direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count2: Option<Value>,
    /// A linear pattern against its second direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse2: Option<bool>,
    /// The instances a pattern leaves out: `i`, or `[i, j]` with two
    /// directions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<Value>,
    /// The part an instance places.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    /// The revision an instance places: `working` (the default) or a
    /// release label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Whether mates hold an instance where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    /// A mate's first connector: an entity of an occurrence,
    /// `<assembly>/<instance>[/…]/<solid>/<name>`, or an occurrence's own
    /// frame, `<assembly>/<instance>[/…]`. A path may start at a pattern's
    /// copy, `<pattern>:<i>/<seed>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a: Option<String>,
    /// A mate's second connector, as `a`; its frame follows `a`'s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<String>,
    /// `a`'s offset in its own axes: `{origin: [x, y, z], rotate, flip}`,
    /// any part; `{}` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_offset: Option<Value>,
    /// `b`'s offset, as `a_offset`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b_offset: Option<Value>,
    /// A slider's, cylindrical's or planar mate's distance along `a`'s z;
    /// `""` lets it go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<Value>,
    /// The instances an instance pattern copies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<Vec<String>>,
}

impl FeatureFields {
    /// The fields given, as a feature file has them.
    pub fn given(&self) -> Map<String, Value> {
        match serde_json::to_value(self).expect("fields serialize") {
            Value::Object(m) => m,
            _ => unreachable!("a struct serializes to an object"),
        }
    }
}

/// A placement as given by an agent: any part left out keeps its current
/// value (or the identity's, for a new feature).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementArg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<[Value; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<[f64; 3]>,
}

/// How `assembly.solve` moves one instance, as `assembly.move` does.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceMove {
    pub instance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementArg>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate: Option<[Value; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<Value>,
}

/// How one merge conflict is settled: by one side's value, a value given,
/// or one of its planned repairs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolution {
    /// The conflict's ID, as `merge.status` gives it.
    pub conflict: String,
    /// `ours`, `theirs`, `value` (with `value`) or `repair` (with `repair`).
    pub choice: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<String>,
}

/// Every request a session answers, by JSON-RPC method name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "project.init")]
    ProjectInit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_unit: Option<Unit>,
    },
    #[serde(rename = "project.info")]
    ProjectInfo {},
    /// Creates an empty part, or with `kind` `assembly` an assembly.
    #[serde(rename = "part.create")]
    PartCreate {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
    #[serde(rename = "part.delete")]
    PartDelete {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "part.list")]
    PartList {},
    #[serde(rename = "feature.add")]
    FeatureAdd {
        part: String,
        #[serde(rename = "type")]
        ty: String,
        #[serde(default)]
        params: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<PlacementArg>,
        #[serde(default, flatten)]
        fields: FeatureFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        position: Position,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "feature.edit")]
    FeatureEdit {
        part: String,
        id: String,
        /// Merged into the current parameters; `null` removes one.
        #[serde(default)]
        params: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<PlacementArg>,
        #[serde(default, flatten)]
        fields: FeatureFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "feature.delete")]
    FeatureDelete {
        part: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "feature.move")]
    FeatureMove {
        part: String,
        id: String,
        #[serde(flatten)]
        position: Position,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "feature.list")]
    FeatureList { part: String },
    #[serde(rename = "feature.show")]
    FeatureShow { part: String, id: String },
    #[serde(rename = "param.list")]
    ParamList { part: String },
    /// Sets or adds parameters, each an expression.
    #[serde(rename = "param.set")]
    ParamSet {
        part: String,
        parameters: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Removes a parameter nothing reads.
    #[serde(rename = "param.delete")]
    ParamDelete {
        part: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Renames a parameter and every expression that reads it.
    #[serde(rename = "param.rename")]
    ParamRename {
        part: String,
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Gives the project its ID and ryupdm's default settings.
    #[serde(rename = "pdm.init")]
    PdmInit {},
    /// ryupdm's settings, with their defaults filled in.
    #[serde(rename = "pdm.config")]
    PdmConfig {},
    /// Changes ryupdm's settings; what isn't given stays.
    #[serde(rename = "pdm.configure")]
    PdmConfigure {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revisions: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release_branch: Option<String>,
        /// A part number pattern, or `""` for none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        /// Custom property declarations by name; `null` removes one.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        properties: Map<String, Value>,
        /// Write even if parts' properties stop fitting.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A part's properties: every core and declared one, set or not.
    #[serde(rename = "props.get")]
    PropsGet { part: String },
    /// Sets a part's properties; `null` clears one.
    #[serde(rename = "props.set")]
    PropsSet {
        part: String,
        properties: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Writes the record of a part's next revision, as a proposal.
    #[serde(rename = "release.propose")]
    ReleasePropose {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
        /// Write nothing: answer the label it would take and every reason
        /// it would be refused.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
    },
    /// Release records (or one part's), where each became official, their
    /// tags, and each part's state.
    #[serde(rename = "release.list")]
    ReleaseList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
    /// The release checks; with `results`, official releases are rebuilt.
    #[serde(rename = "release.check")]
    ReleaseCheck {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        results: bool,
    },
    /// Creates the local tag of every official release that has none, and
    /// moves those that point elsewhere. Logged, not undoable.
    #[serde(rename = "release.sync")]
    ReleaseSync {},
    /// Writes an obsolete record for a part's latest official revision.
    #[serde(rename = "release.obsolete")]
    ReleaseObsolete {
        part: String,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Applies a batch of sketch edits with one solve; atomic.
    #[serde(rename = "sketch.edit")]
    SketchEdit {
        part: String,
        sketch: String,
        #[serde(default)]
        ops: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Solves a sketch, or a proposed batch of edits, without writing.
    #[serde(rename = "sketch.solve")]
    SketchSolve {
        part: String,
        sketch: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ops: Vec<Value>,
    },
    /// A sketch's solved geometry, states and report, and how it is drawn.
    #[serde(rename = "sketch.get")]
    SketchGet {
        part: String,
        sketch: String,
        /// coarse, normal or fine: how finely arcs and circles are drawn;
        /// the user's display quality by default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quality: Option<String>,
    },
    /// Moves an instance of an assembly: to the pose `placement` gives
    /// (what it leaves out stays), then by `translate` and a turn of
    /// `angle` about `axis` through its origin, in the assembly's frame.
    #[serde(rename = "assembly.move")]
    AssemblyMove {
        part: String,
        instance: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<PlacementArg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        translate: Option<[Value; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<[f64; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        angle: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// An assembly's mates solved, writing nothing: as it is, or with
    /// `moves` held where they put instances for a first solve. Each
    /// instance's pose and the solve's report. With `component`, only
    /// what the moved instances' mates reach is solved and answered, from
    /// the poses the assembly's rebuild has (a drag's preview).
    #[serde(rename = "assembly.solve")]
    AssemblySolve {
        part: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        moves: Vec<InstanceMove>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        component: bool,
    },
    /// An assembly's instances as a tree: each with its part, revision and
    /// how it resolved, its pose in the assembly, and a subassembly's own.
    #[serde(rename = "assembly.tree")]
    AssemblyTree { part: String },
    /// Which solids of an assembly's occurrences overlap: every pair whose
    /// boxes do, or those with a side under `instances` (instance paths
    /// under the assembly, as `s1/b2` or `holes:2/bolt`).
    #[serde(rename = "interference")]
    Interference {
        part: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        instances: Vec<String>,
    },
    /// Imports a STEP file the daemon can read (absolute, or relative to
    /// the project): stores it under `imports/` and makes a part of each
    /// product's bodies and an assembly of each product's children
    /// (`part` names the file's one root), or with `into` adds one
    /// product's bodies to a part. With `update`, the file's bytes
    /// replace that imported file and what was made of it follows.
    /// `dry_run` answers the plan, writing nothing.
    #[serde(rename = "import")]
    Import {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        into: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        product: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        /// The project's revision, or with `into` that part's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
        /// A file under `imports/` to replace: re-import.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update: Option<String>,
    },
    #[serde(rename = "rebuild")]
    Rebuild {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        /// A Git revision to rebuild instead of the working files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    #[serde(rename = "inspect")]
    Inspect {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<String>,
        /// A Git revision to inspect instead of the working files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    #[serde(rename = "check_format")]
    CheckFormat {},
    /// Rebuilds the project from its files alone and verifies it.
    #[serde(rename = "check")]
    Check {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        /// Also report what changed from this revision, as `diff --model`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        against: Option<String>,
        /// Also rebuild official releases and compare their bodies.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        results: bool,
    },
    /// What changed between two revisions (`HEAD` and the working files by
    /// default): files as ryucad reads them, and with `model` the bodies.
    #[serde(rename = "diff")]
    Diff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        /// Rebuild both and add what changed in the bodies.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        model: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        positions: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
    },
    /// What the merge Git is in the middle of leaves to settle.
    #[serde(rename = "merge.status")]
    MergeStatus {},
    /// Settles merge conflicts, as one change.
    #[serde(rename = "merge.resolve")]
    MergeResolve {
        resolutions: Vec<Resolution>,
        /// The part revisions the caller read, by part.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        expected_revisions: BTreeMap<String, String>,
    },
    #[serde(rename = "daemon.status")]
    DaemonStatus {},
    /// Registers the connection as a participant of the live session.
    #[serde(rename = "session.hello")]
    SessionHello {
        kind: Kind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Join this participant instead (an app's second connection).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    #[serde(rename = "participant.list")]
    ParticipantList {},
    /// Turns the connection into a subscription: after the answer, it
    /// receives notifications for `topics` (all when empty).
    #[serde(rename = "subscribe")]
    Subscribe {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        topics: Vec<String>,
    },
    /// A participant's selection: `participant`, or the one human.
    #[serde(rename = "selection.get")]
    SelectionGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// Changes the caller's own selection.
    #[serde(rename = "selection.set")]
    SelectionSet {
        #[serde(default)]
        entities: Vec<String>,
        #[serde(default)]
        mode: SelectMode,
    },
    /// A participant's camera: `participant`, or the one human.
    #[serde(rename = "camera.get")]
    CameraGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// Sets the caller's own camera: explicitly, or from a standard `view`
    /// and/or framing the entities in `fit` (the whole model when empty).
    #[serde(rename = "camera.set")]
    CameraSet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        camera: Option<CameraArg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fit: Vec<String>,
    },
    /// What a participant (`participant`, or the one human) compares the
    /// working files against, with the commit that revision names now.
    #[serde(rename = "compare.get")]
    CompareGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// Puts a participant into a compare with Git revision `rev`, or takes
    /// it out (`off`): `participant`, else the one human, else the caller.
    #[serde(rename = "compare.set")]
    CompareSet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        off: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// What a participant's window shows: with `part`, that part or
    /// assembly; with `project`, the project's model; with neither, what it
    /// shows now. `participant`, else the one human, else the caller.
    #[serde(rename = "view.focus")]
    ViewFocus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        project: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// Changes what a participant's window hides (`participant`, else the
    /// one human, else the caller): everything shown first with `all`,
    /// then everything else in their parts hidden for `isolate`, then
    /// `show`, then `hide`. References name parts, occurrences (at any
    /// depth, a subassembly's reaching all under it) or a part's solids.
    #[serde(rename = "visibility.set")]
    VisibilitySet {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        hide: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        show: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        isolate: Vec<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// Highlights entities for everyone to see, replacing the caller's
    /// highlight of the same style and label.
    #[serde(rename = "highlight.set")]
    HighlightSet {
        entities: Vec<String>,
        #[serde(default)]
        style: Style,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Removes the caller's highlights of `style`, or all of them.
    #[serde(rename = "highlight.clear")]
    HighlightClear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<Style>,
    },
    /// Entries of the activity log older than `before`, oldest first.
    #[serde(rename = "activity.log")]
    ActivityLog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
    },
    /// Undoes `entry`, or the caller's latest change.
    #[serde(rename = "undo")]
    Undo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<u64>,
    },
    /// Redoes `entry` (an undo), or the caller's latest undo.
    #[serde(rename = "redo")]
    Redo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<u64>,
    },
    /// One setting, or all of them (of `scope`, if given).
    #[serde(rename = "config.get")]
    ConfigGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    /// Sets a project setting (in `ryucad.json`) or a user preference.
    #[serde(rename = "config.set")]
    ConfigSet {
        key: String,
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        /// For project settings: the project revision last seen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// What the disk cache of results and meshes holds: its folder, its
    /// bound, and its entries and bytes, all and by kind.
    #[serde(rename = "cache.stats")]
    CacheStats {},
    /// Empties the disk cache; answers what it removed.
    #[serde(rename = "cache.clear")]
    CacheClear {},
    /// The project's Git commits (or a part's), and what is uncommitted.
    #[serde(rename = "history.log")]
    HistoryLog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Display meshes of a part's bodies (or one feature's).
    #[serde(rename = "mesh.get")]
    MeshGet {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feature: Option<String>,
        /// coarse, normal or fine; the user's display quality by default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quality: Option<String>,
        /// `binary` sends `ryucad.mesh.1` bytes after the answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transfer: Option<Transfer>,
        /// A Git revision to mesh instead of the working files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// Draws a part (or the whole project) to a PNG, sent as bytes after
    /// the answer. The view is `camera`, a participant's camera, a standard
    /// `view`, or iso framing the model. `rev` renders a Git revision;
    /// `compare` paints what changed since a revision and ghosts it;
    /// `frame` widens the framing to other revisions.
    #[serde(rename = "render")]
    Render {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        camera: Option<CameraArg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        /// Entities to point out, drawn as notes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        highlight: Vec<String>,
        /// Draw edges and silhouettes (default true).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edges: Option<bool>,
        /// Label highlighted entities and sketches' dimensions (default
        /// false).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        labels: Option<bool>,
        /// Draw sketches (default true).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sketches: Option<bool>,
        /// Render the project as this Git revision has it (the working
        /// files by default).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        /// Frame these revisions' models too (`working` for the working
        /// files), so renders of each share a camera.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        frame: Vec<String>,
        /// Paint what changed since this revision (faces new and changed)
        /// and ghost what it had, as the app's compare view does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compare: Option<String>,
        /// Leave these out (parts, occurrences, solids); the participant's
        /// hidden set by default, else the caller's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hidden: Option<Vec<String>>,
    },
    /// Diagnoses the project folder: every problem, with a stable code.
    #[serde(rename = "doctor")]
    Doctor {},
    /// Applies repairs `doctor` offered, by ID.
    #[serde(rename = "doctor.repair")]
    DoctorRepair {
        repairs: Vec<String>,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        params: Map<String, Value>,
    },
    /// Says what the caller is doing; an empty status clears it.
    #[serde(rename = "activity.set")]
    ActivitySet {
        status: String,
        #[serde(default)]
        entities: Vec<String>,
    },
    /// Asks attached app windows to come to the front: `participant`'s,
    /// or every human's.
    #[serde(rename = "app.open")]
    AppOpen {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    #[serde(rename = "daemon.shutdown")]
    DaemonShutdown {},
}

impl Request {
    /// The request for JSON-RPC `method` with `params` (absent means `{}`).
    pub fn from_rpc(method: &str, params: Option<Value>) -> Result<Request> {
        let params = params.unwrap_or_else(|| json!({}));
        serde_json::from_value(json!({"method": method, "params": params})).map_err(|e| {
            let msg = e.to_string();
            let code = if msg.starts_with("unknown variant") {
                Code::Usage
            } else {
                Code::InvalidDocument
            };
            Error::new(code, format!("{method}: {msg}")).with("method", method)
        })
    }

    pub fn method(&self) -> String {
        self.to_value()["method"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    pub fn params(&self) -> Value {
        self.to_value()["params"].take()
    }

    /// Whether the request writes the project's files.
    pub fn mutates(&self) -> bool {
        if let Request::ConfigSet { key, .. } = self {
            return crate::config::scope_of(key).is_ok_and(|s| s == crate::config::Scope::Project);
        }
        matches!(
            self,
            Request::ProjectInit { .. }
                | Request::PartCreate { .. }
                | Request::PartDelete { .. }
                | Request::FeatureAdd { .. }
                | Request::FeatureEdit { .. }
                | Request::FeatureDelete { .. }
                | Request::FeatureMove { .. }
                | Request::ParamSet { .. }
                | Request::ParamDelete { .. }
                | Request::ParamRename { .. }
                | Request::PdmInit { .. }
                | Request::PdmConfigure { .. }
                | Request::PropsSet { .. }
                | Request::ReleasePropose { dry_run: false, .. }
                | Request::Import { dry_run: false, .. }
                | Request::ReleaseObsolete { .. }
                | Request::ReleaseSync { .. }
                | Request::SketchEdit { .. }
                | Request::AssemblyMove { .. }
                | Request::Undo { .. }
                | Request::Redo { .. }
                | Request::DoctorRepair { .. }
                | Request::MergeResolve { .. }
        )
    }

    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("requests serialize")
    }
}

/// One JSON-RPC request line.
pub fn rpc_request(id: u64, req: &Request) -> String {
    ryu_session::rpc::request_line(id, &req.method(), req.params())
}

/// Parses one JSON-RPC request line into its id and request.
pub fn parse_rpc_request(line: &str) -> (Value, Result<Request>) {
    let (id, call) = ryu_session::rpc::parse_request_line(line);
    (
        id,
        call.and_then(|(method, params)| Request::from_rpc(&method, params)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_through_rpc() {
        let reqs = [
            Request::ProjectInfo {},
            Request::ProjectInit {
                default_unit: Some(Unit::In),
            },
            Request::FeatureAdd {
                part: "p".into(),
                ty: "box".into(),
                params: [("x".to_owned(), json!("1 mm"))].into_iter().collect(),
                placement: Some(PlacementArg {
                    z: Some([0.0, 1.0, 0.0]),
                    ..Default::default()
                }),
                fields: FeatureFields::default(),
                id: Some("b".into()),
                position: Position {
                    after: Some("a".into()),
                    first: false,
                },
                expected_revision: None,
            },
            Request::FeatureAdd {
                part: "p".into(),
                ty: "sketch".into(),
                params: Map::new(),
                placement: None,
                fields: FeatureFields {
                    plane: Some("zx".into()),
                    ..Default::default()
                },
                id: None,
                position: Position::default(),
                expected_revision: None,
            },
            Request::FeatureMove {
                part: "p".into(),
                id: "b".into(),
                position: Position {
                    after: None,
                    first: true,
                },
                expected_revision: Some("r".into()),
            },
            Request::Rebuild {
                part: None,
                rev: None,
            },
            Request::SessionHello {
                kind: Kind::Human,
                name: Some("Dana".into()),
                participant: None,
            },
            Request::ParticipantList {},
            Request::CompareSet {
                rev: Some("HEAD~1".into()),
                off: false,
                participant: Some("p-00000001".into()),
            },
            Request::CompareSet {
                rev: None,
                off: true,
                participant: None,
            },
            Request::PdmConfigure {
                revisions: Some("numeric".into()),
                release_branch: None,
                pattern: Some(String::new()),
                properties: [("finish".to_owned(), Value::Null)].into_iter().collect(),
                force: true,
                expected_revision: None,
            },
            Request::PropsSet {
                part: "p".into(),
                properties: [("number".to_owned(), json!("100"))].into_iter().collect(),
                expected_revision: Some("r".into()),
            },
            Request::ReleasePropose {
                part: "p".into(),
                note: Some("why".into()),
                expected_revision: None,
                dry_run: false,
            },
            Request::ReleasePropose {
                part: "p".into(),
                note: None,
                expected_revision: None,
                dry_run: true,
            },
            Request::ReleaseObsolete {
                part: "p".into(),
                label: "A".into(),
                note: None,
                expected_revision: Some("r".into()),
            },
        ];
        for (i, r) in reqs.into_iter().enumerate() {
            let line = rpc_request(i as u64, &r);
            assert!(!line.contains('\n'));
            let (id, back) = parse_rpc_request(&line);
            assert_eq!(id, json!(i));
            assert_eq!(back.unwrap(), r);
        }
    }

    #[test]
    fn missing_params_mean_empty() {
        let (_, r) = parse_rpc_request(r#"{"jsonrpc":"2.0","id":1,"method":"part.list"}"#);
        assert_eq!(r.unwrap(), Request::PartList {});
    }

    #[test]
    fn bad_requests() {
        let (_, r) = parse_rpc_request(r#"{"jsonrpc":"2.0","id":1,"method":"part.explode"}"#);
        assert_eq!(r.unwrap_err().code, Code::Usage);
        let (_, r) = parse_rpc_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"part.create","params":{"prt":"x"}}"#,
        );
        assert_eq!(r.unwrap_err().code, Code::InvalidDocument);
        // A misspelt optional field must not be silently ignored.
        for line in [
            r#"{"jsonrpc":"2.0","id":1,"method":"part.create","params":{"part":"x","bogus":1}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"feature.delete","params":{"part":"p","id":"a","expected_revison":"r"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"feature.add","params":{"part":"p","type":"box","expected_revison":"r"}}"#,
        ] {
            let (_, r) = parse_rpc_request(line);
            assert_eq!(r.unwrap_err().code, Code::InvalidDocument, "{line}");
        }
        let (id, r) = parse_rpc_request("{nope");
        assert_eq!((id, r.unwrap_err().code), (Value::Null, Code::InvalidJson));
        let (_, r) = parse_rpc_request(r#"{"id":1,"method":"part.list"}"#);
        assert_eq!(r.unwrap_err().code, Code::Usage);
    }

    #[test]
    fn responses_round_trip() {
        let ok = rpc_response(json!(3), &Ok(json!({"a": 1})));
        assert_eq!(parse_rpc_response(&ok).unwrap(), json!({"a": 1}));
        let e = Error::new(Code::Conflict, "changed").with("expected", "x");
        let line = rpc_response(json!(3), &Err(e.clone()));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["error"]["code"], RPC_ERROR);
        assert_eq!(parse_rpc_response(&line).unwrap_err(), e);
        assert_eq!(
            parse_rpc_response("garbage").unwrap_err().code,
            Code::Daemon
        );
    }

    #[test]
    fn only_writers_mutate() {
        assert!(
            Request::PartCreate {
                part: "p".into(),
                kind: None,
            }
            .mutates()
        );
        assert!(
            Request::MergeResolve {
                resolutions: Vec::new(),
                expected_revisions: BTreeMap::new(),
            }
            .mutates()
        );
        assert!(!Request::MergeStatus {}.mutates());
        assert!(
            !Request::Rebuild {
                part: None,
                rev: None,
            }
            .mutates()
        );
        assert_eq!(
            Request::Rebuild {
                part: None,
                rev: None,
            }
            .method(),
            "rebuild"
        );
    }
}
