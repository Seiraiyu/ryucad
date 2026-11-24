//! What each error code's `details` may hold (design 698, #771): the keys
//! a code declares, the keys every code may carry, and the check every
//! error passes as it leaves a product in a debug build. A code with an
//! entry in the table is declared; one without is not yet, and every code
//! added from now on must be (`tests/golden/undeclared.txt` only shrinks).

use super::Code;
use serde_json::Value;
use std::fmt;

/// What a key's values are. `null` is every kind: a key with no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Str,
    Num,
    Bool,
    Arr,
    Obj,
    Any,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Str => "str",
            Kind::Num => "num",
            Kind::Bool => "bool",
            Kind::Arr => "arr",
            Kind::Obj => "obj",
            Kind::Any => "any",
        }
    }

    /// Whether `v` is of this kind.
    pub fn admits(self, v: &Value) -> bool {
        matches!(
            (self, v),
            (_, Value::Null)
                | (Kind::Any, _)
                | (Kind::Str, Value::String(_))
                | (Kind::Num, Value::Number(_))
                | (Kind::Bool, Value::Bool(_))
                | (Kind::Arr, Value::Array(_))
                | (Kind::Obj, Value::Object(_))
        )
    }

    /// The kind of `v` (`any` for `null`).
    pub fn of(v: &Value) -> Kind {
        match v {
            Value::Null => Kind::Any,
            Value::String(_) => Kind::Str,
            Value::Number(_) => Kind::Num,
            Value::Bool(_) => Kind::Bool,
            Value::Array(_) => Kind::Arr,
            Value::Object(_) => Kind::Obj,
        }
    }
}

/// One key a code's details may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub name: &'static str,
    /// An alias's is its declared key's ([`kind`]).
    pub kind: Kind,
    /// What it holds, for agents (empty for an alias: [`about`]).
    pub about: &'static str,
    /// The declared key this older spelling is carried beside: an error
    /// that holds it holds the declared key too (`Error::with`).
    pub alias_of: Option<&'static str>,
}

/// A declared key.
pub const fn key(name: &'static str, kind: Kind, about: &'static str) -> Key {
    Key {
        name,
        kind,
        about,
        alias_of: None,
    }
}

/// An older spelling of the declared key `of`, still declared, so nothing
/// an agent reads today disappears.
pub const fn alias(name: &'static str, of: &'static str) -> Key {
    Key {
        name,
        kind: Kind::Any,
        about: "",
        alias_of: Some(of),
    }
}

/// The keys every code may carry, declared or not.
pub const COMMON: &[Key] = &[
    key(
        "hint",
        Kind::Str,
        "What to do about it: a command to run or a sentence.",
    ),
    key("commit", Kind::Str, "The commit the request read or wrote."),
    key(
        "pointer",
        Kind::Str,
        "A JSON pointer to the value at fault, in the request or the file.",
    ),
    key("path", Kind::Str, "The file or folder at fault."),
    key(
        "op",
        Kind::Num,
        "In a batch, the index of the op that failed.",
    ),
    key(
        "method",
        Kind::Str,
        "In a batch, the method of the op that failed.",
    ),
    key(
        "faults",
        Arr,
        "The faults found; a write refused for the faults it would leave has them under the first one's code.",
    ),
];

use Kind::{Any, Arr, Bool, Num, Obj, Str};

/// The table: each declared code and its own keys, beside [`COMMON`].
macro_rules! details {
    ($($code:ident => [$($key:expr),* $(,)?],)*) => {
        /// The keys `code` declares, beside [`COMMON`]; `None` while it is
        /// undeclared.
        pub fn keys(code: Code) -> Option<&'static [Key]> {
            match code {
                $(Code::$code => {
                    const KEYS: &[Key] = &[$($key),*];
                    Some(KEYS)
                })*
                _ => None,
            }
        }

        /// Every declared code, in the table's order.
        pub const DECLARED: &[Code] = &[$(Code::$code),*];
    };
}

