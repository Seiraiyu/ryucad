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

pub use ryu_session::rpc::{Answer, RPC_ERROR, Raw, parse_rpc_response, rpc_response};

/// How a request wants bulk data: in the JSON, or as bytes after it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    #[default]
    Json,
    Binary,
    /// Bytes after the answer in a compact encoding (`ryucad.mesh.2`),
    /// for a reader that knows it: the app.
    Compact,
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

/// A pin as a request gives it: an entity reference (`ref`, optionally
/// with the clicked `point`), or a bare `point` in `part`'s frame. A
/// point's lengths are quantities, bare numbers in the project's unit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinArg {
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<[Value; 3]>,
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
    /// A planar face to sketch on, the face a chamfer's first distance
    /// runs along, or the planar face whose loops a sweep carries:
    /// `<part>/<feature>/<name>`.
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
    /// The sketch an extrude, revolve or sweep sweeps, or a hole drills
    /// at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sketch: Option<String>,
    /// An entity on each loop to sweep (for a sweep of a face, an edge or
    /// vertex of it by name); empty means every closed loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loops: Option<Vec<String>>,
    /// An extrude's extent: `{"type": "blind" | "symmetric", "length"}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extent: Option<Value>,
    /// Extrude against the sketch's normal; a linear pattern against its
    /// direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<bool>,
    /// Sweep the profile's loops and open chains into a sheet of its own
    /// (an extrude, revolve, sweep or loft; ryucad 18); false a solid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<bool>,
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
    /// A boolean's target solid; a shell's or offset's.
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
    /// planar face as `<part>/<feature>/<name>`. A binormal sweep's: any
    /// axis, or a plane's normal (a datum, a helix's axis by its ID). A
    /// shell's: `inward` or `outward`.
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
    /// `""` lets it go. An offset's: positive outwards, zero allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<Value>,
    /// The instances an instance pattern copies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<Vec<String>>,
    /// How a datum plane, axis or point is built: a plane `offset`,
    /// `angle`, `three_points`, `parallel`, `axis_point`, `normal_to` or
    /// `midplane`; an axis `of`, `two_points`, `planes` or `normal`; a
    /// point `of`, `coordinates`, `along`, `intersection`, `projection` or
    /// `midpoint`. A new `by` keeps only the fields it takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// A datum plane's base plane (`offset`, `angle`, `parallel`), or
    /// where a point along a curve is measured from (`start`, `end`).
    /// Geometric references are text: `xy`, `x`, `origin`,
    /// `<part>/<feature>`, `<part>/<feature>/<name>`,
    /// `<part>/<sketch>/<entity>`, or a bare feature ID of the part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// One point a datum takes (`axis_point`, `normal`, `of`,
    /// `projection`); `points` takes several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<String>,
    /// The axis an `angle` plane turns about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// The point a `parallel` plane passes through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,
    /// The curve a `normal_to` plane or an `along` point is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<String>,
    /// Where on its curve a `normal_to` plane is: `start`, `end` or a
    /// point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Two planes (`midplane`, `planes`), or what an `intersection`
    /// point meets (its file's `of`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub between: Option<Vec<String>>,
    /// A `coordinates` point's three lengths (its file's `at`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<Vec<Value>>,
    /// The plane a `coordinates` point's are in; `""` for the part's.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub in_plane: Option<String>,
    /// How far along its curve an `along` point is, from 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<Value>,
    /// What a `projection` point is projected onto: a plane or an axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onto: Option<String>,
    /// A datum plane's frame origin: a point, projected; `""` for the
    /// construction's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// A datum plane's frame `x`: an axis, projected; `""` for the
    /// construction's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    /// A datum plane's normal, or a datum axis's direction, turned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip: Option<bool>,
    /// A path's segments in order, replacing them all: each a curve as
    /// text (an edge `<part>/<solid>/<name>`, a sketch's line, arc or
    /// circle `<part>/<sketch>/<entity>`, a path or helix
    /// `<part>/<feature>` or a bare ID), or `{"curve", "propagate": true}`
    /// for an edge grown to its tangent chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<Value>>,
    /// Where a helix starts: a point, its distance from the axis the
    /// radius (in place of `radius`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// A helix's rise per turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<Value>,
    /// A helix's number of turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<Value>,
    /// A helix's height along its axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<Value>,
    /// A helix's hand: `right` (the default) or `left`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hand: Option<String>,
    /// A helix's taper, an angle (positive widens along the axis); `""`
    /// for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taper: Option<Value>,
    /// What a sweep follows: any curve as text (a path or helix
    /// `<part>/<feature>` or its ID, an edge `<part>/<solid>/<name>`, a
    /// sketch's line, arc or circle `<part>/<sketch>/<entity>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How a sweep carries its profile: `minimal` (rotation-minimizing)
    /// or `binormal` (about `direction`). A new sweep that leaves it out
    /// gets `minimal`, or along exactly one helix `binormal` about its
    /// axis, written into its file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub law: Option<String>,
    /// How far a sweep turns its profile about the path from start to
    /// end, an angle; `""` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twist: Option<Value>,
    /// A sweep's end size over its start size, a positive number; `""`
    /// for 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Value>,
    /// A loft's sections, in order, replacing them all: each text (a
    /// sketch's ID or `<part>/<sketch>`, or a planar face
    /// `<part>/<feature>/<name>`), or `{"section": text, "loops": [..]}`
    /// naming each loop and where it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<Value>>,
    /// Whether a loft runs round from its last section to its first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
    /// Which way a loft leaves its first section: `normal`, or an axis or
    /// a plane as text; `""` for no tangent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_tangent: Option<String>,
    /// A loft's start tangent's weight, a positive number times the
    /// distance between its end sections' centroids; `""` for 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_weight: Option<Value>,
    /// Which way a loft enters its last section, as `start_tangent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_tangent: Option<String>,
    /// A loft's end tangent's weight, as `start_weight`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_weight: Option<Value>,
    /// A shell's wall, a length more than nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickness: Option<Value>,
    /// The faces a shell leaves open, each `<part>/<solid>/<name>`;
    /// given, they replace them all (`[]` for none: a closed shell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<Vec<String>>,
    /// A shell's or offset's face groups, replacing them all:
    /// `[{"thickness" | "distance": Q, "faces": ["<part>/<solid>/<name>", ...]}]`
    /// (`[]` for none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<Value>,
    /// The solid a mirror copies, by the feature that founded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solid: Option<String>,
    /// Which sides a split keeps: `both` (the default) or `behind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<String>,
    /// A base flange's or unfold's bend allowance: `{"k_factor": n}`,
    /// `{"deduction": length}` or (an unfold's) `{"allowance": length}`;
    /// `""` drops an unfold's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowance: Option<Value>,
    /// A base flange's or flange's relief: `{"kind": none | rectangular |
    /// obround | tear, width?, depth?}`; `""` drops a flange's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relief: Option<Value>,
    /// A flange's length (also read from an extent of only a length).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<Value>,
    /// Where a flange's length is measured from: outer_mould, inner_mould
    /// or tangent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<String>,
    /// Where a flange's bend sits: material_inside, material_outside or
    /// bend_outside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    /// A flange's setback at both ends of every edge; `""` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setback: Option<Value>,
    /// A flange's ends, each `{vertex, setback?, relief?}`, `[]` for none:
    /// given, they replace them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends: Option<Value>,
    /// The side face each named edge of a flange folds towards, each
    /// `{edge, side}`; `[]` takes every side away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sides: Option<Value>,
    /// An unfold's fixed face, a side face of the flange held still
    /// (`<part>/<solid>/<name>`, written `fixed`); `""` for the kernel's
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_face: Option<String>,
    /// An unfold's or refold's bends, each by one of its faces; given,
    /// they replace them all (`[]` for every bend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    /// An unfold's allowance overrides, replacing them all:
    /// `[{"bend": "<part>/<solid>/<name>", "k_factor" | "deduction" |
    /// "allowance": Q}]` (`[]` for none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bends: Option<Value>,
    /// The unfold a refold folds back by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfold: Option<String>,
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
        /// Expressions by JSON pointer, each one of the feature's
        /// expression fields (as `equations.list` lists them), given
        /// alone.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        expressions: Map<String, Value>,
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
    ParamList {
        /// The part, or none with `scope: project`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<crate::param::Scope>,
    },
    /// Sets or adds parameters, each an expression.
    #[serde(rename = "param.set")]
    ParamSet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<crate::param::Scope>,
        parameters: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Removes a parameter nothing reads.
    #[serde(rename = "param.delete")]
    ParamDelete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<crate::param::Scope>,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Renames a parameter and every expression that reads it.
    #[serde(rename = "param.rename")]
    ParamRename {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<crate::param::Scope>,
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Gives the project its ID and ryupdm's default settings.
    #[serde(rename = "pdm.init")]
    PdmInit {},
    /// Gives a copied project an ID of its own, naming the one it had and
    /// the commit it was at as `forked_from`.
    #[serde(rename = "pdm.new_id")]
    PdmNewId {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
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
        /// Approval rules and trusted keys as `ryupdm.json` holds them
        /// (`{"rules": […], "keys": {…}}`), or `false` to remove them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approvals: Option<Value>,
        /// `none` or `pr` (`dependencies.bump`): whether the hosted
        /// service opens pull requests moving to newer releases.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bump: Option<String>,
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
    /// An assembly's bill of materials, from its files alone: indented, or
    /// flat (every leaf with its total), as JSON or CSV, at the working
    /// files, a Git revision or `label:<L>`.
    #[serde(rename = "bom.get")]
    BomGet {
        /// The part or assembly; the project's one root assembly if left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flat: bool,
        /// Levels below the root to list (the indented BOM only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<usize>,
        /// `json` (the default) or `csv`, which adds the CSV as `csv`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// What changed in a BOM between two revisions (`HEAD` and `working`
    /// by default).
    #[serde(rename = "bom.diff")]
    BomDiff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    /// Every assembly that uses a part, in the working files and in every
    /// official release.
    #[serde(rename = "bom.where_used")]
    BomWhereUsed {
        part: String,
        /// Only uses of this release label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Only uses at `working`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        working: bool,
        /// Only assemblies that place it themselves.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        direct: bool,
        /// Other projects' folders to search too (absolute); none for the
        /// `pdm.dependents` preference's.
        #[serde(rename = "in", default, skip_serializing_if = "Vec::is_empty")]
        within: Vec<String>,
        /// Search no other folders.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        no_dependents: bool,
    },
    /// Sets an assembly's item numbers (`items`, part to number), or with
    /// `compact` numbers its parts 1 to n in their current order.
    #[serde(rename = "bom.renumber")]
    BomRenumber {
        part: String,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        items: Map<String, Value>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        compact: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
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
    /// A path's or helix's polyline in model space, as the app draws it.
    #[serde(rename = "path.get")]
    PathGet {
        part: String,
        path: String,
        /// coarse, normal or fine: how finely it is cut; the user's display
        /// quality by default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quality: Option<String>,
    },
    /// What a sweep or a loft is built from, drawn in model space as the
    /// app draws it: a sweep's profile and path, a loft's sections in
    /// order with each loop's start.
    #[serde(rename = "sweep.get")]
    SweepGet {
        part: String,
        feature: String,
        /// coarse, normal or fine: how finely it is cut; the user's display
        /// quality by default.
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
    /// `path` (instance IDs, `/` between: `m3/s31`) starts the tree at
    /// the subassembly it reaches; `depth` lists that many levels of
    /// instances (all by default), a subassembly below them saying
    /// `more`: a window pages a tree of 100,000 occurrences this way.
    #[serde(rename = "assembly.tree")]
    AssemblyTree {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
    },
    /// Which solids of an assembly's occurrences overlap: every pair whose
    /// boxes do, or those with a side under `instances` (instance paths
    /// under the assembly, as `s1/b2` or `holes:2/bolt`).
    #[serde(rename = "interference")]
    Interference {
        part: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        instances: Vec<String>,
    },
    /// Measures one entity (its properties: a point, a length, an area
    /// and its shape; a solid's or an occurrence's volume and area) or two
    /// (the distance between them with the nearest points, and the angle
    /// where both have directions), at the working files or a Git
    /// revision `rev`. Values are in meters and radians, and as the
    /// project's default unit writes them, each `exact` or with its bound.
    #[serde(rename = "measure")]
    Measure {
        entities: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// The minimum distance between pairs of an assembly's solids in
    /// different occurrences whose boxes come within `below` (a length;
    /// by default a hundredth of the assembly's diagonal), closest first,
    /// with the nearest points; with `instances`, the pairs with a side
    /// under those instance paths. Pairs at 0 touch or interfere.
    #[serde(rename = "clearance")]
    Clearance {
        part: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        instances: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        below: Option<String>,
    },
    /// A solid read as sheet metal, as the part shows it: its thickness,
    /// flanges and bend table, each bend's allowance worked out to its
    /// developed length. `solid` is the part's one sheet metal solid
    /// unless named.
    #[serde(rename = "sheet.get")]
    SheetGet {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        solid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// Mass properties of a part or an assembly (its ID), a solid
    /// (`<part>/<solid>`) or an occurrence (`<asm>/<path>`): volume, area,
    /// centre of mass and, where every part has a material with a
    /// density, mass and the inertia tensor about the centre with its
    /// principal moments and axes; per part, its solids, material and
    /// mass. `tolerance` is the relative agreement the integration
    /// reaches (1e-9).
    #[serde(rename = "mass")]
    Mass {
        of: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tolerance: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// Fetches the Git repository at `url`, reads the ryucad project's ID
    /// in its folder `path` (`""` for the root) and lists it as a
    /// dependency named `name` (else after its folder or repository).
    #[serde(rename = "deps.add")]
    DepsAdd {
        url: String,
        #[serde(default)]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Fetches every dependency (or those `names` names, by name or
    /// project ID) and each project the lock reaches through them: their
    /// release branches and `ryupdm/` tags. Writes nothing in the project.
    #[serde(rename = "deps.fetch")]
    DepsFetch {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        names: Vec<String>,
    },
    /// Each dependency and each project the lock reaches through one,
    /// offline: where it is, whether it is fetched, the labels the lock
    /// holds and the instances placing them.
    #[serde(rename = "deps.list")]
    DepsList {},
    /// Fetches (unless `offline`), then moves every instance of the named
    /// dependencies' parts (all of them when `names` is empty; or of one
    /// `part`) to its part's latest official label that isn't obsolete,
    /// or to `to`, and writes the lock: what moved, and what builds now and
    /// wouldn't after (`failing`). `dry_run` writes nothing.
    #[serde(rename = "deps.update")]
    DepsUpdate {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        names: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        offline: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// What dependency `name` (a name or project ID) has released on its
    /// release branch as fetched, offline: each part (or only `part`) with
    /// its number, name, kind and labels, latest first.
    #[serde(rename = "deps.parts")]
    DepsParts {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
    /// Every instance, of the project's own parts or another project's, at
    /// a label older than its part's latest official one, offline.
    #[serde(rename = "deps.outdated")]
    DepsOutdated {},
    /// Every other project's part and label the assemblies place at `rev`
    /// (the working files by default), and their official releases',
    /// offline: what the review service indexes.
    #[serde(rename = "deps.uses")]
    DepsUses {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// Takes dependency `name` (a name or project ID) out of the manifest
    /// and the lock; `has_dependents` while anything places or reaches it.
    #[serde(rename = "deps.remove")]
    DepsRemove {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Every parameter of the project and of a part (else of every part),
    /// and every expression field of its features, with values and what
    /// each reads; with `all`, fields that are plain values too.
    #[serde(rename = "equations.list")]
    EquationsList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// The material library: each material's density and colour, the
    /// parts that name it, the default, and the project's revision.
    #[serde(rename = "materials.list")]
    MaterialsList {},
    /// Adds or replaces a material: `density` as `7850 kg/m^3` or
    /// `7.85 g/cm^3`, `colour` as `#rrggbb` (left out, it keeps its own),
    /// and with `default` the material of parts that name none.
    #[serde(rename = "materials.set")]
    MaterialsSet {
        name: String,
        density: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        colour: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        default: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Every solid `part` draws (else `participant`'s focus, else the
    /// project's model) with its colour and opacity and the faces drawn
    /// otherwise, at `rev`; entries naming what the model no longer has
    /// as warnings.
    #[serde(rename = "appearance.get")]
    AppearanceGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// Sets the colour (`#rrggbb`), the material's colour (`material`)
    /// or the opacity of each of `entities`, all in one part: the part,
    /// solids and faces of a part, occurrences and their solids of an
    /// assembly. `unset` takes out `colour` or `opacity`.
    #[serde(rename = "appearance.set")]
    AppearanceSet {
        entities: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        colour: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        material: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opacity: Option<f64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unset: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Takes out the appearance entries of `entities`, all in one part,
    /// whether or not the model still has what they name.
    #[serde(rename = "appearance.clear")]
    AppearanceClear {
        entities: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Removes a material no part names.
    #[serde(rename = "materials.delete")]
    MaterialsDelete {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Imports a STEP file the daemon can read (absolute, or relative to
    /// the project): stores it under `imports/` and makes a part of each
    /// product's bodies and an assembly of each product's children
    /// (`part` names the file's one root), or with `into` adds one
    /// product's bodies to a part. With `update`, the file's bytes
    /// replace that imported file and what was made of it follows.
    /// `dry_run` answers the plan, writing nothing. Bodies are kept as
    /// references (the file's topology, shown and read, never changed)
    /// unless `solid` asks for healed solids.
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
        /// Heal each body into a checked solid, instead of keeping it as
        /// a reference (design 364).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        solid: bool,
    },
    /// Sets an import feature's mode (`feature`), or every import feature
    /// of `part`'s: a healed solid, or with `reference` a reference
    /// (design 364). One logged, undoable change; answers the part's
    /// rebuild and `converted`.
    #[serde(rename = "import.convert")]
    ImportConvert {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feature: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reference: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Writes a part or an assembly as a file: at the working files, a Git
    /// revision `rev`, or a release `label:<L>`; in `format` (`step`,
    /// `stl`, `3mf`, `usda`, `usdc`, `usdz`), else the one `out`'s
    /// extension names (`.usd` is usdc), else STEP. With `out`, an
    /// absolute path, the daemon writes the file atomically and answers
    /// its size and BLAKE3; without, its bytes follow the answer. STEP is
    /// `ap242` (default) or `ap214`, in `mm` (default) or `m`; mesh
    /// formats and USD are in `mm` (default), `cm` or `m`, meshed to
    /// `chord` (default `0.01 mm`), `angle` (default `15 deg`) and
    /// `max_edge`, literals; STL is binary unless `ascii`. USD, as text,
    /// crate bytes or a package of them, writes each solid's B-rep
    /// (`brep`) and mesh (`mesh`), both unless one is `false`, pipes and
    /// sweeps within `pipe_tolerance`
    /// (default `0.0001 mm`). The header's `file_name` (else `out`'s) and
    /// `time_stamp` are the caller's; the time stamp is else the commit's
    /// time when exporting a commit, else now. A feature failing in what
    /// it places is `export_failed`, unless `skip_failing`: then the rest
    /// is written, and the answer lists what was left out (`skipped`, and
    /// an `export_skipped` warning each).
    #[serde(rename = "export")]
    Export {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        out: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_stamp: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chord: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        angle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_edge: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        ascii: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brep: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mesh: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pipe_tolerance: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        skip_failing: bool,
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
    /// Every worktree of the repository that holds the project: branch,
    /// head, ahead and behind, uncommitted changes, and who is attached to
    /// its daemon when one is running.
    /// What merging `rev` into the working files would leave to settle, as
    /// `merge.status` would say after the merge, merging nothing.
    #[serde(rename = "merge.preview")]
    MergePreview {
        /// A Git revision, or another worktree's working files as
        /// `worktree:<path>` (absolute).
        rev: String,
    },
    #[serde(rename = "worktree.list")]
    WorktreeList {},
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
    /// Cancels the caller's running long request `token`: the
    /// connection's participant's, or (on a connection that said no
    /// hello) the agent `as`'s. Answered without waiting for the request.
    #[serde(rename = "request.cancel")]
    RequestCancel {
        token: String,
        #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
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
    /// or one normal to the face `normal_to`, and/or framing the entities
    /// in `fit` (the whole model when empty). A `view` that names no
    /// standard view is a named view of `part` (else the caller's focus,
    /// else the project's model when it is one part), restored as saved.
    #[serde(rename = "camera.set")]
    CameraSet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        camera: Option<CameraArg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fit: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        normal_to: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
    /// The named views of `part`, or of every part, with each listed
    /// part's revision.
    #[serde(rename = "view.list")]
    ViewList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
    /// Saves a named view of `part`: `camera`, else `participant`'s
    /// camera, else the one human's, else the caller's. An existing view
    /// of that ID is replaced.
    #[serde(rename = "view.save")]
    ViewSave {
        part: String,
        view: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        camera: Option<CameraArg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Deletes a named view of `part`.
    #[serde(rename = "view.delete")]
    ViewDelete {
        part: String,
        view: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Renames a named view of `part`.
    #[serde(rename = "view.rename")]
    ViewRename {
        part: String,
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
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
    /// Sets the caller's own participant's mode: `view`, `annotate` or
    /// `edit`. No request sets another participant's.
    #[serde(rename = "participant.mode")]
    ParticipantMode { mode: crate::live::Mode },
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
    /// The exploded views of assembly `part`, or of every assembly: each
    /// one's steps as written and as they resolve now, with warnings, and
    /// each listed assembly's revision.
    #[serde(rename = "explode.list")]
    ExplodeList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
    /// Saves an exploded view of assembly `part` from `steps` (as the
    /// file writes them), replacing one of that ID.
    #[serde(rename = "explode.save")]
    ExplodeSave {
        part: String,
        explode: String,
        steps: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Deletes an exploded view of assembly `part`.
    #[serde(rename = "explode.delete")]
    ExplodeDelete {
        part: String,
        explode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Records a proposed change: `request` (`{method, params}`, one of
    /// `annotation::PROPOSABLE`) run over the caller's draft in memory,
    /// what it would have written kept in a new annotation.
    #[serde(rename = "annotation.propose")]
    AnnotationPropose {
        request: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pins: Vec<PinArg>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        rough: bool,
        /// The author with no daemon: an agent's name (`--as`); a daemon
        /// takes the connection's participant.
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    /// Writes an annotation's proposed change, after every open change it
    /// stands on that isn't applied yet, if every file each touches is
    /// still the bytes it saw (`annotation_stale` otherwise), and closes
    /// each `done` (`applied: true`) unless `keep_open`.
    #[serde(rename = "annotation.apply")]
    AnnotationApply {
        id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        keep_open: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    /// A comment pinned to references or points, or to none.
    #[serde(rename = "annotation.create")]
    AnnotationCreate {
        #[serde(default)]
        pins: Vec<PinArg>,
        comment: String,
        /// The camera the author looked through.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        camera: Option<crate::view::CameraArg>,
        /// Take the caller's participant's camera.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        with_view: bool,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    /// Every annotation with its derived states, newest first.
    #[serde(rename = "annotation.list")]
    AnnotationList {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        status: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
        /// `open` and `in_progress` only.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        open: bool,
    },
    #[serde(rename = "annotation.show")]
    AnnotationShow { id: String },
    #[serde(rename = "annotation.reply")]
    AnnotationReply {
        id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "annotation.claim")]
    AnnotationClaim {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "annotation.resolve")]
    AnnotationResolve {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "annotation.reject")]
    AnnotationReject {
        id: String,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "annotation.reopen")]
    AnnotationReopen {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    /// Its author changes the comment or pins while it is open and
    /// unclaimed.
    #[serde(rename = "annotation.edit")]
    AnnotationEdit {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pins: Option<Vec<PinArg>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    /// Its author's while open and unclaimed; anyone's once closed.
    #[serde(rename = "annotation.delete")]
    AnnotationDelete {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    /// Shows an exploded view in a participant's window (`participant`,
    /// else the one human, else the caller), `fraction` of the way through
    /// (1 by default; alone, it moves the one shown), or none with `off`.
    #[serde(rename = "explode.show")]
    ExplodeShow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fraction: Option<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        off: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// A participant's section planes: `participant`, else the one human,
    /// else the caller.
    #[serde(rename = "section.get")]
    SectionGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// Sets what a participant's window cuts away (`participant`, else the
    /// one human, else the caller): `planes` replace its planes, or with
    /// `add` go after them; `off` takes them all away. Up to three.
    #[serde(rename = "section.set")]
    SectionSet {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        planes: Vec<crate::section::PlaneArg>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        add: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        off: bool,
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
    /// Stores the token sent to the shared cache (`cache.remote`), in a
    /// file beside the user's preferences readable by its owner alone, and
    /// asks the server what it allows; answers where it went and the
    /// remote's state.
    #[serde(rename = "cache.login")]
    CacheLogin { token: String },
    /// Removes the stored token.
    #[serde(rename = "cache.logout")]
    CacheLogout {},
    /// Rebuilds `part` (with every part it places), or every part, through
    /// the disk cache and the shared cache behind it, so a person opening
    /// the project waits for nothing the team already built. Answers how
    /// many entries came from the shared cache and how many were built.
    #[serde(rename = "cache.warm")]
    CacheWarm {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
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
        /// Several of an assembly's occurrences (paths), meshed side by
        /// side and answered together: a window fetching 10,000 bodies.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
        /// coarse, normal or fine; the user's display quality by default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quality: Option<String>,
        /// `binary` sends `ryucad.mesh.1` bytes after the answer,
        /// `compact` `ryucad.mesh.2`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transfer: Option<Transfer>,
        /// A Git revision to mesh instead of the working files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// Draws a part (or the whole project) to a PNG, sent as bytes after
    /// the answer. The view is `camera`, a participant's camera, a standard
    /// `view` or a named view of the rendered part, or iso framing the
    /// model. `origin` draws the origin's axes and planes. `rev` renders a Git revision;
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
        /// Draw the shown part's datums (default true).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        datums: Option<bool>,
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
        /// Draw the origin's axes and planes (default false).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        origin: bool,
        /// Draw faces in their appearance (default true); false draws the
        /// fixed theme.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        appearance: Option<bool>,
        /// Cut with these section planes (none: `[]`); by default the
        /// participant's, as `hidden`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<Vec<crate::section::PlaneArg>>,
        /// Draw the rendered assembly exploded by this explode (none:
        /// `""`); by default the participant's, as `hidden`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explode: Option<String>,
        /// How far through the explode, from 0 to 1 (1 by default, or the
        /// participant's).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fraction: Option<f64>,
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
                | Request::PdmNewId { .. }
                | Request::PdmConfigure { .. }
                | Request::PropsSet { .. }
                | Request::BomRenumber { .. }
                | Request::MaterialsSet { .. }
                | Request::MaterialsDelete { .. }
                | Request::AppearanceSet { .. }
                | Request::AppearanceClear { .. }
                | Request::DepsAdd { .. }
                | Request::DepsUpdate { dry_run: false, .. }
                | Request::DepsRemove { .. }
                | Request::ViewSave { .. }
                | Request::ViewDelete { .. }
                | Request::ViewRename { .. }
                | Request::ExplodeSave { .. }
                | Request::ExplodeDelete { .. }
                | Request::AnnotationPropose { .. }
                | Request::AnnotationApply { .. }
                | Request::AnnotationCreate { .. }
                | Request::AnnotationReply { .. }
                | Request::AnnotationClaim { .. }
                | Request::AnnotationResolve { .. }
                | Request::AnnotationReject { .. }
                | Request::AnnotationReopen { .. }
                | Request::AnnotationEdit { .. }
                | Request::AnnotationDelete { .. }
                | Request::ReleasePropose { dry_run: false, .. }
                | Request::Import { dry_run: false, .. }
                | Request::ImportConvert { .. }
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

    /// The modes a human participant must be in to send this request:
    /// annotation writes need `annotate` or `edit`, every other write
    /// `edit`; `None` for a request that writes nothing.
    pub fn needs(&self) -> Option<&'static [crate::live::Mode]> {
        use crate::live::Mode;
        if self.annotates() {
            Some(&[Mode::Annotate, Mode::Edit])
        } else if self.mutates() {
            Some(&[Mode::Edit])
        } else {
            None
        }
    }

    /// Whether the request writes only annotations: what View mode
    /// refuses and Annotate allows. `annotation.apply` writes the model.
    pub fn annotates(&self) -> bool {
        matches!(
            self,
            Request::AnnotationCreate { .. }
                | Request::AnnotationPropose { .. }
                | Request::AnnotationReply { .. }
                | Request::AnnotationClaim { .. }
                | Request::AnnotationResolve { .. }
                | Request::AnnotationReject { .. }
                | Request::AnnotationReopen { .. }
                | Request::AnnotationEdit { .. }
                | Request::AnnotationDelete { .. }
        )
    }

    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("requests serialize")
    }
}

impl ryu_session::client::Call for Request {
    fn method(&self) -> String {
        Request::method(self)
    }

    fn params(&self) -> Value {
        Request::params(self)
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
                approvals: Some(json!(false)),
                bump: Some("pr".into()),
                force: true,
                expected_revision: None,
            },
            Request::PropsSet {
                part: "p".into(),
                properties: [("number".to_owned(), json!("100"))].into_iter().collect(),
                expected_revision: Some("r".into()),
            },
            Request::BomRenumber {
                part: "asm".into(),
                items: [("bolt".to_owned(), json!(3))].into_iter().collect(),
                compact: false,
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
            Request::BomRenumber {
                part: "asm".into(),
                items: Map::new(),
                compact: true,
                expected_revision: None,
            }
            .mutates()
        );
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
