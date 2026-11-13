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

/// Marks as a request gives them (design 423): the part or assembly
/// they were drawn on, marks as the file has them (`touches` optional:
/// the daemon works them out), and shorthand marks in the view's plane in
/// the project's default unit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MarksArg {
    pub part: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub list: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub short: Vec<ShortMark>,
}

/// A mark as the CLI's shorthand writes it: `kind` `line`, `arrow`,
/// `ellipse` or `rectangle` with `at` `X1,Y1:X2,Y2`, or `text` with
/// `X,Y=TEXT`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShortMark {
    pub kind: String,
    pub at: String,
}

/// A request's `flip`: a datum's or split's turn, or a draft's flipped
/// faces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Flip {
    On(bool),
    Faces(Vec<String>),
}

impl Flip {
    /// The turn a datum or split takes: `invalid_param` at `flip` for
    /// faces.
    pub fn on(&self) -> crate::error::Result<bool> {
        match self {
            Flip::On(b) => Ok(*b),
            Flip::Faces(_) => Err(crate::error::Error::new(
                crate::error::Code::InvalidParam,
                "flip is true or false here; a list of faces is a draft's",
            )
            .with("param", "flip")),
        }
    }
}

impl From<bool> for Flip {
    fn from(b: bool) -> Flip {
        Flip::On(b)
    }
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
    /// The body a split cuts with, by its founder: a sheet divides, a
    /// solid's inside is behind. Replaces its plane or faces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Faces of one body a split cuts with, taken off as a sheet:
    /// `<part>/<solid>/<name>`. Replaces its plane or tool body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_faces: Option<Vec<String>>,
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
    /// direction; a draft's pull turned round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<bool>,
    /// Sweep the profile's loops and open chains into a sheet of its own
    /// (an extrude, revolve, sweep or loft; ryucad 18); false a solid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<bool>,
    /// A revolve's axis: a line of its sketch, or a straight edge as
    /// `<part>/<feature>/<name>`. A circular pattern's: `x`, `y`, `z`, or
    /// an edge or face as `<part>/<feature>/<name>`. A move_face's
    /// (rotate): any axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    /// A revolve's angle, or `"full"`; a chamfer's angle to its face; a
    /// move_face's (rotate), right-handed about its axis; a draft's angle
    /// from its pull.
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
    /// A move_face's `[{distance, faces}]`, a replace_face's
    /// `[{with, faces}]` (`with` a face or a plane as text); a draft's
    /// `[{angle, faces}]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Value>,
    /// Whether a fillet or chamfer grows each edge to its tangent chain
    /// (true when left out of a new one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub propagate: Option<bool>,
    /// Whether a face tweak moves faces tangent to the ones it names with
    /// them (true when left out of a new one; `false` keeps the kernel's
    /// `tangency_broken`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tangent: Option<bool>,
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
    /// A hole's spec: "M6x1.0-6H THRU", "CBORE ISO4762 M5", "DIA 6 H7
    /// THRU" (`hole::spec`). It sets the fields it names; a field it sets
    /// and given beside it is `usage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    /// A tapped hole's thread: `{standard, size, pitch?, tpi?, class?,
    /// hand?, starts?, depth?, drill?}`, its fields laid over the
    /// current thread's; `"STANDARD:SIZE"` for a new one; `""` takes it
    /// away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Value>,
    /// A clearance hole's screw: `{standard, size, fit?}`,
    /// `"STANDARD:SIZE"`, or `""`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fastener: Option<Value>,
    /// A clearance hole's fit: close, normal or loose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fastener_fit: Option<String>,
    /// A hole's ISO 286 class ("H7"); `""` takes it away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<String>,
    /// A tapped hole's TPI, class, starts,
    /// thread depth ("full" or a length) and tap drill ("table",
    /// "roll-form" or "engagement:N"), each laid into its thread. A
    /// hole's `pitch` and `hand` are its thread's (a helix's its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpi: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_depth: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap_drill: Option<String>,
    /// The joins, cuts and holes a pattern repeats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
    /// A linear pattern's direction: `x`, `y`, `z`, or a straight edge or
    /// planar face as `<part>/<feature>/<name>`. A binormal sweep's: any
    /// axis, or a plane's normal (a datum, a helix's axis by its ID). A
    /// shell's: `inward` or `outward`. A move_face's (translate): any
    /// axis, or a plane's normal.
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
    /// `""` lets it go. An offset's: positive outwards, zero allowed. A
    /// move_face's: along its direction, or (offset) positive outwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<Value>,
    /// A mate's stops: `{angle: {min, max}, distance: {min, max}}`, any
    /// part, laid over what it has; a bound `""` clears it, a value `""`
    /// clears its range. Each bound is an expression (an angle, a length).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Value>,
    /// The instances an instance pattern copies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<Vec<String>>,
    /// How a datum plane, axis or point is built: a plane `offset`,
    /// `angle`, `three_points`, `parallel`, `axis_point`, `normal_to` or
    /// `midplane`; an axis `of`, `two_points`, `planes` or `normal`; a
    /// point `of`, `coordinates`, `along`, `intersection`, `projection` or
    /// `midpoint`. A move_face's motion: `translate`, `rotate` or
    /// `offset`. A new `by` keeps only the fields it takes.
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
    /// A datum plane's normal, or a datum axis's direction, turned; a
    /// split's plane turned: `true` or `false`. A draft's faces drafted
    /// against its pull: a list of faces, each `<part>/<solid>/<name>`,
    /// replacing them all (`[]` for none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip: Option<Flip>,
    /// A draft's neutral plane: any plane (an origin or datum plane, a
    /// sketch, a planar face `<part>/<solid>/<name>`); its normal is the
    /// pull. Given, a parting line and its pull go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neutral: Option<String>,
    /// A draft's parting line, replacing it: edges of its target, each
    /// `<part>/<solid>/<name>`. Given, a neutral plane goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parting: Option<Vec<String>>,
    /// A parting-line draft's pull: any axis, or a plane's normal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<String>,
    /// A face tweak's topology: `change` lets its faces grow over their
    /// neighbours, shrink away or split, and edges and vertices come and
    /// go; `keep` (the default) refuses that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<String>,
    /// A face tweak's blends: `rebuild` makes again the fillets and
    /// chamfers it breaks; `keep` (the default) refuses that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blends: Option<String>,
    /// A delete_face's healing: `auto` (a cap where one closes a gap, else
    /// its neighbours grow; the default), `cap` or `grow`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heal: Option<String>,
    /// A delete_face's seeds, replacing them all: one face of each hole,
    /// boss or pocket deleted whole, each `<part>/<solid>/<name>` (`[]` for
    /// none). The CLI sends `--seed` as `features`, which a delete_face
    /// takes as its seeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeds: Option<Vec<String>>,
    /// A delete_blend's size: every blend at most this length goes (a
    /// fillet's radius, a chamfer's larger distance; `""` takes it away).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<Value>,
    /// A resize_blend's one chamfer group: the face its first distance
    /// runs along, `<part>/<solid>/<name>` (its blends are `faces`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamfer_face: Option<String>,
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
    /// (`[]` for none). A move_face's faces, each `<part>/<solid>/<name>`,
    /// replacing them all; a draft's faces, each `<part>/<solid>/<name>`,
    /// with `{angle, faces}` objects for groups as the CLI sends them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<Value>,
    /// The solid a mirror copies, by the feature that founded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solid: Option<String>,
    /// The solids a delete_body takes out of the part's result, by key
    /// (`<founder>` or `<insert>/<source key>`; an insert's ID for every
    /// solid it founded), replacing them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solids: Option<Vec<String>>,
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
    /// `{edge, side}`; `[]` takes every side away. A fill's sides in
    /// order, replacing them all: each `{"edge": text, "tangent": bool}`
    /// or `{"curve": text}` (an edge `<part>/<solid>/<name>`, a sketch's
    /// curve `<part>/<sketch>/<entity>`, a path or helix
    /// `<part>/<feature>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sides: Option<Value>,
    /// A fill's or a sew's tolerance: a length from 0.00001 mm to 0.1 mm;
    /// a hole's bore tolerance, `{symmetric}` or `{deviation}`, `""` for
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<Value>,
    /// Faces given one by one, two or more (`--face` repeated): an
    /// extract's faces, or the faces a thicken thickens. One is `face`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_list: Option<Vec<String>>,
    /// A sew's sheets, by the features that founded them (two or more);
    /// from the CLI, a thicken's one sheet too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheets: Option<Vec<String>>,
    /// The sheet a thicken thickens and consumes, by its founder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    /// Whether a sew that closes up makes a solid (true by default; the
    /// file's `solid`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub make_solid: Option<bool>,
    /// The faces a thicken thickens, each `<part>/<solid>/<name>`, of one
    /// body (they are extracted first; the body is left as it was).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_faces: Option<Vec<String>>,
    /// A thicken's thickness along its faces' normals, zero or more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front: Option<Value>,
    /// A thicken's thickness against its faces' normals, zero or more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<Value>,
    /// A thicken's face groups, replacing them all:
    /// `[{"front": Q, "back": Q, "faces": ["<part>/<solid>/<name>", ...]}]`
    /// (`[]` for none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_sides: Option<Value>,
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
    /// The point the turn's axis runs through; the instance's origin when
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<[Value; 3]>,
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
        /// Validate and derive, and write nothing: what the feature would
        /// be, sizes and all.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
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
        /// Validate and derive, and write nothing: what the feature would
        /// be, sizes and all.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
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
    FeatureShow {
        part: String,
        id: String,
        /// Read the project as this revision has it (the working files by
        /// default): a Git revision, `annotation:<id>`, `draft:<name>` or
        /// `worktree:<path>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    #[serde(rename = "param.list")]
    ParamList {
        /// The part, or none with `scope: project`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<crate::param::Scope>,
        /// Read the project as this revision has it (the working files by
        /// default): a Git revision, `annotation:<id>`, `draft:<name>` or
        /// `worktree:<path>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
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
        /// `packages.formats`: what a release's package holds (bom, dxf,
        /// pdf, render, step); `[]` for the default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        package_formats: Option<Vec<String>>,
        /// Write even if parts' properties stop fitting.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A part's properties: every core and declared one, set or not;
    /// and `file`, the attributes its imported STEP files give it that no
    /// declared property takes (design 448, plan 24.C), read-only.
    #[serde(rename = "props.get")]
    PropsGet {
        part: String,
        /// Read the project as this revision has it (the working files by
        /// default): a Git revision, `annotation:<id>`, `draft:<name>` or
        /// `worktree:<path>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
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
    /// Builds an official release's package (design 436): to `out` (a
    /// zip, or a folder), into `into` as `<number>-<label>.zip`, or as the
    /// bytes after the answer.
    #[serde(rename = "release.package")]
    ReleasePackage {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        out: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        into: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        formats: Option<Vec<String>>,
    },
    /// Judges a package someone was sent (an absolute path) against this
    /// clone, offline; with `rebuild` built again and compared.
    #[serde(rename = "release.package.verify")]
    ReleasePackageVerify {
        file: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        rebuild: bool,
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
        /// Read the project as this revision has it (the working files by
        /// default): a Git revision, `annotation:<id>`, `draft:<name>` or
        /// `worktree:<path>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
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
        /// Read the project as this revision has it (the working files by
        /// default): a Git revision, `annotation:<id>`, `draft:<name>` or
        /// `worktree:<path>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
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
        /// Read the project as this revision has it (the working files by
        /// default): a Git revision, `annotation:<id>`, `draft:<name>` or
        /// `worktree:<path>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// Moves an instance of an assembly: to the pose `placement` gives
    /// (what it leaves out stays), then by `translate` and a turn of
    /// `angle` about `axis` through `about` (else its origin), in the
    /// assembly's frame.
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
        /// The point the turn's axis runs through, in the assembly's
        /// frame (three quantities); the instance's origin when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        about: Option<[Value; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// An assembly's mates solved, writing nothing: as it is, or with
    /// `moves` held where they put instances for a first solve. Each
    /// instance's pose and the solve's report. With `component`, only
    /// what the moved instances' mates reach is solved and answered, from
    /// the poses the assembly's rebuild has (a drag's preview). At `rev`, and
    /// with `mates` added in memory, it previews over a draft or a mate.
    #[serde(rename = "assembly.solve")]
    AssemblySolve {
        part: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        moves: Vec<InstanceMove>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        component: bool,
        /// The revision to solve over (the working files when left out):
        /// any a read takes, a person's `draft:` included.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        /// Mates to solve with, as `feature.add` of a mate takes their
        /// fields (`form`, `a`, `b`, `a_offset`, `b_offset`, `angle`,
        /// `distance`, `limits`), added to the assembly in memory and
        /// never written.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mates: Vec<Map<String, Value>>,
    },
    /// Sweeps a motion of assembly `part`, writing nothing: a saved one
    /// (`motion`), inline `drives` (as a motion file writes them) with
    /// `steps`, or one mate's angle (a slider's distance) from `from` to
    /// `to`. Each step solved from the last; with `interference` what
    /// collides that doesn't at rest, refined `refine` times (10 by
    /// default); with `clearance` the closest moving pair within `below`;
    /// with `poses` each step's moved instances' poses.
    #[serde(rename = "assembly.sweep")]
    AssemblySweep {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        motion: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drives: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mate: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u32>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interference: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clearance: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        below: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        poses: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refine: Option<u32>,
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
    /// Every hole of a part (`holes`): each place of each hole feature,
    /// with patterns and mirrors expanded, in a fixed order; or one by its
    /// reference, as CSV, or grouped by spec.
    #[serde(rename = "holes.list")]
    HolesList {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        solid: Option<String>,
        /// One row per hole as CSV text, in `csv`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        csv: bool,
        /// Rows of one spec together: `{spec, count, refs}`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        group: bool,
        /// One hole by its reference.
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// What the hole standards hold: `threads`, `fasteners` or `drills`,
    /// optionally one standard (or drill series) and size, each value
    /// with its source.
    #[serde(rename = "standards.list")]
    StandardsList {
        table: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        standard: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<String>,
    },
    /// A hole's every size, limit and source, and its canonical spec, for
    /// a spec or fields with no model (`hole derive`). Reads the project's
    /// hole_standards.json, and `part`'s parameters when given.
    #[serde(rename = "hole.derive")]
    HoleDerive {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spec: Option<String>,
        /// Hole fields as a file holds them, laid over the spec's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
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
    /// The faces of the hole, boss or pocket a face of a part's solid
    /// belongs to, as a `delete_face` seed takes them: `face` is
    /// `<part>/<solid>/<name>`, in the part as it shows at `rev`. The
    /// smallest set a cap closes: a counterbored hole's bore is the bore
    /// alone, and its counterbore's wall takes it all.
    #[serde(rename = "feature_faces.get")]
    FeatureFacesGet {
        face: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// A solid's fillets, chamfers and ball corners as `delete_blend` and
    /// `resize_blend` read them: each blend's faces, kind and size,
    /// supports and chain, by least face name, in the part as it shows at
    /// `rev`. With `below` (a length), only those a `delete_blend` of that
    /// size takes, closed over chains and corners. `solid` is the part's
    /// one solid unless named; `tolerance` is recognition's (10 µm).
    #[serde(rename = "blends.get")]
    BlendsGet {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        solid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tolerance: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        below: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// A solid's flat pattern, as the part shows it: unfolded whole in
    /// memory when folded (about `fixed`, a side face, each bend at its
    /// override in `bends` else the solid's allowance), else redrawn
    /// through the unfold in force. Its loops' segments exact in
    /// millimetres with their edges and polylines, its bend lines with
    /// their notes. `solid` is the part's one sheet metal solid unless
    /// named.
    #[serde(rename = "flat.get")]
    FlatGet {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        solid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fixed: Option<String>,
        /// `{bend, k_factor | deduction | allowance}`, as an unfold's.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bends: Vec<Value>,
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
    /// Moves an insert's pin (design 521): to `working`, `label:L` (or
    /// `L`), or the source's newest official label (`newest`, the
    /// default); answers what moving it changes, and with `dry_run`
    /// writes nothing.
    #[serde(rename = "insert.update")]
    InsertUpdate {
        part: String,
        feature: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
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
        /// The part entry's `file_styles` (design 448): `false` draws the
        /// part without its imported files' colours, `true` with them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_styles: Option<bool>,
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
    /// unless `solid` asks for healed solids. With `flatten`, one part of
    /// every body.
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
        /// Make one part of every body of every product, placed where
        /// the file's assemblies put it (#499).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flatten: bool,
        /// Folders to look in for the files an assembly's parts are kept
        /// in (design 448, 24.D), after the folder of the file naming
        /// each: absolute, or relative to the project.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        search: Vec<String>,
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
    /// The transfer report of a file the project has imported
    /// (`imports/…`), read again from its committed bytes: what the
    /// project's import features interpret, retain, find unreferenced and
    /// fail (design 448). `detail` lists every entity name with its first
    /// 64 instances. Writes nothing.
    #[serde(rename = "import.report")]
    ImportReport {
        file: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        detail: bool,
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
    /// an `export_skipped` warning each). A STEP export's answer gives
    /// `transfer`: what it wrote by category, what it left out
    /// (`omitted`, with an `export_metadata_omitted` warning for
    /// appearance and properties) and what it approximated (design 448).
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
        /// STEP: `false` writes no styles (design 448).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        appearance: Option<bool>,
        /// STEP: `false` writes no materials or properties (design 448).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        properties: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        solid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fixed: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bends: Vec<Value>,
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
        /// Also find which instances of each assembly checked interfere
        /// (`instances_interfere`, `cannot_check`). Without it, interference
        /// isn't run (design 525) and each assembly gets an
        /// `interference_not_checked` note.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interference: bool,
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
    /// What merging `rev` into the working files would leave to settle, as
    /// `merge.status` would say after the merge, merging nothing.
    #[serde(rename = "merge.preview")]
    MergePreview {
        /// A Git revision, or another worktree's working files as
        /// `worktree:<path>` (absolute).
        rev: String,
    },
    /// Every worktree of the repository that holds the project: branch,
    /// head, ahead and behind, uncommitted changes, and who is attached to
    /// its daemon when one is running.
    #[serde(rename = "worktree.list")]
    WorktreeList {},
    /// The repository's branches: local ones, and remote-tracking ones
    /// with no local branch of their name, each with its tip, upstream,
    /// commits ahead of and behind this worktree's head, and the worktree
    /// it is checked out in; with the project revision a switch is
    /// guarded by.
    #[serde(rename = "branch.list")]
    BranchList {},
    /// Checks out `branch` in the project's work tree (`git switch`):
    /// refused while project files differ from `HEAD` (`checkout_dirty`)
    /// or a merge is in progress; the session reloads at once, and the
    /// activity log records a `switch` entry undo never takes.
    #[serde(rename = "branch.switch")]
    BranchSwitch {
        branch: String,
        /// The project revision the caller read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
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
    /// The motions of assembly `part`, or of every assembly: each one's
    /// steps and drives as written and as they resolve now, with
    /// warnings or its error, and each listed assembly's revision.
    #[serde(rename = "motion.list")]
    MotionList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
    /// Saves a motion of assembly `part` from `drives` (as the file
    /// writes them) and `steps` (11 when left out), replacing one of that
    /// ID. Refused when it doesn't read or doesn't resolve.
    #[serde(rename = "motion.save")]
    MotionSave {
        part: String,
        motion: String,
        drives: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Deletes a motion of assembly `part`.
    #[serde(rename = "motion.delete")]
    MotionDelete {
        part: String,
        motion: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Where a motion of assembly `part` (a saved `motion`, or inline
    /// `drives` and `steps`) puts each moved body at `frames` even
    /// fractions (120 by default, at most 1,000): each frame's state,
    /// drive values, the moved instances' poses and each moved body's
    /// offset (by instance, or copy and seed). Solved by continuation as
    /// a sweep is; nothing is intersected. A window fetches them once and
    /// plays locally.
    #[serde(rename = "motion.frames")]
    MotionFrames {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        motion: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drives: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frames: Option<u32>,
    },
    /// A part's drawings, or every part's (design 417).
    #[serde(rename = "drawing.list")]
    DrawingList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
    /// A drawing of one sheet; with `views: standard` a base front view,
    /// its top and right and an isometric, laid out.
    #[serde(rename = "drawing.create")]
    DrawingCreate {
        part: String,
        drawing: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        views: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A batch of `add`, `remove`, `set` and `move` ops on a drawing's
    /// sheets and views, all or nothing, with one build to check it.
    #[serde(rename = "drawing.edit")]
    DrawingEdit {
        part: String,
        drawing: String,
        ops: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "drawing.delete")]
    DrawingDelete {
        part: String,
        drawing: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A drawing takes a new ID.
    #[serde(rename = "drawing.rename")]
    DrawingRename {
        part: String,
        drawing: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The built drawing: sheets, views with their boxes and frames, the
    /// findings, and with `items` every item's exact geometry.
    #[serde(rename = "drawing.get")]
    DrawingGet {
        part: String,
        drawing: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        items: bool,
    },
    /// Each sheet in words, for an agent.
    #[serde(rename = "drawing.describe")]
    DrawingDescribe {
        part: String,
        drawing: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
    },
    /// A sheet as PNG or SVG, the bytes after the answer.
    #[serde(rename = "drawing.render")]
    DrawingRender {
        part: String,
        drawing: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f32>,
    },
    /// Every sheet as PDF, or one as SVG or DXF (`unit` `mm` or `in`),
    /// written by the daemon to `out` (an absolute path, never a project
    /// file), else after the answer.
    #[serde(rename = "drawing.export")]
    DrawingExport {
        part: String,
        drawing: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        out: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// The views laid out again from nothing; written unless `dry_run`.
    #[serde(rename = "drawing.layout")]
    DrawingLayout {
        part: String,
        drawing: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The drawing lint's findings and each sheet's score; with
    /// `explain`, a rule's entry (no drawing needed).
    #[serde(rename = "drawing.lint")]
    DrawingLint {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drawing: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explain: Option<String>,
    },
    /// Sets the drawing lint's weights and severities by rule
    /// (`drawing_lint.json`; `default` puts one back).
    #[serde(rename = "drawing.lint.configure")]
    DrawingLintConfigure {
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        weights: std::collections::BTreeMap<String, Value>,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        severity: std::collections::BTreeMap<String, String>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        marks: Option<MarksArg>,
        /// A named view of the marks' part (else the first pin's) as the
        /// annotation's view.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<String>,
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
        marks: Option<MarksArg>,
        /// Take the marks away.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        no_marks: bool,
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
    /// Shows a motion in a participant's window (`participant`, else the
    /// one human, else the caller): `motion` of `part` (else the window's
    /// focus, else the project's model), `fraction` of the way through (0
    /// for a new one; alone, it moves the one shown and pauses it), and
    /// whether it plays; or none with `off`. A motion shown takes an
    /// explode away.
    #[serde(rename = "motion.show")]
    MotionShow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        motion: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fraction: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        playing: Option<bool>,
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
        /// Mesh a sheet metal solid's flat body instead (`""`: the part's
        /// one sheet metal solid), as `flat.get` and `render --flat` lay
        /// it out: labelled by the solid, every name the shown body's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flat: Option<String>,
        /// Mesh solid `feature` as this feature of the part read it,
        /// whether or not the model still shows it: what a delete takes
        /// out, ghosted where it was (ryucad 25.B).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
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
        /// Draw tapped holes' cosmetic threads, a ring at each end
        /// (default true).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threads: Option<bool>,
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
        /// Draw open annotations' pins, numbered, and their marks
        /// (default false).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        annotations: bool,
        /// Render this annotation: its view, its pins and its marks (design
        /// 423). With `camera` or `view` it is `usage`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotation: Option<String>,
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
        /// How far through the explode (1 by default) or the motion (0 by
        /// default), from 0 to 1; or the participant's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fraction: Option<f64>,
        /// Draw the rendered assembly where this motion puts it at
        /// `fraction` (0 by default, or the participant's; none: `""`);
        /// by default the participant's shown motion, as `hidden`. Never
        /// with an explode.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        motion: Option<String>,
        /// Paint the faces that collide at the motion's fraction, and
        /// don't at rest, in the removed colour (intersects pairs).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interference: bool,
        /// Draw a sheet metal solid's flat pattern instead of the model:
        /// the solid's flat body seen from its viewer side, its bend lines
        /// dashed with their notes; `""` for the part's one sheet metal
        /// solid.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flat: Option<String>,
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
                // A dry run reads the model without taking the write lock.
                | Request::FeatureAdd { dry_run: false, .. }
                | Request::FeatureEdit { dry_run: false, .. }
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
                | Request::InsertUpdate { dry_run: false, .. }
                | Request::DepsRemove { .. }
                | Request::ViewSave { .. }
                | Request::ViewDelete { .. }
                | Request::ViewRename { .. }
                | Request::ExplodeSave { .. }
                | Request::ExplodeDelete { .. }
                | Request::MotionSave { .. }
                | Request::MotionDelete { .. }
                | Request::DrawingCreate { .. }
                | Request::DrawingEdit { .. }
                | Request::DrawingDelete { .. }
                | Request::DrawingRename { .. }
                | Request::DrawingLayout { dry_run: false, .. }
                | Request::DrawingLintConfigure { .. }
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
        // A switch rewrites the project's files, which only Direct edit
        // does; it isn't `mutates()`, which would log it as an undoable
        // change.
        } else if self.mutates() || matches!(self, Request::BranchSwitch { .. }) {
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
    fn import_takes_search_folders() {
        let r = Request::from_rpc(
            "import",
            Some(json!({"path": "/a/asm.stp", "search": ["/lib", "parts"]})),
        )
        .unwrap();
        let Request::Import { search, .. } = r else {
            panic!("{r:?}")
        };
        assert_eq!(search, ["/lib", "parts"]);
        // Left out, it is empty and not written.
        let r = Request::from_rpc("import", Some(json!({"path": "/a/asm.stp"}))).unwrap();
        assert!(!serde_json::to_string(&r).unwrap().contains("search"));
    }

    #[test]
    fn requests_round_trip_through_rpc() {
        let reqs = [
            Request::ImportReport {
                file: "imports/box.step".into(),
                detail: true,
            },
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
                dry_run: false,
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
                dry_run: false,
            },
            Request::FeatureAdd {
                part: "p".into(),
                ty: "hole".into(),
                params: Map::new(),
                placement: None,
                fields: FeatureFields {
                    spec: Some("M6x1.0-6H THRU".into()),
                    ..Default::default()
                },
                id: Some("h".into()),
                position: Position::default(),
                expected_revision: None,
                dry_run: true,
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
                package_formats: Some(vec!["dxf".into(), "step".into()]),
                force: true,
                expected_revision: None,
            },
            Request::ReleasePackage {
                part: "p".into(),
                label: Some("B".into()),
                out: None,
                into: Some("/tmp/out".into()),
                formats: Some(vec!["step".into()]),
            },
            Request::ReleasePackageVerify {
                file: "/tmp/out/100-B.zip".into(),
                rebuild: true,
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

    #[test]
    fn flip_is_a_bool_or_faces() {
        let f: FeatureFields = serde_json::from_value(json!({"flip": true})).unwrap();
        assert_eq!(f.flip, Some(Flip::On(true)));
        assert_eq!(f.flip.as_ref().unwrap().on(), Ok(true));
        let f: FeatureFields =
            serde_json::from_value(json!({"flip": ["p/b/0123456789abcdef0123456789abcdef"]}))
                .unwrap();
        let Some(Flip::Faces(faces)) = &f.flip else {
            panic!("{:?}", f.flip)
        };
        assert_eq!(faces.len(), 1);
        let e = f.flip.as_ref().unwrap().on().unwrap_err();
        assert_eq!(
            (e.code, e.details["param"].as_str()),
            (crate::error::Code::InvalidParam, Some("flip"))
        );
        // Written back as given.
        assert_eq!(
            serde_json::to_value(&f).unwrap()["flip"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let f: FeatureFields =
            serde_json::from_value(json!({"neutral": "xy", "parting": ["p/b/e"], "pull": "z"}))
                .unwrap();
        assert_eq!(
            (
                f.neutral.as_deref(),
                f.parting.as_deref(),
                f.pull.as_deref()
            ),
            (Some("xy"), Some(&["p/b/e".to_owned()][..]), Some("z"))
        );
    }
}