details! {
    InvalidParam => [
        key("param", Str, "The parameter at fault, by its name in the request or the file."),
        alias("parameter", "param"),
        alias("field", "param"),
        key("value", Any, "The value given."),
        key("reason", Str, "Why, as a word agents branch on (`not_drawn`, `thread_from_needed`)."),
        key("wanted", Any, "What would have been accepted."),
        alias("expected", "wanted"),
        key("known", Arr, "The names accepted."),
        key("feature", Str, "The feature at fault."),
        key("part", Str, "The part at fault."),
        key("solid", Str, "The solid at fault."),
        key("solids", Any, "The solids there were to choose from."),
        key("entity", Str, "The entity reference at fault."),
        key("reference", Str, "A reference at fault, as text."),
        alias("ref", "reference"),
        alias("file", "path"),
        key("first", Str, "Where the clashing value was first given."),
        key("rev", Str, "The revision asked for."),
        key("annotation", Str, "The annotation at fault."),
        key("participant", Str, "The participant at fault."),
        key("role", Str, "The role the value plays in its feature (`axis`, `on`)."),
        key("seed", Any, "The reference's seed."),
        key("fraction", Num, "The fraction given."),
        key("face", Str, "The face at fault."),
        key("flag", Str, "The command-line flag at fault."),
        key("rules", Arr, "The rule codes accepted."),
        key("codes", Arr, "The codes accepted."),
        key("classes", Arr, "The classes accepted."),
        key("templates", Arr, "The templates accepted."),
        key("catalogues", Arr, "The catalogues accepted."),
        key("distributors", Arr, "The distributors accepted."),
        key("sources", Any, "The sources accepted."),
        key("keys", Arr, "The keys there are."),
        key("boards", Arr, "The boards there are."),
        key("designs", Any, "The designs it could mean."),
        key("pins", Any, "The pins it could mean."),
        key("products", Arr, "The products there are."),
        key("roots", Arr, "The root assemblies there are."),
        key("bends", Arr, "The bends there are."),
        key("fields", Any, "The fields at fault."),
        key("columns", Any, "The columns read."),
        key("areas", Any, "The areas there are."),
        key("pulls", Any, "The pull requests it could mean."),
        key("loops", Any, "The loops at fault."),
        key("expected_revisions", Any, "The revisions given to expect."),
        key("key", Str, "The key at fault."),
        key("kind", Str, "The kind at fault."),
        key("type", Str, "The type at fault."),
        key("name", Str, "The name given."),
        key("layer", Str, "The layer at fault."),
        key("board", Str, "The board at fault."),
        key("block", Str, "The block at fault."),
        key("choice", Any, "The choice at fault."),
        key("conflict", Any, "The conflict the choice is for."),
        key("latest", Str, "The latest label."),
        key("token", Str, "The token at fault."),
        key("topic", Str, "The topic at fault."),
        key("source", Str, "The source at fault."),
        key("library", Str, "The library at fault."),
        key("condition", Str, "The condition at fault."),
        key("instruction", Str, "The instruction at fault."),
        key("form", Str, "The form at fault."),
        key("index", Num, "The index at fault."),
        key("text", Str, "The text at fault."),
        key("move", Num, "The move at fault, by its index."),
        key("pattern", Str, "The pattern at fault."),
        key("colour", Str, "The colour at fault."),
        key("plane", Str, "The plane at fault."),
        key("progress", Any, "The progress value given."),
        key("explode", Str, "The exploded view at fault."),
        key("motion", Str, "The motion at fault."),
        key("planes", Num, "How many planes there are."),
        key("origin_m", Arr, "The origin, in metres."),
        key("on", Str, "The feature it is on."),
        key("length_m", Num, "The length, in metres."),
        key("instance", Any, "The instance at fault."),
        key("tolerance", Any, "The tolerance given."),
        key("of", Str, "What it is of."),
        key("frames", Num, "How many frames."),
        key("below", Any, "The limit it is below."),
        key("status", Any, "The status at fault."),
        key("by", Any, "Who or what it is by."),
        key("repair", Str, "The repair at fault."),
        key("opacity", Num, "The opacity given."),
        key("unset", Any, "What was unset."),
        key("git", Any, "The Git command that failed."),
        key("stderr", Str, "What the command printed."),
        key("view", Str, "The view at fault."),
        key("fit", Any, "The fit at fault."),
        key("bend", Str, "The bend at fault."),
        key("place", Str, "The place at fault."),
        key("depth", Num, "The greatest depth allowed."),
        key("position", Any, "The position at fault."),
        key("standard", Str, "The standard at fault."),
        key("size", Any, "The size at fault."),
        key("missing", Any, "What is missing."),
        key("entry", Any, "The entry at fault."),
        key("listed", Arr, "The values the standard lists."),
        key("todo", Arr, "What is left to fix before it can be saved."),
        key("constraint", Str, "The sketch constraint at fault."),
        key("allowed", Arr, "The values the parameter takes."),
    ],
    NotFound => [
        key("part", Str, "The part asked for."),
        key("feature", Str, "The feature asked for."),
        key("solid", Str, "The solid asked for."),
        key("solids", Any, "The solids there are."),
        key("reference", Str, "The reference that resolved to nothing, as text."),
        alias("ref", "reference"),
        key("entity", Str, "The entity reference asked for."),
        key("param", Str, "The parameter asked for, or the request parameter that named it."),
        alias("parameter", "param"),
        key("sketch", Str, "The sketch asked for."),
        key("block", Str, "The block asked for."),
        key("sheet", Str, "The sheet asked for."),
        key("sheets", Arr, "The sheets there are."),
        key("net", Str, "The net asked for."),
        key("layer", Str, "The layer asked for."),
        key("layers", Arr, "The layers there are."),
        key("board", Str, "The board asked for."),
        key("boards", Arr, "The boards there are."),
        key("after", Str, "The feature to insert after."),
        key("before", Str, "The feature to insert before."),
        key("finding", Str, "The finding asked for."),
        key("mpn", Str, "The manufacturer part number asked for."),
        key("design", Str, "The design asked for."),
        key("index", Num, "The index asked for."),
        key("property", Str, "The property asked for."),
        key("folder", Str, "The folder asked for."),
        key("project", Str, "The project asked for."),
        key("dependency", Str, "The dependency asked for."),
        key("dependencies", Arr, "The dependencies there are."),
        key("instance", Str, "The instance asked for."),
        key("occurrence", Any, "The occurrence asked for."),
        key("role", Str, "The role the reference plays."),
        key("graphic", Str, "The graphic asked for."),
        key("class", Str, "The class asked for."),
        key("regions", Any, "The regions there are."),
        key("rail", Str, "The rail asked for."),
        key("alias", Str, "The dependency alias asked for."),
        key("component", Str, "The library component asked for."),
        key("footprint", Str, "The footprint asked for."),
        key("codes", Arr, "The codes there are."),
        key("text", Str, "The text asked for."),
        key("point", Str, "The point asked for."),
        key("track", Any, "The track asked for."),
        key("extends", Str, "The template it extends."),
        key("entry", Any, "The activity entry asked for."),
        key("participant", Str, "The participant asked for."),
        key("island", Str, "The island asked for."),
        key("step", Any, "The step asked for."),
        key("steps", Arr, "The steps there are."),
        key("standard", Str, "The standard asked for."),
        key("rev", Str, "The revision asked for."),
        key("material", Str, "The material asked for."),
        key("input", Str, "The input asked for."),
        key("of", Str, "What it was looked for in."),
        key("removed_by", Str, "The feature that took it away."),
        key("insert", Str, "The insert asked for."),
        key("id", Str, "The ID asked for."),
        key("file", Str, "The file it was looked for in."),
        key("size", Any, "The size asked for."),
        key("import", Str, "The mapping asked for."),
        key("imports", Arr, "The mappings there are."),
        key("move", Num, "The index of the schematic move that named it."),
        key("drawing", Str, "The drawing asked for."),
        key("group", Str, "The group asked for."),
        key("keepout", Str, "The keepout asked for."),
        key("unresolved", Any, "What didn't resolve."),
        key("errors", Any, "Why each place looked failed."),
        key("hole", Str, "The hole asked for."),
        key("reason", Str, "Why, as a word agents branch on (`no_hole`)."),
        key("seed", Any, "The reference's seed."),
        key("axis", Any, "The axis asked for."),
    ],
    InvalidDocument => [
        key("entity", Str, "The entity at fault."),
        key("reason", Str, "Why, as a word agents branch on (`shadows_builtin`)."),
        key("known", Arr, "The names accepted."),
    ],
    ReferenceWrongKind => [
        key("feature", Str, "The feature whose reference it is."),
        key("role", Str, "The role the reference plays (`axis`, `on`)."),
        key("reference", Str, "The reference, as text."),
        key("entity", Str, "The entity it resolved to."),
        key("wanted", Arr, "The kinds the role takes."),
        key("got", Str, "The kind it is."),
        key("kind", Str, "The kind of entity it resolved to."),
        key("seed", Any, "The reference's seed."),
        key("sketch", Str, "The sketch it is in."),
        key("part", Str, "The part it is in."),
        key("param", Str, "The parameter that holds it."),
        key("names", Any, "The names it could take."),
        key("axis", Any, "The axis given."),
        key("connector", Str, "The connector, as text."),
        key("rev", Str, "The revision of the part it is in."),
    ],
    DuplicateId => [
        key("id", Str, "The ID already taken."),
        key("part", Str, "The part it is in, or the part ID taken."),
        key("feature", Str, "The feature ID taken."),
        key("param", Str, "The parameter name taken, or the request parameter."),
        alias("parameter", "param"),
        key("reference", Str, "The reference already taken."),
        key("pin", Str, "The pin taken."),
        key("alias", Str, "The dependency alias taken."),
        key("drawing", Str, "The drawing ID taken."),
        key("class", Str, "The class taken."),
        key("board", Str, "The board taken."),
        key("dependency", Str, "The dependency taken."),
        key("project", Str, "The project ID taken."),
        key("view", Str, "The view taken."),
    ],
    Internal => [
        key("kernel", Str, "What ryukei said, as text."),
        key("feature", Str, "The feature being built."),
        key("panic", Str, "The panic's message."),
        key("part", Str, "The part being built."),
        key("tag", Str, "The tag at fault."),
        key("created", Any, "What was created."),
        key("moved", Any, "What was moved."),
        key("layer", Str, "The layer at fault."),
        alias("file", "path"),
    ],
    DependencyFailed => [
        key("feature", Str, "The feature that couldn't be built."),
        key("dependency", Str, "What it depends on that failed."),
        key("cause", Str, "The failed dependency's own code."),
        key("entity", Str, "The entity it reads."),
        key("solid", Str, "The solid it reads."),
        key("view", Str, "The view it reads."),
    ],
    RebuildFailed => [
        key("feature", Str, "The feature that failed."),
        key("kernel", Str, "What ryukei said, as text."),
        key("point", Str, "The point at fault."),
        key("place", Str, "The place at fault."),
        key("seed", Any, "The seed at fault."),
        key("instance", Any, "The instance at fault."),
        key("parts", Arr, "Every part's report, failures and all: `rebuild`'s answer."),
        key("placed", Arr, "The assemblies' placed parts that failed."),
        key("warnings", Arr, "The rebuild's warnings."),
    ],
    InvalidExtent => [
        key("feature", Str, "The feature whose extent it is."),
        key("role", Str, "The role at fault (`axis`)."),
        key("reference", Str, "The reference it extends to."),
        key("kernel", Str, "What ryukei said, as text."),
    ],
    DependencyProjectMismatch => [
        key("project", Str, "The project ID locked or expected."),
        key("got", Str, "The project ID found there."),
        alias("found", "got"),
        key("url", Str, "The dependency's URL."),
        key("alias", Str, "The dependency's alias."),
    ],
    PropertyInvalid => [
        key("property", Str, "The property at fault."),
        key("part", Str, "The part it is on."),
        key("value", Any, "The value given."),
        key("missing", Any, "What is missing."),
    ],
    ProfileInvalid => [
        key("entities", Arr, "The sketch entities at fault."),
        key("reason", Str, "Why, as a word agents branch on (`branches`, `open_profile`)."),
        key("kernel", Str, "What ryukei said, as text."),
        key("feature", Str, "The feature whose profile it is."),
        key("points", Any, "The points at fault."),
        key("role", Str, "The role the profile plays."),
        key("face", Str, "The face used as the profile."),
    ],
    WrongPartKind => [
        key("part", Str, "The part of the wrong kind."),
        key("feature", Str, "The feature that needs another kind."),
        key("kind", Str, "The kind it is (`assembly`)."),
        key("type", Str, "The feature type that needs another kind."),
    ],
    OperationNoEffect => [
        key("feature", Str, "The feature that would change nothing."),
        key("tool", Str, "The tool that touches nothing."),
        key("target", Str, "The target left unchanged."),
        key("side", Str, "The side kept."),
        key("point", Str, "The point at fault."),
        key("annotation", Str, "The annotation that would change nothing."),
    ],
    LabelNotFound => [
        key("part", Str, "The part whose label is missing."),
        key("label", Str, "The label asked for."),
        key("project", Str, "The project ID the label was looked for in."),
        key("alias", Str, "The dependency alias the reference went through."),
        key("component", Str, "The library component whose label is missing (ryueda)."),
        key("param", Str, "The request parameter that named the label."),
    ],
    DrcFailed => [
        key("report", Obj, "The board's DRC report: `board`, `findings` and `drc`."),
    ],
    ItemNotFound => [
        key("item", Str, "The item ID or part number asked for."),
    ],
    ItemClassUnknown => [
        key("item", Str, "The item whose class it is."),
        key("class", Str, "The class `ryuplm.json` doesn't declare."),
    ],
    PartNumberUnschemed => [
        key("item", Str, "The item whose number matches no scheme."),
        key("value", Str, "The number."),
    ],
    PartNumberDuplicateElsewhere => [
        key("item", Str, "The item whose number is taken elsewhere."),
        key("value", Str, "The number."),
    ],
    ItemLinkDuplicate => [
        key("item", Str, "The item whose link it is."),
        key("product", Str, "The product the link is to (`ryucad`, `ryueda`)."),
        key("other", Str, "The item that already links the representation."),
    ],
    ItemLinkUnresolved => [
        key("item", Str, "The item whose link it is."),
        key("product", Str, "The product the link is to."),
        key("project", Str, "The project ID the link names."),
        key("part", Str, "The part the link names."),
        key("searched", Arr, "The folders searched for the project."),
        key("folder", Str, "The folder that holds the project."),
        key("exe", Str, "The program that couldn't be run."),
    ],
    ItemNumberMismatch => [
        key("item", Str, "The item whose number it is."),
        key("product", Str, "The product of the representation."),
        key("project", Str, "The project ID of the representation."),
        key("part", Str, "The representation's part."),
        key("number", Str, "The number the representation says."),
    ],
    BomUnlinked => [
        key("item", Str, "The item whose BOM it is."),
        key("unlinked", Arr, "The representation's lines no item links."),
    ],
    BomLineTwice => [
        key("item", Str, "The item whose BOM it is."),
        key("line", Str, "The line that names the child."),
        key("child", Str, "The item its representation places too."),
    ],
    BomCycle => [
        key("item", Str, "The first item of the cycle."),
        key("cycle", Arr, "The items of the cycle, in order."),
    ],
    BoardStackupInvalid => [],
    BoardLayerNotFound => [
        key("layer", Str, "The layer named, which the board's stackup doesn't have."),
    ],
    DrcViaSpan => [
        key("from", Str, "The copper layer the via starts on."),
        key("to", Str, "The copper layer the via ends on."),
    ],
    DrcHoleClearance => [
        key("layer", Str, "The copper layer the gap was measured on."),
        key("layers", Arr, "The copper layers the item is on."),
        key("distance_nm", Num, "The gap to the hole wall, in nanometres."),
        key("exact", Bool, "Whether the distance is exact."),
        key("bound_nm", Any, "The distance's bound, in nanometres, when it isn't exact."),
        key("uncertain", Bool, "Whether the verdict is uncertain."),
        key("plated", Bool, "Whether the hole is plated."),
        key("rule_nm", Num, "The hole clearance asked for, in nanometres."),
    ],
    ImportConflict => [
        key("item", Str, "The item whose field changed on both sides."),
        key("key", Str, "The item's key in the source."),
        key("row", Num, "The source's row."),
        key("field", Str, "The field that changed."),
        key("base", Any, "The value the last import wrote."),
        key("catalogue", Any, "The value in the catalogue."),
        key("source", Any, "The value in the source."),
    ],
    ImportMissing => [
        key("item", Str, "The item the source no longer has, or whose line it lost."),
        key("key", Str, "The item's key in the source."),
        key("revision", Str, "The revision label the source no longer has."),
        key("line", Str, "The BOM line the source no longer has."),
        key("child", Str, "The child item of that line."),
    ],
    ImportMappingInvalid => [
        key("import", Str, "The mapping's name."),
        key("missing", Arr, "The columns the mapping names that the source hasn't."),
        key("columns", Arr, "The columns the source has."),
    ],
    ItemHistoryLabelForeign => [
        key("item", Str, "The item whose history it is."),
        key("label", Str, "The revision label outside the scheme."),
        key("scheme", Str, "The catalogue's revision scheme."),
    ],
    PartNumberTaken => [
        key("status", Num, "The HTTP status the ledger answered."),
        key("service_code", Str, "The ledger's own code."),
        key("service", Any, "The ledger's details: who holds each number."),
    ],
    LedgerUnavailable => [
        key("why", Str, "Why the ledger can't be asked, as a code (`ledger_off`, `review_unreachable`)."),
        key("cause", Obj, "The error asking it gave."),
    ],
    ReleaseTagClash => [
        key("tag", Str, "The tag that couldn't be written."),
        key("clashes", Str, "The tag Git already holds that it clashes with."),
        key("created", Any, "The tags written before it, which are kept."),
        key("moved", Any, "The tags moved before it, which are kept."),
    ],
    BoardClassNotFound => [
        key("class", Str, "The net class the rules name, which the design doesn't have."),
    ],
    BoardPresetNotFound => [
        key("preset", Str, "The preset asked for."),
    ],
    RuleBelowPreset => [
        key("floor_nm", Num, "The preset's floor, in nanometres."),
        key("max_nm", Num, "The preset's greatest drill, in nanometres."),
    ],
    WaiverExceeded => [
        key("reason", Str, "Why the finding was waived."),
        key("by", Str, "Who waived it."),
        key("limit", Num, "The waiver's limit, which the finding now passes."),
    ],
    FindingNotFound => [
        key("finding", Str, "The finding asked for."),
    ],
    FindingNotWaivable => [
        key("finding", Str, "The finding asked for."),
    ],
    PresetInUse => [
        key("boards", Arr, "The boards that use the preset."),
    ],
    PlacementConstraintUnmet => [
        key("constraint", Any, "The placement constraint that isn't met."),
        key("distance_nm", Num, "How far the part is from its target, in nanometres."),
        key("within_nm", Num, "How near the constraint asks it to be, in nanometres."),
        key("uncertain", Bool, "Whether the verdict is uncertain."),
        key("missing", Bool, "Whether the constraint's target is missing."),
    ],
    PlacementNoRoom => [
        key("blocked_by", Str, "What blocked the nearest point that held the constraints."),
        key("searched_nm", Num, "How far from the target was searched, in nanometres."),
    ],
    DrcKeepout => [
        key("uncertain", Bool, "Whether the verdict is uncertain."),
        key("keeps", Arr, "What the keepout keeps out."),
    ],
    BoardContourInvalid => [],
    PourEmpty => [
        key("net", Str, "The pour's net."),
        key("islands_removed", Num, "How many islands its net's copper doesn't reach were dropped."),
    ],
    PourThermalStarved => [
        key("spokes", Num, "The thermal spokes that reach the pad."),
    ],
    DocumentNotFound => [
        key("document", Str, "The document asked for, by ID or number."),
    ],
    DocumentBytesMissing => [
        key("document", Str, "The document at fault."),
    ],
    DocumentBytesChanged => [
        key("document", Str, "The document at fault."),
        key("expected", Obj, "The size and BLAKE3 hash its record names."),
        key("actual", Obj, "The size and BLAKE3 hash of the bytes there."),
    ],
    DocumentFileUnknown => [
        key("document", Str, "The document at fault."),
    ],
    DocumentLarge => [
        key("document", Str, "The document at fault."),
        key("size", Num, "The file's size, in bytes."),
    ],
    DocumentTooLarge => [
        key("document", Str, "The document at fault."),
        key("file", Str, "The file given."),
        key("size", Num, "The file's size, in bytes."),
    ],
    DocumentMissing => [
        key("item", Str, "The item whose record names the document."),
        key("document", Str, "The document named, which the catalogue doesn't have."),
    ],
    DocumentUnresolved => [
        key("item", Str, "The item whose record names the document."),
        key("document", Str, "The document named, as text."),
        key("searched", Arr, "The catalogues searched."),
    ],
    DocumentInUse => [
        key("document", Str, "The document at fault."),
        key("items", Arr, "The items whose records name it."),
    ],
    DocumentReleased => [
        key("document", Str, "The document at fault."),
    ],
    DocumentUnreleased => [
        key("item", Str, "The item whose record names the document."),
        key("document", Str, "The document with no release of its current content."),
    ],
}

/// Words written their own way in titles.
const WORDS: &[(&str, &str)] = &[
    ("json", "JSON"),
    ("id", "ID"),
    ("ids", "IDs"),
    ("bom", "BOM"),
    ("pdm", "PDM"),
    ("odb", "ODB++"),
    ("kicad", "KiCad"),
    ("eda", "EDA"),
    ("step", "STEP"),
    ("git", "Git"),
    ("github", "GitHub"),
    ("erc", "ERC"),
    ("drc", "DRC"),
    ("nc", "No-connect"),
    ("refdes", "Reference designator"),
    ("demorgan", "De Morgan"),
    ("io", "I/O"),
];

/// Titles that read better than their codes' words.
const TITLES: &[(Code, &str)] = &[
    (Code::Io, "Couldn't read or write a file"),
    (Code::Usage, "Bad arguments"),
    (Code::Daemon, "The daemon didn't answer"),
    (Code::Internal, "Internal error"),
    (Code::NotFound, "Not found"),
    (Code::Conflict, "Changed since you read it"),
    (Code::NotAGitRepo, "Not a Git repository"),
    (Code::NothingToUndo, "Nothing to undo"),
    (Code::UndoConflict, "Can't undo: the files changed since"),
    (Code::Cancelled, "Cancelled"),
    (Code::CheckFailed, "Check failed"),
    (Code::RebuildFailed, "Rebuild failed"),
    (Code::BlendFailed, "Fillet or chamfer failed"),
    (Code::BooleanFailed, "Boolean failed"),
    (Code::ReferenceUnresolved, "A reference no longer resolves"),
    (Code::ReferenceForward, "A reference to a later feature"),
    (Code::ReferenceBody, "A reference import can't be changed"),
    (
        Code::ReferenceNotSolid,
        "An open reference import has no mass",
    ),
    (Code::FormatTooNew, "Written by a newer ryucad"),
    (Code::AppNotInstalled, "The app isn't installed"),
];

/// A short title for `code`: its own from [`TITLES`], else its words
/// (`unit_mismatch` is `Unit mismatch`), names written as they are
/// (`invalid_json` is `Invalid JSON`).
pub fn title(code: Code) -> String {
    if let Some((_, t)) = TITLES.iter().find(|(c, _)| *c == code) {
        return (*t).to_owned();
    }
    let name = serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let words: Vec<String> = name
        .split('_')
        .map(|w| match WORDS.iter().find(|(from, _)| *from == w) {
            Some((_, to)) => (*to).to_owned(),
            None => w.to_owned(),
        })
        .collect();
    let mut s = words.join(" ");
    if let Some(first) = s.get(..1) {
        s = first.to_uppercase() + &s[1..];
    }
    s
}

/// `code` as agents see it: `not_found`.
pub fn name(code: Code) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The key `name` that `code` declares, common keys included; `None` when
/// `code` is undeclared or doesn't declare it.
pub fn find(code: Code, name: &str) -> Option<&'static Key> {
    keys(code)?.iter().chain(COMMON).find(|k| k.name == name)
}

/// The declared key that `name` is an older spelling of, in `code`.
pub fn alias_of(code: Code, name: &str) -> Option<&'static str> {
    find(code, name)?.alias_of
}

/// The kind of `key`'s values in `code`: an alias's is its declared key's.
pub fn kind(code: Code, key: &Key) -> Kind {
    match key.alias_of {
        Some(of) => find(code, of).map_or(Kind::Any, |k| k.kind),
        None => key.kind,
    }
}

/// What a key's words are: an alias says what it is beside.
pub fn about(key: &Key) -> String {
    match key.alias_of {
        Some(of) => format!("An older spelling of `{of}`, carried beside it."),
        None => key.about.to_owned(),
    }
}

/// One way an error's details break its code's declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The key, empty when the details aren't an object.
    pub key: String,
    /// The kind of value it holds.
    pub got: Kind,
    /// The kind declared, `None` when the key isn't declared.
    pub declared: Option<Kind>,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let got = self.got.name();
        match self.declared {
            _ if self.key.is_empty() => write!(f, "details are {got}, not an object"),
            None => write!(f, "`{}` ({got}) isn't declared", self.key),
            Some(k) => write!(f, "`{}` is {got}, declared {}", self.key, k.name()),
        }
    }
}

/// How `details` break `code`'s declaration: a key it doesn't declare, or
/// a value of another kind. Nothing for an undeclared code, and missing
/// keys are never a problem (one code is raised from many places).
pub fn problems(code: Code, details: &Value) -> Vec<Problem> {
    if keys(code).is_none() {
        return Vec::new();
    }
    let map = match details {
        Value::Null => return Vec::new(),
        Value::Object(m) => m,
        other => {
            return vec![Problem {
                key: String::new(),
                got: Kind::of(other),
                declared: Some(Kind::Obj),
            }];
        }
    };
    let mut found = Vec::new();
    for (k, v) in map {
        match find(code, k) {
            None => found.push(Problem {
                key: k.clone(),
                got: Kind::of(v),
                declared: None,
            }),
            Some(key) => {
                let want = kind(code, key);
                if !want.admits(v) {
                    found.push(Problem {
                        key: k.clone(),
                        got: Kind::of(v),
                        declared: Some(want),
                    });
                }
            }
        }
    }
    found
}

/// Set to a file, the check appends what it finds there (one
/// `code<TAB>key<TAB>kind<TAB>declared` line a problem) instead of
/// panicking: the record run of plan 33.E, and of whoever declares the
/// next codes.
pub const RECORD: &str = "RYU_DETAILS_RECORD";

/// Checks `e` against its code's declaration as it leaves a product. A
/// debug build panics on a key the code doesn't declare or a value of
/// another kind (see [`RECORD`]); a release build does nothing.
pub fn check(e: &super::Error) {
    check_value(e.code, &e.details);
}

/// [`check`] for a refusal whose details are already JSON text
/// (`ryu_session::rpc::Raw::Error`: a failed rebuild's report).
pub fn check_raw(code: Code, details: &str) {
    if !cfg!(debug_assertions) || keys(code).is_none() || details == "null" {
        return;
    }
    let v: Value = serde_json::from_str(details)
        .unwrap_or_else(|e| panic!("error `{}`: its details aren't JSON: {e}", name(code)));
    check_value(code, &v);
}

fn check_value(code: Code, details: &Value) {
    if !cfg!(debug_assertions) {
        return;
    }
    let found = problems(code, details);
    if found.is_empty() {
        return;
    }
    if let Some(file) = std::env::var_os(RECORD) {
        record(std::path::Path::new(&file), code, &found);
        return;
    }
    let list: Vec<String> = found.iter().map(ToString::to_string).collect();
    panic!(
        "error `{}`: {}; declare it in crates/ryu-doc/src/error/details.rs or fix the call (design 698, #771)",
        name(code),
        list.join("; ")
    );
}

/// Appends `found` to `file`, in one write so parallel tests' lines don't
/// interleave.
fn record(file: &std::path::Path, code: Code, found: &[Problem]) {
    use std::io::Write;
    let code = name(code);
    let mut text = String::new();
    for p in found {
        let declared = p.declared.map_or("-", Kind::name);
        text.push_str(&format!(
            "{code}\t{}\t{}\t{declared}\n",
            p.key,
            p.got.name()
        ));
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .unwrap_or_else(|e| panic!("{RECORD}={}: {e}", file.display()));
    let _ = f.write_all(text.as_bytes());
}

fn key_json(code: Option<Code>, k: &Key) -> Value {
    let kind = code.map_or(k.kind, |c| kind(c, k));
    let mut v = serde_json::json!({"name": k.name, "kind": kind.name(), "about": about(k)});
    if let Some(of) = k.alias_of {
        v["alias_of"] = serde_json::json!(of);
    }
    v
}

/// One code as `errors` lists it.
fn code_json(code: Code) -> Value {
    let keys: Vec<Value> = keys(code)
        .unwrap_or_default()
        .iter()
        .map(|k| key_json(Some(code), k))
        .collect();
    serde_json::json!({
        "code": name(code),
        "exit": code.exit_code(),
        "title": title(code),
        "declared": code.keys().is_some(),
        "keys": keys,
    })
}

/// What `ryucad errors`, `ryueda errors` and `ryudfm errors` answer:
/// `{codes: [{code, exit, title, declared, keys}], common: [key]}`, every
/// code in `Code::ALL`'s order or just `code`. In process, from this
/// table: no project, daemon or protocol.
pub fn listing(code: Option<&str>) -> super::Result<Value> {
    let codes: Vec<Code> = match code {
        None => Code::ALL.to_vec(),
        Some(text) => {
            let c = serde_json::from_value(serde_json::json!(text)).map_err(|_| {
                super::Error::new(
                    Code::InvalidParam,
                    format!("no error code {text:?}: `errors` alone lists them"),
                )
                .with("param", "code")
                .with("value", text)
            })?;
            vec![c]
        }
    };
    let common: Vec<Value> = COMMON.iter().map(|k| key_json(None, k)).collect();
    Ok(serde_json::json!({
        "codes": codes.into_iter().map(code_json).collect::<Vec<_>>(),
        "common": common,
    }))
}

/// The table as `tests/golden/details.txt` holds it: the common keys, then
/// each declared code in `Code::ALL`'s order with its exit code and title,
/// and its keys one a line (`name kind about`).
pub fn golden() -> String {
    let mut out = String::from("common\n");
    for k in COMMON {
        out.push_str(&format!("  {} {} {}\n", k.name, k.kind.name(), about(k)));
    }
    for &code in Code::ALL {
        let Some(own) = keys(code) else { continue };
        out.push_str(&format!(
            "\n{} exit {} \"{}\"\n",
            name(code),
            code.exit_code(),
            title(code)
        ));
        for k in own {
            out.push_str(&format!(
                "  {} {} {}\n",
                k.name,
                kind(code, k).name(),
                about(k)
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_kind_admits_its_values() {
        let values = [
            json!("a"),
            json!(1.5),
            json!(true),
            json!([1]),
            json!({"a": 1}),
        ];
        let kinds = [Kind::Str, Kind::Num, Kind::Bool, Kind::Arr, Kind::Obj];
        for (i, k) in kinds.iter().enumerate() {
            for (j, v) in values.iter().enumerate() {
                assert_eq!(k.admits(v), i == j, "{k:?} {v}");
            }
            assert!(k.admits(&Value::Null), "null is every kind");
            assert_eq!(Kind::of(&values[i]), *k);
        }
        assert!(values.iter().all(|v| Kind::Any.admits(v)));
    }

    #[test]
    fn problems_name_undeclared_keys_and_wrong_kinds() {
        let d = json!({"part": "p", "label": 3, "colour": "red", "hint": "x", "op": 1});
        let mut found: Vec<String> = problems(Code::LabelNotFound, &d)
            .iter()
            .map(ToString::to_string)
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                "`colour` (str) isn't declared",
                "`label` is num, declared str"
            ]
        );
        assert!(problems(Code::LabelNotFound, &json!({"label": null})).is_empty());
    }

    #[test]
    fn undeclared_codes_and_absent_details_have_no_problems() {
        assert!(problems(Code::Conflict, &json!({"anything": 1})).is_empty());
        assert!(problems(Code::LabelNotFound, &Value::Null).is_empty());
        assert_eq!(
            problems(Code::LabelNotFound, &json!("text"))[0].to_string(),
            "details are str, not an object"
        );
    }

    /// The table's own rules: no code declared twice, no key twice in a
    /// code or beside a common key, every alias resolving to a declared
    /// key that isn't itself an alias, and every declared key explained.
    #[test]
    fn the_table_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for &code in DECLARED {
            assert!(seen.insert(code), "{code:?} declared twice");
            let own = keys(code).expect("declared");
            let mut names = std::collections::HashSet::new();
            for k in own.iter().chain(COMMON) {
                assert!(names.insert(k.name), "{code:?}: `{}` twice", k.name);
                match k.alias_of {
                    Some(of) => {
                        let target = find(code, of)
                            .unwrap_or_else(|| panic!("{code:?}: `{}` aliases nothing", k.name));
                        assert!(
                            target.alias_of.is_none(),
                            "{code:?}: `{}` aliases an alias",
                            k.name
                        );
                    }
                    None => assert!(!k.about.is_empty(), "{code:?}: `{}` unexplained", k.name),
                }
            }
        }
    }

    use crate::error::Error;

    #[test]
    fn a_declared_error_passes_and_undeclared_codes_are_not_read() {
        check(
            &Error::new(Code::LabelNotFound, "x")
                .with("part", "p")
                .with("hint", "h"),
        );
        check_raw(Code::LabelNotFound, r#"{"part":"p","commit":"abc"}"#);
        check_raw(Code::LabelNotFound, "null");
        check_raw(
            Code::Conflict,
            "not even JSON: an undeclared code isn't read",
        );
    }

    /// In a debug build an undeclared key panics, naming the code, the key
    /// and where to declare it. (Not under the record switch, which writes
    /// instead.)
    #[test]
    fn check_panics_in_debug_builds() {
        if !cfg!(debug_assertions) || std::env::var_os(RECORD).is_some() {
            return;
        }
        let e = Error::new(Code::LabelNotFound, "x").with("colour", "red");
        let panic = std::panic::catch_unwind(|| check(&e)).unwrap_err();
        let text = panic.downcast_ref::<String>().expect("a formatted panic");
        assert!(
            text.contains("error `label_not_found`: `colour` (str) isn't declared")
                && text.contains("crates/ryu-doc/src/error/details.rs"),
            "{text}"
        );
        let raw = std::panic::catch_unwind(|| check_raw(Code::LabelNotFound, r#"{"label":1}"#));
        assert!(raw.is_err(), "a raw refusal is checked too");
    }

    #[test]
    fn the_record_switch_appends_one_line_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("found.txt");
        let found = problems(Code::LabelNotFound, &json!({"colour": "red", "label": 1}));
        record(&file, Code::LabelNotFound, &found);
        record(&file, Code::LabelNotFound, &found);
        let once = "label_not_found\tcolour\tstr\t-\nlabel_not_found\tlabel\tnum\tstr\n";
        assert_eq!(std::fs::read_to_string(&file).unwrap(), once.repeat(2));
    }

    /// The codes design 698 declares, and the codes added since it.
    #[test]
    fn the_most_used_codes_are_declared() {
        for c in [
            Code::InvalidParam,
            Code::NotFound,
            Code::InvalidDocument,
            Code::ReferenceWrongKind,
            Code::DuplicateId,
            Code::Internal,
            Code::DependencyFailed,
            Code::RebuildFailed,
            Code::InvalidExtent,
            Code::DependencyProjectMismatch,
            Code::PropertyInvalid,
            Code::ProfileInvalid,
            Code::WrongPartKind,
            Code::OperationNoEffect,
            Code::LabelNotFound,
            Code::DrcFailed,
            Code::ItemNotFound,
            Code::ItemClassUnknown,
            Code::PartNumberUnschemed,
            Code::PartNumberDuplicateElsewhere,
            Code::ItemLinkDuplicate,
            Code::ItemLinkUnresolved,
            Code::ItemNumberMismatch,
            Code::BomUnlinked,
            Code::BomLineTwice,
            Code::BomCycle,
            Code::BoardStackupInvalid,
            Code::BoardLayerNotFound,
            Code::DrcViaSpan,
            Code::DrcHoleClearance,
            Code::ImportConflict,
            Code::ImportMissing,
            Code::ImportMappingInvalid,
            Code::ItemHistoryLabelForeign,
            Code::PartNumberTaken,
            Code::LedgerUnavailable,
        ] {
            assert!(keys(c).is_some(), "{c:?}");
        }
        // The synonyms: the older spelling declared beside the declared one.
        assert_eq!(alias_of(Code::InvalidParam, "field"), Some("param"));
        assert_eq!(alias_of(Code::InvalidParam, "parameter"), Some("param"));
        assert_eq!(alias_of(Code::NotFound, "ref"), Some("reference"));
        assert_eq!(alias_of(Code::Internal, "file"), Some("path"));
        assert_eq!(alias_of(Code::InvalidParam, "expected"), Some("wanted"));
        assert_eq!(
            alias_of(Code::DependencyProjectMismatch, "found"),
            Some("got")
        );
        assert_eq!(
            kind(
                Code::InvalidParam,
                find(Code::InvalidParam, "field").unwrap()
            ),
            Kind::Str
        );
    }

    #[test]
    fn every_code_has_a_title() {
        assert!(Code::ALL.len() > 150, "{}", Code::ALL.len());
        for &c in Code::ALL {
            let t = title(c);
            assert!(!t.is_empty() && !t.contains('_'), "{c:?}: {t}");
            assert!(t.starts_with(char::is_uppercase), "{c:?}: {t}");
        }
        assert_eq!(title(Code::UnitMismatch), "Unit mismatch");
        assert_eq!(title(Code::InvalidJson), "Invalid JSON");
        assert_eq!(title(Code::OdbLayerUnreadable), "ODB++ layer unreadable");
        assert_eq!(title(Code::BomItemDuplicate), "BOM item duplicate");
        assert_eq!(title(Code::Cancelled), "Cancelled");
        assert_eq!(title(Code::BlendFailed), "Fillet or chamfer failed");
    }

    #[test]
    fn listing_names_every_code_or_one() {
        let all = listing(None).unwrap();
        assert_eq!(all["codes"].as_array().unwrap().len(), Code::ALL.len());
        assert_eq!(all["common"].as_array().unwrap().len(), COMMON.len());
        assert_eq!(
            all["common"][0],
            json!({"name": "hint", "kind": "str",
            "about": "What to do about it: a command to run or a sentence."})
        );
        let one = &listing(Some("label_not_found")).unwrap()["codes"];
        assert_eq!(one.as_array().unwrap().len(), 1);
        assert_eq!(
            (
                &one[0]["code"],
                &one[0]["exit"],
                &one[0]["title"],
                &one[0]["declared"]
            ),
            (
                &json!("label_not_found"),
                &json!(1),
                &json!("Label not found"),
                &json!(true)
            )
        );
        let usage = &listing(Some("usage")).unwrap()["codes"][0];
        assert_eq!(
            (&usage["exit"], &usage["declared"], &usage["keys"]),
            (&json!(2), &json!(false), &json!([]))
        );
        let keys = listing(Some("invalid_param")).unwrap()["codes"][0]["keys"].clone();
        let field = keys
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["name"] == "field")
            .unwrap();
        assert_eq!(
            (&field["kind"], &field["alias_of"]),
            (&json!("str"), &json!("param"))
        );
        let e = listing(Some("no_such_code")).unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::InvalidParam, &json!("code"))
        );
    }
}
