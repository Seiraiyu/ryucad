//! The tree's rows (design 199, Tree): every part, its solids and
//! features, an assembly's instances, its mates under one Mates row and
//! its patterns' copies, each row with its icon, its state's badges and
//! today's text (which the window tests read); and the tile's search,
//! filters, open rows and keyboard cursor. It holds no egui: icons and
//! badges are `ui::icon` names, tints are what the look colours them, and
//! `ui::tiles::tree` draws what [`Tree::shown`] keeps.

use crate::core::command;
use crate::core::compare::{Change, Compare, Mark};
use crate::core::model::{BodyKey, Feature, MateInfo, Model, Part, Solid};
use crate::core::pdm::States;
use crate::ui::style::{self, Rgb};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// What colours an icon, a badge or a row's text: the look's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tint {
    /// The text colour.
    Plain,
    /// The look's accent: a part's or an assembly's icon.
    Accent,
    /// Weak text: what is consumed, hidden, obsolete or read only.
    Weak,
    /// Failed or conflicting: red.
    Danger,
    /// Stale, unmeshed, redundant, modified, changed: amber.
    Warn,
    /// Under-defined, proposed: blue.
    Info,
    /// Released, added: green.
    Ok,
}

/// A state after a row's text: an icon and what it says on hover. A badge
/// never says anything the row's text doesn't.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub icon: &'static str,
    pub tint: Tint,
    pub tip: String,
}

impl Badge {
    fn new(icon: &'static str, tint: Tint, tip: impl Into<String>) -> Badge {
        Badge {
            icon,
            tint,
            tip: tip.into(),
        }
    }
}

/// One row of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Unique among the rows, and the same from one rebuild to the next:
    /// what open rows and the cursor are kept by.
    pub id: String,
    pub depth: usize,
    /// Its parent's index in the rows.
    pub parent: Option<usize>,
    /// Whether rows come under it.
    pub children: bool,
    /// Whether it starts open.
    pub open: bool,
    /// The icon's `ui::icon` name, and its tint.
    pub icon: &'static str,
    pub tint: Tint,
    /// Today's words: what the window tests read and search matches.
    pub text: String,
    /// The text's colour where it isn't the icon's tint: a compare's mark.
    pub colour: Option<Rgb>,
    pub badges: Vec<Badge>,
    /// Drawn weak: consumed, hidden, obsolete, read only.
    pub weak: bool,
    /// Bold: a solid's row.
    pub strong: bool,
    /// Failed: the failing filter keeps it.
    pub failed: bool,
    /// A compare marks it: the changed filter keeps it.
    pub changed: bool,
    /// Everything it shows is hidden: the hidden filter keeps it.
    pub hidden: bool,
    /// The feature choosing it chooses (and frames).
    pub choose: Option<BodyKey>,
    /// A feature only the compare's other side has: chosen for its
    /// changes, with no form and nothing to frame.
    pub gone: bool,
    /// The reference its context menu and visibility act on.
    pub reference: Option<String>,
    /// Its kind for the registry's context menu.
    pub menu: Option<command::Row>,
    /// It has a Show checkbox: an instance or a copy.
    pub hideable: bool,
    /// A solid's Hide/Show in its menu.
    pub solid: bool,
    /// What its hover says: why it failed, a mate's connectors.
    pub tip: Option<String>,
    /// A part (or subassembly) the window hasn't loaded: loaded when the
    /// row opens.
    pub load: Option<String>,
    /// More words search matches: a part's PDM number and name.
    pub words: Vec<String>,
    /// The body whose stable names a hex search matches.
    pub body: Option<BodyKey>,
    /// An import's row, or the solid it founds: whether it is a
    /// reference import (design 364), for Convert to solid and Keep as
    /// reference in its context menu.
    pub import: Option<bool>,
    /// A drawing's row: its part and ID, which choosing opens in the
    /// Drawings tile.
    pub drawing: Option<(String, String)>,
}

impl Row {
    fn new(id: String, depth: usize, icon: &'static str, text: String) -> Row {
        Row {
            id,
            depth,
            parent: None,
            children: false,
            open: true,
            icon,
            tint: Tint::Plain,
            text,
            colour: None,
            badges: Vec::new(),
            weak: false,
            strong: false,
            failed: false,
            changed: false,
            hidden: false,
            choose: None,
            gone: false,
            reference: None,
            menu: None,
            hideable: false,
            solid: false,
            tip: None,
            load: None,
            words: Vec::new(),
            body: None,
            import: None,
            drawing: None,
        }
    }

    /// Failed: a red badge, and its icon and text red.
    fn fail(&mut self, why: &str) {
        self.failed = true;
        self.tint = Tint::Danger;
        self.badges.push(Badge::new("x-circle", Tint::Danger, why));
    }
}

/// A feature's line in the tree: its ID and type, a sketch's solve state,
/// and why it failed or how many faces weren't meshed.
/// `holes` is how many holes the feature makes, as its part's
/// `holes.list` counts them: a hole with a spec gives it after the spec
/// when more than one.
pub fn row_text(f: &Feature, faults: usize, holes: usize) -> String {
    let ty = match (&f.ty, &f.operation, &f.instance) {
        // An instance says what it places, and what its mates leave it.
        (_, _, Some(i)) => {
            let freedom = match &i.solve {
                _ if i.fixed => ", fixed".to_owned(),
                Some((state, _)) if state == "failing" => ", failing".to_owned(),
                Some((_, dof)) => format!(", {dof} dof"),
                None => String::new(),
            };
            let of = match &i.dependency {
                Some(d) => format!("{d}:{}", i.part),
                None => i.part.clone(),
            };
            let outdated = if i.outdated.is_some() {
                ", outdated"
            } else {
                ""
            };
            format!("{of} @ {}{freedom}{outdated}", i.rev)
        }
        // A mate its form and what it joins, where its value stands and
        // the stop it rests on; a redundant or conflicting one says so.
        _ if f.mate.is_some() => {
            let m = f.mate.as_ref().expect("a mate");
            let state = match m.state.as_deref() {
                Some("redundant" | "conflicting") => {
                    format!(", {}", m.state.as_deref().unwrap_or_default())
                }
                _ => String::new(),
            };
            let value = m
                .value
                .as_ref()
                .map_or_else(String::new, |v| format!(", {v}"));
            let stop = m
                .stop
                .as_ref()
                .map_or_else(String::new, |s| format!(", {s}"));
            format!(
                "{}: {} → {}{value}{stop}{state}",
                m.form,
                MateInfo::path_of(&m.a),
                MateInfo::path_of(&m.b)
            )
        }
        // An insert what it inserts, at which pin, and how many solids.
        _ if f.insert.is_some() => {
            let i = f.insert.as_ref().expect("an insert");
            let mut s = format!("insert of {} @ {}", i.of(), i.rev_text());
            if i.behind.is_some() {
                s.push_str(", behind");
            }
            if let Some(n) = i.count {
                s.push_str(&format!(", {n} solid{}", if n == 1 { "" } else { "s" }));
            }
            s
        }
        // A delete what it takes out.
        _ if f.delete.is_some() => {
            format!("delete {}", f.delete.as_ref().expect("a delete").text())
        }
        // An import its file and product: `import: bracket.step /
        // BRK-100`, and its body when the product has several.
        _ if f.import.is_some() => {
            let i = f.import.as_ref().expect("an import");
            let mut s = format!("import: {}", i.file_name());
            if let Some((id, _, _)) = &i.product {
                s.push_str(&format!(" / {id}"));
            }
            if i.body > 0 {
                s.push_str(&format!(", body {}", i.body));
            }
            if i.reference {
                s.push_str(", reference");
            }
            s
        }
        // A hole with intent its spec, and how many holes it makes.
        (Some(t), _, None) if t == "hole" && f.spec.is_some() => {
            let spec = f.spec.as_deref().unwrap_or_default();
            if holes > 1 {
                format!("{spec}, {holes}×")
            } else {
                spec.to_owned()
            }
        }
        // An instance pattern how many copies it places; a following one
        // its driver too.
        (Some(t), _, None) if t == "instance_pattern" && f.error.is_none() => match &f.driver {
            Some(d) => format!(
                "{}: {d}, {} copies",
                f.form.as_deref().unwrap_or("feature"),
                f.copies.len()
            ),
            None => format!("{t}, {} copies", f.copies.len()),
        },
        // A new sew or thicken founds its body as any feature does.
        (Some(t), Some(op), None) if op != "new" => format!("{t}, {op}"),
        (Some(t), _, None) => t.clone(),
        (None, _, None) => "?".to_owned(),
    };
    let kind = match &f.sketch {
        Some(s) if s.state == "under" => format!("{ty}, under, {} dof", s.dof),
        Some(s) => format!("{ty}, {}", s.state),
        None => ty,
    };
    let stale = if f.sketch.as_ref().is_some_and(|s| s.stale) {
        ", stale"
    } else {
        ""
    };
    let text = format!("{} ({kind}{stale})", f.id);
    match &f.error {
        Some(e) => match e["code"].as_str() {
            Some(code) => format!("{text}: failed ({code})"),
            None => format!("{text}: failed"),
        },
        None if faults > 0 => format!("{text}: {faults} unmeshed"),
        None => text,
    }
}

/// What an assembly's line adds: ` (assembly)`, and when it has mates
/// its solve: ` (assembly, under, 2 dof)`, `well`, `over`, `unsolved`, and
/// `stale` while its stored poses don't hold.
pub fn assembly_text(solve: Option<&crate::core::model::Solve>) -> String {
    let Some(s) = solve else {
        return " (assembly)".to_owned();
    };
    let state = match (s.converged, s.state.as_str()) {
        (false, _) => "unsolved".to_owned(),
        (true, "under") => format!("under, {} dof", s.dof),
        (true, state) => state.to_owned(),
    };
    let stale = if s.stale { ", stale" } else { "" };
    format!(" (assembly, {state}{stale})")
}

/// How a feature differs in a compare, after its line in the tree:
/// ` [changed, moved, failed at HEAD (rebuild_failed)]`; empty when it
/// doesn't.
pub fn mark_text(m: &Mark, rev: &str) -> String {
    let mut says = Vec::new();
    if m.change != Change::Same {
        says.push(m.change.as_str().to_owned());
    }
    if m.moved {
        says.push("moved".to_owned());
    }
    if let Some(code) = &m.failed_there {
        says.push(format!("failed at {rev} ({code})"));
    }
    if says.is_empty() {
        String::new()
    } else {
        format!(" [{}]", says.join(", "))
    }
}

/// A part's line in the tree: its ID, its release state (`p (B
/// modified, C proposed)`) once known, and in a compare the records it
/// adds (`[proposes C]`).
pub fn part_text(id: &str, badge: Option<&str>, marks: &[String]) -> String {
    let mut s = id.to_owned();
    if let Some(b) = badge {
        s.push_str(&format!(" ({b})"));
    }
    if !marks.is_empty() {
        s.push_str(&format!(" [{}]", marks.join(", ")));
    }
    s
}

/// The colour a compare gives a feature's line.
pub fn mark_colour(m: &Mark) -> Rgb {
    match m.change {
        Change::Added => style::NEW,
        Change::Removed => style::REMOVED,
        _ => style::CHANGED,
    }
}

/// What a reference import's badge says on hover. "Reference import",
/// never "reference" alone: Copy reference copies an entity's text
/// reference, and the hidden set and datums don't use the word.
pub const REFERENCE_TIP: &str = "reference import: the file's geometry, shown, measured and mated, never modelled on; Convert to solid in its menu heals it";

/// A reference import's badge.
fn reference_badge() -> Badge {
    Badge::new("file-lock", Tint::Info, REFERENCE_TIP)
}

/// A compare's badges for a feature: added, changed, moved.
fn mark_badges(m: &Mark, text: &str) -> Vec<Badge> {
    let mut out = Vec::new();
    match m.change {
        Change::Added => out.push(Badge::new("plus-circle", Tint::Ok, text.trim())),
        Change::Removed => out.push(Badge::new("minus-circle", Tint::Danger, text.trim())),
        Change::Changed | Change::Rebuilt => {
            out.push(Badge::new("note-pencil", Tint::Warn, text.trim()));
        }
        Change::Same => {}
    }
    if m.moved {
        out.push(Badge::new("arrows-down-up", Tint::Warn, text.trim()));
    }
    out
}

/// An error's message, as rows say why they failed.
fn message(e: &Value) -> String {
    e["message"].as_str().unwrap_or("failed").to_owned()
}

/// What kind of row a feature is, for its context menu.
pub fn row_of(f: &Feature) -> command::Row {
    use command::Row;
    if f.instance.is_some() {
        Row::Instance
    } else if f.mate.is_some() {
        Row::Mate
    } else {
        match f.ty.as_deref() {
            Some("sketch") => Row::Sketch,
            Some("pattern" | "instance_pattern") => Row::Pattern,
            _ => Row::Feature,
        }
    }
}

/// The icon of a pattern by its form.
fn pattern_icon(form: Option<&str>) -> &'static str {
    match form {
        Some("circular") => "pattern-circular",
        Some("mirror") => "flip-horizontal",
        _ => "pattern-linear",
    }
}

/// The icon of a feature of type `ty` doing `op` (`cut`, `intersect`, a
/// boolean's `subtract`), a pattern of `form`.
pub fn type_icon(ty: &str, op: Option<&str>, form: Option<&str>) -> &'static str {
    match ty {
        "box" => "cube-transparent",
        "cylinder" => "cylinder",
        "cone" => "cone",
        "sphere" => "sphere",
        "torus" => "torus",
        "sketch" => "pencil-simple-line",
        "extrude" if op == Some("cut") => "extrude-cut",
        "extrude" => "extrude",
        "revolve" if op == Some("cut") => "revolve-cut",
        "revolve" => "revolve",
        "boolean" => match op {
            Some("subtract") => "subtract",
            Some("intersect") => "intersect",
            _ => "unite",
        },
        "fillet" => "fillet",
        "chamfer" => "chamfer",
        "hole" => "hole",
        "pattern" | "instance_pattern" => pattern_icon(form),
        "import" => "file-arrow-down",
        "instance" => "cube",
        "mate" => "paperclip",
        "plane" => "datum-plane",
        "axis" => "datum-axis",
        "point" => "datum-point",
        "path" | "helix" => "path",
        "sweep" => "sweep",
        "loft" => "loft",
        "shell" => "shell",
        "offset" => "offset",
        "mirror" => "mirror-body",
        "split" => "split",
        "base_flange" => "base-flange",
        "flange" => "edge-flange",
        "unfold" => "unfold",
        "refold" => "refold",
        "planar" => "planar-face",
        "fill" => "fill",
        "extract" => "extract",
        "sew" => "sew",
        "thicken" => "thicken",
        "draft" => "draft",
        "move_face" => "move-face",
        "replace_face" => "replace-face",
        "delete_face" => "delete-face",
        "delete_blend" => "delete-blend",
        "resize_blend" => "resize-blend",
        "insert" => "arrow-square-in",
        "delete_body" => "trash",
        _ => "question",
    }
}

/// A feature's icon (design 199's table), and the `intersect` badge an
/// extrude or revolve that intersects takes. `assembly` says whether a
/// part an instance places is an assembly.
pub fn feature_icon(f: &Feature, assembly: impl Fn(&str) -> bool) -> (&'static str, bool) {
    if let Some(i) = &f.instance {
        return (
            if assembly(&i.part) {
                "assembly"
            } else {
                "cube"
            },
            false,
        );
    }
    if let Some(m) = &f.mate {
        return (mate_icon(&m.form), false);
    }
    if f.import.is_some() {
        return ("file-arrow-down", false);
    }
    let op = f.operation.as_deref();
    let icon = type_icon(f.ty.as_deref().unwrap_or_default(), op, f.form.as_deref());
    let intersects = matches!(
        f.ty.as_deref(),
        Some(
            "extrude"
                | "revolve"
                | "sweep"
                | "loft"
                | "mirror"
                | "thicken"
                | "box"
                | "cylinder"
                | "cone"
                | "sphere"
                | "torus"
        )
    ) && op == Some("intersect");
    (icon, intersects)
}

/// What a surface badge says: the feature makes a sheet, not a solid.
pub const SURFACE_TIP: &str = "surface: it makes a sheet, with no inside";

/// Whether `f` is an extrude, revolve, sweep or loft that makes a sheet
/// (ryucad 18's `surface`): the body it founds is one.
fn surface(part: &Part, f: &Feature) -> bool {
    matches!(
        f.ty.as_deref(),
        Some("extrude" | "revolve" | "sweep" | "loft")
    ) && part.solid(&f.id).is_some_and(|s| s.sheet)
}

/// A sheet's kind as a solid row says it: `sheet, 1 boundary`, `sheet,
/// 2 boundaries`, `sheet, closed`, or `sheet` when the report doesn't say.
pub fn sheet_text(boundaries: Option<u64>) -> String {
    match boundaries {
        Some(0) => "sheet, closed".to_owned(),
        Some(1) => "sheet, 1 boundary".to_owned(),
        Some(n) => format!("sheet, {n} boundaries"),
        None => "sheet".to_owned(),
    }
}

/// A mate's icon by its form.
pub fn mate_icon(form: &str) -> &'static str {
    match form {
        "fastened" => "lock-simple",
        "revolute" => "arrow-counter-clockwise",
        "slider" => "arrows-left-right",
        "cylindrical" => "mate-cylindrical",
        "planar" => "mate-planar",
        "ball" => "mate-ball",
        _ => "paperclip",
    }
}

/// One line of the icon sheet: what an icon stands for, its name and its
/// tint.
pub type SheetEntry = (&'static str, &'static str, Tint);

/// The icon sheet (design 199, Fonts and icons; `design/app/icons.html`):
/// every icon a row or a badge takes, by section, with what it stands for
/// and its tint, as 11.A's approved sheet lists them. The icon sheet's
/// snapshot draws it in every look.
pub const SHEET: &[(&str, &[SheetEntry])] = &[
    (
        "Structure",
        &[
            ("Part", "cube", Tint::Accent),
            ("Assembly", "assembly", Tint::Accent),
            ("Solid", "cube-focus", Tint::Plain),
            ("Tool solid, consumed", "cube-focus", Tint::Weak),
            ("Sheet", "sheet", Tint::Plain),
            ("Sheet, consumed (sewn, thickened)", "sheet", Tint::Weak),
            (
                "Flattened file (its bodies under one row)",
                "files",
                Tint::Plain,
            ),
            ("A flattened file\u{2019}s product", "package", Tint::Plain),
            ("Sketch", "pencil-simple-line", Tint::Plain),
            ("Drawing", "drawing", Tint::Plain),
            (
                "A feature\u{2019}s \u{201c}uses\u{201d} line",
                "link-simple",
                Tint::Weak,
            ),
        ],
    ),
    (
        "Primitives",
        &[
            ("Box", "cube-transparent", Tint::Plain),
            ("Cylinder", "cylinder", Tint::Plain),
            ("Cone", "cone", Tint::Plain),
            ("Sphere", "sphere", Tint::Plain),
            ("Torus", "torus", Tint::Plain),
        ],
    ),
    (
        "Features",
        &[
            ("Extrude (new, join)", "extrude", Tint::Plain),
            ("Extrude (cut)", "extrude-cut", Tint::Plain),
            ("Revolve (new, join)", "revolve", Tint::Plain),
            ("Revolve (cut)", "revolve-cut", Tint::Plain),
            ("Boolean unite", "unite", Tint::Plain),
            ("Boolean subtract", "subtract", Tint::Plain),
            ("Boolean intersect", "intersect", Tint::Plain),
            ("Fillet", "fillet", Tint::Plain),
            ("Chamfer", "chamfer", Tint::Plain),
            ("Hole (every form)", "hole", Tint::Plain),
            ("Sweep", "sweep", Tint::Plain),
            ("Loft", "loft", Tint::Plain),
            ("Shell", "shell", Tint::Plain),
            ("Offset", "offset", Tint::Plain),
            ("Mirror", "mirror-body", Tint::Plain),
            ("Split", "split", Tint::Plain),
            ("Base flange", "base-flange", Tint::Plain),
            ("Edge flange", "edge-flange", Tint::Plain),
            ("Unfold", "unfold", Tint::Plain),
            ("Refold", "refold", Tint::Plain),
            ("Planar face", "planar-face", Tint::Plain),
            ("Fill", "fill", Tint::Plain),
            ("Extract", "extract", Tint::Plain),
            ("Sew", "sew", Tint::Plain),
            ("Thicken", "thicken", Tint::Plain),
            ("Draft", "draft", Tint::Plain),
            ("Move face", "move-face", Tint::Plain),
            ("Replace face", "replace-face", Tint::Plain),
            ("Delete face", "delete-face", Tint::Plain),
            ("Delete blend", "delete-blend", Tint::Plain),
            ("Resize blend", "resize-blend", Tint::Plain),
            ("Linear pattern", "pattern-linear", Tint::Plain),
            ("Circular pattern", "pattern-circular", Tint::Plain),
            ("Mirror pattern", "flip-horizontal", Tint::Plain),
            ("Import", "file-arrow-down", Tint::Plain),
            (
                "Insert (another part\u{2019}s solids)",
                "arrow-square-in",
                Tint::Plain,
            ),
            ("Delete body", "trash", Tint::Plain),
            ("Unknown feature", "question", Tint::Plain),
        ],
    ),
    (
        "Datums",
        &[
            ("Datum plane", "datum-plane", Tint::Plain),
            ("Datum axis", "datum-axis", Tint::Plain),
            ("Datum point", "datum-point", Tint::Plain),
            ("Path or helix", "path", Tint::Plain),
        ],
    ),
    (
        "Assembly",
        &[
            ("Instance of a part", "cube", Tint::Plain),
            ("Instance of a subassembly", "assembly", Tint::Plain),
            ("Pattern copy", "cube", Tint::Weak),
            ("Mates (the group row)", "paperclip", Tint::Plain),
            ("Mate: fastened", "lock-simple", Tint::Plain),
            ("Mate: revolute", "arrow-counter-clockwise", Tint::Plain),
            ("Mate: slider", "arrows-left-right", Tint::Plain),
            ("Mate: cylindrical", "mate-cylindrical", Tint::Plain),
            ("Mate: planar", "mate-planar", Tint::Plain),
            ("Mate: ball", "mate-ball", Tint::Plain),
        ],
    ),
    (
        "States",
        &[
            ("Failed", "x-circle", Tint::Danger),
            ("Unmeshed faces", "warning", Tint::Warn),
            ("Stale (sketch, assembly)", "arrows-clockwise", Tint::Warn),
            ("Sketch under-defined", "pencil-simple-line", Tint::Info),
            (
                "Sketch over-defined or failing",
                "pencil-simple-line",
                Tint::Danger,
            ),
            ("Hidden", "eye-slash", Tint::Weak),
            (
                "Consumed by a boolean",
                "arrow-elbow-down-right",
                Tint::Weak,
            ),
            ("Reference import", "file-lock", Tint::Info),
            (
                "Insert behind its source\u{2019}s newest release",
                "warning-circle",
                Tint::Warn,
            ),
            ("Surface: a sweep that makes a sheet", "sheet", Tint::Info),
            ("Instance fixed", "push-pin", Tint::Plain),
            ("Mate redundant", "warning-circle", Tint::Warn),
            ("Mate conflicting", "x-circle", Tint::Danger),
            ("Merge conflict", "git-merge", Tint::Danger),
            ("Released", "seal-check", Tint::Ok),
            ("Modified since its release", "seal-warning", Tint::Warn),
            ("Proposed", "seal-question", Tint::Info),
            ("Obsolete", "prohibit", Tint::Weak),
            ("Compare: added", "plus-circle", Tint::Ok),
            ("Compare: changed", "note-pencil", Tint::Warn),
            ("Compare: moved", "arrows-down-up", Tint::Warn),
            ("Compare: removed", "minus-circle", Tint::Danger),
            ("Doctor\u{2019}s issue", "stethoscope", Tint::Warn),
            ("Loading", "spinner-gap", Tint::Weak),
        ],
    ),
];

/// What the rows are built from: the window's model and what it knows
/// about it.
pub struct Source<'a> {
    pub model: &'a Model,
    pub compare: Option<&'a Compare>,
    pub states: &'a States,
    /// Doctor's answer: its issues, by the files they name.
    pub doctor: Option<&'a Value>,
    /// `merge.status`'s answer, while a merge is under way.
    pub merge: Option<&'a Value>,
    /// Each part's PDM number and name, for search.
    pub pdm: &'a BTreeMap<String, String>,
    /// The project is loaded whole: parts start open.
    pub whole: bool,
    /// Each part's drawings (`drawing.list`): ID, name, and whether its
    /// file doesn't read.
    pub drawings: &'a BTreeMap<String, Vec<(String, String, bool)>>,
}

/// The parts and features a merge's conflicts concern: `(part, None)`
/// for a part's own file, `(part, Some(feature))` for a feature's.
fn conflicted(merge: Option<&Value>) -> BTreeSet<(String, Option<String>)> {
    merge
        .and_then(|m| m["conflicts"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let part = c["concerns"]["part"].as_str()?.to_owned();
            Some((part, c["concerns"]["feature"].as_str().map(str::to_owned)))
        })
        .collect()
}

/// A part's release state as badges: released, modified, proposed,
/// obsolete.
fn release_badges(state: &Value, text: &str) -> (Vec<Badge>, bool) {
    let mut out = Vec::new();
    let mut weak = false;
    match state["state"].as_str() {
        Some("released") => out.push(Badge::new("seal-check", Tint::Ok, text)),
        Some("modified") => out.push(Badge::new("seal-warning", Tint::Warn, text)),
        Some("obsolete") => {
            weak = true;
            out.push(Badge::new("prohibit", Tint::Weak, text));
        }
        _ => {}
    }
    if state["proposed"]["label"].is_string() {
        out.push(Badge::new("seal-question", Tint::Info, text));
    }
    (out, weak)
}

/// The newest official label of an insert's source when it is newer than
/// the insert's own (design 521's `insert_behind`, as the window knows it
/// from the release states): `None` at working, for another project's
/// part (its Dependencies tab says), or at the newest.
pub fn behind(i: &crate::core::model::InsertInfo, states: &States) -> Option<String> {
    let pinned = i.label.as_deref().filter(|_| i.project.is_none())?;
    let s = states.by_part.get(&i.part)?;
    if !matches!(s["state"].as_str(), Some("released" | "modified")) {
        return None;
    }
    let newest = s["label"].as_str()?;
    (newest != pinned).then(|| newest.to_owned())
}

/// The import features of `part` founding a solid the model shows, by
/// the file they read, for each file two or more read: a flattened
/// file's bodies (question 627), or several bodies imported one by one.
fn import_files(part: &Part) -> BTreeMap<&str, Vec<&Feature>> {
    let mut by: BTreeMap<&str, Vec<&Feature>> = BTreeMap::new();
    for f in &part.features {
        if let Some(i) = &f.import
            && f.founds()
            && part.solid(&f.id).is_some()
        {
            by.entry(i.file.as_str()).or_default().push(f);
        }
    }
    by.retain(|_, v| v.len() > 1);
    by
}

struct Builder<'a> {
    src: &'a Source<'a>,
    rows: Vec<Row>,
    conflicts: BTreeSet<(String, Option<String>)>,
}

impl Builder<'_> {
    /// Adds `row` under `parent`, marking the parent as having rows.
    fn push(&mut self, mut row: Row, parent: Option<usize>) -> usize {
        row.parent = parent;
        if let Some(p) = parent {
            self.rows[p].children = true;
        }
        self.rows.push(row);
        self.rows.len() - 1
    }

    fn is_assembly(&self, id: &str) -> bool {
        let m = self.src.model;
        m.parts.get(id).map_or_else(
            || m.listed.get(id).is_some_and(|l| l.assembly),
            |p| p.assembly,
        )
    }

    /// A feature's row, and under it what it reads, a subassembly's
    /// instances and a pattern's copies.
    fn feature(&mut self, part: &Part, f: &Feature, depth: usize, parent: Option<usize>) {
        // An insert behind its source is read with that said.
        let marked;
        let f = match f.insert.as_ref().and_then(|i| behind(i, self.src.states)) {
            Some(newest) => {
                let mut g = f.clone();
                g.insert.as_mut().expect("an insert").behind = Some(newest);
                marked = g;
                &marked
            }
            None => f,
        };
        let key = (part.id.clone(), f.id.clone());
        let mark = self.src.compare.and_then(|c| {
            let m = c.features.get(&key)?;
            Some((
                mark_text(m, &c.rev),
                mark_colour(m),
                mark_badges(m, &mark_text(m, &c.rev)),
            ))
        });
        let holes = self
            .src
            .model
            .holes
            .get(&part.id)
            .map_or(0, |t| t.count(&f.id));
        let mut text = row_text(f, 0, holes);
        let (icon, intersects) = feature_icon(f, |p| self.is_assembly(p));
        let reference = format!("{}/{}", part.id, f.id);
        let mut row = Row::new(format!("feature:{reference}"), depth, icon, String::new());
        if intersects {
            row.badges
                .push(Badge::new("intersect", Tint::Plain, "intersect"));
        }
        if surface(part, f) {
            row.badges
                .push(Badge::new("sheet", Tint::Info, SURFACE_TIP));
        }
        if let Some((t, colour, badges)) = mark {
            text.push_str(&t);
            row.colour = Some(colour);
            row.changed = true;
            row.badges.extend(badges);
        }
        row.text = text;
        row.choose = Some(key.clone());
        row.reference = Some(reference.clone());
        row.menu = Some(row_of(f));
        // A sketch is tinted by how defined it is.
        if let Some(s) = &f.sketch {
            row.tint = match s.state.as_str() {
                "under" => Tint::Info,
                "over" | "failing" => Tint::Danger,
                _ => Tint::Plain,
            };
            if s.stale {
                row.badges
                    .push(Badge::new("arrows-clockwise", Tint::Warn, "stale"));
            }
        }
        if let Some(i) = &f.import {
            row.import = Some(i.reference);
            if i.reference {
                row.badges.push(reference_badge());
            }
        }
        if let Some(i) = &f.insert {
            if let Some(newest) = &i.behind {
                row.badges
                    .push(Badge::new("warning-circle", Tint::Warn, "behind"));
                row.tip = Some(format!(
                    "{} is released at {newest}: Update in the insert\u{2019}s properties moves its pin",
                    i.of()
                ));
            }
            if i.warned("insert_source_failed") {
                let tip = format!(
                    "{}\u{2019}s own rebuild failed: the insert shows its model as it stands",
                    i.of()
                );
                row.badges
                    .push(Badge::new("warning", Tint::Warn, tip.clone()));
                row.tip = Some(tip);
            }
            if i.warned("insert_obsolete") {
                row.badges
                    .push(Badge::new("prohibit", Tint::Weak, "obsolete label"));
            }
        }
        if let Some(i) = &f.instance {
            if i.fixed {
                row.badges
                    .push(Badge::new("push-pin", Tint::Plain, "fixed"));
            }
            if i.solve.as_ref().is_some_and(|(s, _)| s == "failing") {
                row.fail("failing");
            }
            // An older label than its part's latest says what's newer.
            if let Some(l) = &i.outdated {
                row.badges
                    .push(Badge::new("warning-circle", Tint::Warn, "outdated"));
                row.tip = Some(format!(
                    "{l} is released: Project > Dependencies > Update moves it"
                ));
            }
            if part.assembly {
                row.hideable = true;
                row.hidden = self.src.model.all_hidden(&reference);
                if row.hidden {
                    row.weak = true;
                    row.badges
                        .push(Badge::new("eye-slash", Tint::Weak, "hidden"));
                }
            }
        }
        // A datum, path or helix that built shows and hides as a solid
        // does: it is a leaf of the hidden set.
        if f.error.is_none()
            && f.ty
                .as_deref()
                .and_then(crate::core::datums::kind_of)
                .is_some()
        {
            row.hideable = true;
            row.hidden = self.src.model.all_hidden(&reference);
            if row.hidden {
                row.weak = true;
                row.badges
                    .push(Badge::new("eye-slash", Tint::Weak, "hidden"));
            }
        }
        if let Some(m) = &f.mate {
            match m.state.as_deref() {
                Some("redundant") => {
                    row.badges
                        .push(Badge::new("warning-circle", Tint::Warn, "redundant"));
                }
                Some("conflicting") => {
                    row.failed = true;
                    row.badges
                        .push(Badge::new("x-circle", Tint::Danger, "conflicting"));
                }
                _ => {}
            }
            row.tip = Some(format!("a = {}\nb = {}", m.a, m.b));
        }
        // What a direct edit did, or a tweak may do (ryucad 22): its tip.
        if let Some(lines) = f
            .tweak
            .as_ref()
            .map(crate::core::tweaks::TweakInfo::summary)
            .filter(|l| !l.is_empty())
        {
            row.tip = Some(lines.join("\n"));
        }
        // A draft whose angle varies (a parting line climbing along the
        // pull): the rebuild's `draft_angle_varies` as a badge.
        if f.tweak
            .as_ref()
            .is_some_and(crate::core::tweaks::TweakInfo::varies)
        {
            let tip = "a drafted face's angle varies from the angle asked: \
                       see its angles in the form"
                .to_owned();
            row.badges
                .push(Badge::new("warning", Tint::Warn, tip.clone()));
            row.tip = Some(tip);
        }
        if let Some(e) = &f.error {
            row.fail(&message(e));
            row.tip = Some(message(e));
        }
        if self
            .conflicts
            .contains(&(part.id.clone(), Some(f.id.clone())))
        {
            row.failed = true;
            row.badges
                .push(Badge::new("git-merge", Tint::Danger, "merge conflict"));
        }
        let me = self.push(row, parent);
        if !f.uses.is_empty() {
            let mut u = Row::new(
                format!("uses:{reference}"),
                depth + 1,
                "link-simple",
                format!("uses {}", f.uses.join(", ")),
            );
            u.tint = Tint::Weak;
            u.weak = true;
            self.push(u, Some(me));
        }
        // A subassembly's own instances, as the window knows that assembly.
        if let Some(i) = &f.instance {
            self.placed(&i.part, &reference, depth + 1, me, 1);
        }
        // A pattern's copies, under it, each to hide or isolate.
        for c in &f.copies {
            let copy_ref = format!("{}/{c}", part.id);
            let seed = c.split('/').nth(1).unwrap_or_default();
            let seed_part = part
                .feature(seed)
                .and_then(|s| s.instance.as_ref())
                .map(|i| i.part.clone());
            let icon = match seed_part {
                Some(p) if self.is_assembly(&p) => "assembly",
                _ => "cube",
            };
            let mut r = Row::new(format!("copy:{copy_ref}"), depth + 1, icon, c.clone());
            r.tint = Tint::Weak;
            r.weak = true;
            r.reference = Some(copy_ref.clone());
            r.menu = Some(command::Row::Copy);
            r.hideable = true;
            r.hidden = self.src.model.all_hidden(&copy_ref);
            if r.hidden {
                r.badges.push(Badge::new("eye-slash", Tint::Weak, "hidden"));
            }
            self.push(r, Some(me));
        }
    }

    /// An insert's row (design 521, The app): one row whatever its source
    /// shows; under it each inserted solid a later feature acted on, by
    /// its key, with its chain as any solid's.
    fn insert(&mut self, part: &Part, f: &Feature, depth: usize, parent: Option<usize>) {
        let me = self.rows.len();
        self.feature(part, f, depth, parent);
        for key in f.solids.iter().filter(|k| **k != f.id) {
            if part.solid(key).is_some() {
                self.solid(part, key, false, depth + 1, Some(me));
            }
        }
    }

    /// A file's row, `nucleo.step (690 bodies, flattened)`, closed; under
    /// it a row per product when its bodies come from more than one
    /// (`U1 (STM32F4, 3 bodies)`), closed; under those each body's solid
    /// row as any (design 521, question 627).
    fn file_group(&mut self, part: &Part, file: &str, imports: &[&Feature], parent: usize) {
        let info = |f: &Feature| f.import.clone().expect("an import");
        let first = info(imports[0]);
        let n = imports.len();
        let mut words = vec![format!("{n} bodies")];
        if imports.iter().any(|f| info(f).flattened) {
            words.push("flattened".into());
        }
        let reference = imports.iter().all(|f| info(f).reference);
        if reference {
            words.push("reference".into());
        }
        let failed = imports
            .iter()
            .filter(|f| part.solid(&f.id).and_then(Solid::failure).is_some())
            .count();
        if failed > 0 {
            words.push(format!("{failed} failed"));
        }
        let text = format!("{} ({})", first.file_name(), words.join(", "));
        let mut row = Row::new(format!("file:{}/{file}", part.id), 1, "files", text);
        row.open = false;
        if reference {
            row.badges.push(reference_badge());
        }
        if failed > 0 {
            row.fail(&format!("{failed} bodies failed"));
        }
        let me = self.push(row, Some(parent));
        // Products in the order their first bodies are listed.
        // A product's id, name and ordinal, and the bodies from it.
        type Product = (String, String, u64);
        let mut products: Vec<(Option<Product>, Vec<&Feature>)> = Vec::new();
        for f in imports {
            let p = info(f).product;
            match products.iter_mut().find(|(q, _)| *q == p) {
                Some((_, v)) => v.push(f),
                None => products.push((p, vec![f])),
            }
        }
        if products.len() < 2 {
            for f in imports {
                self.solid(part, &f.id, false, 2, Some(me));
            }
            return;
        }
        for (product, bodies) in products {
            let (under, depth) = match &product {
                Some((id, name, ordinal)) => {
                    let k = bodies.len();
                    let text =
                        format!("{id} ({name}, {k} bod{})", if k == 1 { "y" } else { "ies" });
                    let mut r = Row::new(
                        format!("product:{}/{file}/{id}#{ordinal}", part.id),
                        2,
                        "package",
                        text,
                    );
                    r.open = false;
                    (self.push(r, Some(me)), 3)
                }
                None => (me, 2),
            };
            for f in bodies {
                self.solid(part, &f.id, false, depth, Some(under));
            }
        }
    }

    /// The instances of assembly `id` (placed at `at`), under the instance
    /// that places it, to any depth: read-only rows. One the window hasn't
    /// loaded is a closed row, loaded when opened.
    fn placed(&mut self, id: &str, at: &str, depth: usize, parent: usize, level: usize) {
        let model = self.src.model;
        if !model.parts.contains_key(id) && model.listed.get(id).is_some_and(|l| l.assembly) {
            let mut r = Row::new(
                format!("placed:{at}"),
                depth,
                "assembly",
                format!("{id} (assembly)"),
            );
            r.weak = true;
            r.tint = Tint::Weak;
            r.open = false;
            r.load = Some(id.to_owned());
            let me = self.push(r, Some(parent));
            let mut l = Row::new(
                format!("loading:{at}"),
                depth + 1,
                "spinner-gap",
                "loading…".into(),
            );
            l.weak = true;
            l.tint = Tint::Weak;
            self.push(l, Some(me));
            return;
        }
        let Some(sub) = model.parts.get(id).filter(|p| p.assembly) else {
            return;
        };
        // A cycle fails its instances; this only guards the drawing.
        if level > 16 {
            return;
        }
        for f in &sub.features {
            let (icon, _) = feature_icon(f, |p| self.is_assembly(p));
            let path = format!("{at}/{}", f.id);
            let holes = model.holes.get(&sub.id).map_or(0, |t| t.count(&f.id));
            let mut r = Row::new(format!("placed:{path}"), depth, icon, row_text(f, 0, holes));
            r.weak = true;
            r.tint = Tint::Weak;
            let me = self.push(r, Some(parent));
            if let Some(i) = &f.instance {
                self.placed(&i.part, &path, depth + 1, me, level + 1);
            }
        }
    }

    /// A solid's row: its founder, its state and mesh faults, and its
    /// chain of features in order; a boolean's consumed tools under it.
    fn solid(&mut self, part: &Part, solid: &str, tool: bool, depth: usize, parent: Option<usize>) {
        let shown = part.solid(solid);
        let key = (part.id.clone(), solid.to_owned());
        let faults = self
            .src
            .model
            .meshes
            .get(&key)
            .and_then(|m| m.mesh.as_ref().ok())
            .map_or(0, |m| m.faults.len());
        // An inserted reference has no import row of its own here.
        let import = part
            .feature(solid)
            .and_then(|f| f.import.as_ref())
            .map(|i| i.reference)
            .or_else(|| shown.filter(|s| s.reference).map(|_| true));
        // A consumed body isn't among the shown ones: a sew's or
        // thicken's is a sheet.
        let sheet = shown.map_or_else(
            || {
                tool && part.features.iter().any(|f| {
                    f.consumes.iter().any(|c| c == solid)
                        && matches!(f.ty.as_deref(), Some("sew" | "thicken"))
                })
            },
            |s| s.sheet,
        );
        let mut title = if tool && sheet {
            format!("{solid} (sheet, consumed)")
        } else if tool {
            format!("{solid} (tool, consumed)")
        } else if sheet {
            format!("{solid} ({})", sheet_text(shown.and_then(|s| s.boundaries)))
        } else if import == Some(true) {
            format!("{solid} (solid, reference)")
        } else {
            format!("{solid} (solid)")
        };
        let failure = shown.and_then(crate::core::model::Solid::failure);
        if let Some(failed) = &failure {
            title.push_str(&format!(": {failed}"));
        } else if faults > 0 {
            title.push_str(&format!(": {faults} unmeshed"));
        }
        let reference = format!("{}/{solid}", part.id);
        let hidden = self.src.model.all_hidden(&reference);
        if hidden {
            title.push_str(" (hidden)");
        }
        let icon = if sheet { "sheet" } else { "cube-focus" };
        let mut row = Row::new(format!("solid:{reference}"), depth, icon, title);
        row.strong = !tool;
        row.open = !tool;
        row.body = Some(key);
        if !tool {
            row.import = import;
            if import == Some(true) {
                row.badges.push(reference_badge());
            }
        }
        if tool {
            row.weak = true;
            row.tint = Tint::Weak;
            row.badges
                .push(Badge::new("arrow-elbow-down-right", Tint::Weak, "consumed"));
        } else {
            row.reference = Some(reference.clone());
            row.menu = Some(command::Row::Solid);
            row.solid = true;
        }
        if let Some(f) = failure {
            row.fail(&f);
        } else if faults > 0 {
            let tip = format!("{faults} face(s) couldn't be meshed: the kernel left them out");
            row.badges
                .push(Badge::new("warning", Tint::Warn, tip.clone()));
            row.tip = Some(tip);
        }
        if hidden {
            row.hidden = true;
            row.weak = true;
            row.badges
                .push(Badge::new("eye-slash", Tint::Weak, "hidden"));
        }
        let me = self.push(row, parent);
        // A split that founded this solid is listed in its target's chain,
        // this solid under it: its chain here starts after it.
        let chain: Vec<&Feature> = part
            .features
            .iter()
            .filter(|f| f.solids.iter().any(|s| s == solid))
            .filter(|f| !(f.id == solid && f.solids.len() > 1))
            // An insert's row is the parent of its acted-on solids.
            .filter(|f| f.insert.is_none())
            .collect();
        for f in chain {
            self.feature(part, f, depth + 1, Some(me));
            for t in &f.consumes {
                self.solid(part, t, true, depth + 2, Some(me));
            }
            if f.id != solid && f.solids.contains(&f.id) && part.solid(&f.id).is_some() {
                self.solid(part, &f.id, false, depth + 2, Some(me));
            }
        }
    }

    fn part(&mut self, part: &Part) {
        let src = self.src;
        let marks = src
            .compare
            .and_then(|c| c.pdm.get(&part.id))
            .map_or(&[][..], |m| m.marks.as_slice());
        let state = src.states.get(&part.id);
        let badge = state.map(crate::core::pdm::badge);
        let mut text = part_text(&part.id, badge.as_deref(), marks);
        if part.assembly {
            text.push_str(&assembly_text(part.solve.as_ref()));
        }
        let icon = if part.assembly { "assembly" } else { "cube" };
        let mut row = Row::new(format!("part:{}", part.id), 0, icon, String::new());
        row.tint = Tint::Accent;
        row.open = src.whole;
        row.reference = Some(part.id.clone());
        row.menu = Some(command::Row::Part);
        if let Some(n) = src.pdm.get(&part.id) {
            row.words.push(n.clone());
        }
        if let Some(s) = state {
            let (b, weak) = release_badges(s, &badge.clone().unwrap_or_default());
            row.badges.extend(b);
            row.weak = weak;
            if let Some(w) = crate::core::pdm::reason(s) {
                row.tip = Some(format!("release state unknown: {w}"));
            }
        }
        if part.solve.as_ref().is_some_and(|s| s.stale) {
            row.badges
                .push(Badge::new("arrows-clockwise", Tint::Warn, "stale"));
        }
        if !marks.is_empty() {
            row.colour = Some(style::NEW);
            row.changed = true;
        }
        if let Some(e) = &part.error {
            text.push_str(" (broken)");
            row.fail(&message(e));
            row.tint = Tint::Danger;
            row.tip = Some(message(e));
        }
        if self.conflicts.contains(&(part.id.clone(), None)) {
            row.failed = true;
            row.badges
                .push(Badge::new("git-merge", Tint::Danger, "merge conflict"));
        }
        row.text = text;
        let me = self.push(row, None);
        let files = import_files(part);
        let mut listed: BTreeSet<&str> = BTreeSet::new();
        // An assembly's mates go under one Mates row, all of them where
        // the first is listed.
        let mut grouped = false;
        for f in &part.features {
            if part.assembly && f.mate.is_some() {
                if !grouped {
                    grouped = true;
                    let mut r = Row::new(
                        format!("mates:{}", part.id),
                        1,
                        "paperclip",
                        "Mates".to_owned(),
                    );
                    r.tint = Tint::Weak;
                    let g = self.push(r, Some(me));
                    for m in part.features.iter().filter(|m| m.mate.is_some()) {
                        self.feature(part, m, 2, Some(g));
                    }
                }
                continue;
            }
            // An insert is one row, whatever its source shows (design 521).
            if f.insert.is_some() {
                self.insert(part, f, 1, Some(me));
                continue;
            }
            // A file several bodies come from is one row (question 627).
            if let Some(i) = &f.import
                && let Some(group) = files.get(i.file.as_str())
                && group.iter().any(|g| g.id == f.id)
            {
                if listed.insert(i.file.as_str()) {
                    self.file_group(part, &i.file, group, me);
                }
                continue;
            }
            match f.solids.as_slice() {
                // A sketch, an instance, or a file that doesn't load.
                [] => self.feature(part, f, 1, Some(me)),
                // A solid shows where its founder is listed; what acts on
                // solids shows in them, a consumed solid under its boolean.
                [s] if f.founds() && part.solid(s).is_some() => {
                    let s = s.clone();
                    self.solid(part, &s, false, 1, Some(me));
                }
                _ => {}
            }
        }
        // Its drawings, after its features: each opens in the Drawings
        // tile.
        for (d, name, failed) in src.drawings.get(&part.id).into_iter().flatten() {
            let text = if *failed {
                format!("{d} (drawing, doesn't read)")
            } else {
                format!("{d} (drawing: {name})")
            };
            let mut r = Row::new(format!("drawing:{}/{d}", part.id), 1, "drawing", text);
            r.drawing = Some((part.id.clone(), d.clone()));
            if *failed {
                r.failed = true;
                r.tint = Tint::Danger;
                r.badges.push(Badge::new(
                    "x-circle",
                    Tint::Danger,
                    "its file doesn't read",
                ));
            }
            self.push(r, Some(me));
        }
        let removed = src.compare.and_then(|c| c.removed.get(&part.id));
        for id in removed.into_iter().flatten() {
            let mut r = Row::new(
                format!("removed:{}/{id}", part.id),
                1,
                "minus-circle",
                format!("{id} [removed]"),
            );
            r.tint = Tint::Danger;
            r.colour = Some(style::REMOVED);
            r.changed = true;
            r.choose = Some((part.id.clone(), id.clone()));
            r.gone = true;
            self.push(r, Some(me));
        }
        // Damage doctor found in the part's files.
        let prefix = format!("parts/{}/", part.id);
        let damage: Vec<String> = src
            .doctor
            .and_then(|d| d["issues"].as_array())
            .into_iter()
            .flatten()
            .filter(|i| {
                i["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|f| f.as_str().is_some_and(|f| f.starts_with(&prefix)))
            })
            .map(|i| i["message"].as_str().unwrap_or_default().to_owned())
            .collect();
        if !damage.is_empty() {
            let mut r = Row::new(
                format!("doctor:{}", part.id),
                0,
                "stethoscope",
                format!("{} doctor issue(s)", damage.len()),
            );
            r.tint = Tint::Warn;
            r.colour = Some(style::CHANGED);
            r.tip = Some(damage.join("\n"));
            self.push(r, None);
        }
    }
}

/// Every row of the tree, in order: parts the window hasn't loaded
/// first (each loaded when its row opens), then each loaded part.
pub fn rows(src: &Source<'_>) -> Vec<Row> {
    let mut b = Builder {
        src,
        rows: Vec::new(),
        conflicts: conflicted(src.merge),
    };
    for (id, l) in &src.model.listed {
        if src.model.parts.contains_key(id) {
            continue;
        }
        let state = src.states.get(id);
        let badge = state.map(crate::core::pdm::badge);
        let mut text = part_text(id, badge.as_deref(), &[]);
        if l.assembly {
            text.push_str(" (assembly)");
        }
        if let Some(n) = l.failed.filter(|n| *n > 0) {
            text.push_str(&format!(" ({n} failed)"));
        }
        let icon = if l.assembly { "assembly" } else { "cube" };
        let mut r = Row::new(format!("part:{id}"), 0, icon, text);
        r.tint = Tint::Accent;
        r.open = false;
        r.load = Some(id.clone());
        r.reference = Some(id.clone());
        r.menu = Some(command::Row::Part);
        if l.failed.is_some_and(|n| n > 0) {
            r.failed = true;
            r.badges
                .push(Badge::new("x-circle", Tint::Danger, "a feature failed"));
        }
        if let Some(n) = src.pdm.get(id) {
            r.words.push(n.clone());
        }
        let me = b.push(r, None);
        let mut l = Row::new(format!("loading:{id}"), 1, "spinner-gap", "loading…".into());
        l.weak = true;
        l.tint = Tint::Weak;
        b.push(l, Some(me));
    }
    for part in src.model.parts.values() {
        b.part(part);
    }
    b.rows
}

/// Whether a query is a stable name's prefix: four hex digits or more.
pub fn hex_query(q: &str) -> bool {
    q.len() >= 4 && q.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Which rows are `of` or under it (every row for `None`), by index:
/// rows come in order, each after its parent.
fn subtree(rows: &[Row], of: Option<&str>) -> Vec<bool> {
    let Some(of) = of else {
        return vec![true; rows.len()];
    };
    let mut inside = vec![false; rows.len()];
    for (i, r) in rows.iter().enumerate() {
        inside[i] = r.id == of || r.parent.is_some_and(|p| inside[p]);
    }
    inside
}

/// A row kept by [`Tree::shown`], with where the query is in its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub index: usize,
    /// The byte range of the query in the row's text, to mark.
    pub mark: Option<(usize, usize)>,
}

/// A key the tree takes while it has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Space,
    /// The Menu key or Shift+F10.
    Menu,
}

/// What a key or a click asks of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Choose the feature (and frame it); `gone` for one only the
    /// compare's other side has.
    Choose {
        key: BodyKey,
        gone: bool,
    },
    /// Hide or show what the reference reaches.
    Hide(String),
    Show(String),
    /// Open the row's context menu.
    Menu(String),
    /// Show a part's drawing in the Drawings tile.
    Drawing(String, String),
}

/// The tile's state: what is searched for, the filters, the rows opened
/// or closed by hand, and the keyboard's row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tree {
    pub query: String,
    /// Keep only failing rows.
    pub failing: bool,
    /// Keep only what a compare marks.
    pub changed: bool,
    /// Keep only hidden rows.
    pub hidden: bool,
    /// Rows opened or closed by hand, by row id.
    open: HashMap<String, bool>,
    /// The keyboard's row, by id.
    pub cursor: Option<String>,
}

impl Tree {
    /// Whether `row` is open: as set by hand, else as it starts.
    pub fn is_open(&self, row: &Row) -> bool {
        self.open.get(&row.id).copied().unwrap_or(row.open)
    }

    /// The part `row` asks the window to load, if any: only one whose row
    /// was opened by hand. A search or filter shows rows open whatever
    /// their state, and loading every part it shows rebuilt a 10,000-part
    /// project one part at a time past the daemon's budget.
    pub fn loads<'a>(&self, row: &'a Row) -> Option<&'a str> {
        row.load.as_deref().filter(|_| self.is_open(row))
    }

    pub fn set_open(&mut self, id: &str, open: bool) {
        self.open.insert(id.to_owned(), open);
    }

    /// Opens every row with rows under it (Expand all, `None`), or `of`
    /// and every row under it (Expand below, by row id). A part or
    /// subassembly the window hasn't loaded opens, and so loads, only
    /// when `load` says the window may load what it opens (a project of
    /// at most `LOAD_WHOLE` parts): at scale it is left closed, so one
    /// command never starts thousands of rebuilds (#446). Returns how
    /// many were left closed. A search or filter shows rows open whatever
    /// this sets: only the hand-set state underneath changes.
    pub fn expand(&mut self, rows: &[Row], of: Option<&str>, load: bool) -> usize {
        let mut left = 0;
        for (i, inside) in subtree(rows, of).into_iter().enumerate() {
            let r = &rows[i];
            if !inside || !r.children {
                continue;
            }
            if r.load.is_some() && !load {
                left += 1;
                self.set_open(&r.id, false);
            } else {
                self.set_open(&r.id, true);
            }
        }
        left
    }

    /// Closes every row with rows under it (Collapse all, `None`), or
    /// `of` and every row under it (Collapse below), so opening one again
    /// shows its own rows closed.
    pub fn collapse(&mut self, rows: &[Row], of: Option<&str>) {
        for (i, inside) in subtree(rows, of).into_iter().enumerate() {
            if inside && rows[i].children {
                self.set_open(&rows[i].id, false);
            }
        }
    }

    /// Whether anything narrows the rows: a query or a filter.
    pub fn narrowing(&self) -> bool {
        !self.query.trim().is_empty() || self.failing || self.changed || self.hidden
    }

    /// Whether `row` passes the query and the filters by itself. `names`
    /// says whether a body has a stable name starting with a hex query.
    fn passes(
        &self,
        row: &Row,
        names: &dyn Fn(&BodyKey, &str) -> bool,
    ) -> (bool, Option<(usize, usize)>) {
        if (self.failing && !row.failed)
            || (self.changed && !row.changed)
            || (self.hidden && !row.hidden)
        {
            return (false, None);
        }
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            return (true, None);
        }
        let text = row.text.to_lowercase();
        if let Some(at) = text.find(&q) {
            return (true, Some((at, at + q.len())));
        }
        let worded = row.words.iter().any(|w| w.to_lowercase().contains(&q));
        let named = hex_query(&q) && row.body.as_ref().is_some_and(|b| names(b, &q));
        (worded || named, None)
    }

    /// The rows to draw, in order. With nothing narrowing them, the open
    /// rows' rows; otherwise each row that passes, with its ancestors
    /// (open whatever their state), the query marked in its text.
    pub fn shown(&self, rows: &[Row], names: &dyn Fn(&BodyKey, &str) -> bool) -> Vec<Shown> {
        if !self.narrowing() {
            let mut out = Vec::new();
            // A row is drawn when every ancestor is open.
            let mut drawn = vec![false; rows.len()];
            for (i, r) in rows.iter().enumerate() {
                let visible = r.parent.is_none_or(|p| drawn[p] && self.is_open(&rows[p]));
                drawn[i] = visible;
                if visible {
                    out.push(Shown {
                        index: i,
                        mark: None,
                    });
                }
            }
            return out;
        }
        let mut keep = vec![false; rows.len()];
        let mut marks = vec![None; rows.len()];
        for (i, r) in rows.iter().enumerate() {
            let (ok, mark) = self.passes(r, names);
            if ok {
                marks[i] = mark;
                let mut at = Some(i);
                while let Some(j) = at {
                    if keep[j] && j != i {
                        break;
                    }
                    keep[j] = true;
                    at = rows[j].parent;
                }
            }
        }
        (0..rows.len())
            .filter(|i| keep[*i])
            .map(|i| Shown {
                index: i,
                mark: marks[i],
            })
            .collect()
    }

    /// The cursor's place among `shown`, if it is there.
    fn place(&self, rows: &[Row], shown: &[Shown]) -> Option<usize> {
        let c = self.cursor.as_ref()?;
        shown.iter().position(|s| rows[s.index].id == *c)
    }

    /// Takes a key: moves the cursor, opens or closes its row, or says
    /// what the window should do.
    pub fn key(&mut self, rows: &[Row], shown: &[Shown], key: Key) -> Option<Act> {
        if shown.is_empty() {
            return None;
        }
        let Some(at) = self.place(rows, shown) else {
            // The first key puts the cursor on the first row.
            self.cursor = Some(rows[shown[0].index].id.clone());
            return None;
        };
        let i = shown[at].index;
        let row = &rows[i];
        let narrowing = self.narrowing();
        let open = narrowing || self.is_open(row);
        let go = |tree: &mut Tree, to: usize| tree.cursor = Some(rows[to].id.clone());
        match key {
            Key::Up => {
                if at > 0 {
                    go(self, shown[at - 1].index);
                }
                None
            }
            Key::Down => {
                if at + 1 < shown.len() {
                    go(self, shown[at + 1].index);
                }
                None
            }
            Key::Left => {
                if row.children && open && !narrowing {
                    self.set_open(&row.id, false);
                } else if let Some(p) = row.parent {
                    go(self, p);
                }
                None
            }
            Key::Right => {
                if row.children && !open {
                    self.set_open(&row.id, true);
                } else if row.children
                    && let Some(next) = shown
                        .get(at + 1)
                        .filter(|s| rows[s.index].parent == Some(i))
                {
                    go(self, next.index);
                }
                None
            }
            Key::Enter => match &row.choose {
                Some(k) => Some(Act::Choose {
                    key: k.clone(),
                    gone: row.gone,
                }),
                None if row.children && !narrowing => {
                    let open = self.is_open(row);
                    self.set_open(&row.id, !open);
                    None
                }
                None => None,
            },
            Key::Space => {
                let r = row.reference.clone()?;
                if !row.hideable && !row.solid {
                    return None;
                }
                Some(if row.hidden {
                    Act::Show(r)
                } else {
                    Act::Hide(r)
                })
            }
            Key::Menu => row.menu.and(row.reference.clone()).map(Act::Menu),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feature(ty: &str) -> Feature {
        Feature {
            id: "f".into(),
            ty: Some(ty.into()),
            hash: None,
            sketch: None,
            uses: Vec::new(),
            operation: None,
            solids: Vec::new(),
            consumes: Vec::new(),
            error: None,
            instance: None,
            mate: None,
            copies: Vec::new(),
            import: None,
            form: None,
            driver: None,
            datum: None,
            body: None,
            sheet: None,
            tweak: None,
            insert: None,
            delete: None,
            spec: None,
        }
    }

    #[test]
    fn a_holes_row_gives_its_spec_and_count() {
        let mut h = feature("hole");
        h.id = "h1".into();
        h.spec = Some("M6x1.0-6H THRU".into());
        assert_eq!(row_text(&h, 0, 100), "h1 (M6x1.0-6H THRU, 100×)");
        assert_eq!(row_text(&h, 0, 1), "h1 (M6x1.0-6H THRU)");
        assert_eq!(row_text(&h, 0, 0), "h1 (M6x1.0-6H THRU)");
        // A hole with no spec, and everything else, read as before.
        h.spec = None;
        assert_eq!(row_text(&h, 0, 3), "h1 (hole)");
        assert_eq!(row_text(&feature("box"), 0, 0), "f (box)");
    }

    #[test]
    fn a_foreign_instance_names_its_dependency_and_a_newer_label() {
        use crate::core::model::Placed;
        let mut b = feature("instance");
        b.id = "b1".into();
        b.instance = Some(Placed {
            part: "m6-bolt".into(),
            rev: "label B".into(),
            fixed: false,
            solve: None,
            pose: None,
            project: Some("aa".into()),
            dependency: Some("fasteners".into()),
            outdated: Some("C".into()),
        });
        assert_eq!(
            row_text(&b, 0, 0),
            "b1 (fasteners:m6-bolt @ label B, outdated)"
        );
        b.instance.as_mut().unwrap().outdated = None;
        assert_eq!(row_text(&b, 0, 0), "b1 (fasteners:m6-bolt @ label B)");
        // The project's own part at an old label says so too.
        let i = b.instance.as_mut().unwrap();
        (i.project, i.dependency, i.outdated) = (None, None, Some("C".into()));
        assert_eq!(row_text(&b, 0, 0), "b1 (m6-bolt @ label B, outdated)");
    }

    /// A part with a solid built by a box, a sketch, an extrude cutting it,
    /// and a fillet that failed; and an assembly placing it with two mates
    /// and a pattern of its pin.
    fn model() -> Model {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "bracket", "revision": "r", "features": [
            {"id": "base", "type": "box", "ok": true, "hash": "1", "solids": ["base"]},
            {"id": "s1", "type": "sketch", "ok": true,
             "sketch": {"state": "under", "dof": 2, "stale": true}},
            {"id": "e1", "type": "extrude", "ok": true, "hash": "2", "solids": ["base"],
             "operation": "cut", "uses": ["s1"]},
            {"id": "f1", "type": "fillet", "ok": false, "solids": ["base"],
             "error": {"code": "blend_failed", "message": "the radius is too large"}}],
            "solids": [{"solid": "base", "ok": false, "features": ["base", "e1", "f1"],
                        "hash": "2", "error": {"code": "blend_failed"}}]}));
        m.apply_part(&json!({"part": "frame", "revision": "r", "kind": "assembly",
            "solve": {"state": "under", "dof": 1, "converged": true, "stale": true},
            "features": [
            {"id": "plate", "type": "instance", "ok": true, "hash": "3",
             "instance": {"part": "bracket", "rev": "working", "fixed": true}},
            {"id": "m1", "type": "mate", "ok": true, "hash": "4",
             "mate": {"state": "well", "form": "fastened", "a": "frame/plate", "b": "frame/pin"}},
            {"id": "pin", "type": "instance", "ok": true, "hash": "5",
             "instance": {"part": "bracket", "rev": "working", "solve": {"state": "under", "dof": 1}}},
            {"id": "m2", "type": "mate", "ok": true, "hash": "6",
             "mate": {"state": "redundant", "form": "cylindrical", "a": "frame/pin", "b": "frame/plate"}},
            {"id": "ring", "type": "instance_pattern", "ok": true, "hash": "7", "form": "circular",
             "copies": [{"copy": "ring:1", "seed": "pin"}]}],
            "occurrences": [{"path": "plate", "part": "bracket", "solids": {"base": "2"}},
                            {"path": "pin", "part": "bracket", "solids": {"base": "2"}},
                            {"path": "ring:1/pin", "part": "bracket", "solids": {"base": "2"}}]}));
        m
    }

    fn source<'a>(
        m: &'a Model,
        states: &'a States,
        pdm: &'a BTreeMap<String, String>,
    ) -> Source<'a> {
        Source {
            model: m,
            compare: None,
            states,
            doctor: None,
            merge: None,
            pdm,
            whole: true,
            drawings: &NO_DRAWINGS,
        }
    }

    static NO_DRAWINGS: BTreeMap<String, Vec<(String, String, bool)>> = BTreeMap::new();

    fn texts(rows: &[Row], tree: &Tree) -> Vec<String> {
        tree.shown(rows, &|_, _| false)
            .iter()
            .map(|s| format!("{}{}", "  ".repeat(rows[s.index].depth), rows[s.index].text))
            .collect()
    }

    #[test]
    fn an_insert_is_one_row_and_a_delete_says_what_it_deletes() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "e", "revision": "r", "features": [
            {"id": "brd", "type": "insert", "ok": true, "hash": "1", "solids": ["brd", "brd/base"],
             "insert": {"of": "pcb", "rev": {"label": "A"}, "count": 690}},
            {"id": "nub", "type": "box", "ok": true, "hash": "2", "solids": ["brd/base"],
             "operation": "join"},
            {"id": "d", "type": "delete_body", "ok": true, "hash": "3",
             "delete_body": {"solids": ["brd/boss"]}}],
          "solids": [
            {"solid": "brd/base", "ok": true, "features": ["brd", "nub"], "hash": "2"},
            {"solid": "brd/u1", "ok": true, "features": ["brd"], "hash": "4"}]}));
        let mut states = States::default();
        states
            .by_part
            .insert("pcb".into(), json!({"state": "released", "label": "B"}));
        let pdm = BTreeMap::new();
        let rows = rows(&source(&m, &states, &pdm));
        assert_eq!(
            texts(&rows, &Tree::default()),
            [
                "e",
                "  brd (insert of pcb @ label A, behind, 690 solids)",
                "    brd/base (solid)",
                "      nub (box, join)",
                "  d (delete brd/boss)",
            ]
        );
        let row = |t: &str| rows.iter().find(|r| r.text.starts_with(t)).unwrap();
        let brd = row("brd (");
        assert_eq!(brd.icon, "arrow-square-in");
        assert_eq!(brd.choose, Some(("e".to_owned(), "brd".to_owned())));
        assert!(brd.badges.iter().any(|b| b.icon == "warning-circle"));
        assert!(brd.tip.as_deref().unwrap().contains("released at B"));
        assert_eq!(row("d (").icon, "trash");
        // An inserted solid nothing acted on is never listed.
        assert!(!rows.iter().any(|r| r.text.starts_with("brd/u1")));
        // At its newest label, it isn't behind.
        states
            .by_part
            .insert("pcb".into(), json!({"state": "released", "label": "A"}));
        let rows = super::rows(&source(&m, &states, &pdm));
        assert!(
            rows.iter()
                .any(|r| r.text == "brd (insert of pcb @ label A, 690 solids)")
        );
    }

    #[test]
    fn a_flattened_files_bodies_are_one_row_then_one_per_product() {
        let imp = |id: &str, product: &str, name: &str| {
            json!({"id": id, "type": "import", "ok": true, "hash": id, "solids": [id],
                   "import": {"file": "imports/n.step", "blake3": "00", "body": 0,
                              "mode": "reference", "flattened": true,
                              "product": {"id": product, "name": name}}})
        };
        let shown = |id: &str| json!({"solid": id, "ok": true, "features": [id], "hash": id, "reference": true});
        let mut m = Model::default();
        m.apply_part(&json!({"part": "f", "revision": "r", "features": [
            imp("a", "U1", "MCU"), imp("b", "U1", "MCU"), imp("c", "J1", "USB"),
            {"id": "x", "type": "box", "ok": true, "hash": "x", "solids": ["x"]}],
          "solids": [shown("a"), shown("b"), shown("c"),
                     {"solid": "x", "ok": true, "features": ["x"], "hash": "x"}]}));
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let all: Vec<String> = rows
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.text))
            .filter(|t| !t.trim_start().starts_with(['a', 'b', 'c']) || t.contains("(solid"))
            .collect();
        assert_eq!(
            all,
            [
                "f",
                "  n.step (3 bodies, flattened, reference)",
                "    U1 (MCU, 2 bodies)",
                "      a (solid, reference)",
                "      b (solid, reference)",
                "    J1 (USB, 1 body)",
                "      c (solid, reference)",
                "  x (solid)",
                "    x (box)",
            ]
        );
        let file = rows.iter().find(|r| r.text.starts_with("n.step")).unwrap();
        assert_eq!((file.icon, file.open), ("files", false));
        let product = rows.iter().find(|r| r.text.starts_with("U1")).unwrap();
        assert_eq!((product.icon, product.open), ("package", false));
        // Closed, the file shows its row alone.
        let shown = texts(&rows, &Tree::default());
        assert!(shown.contains(&"  n.step (3 bodies, flattened, reference)".to_owned()));
        assert!(!shown.iter().any(|t| t.contains("U1 (")), "{shown:?}");
    }

    #[test]
    fn a_following_pattern_names_its_driver() {
        let mut m = Model::default();
        m.apply_part(
            &json!({"part": "pack", "revision": "r", "kind": "assembly", "features": [
            {"id": "s0", "type": "instance", "ok": true, "hash": "1",
             "instance": {"part": "screw", "rev": "working"}},
            {"id": "screws", "type": "instance_pattern", "ok": true, "hash": "2",
             "form": "feature", "driver": "lid/hole-pattern",
             "copies": [{"copy": "screws:p2", "seed": "s0"}, {"copy": "screws:p5", "seed": "s0"}]}],
            "occurrences": [{"path": "s0", "part": "screw", "solids": {"shank": "1"}},
                            {"path": "screws:p2/s0", "part": "screw", "solids": {"shank": "1"}},
                            {"path": "screws:p5/s0", "part": "screw", "solids": {"shank": "1"}}]}),
        );
        let (states, pdm) = (States::default(), BTreeMap::new());
        let made = rows(&source(&m, &states, &pdm));
        let shown = texts(&made, &Tree::default());
        assert!(
            shown.contains(&"  screws (feature: lid/hole-pattern, 2 copies)".to_owned()),
            "{shown:?}"
        );
        assert!(shown.contains(&"    screws:p5/s0".to_owned()), "{shown:?}");
        // A linear or circular pattern reads as it did.
        assert!(
            texts(&rows(&source(&model(), &states, &pdm)), &Tree::default())
                .contains(&"  ring (instance_pattern, 1 copies)".to_owned())
        );
    }

    #[test]
    fn rows_keep_today_s_words_with_icons_and_badges() {
        let m = model();
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        assert_eq!(
            texts(&rows, &Tree::default()),
            [
                "bracket",
                "  base (solid): failed (blend_failed), showing its last good state",
                "    base (box)",
                "    e1 (extrude, cut)",
                "      uses s1",
                "    f1 (fillet): failed (blend_failed)",
                "  s1 (sketch, under, 2 dof, stale)",
                "frame (assembly, under, 1 dof, stale)",
                "  plate (bracket @ working, fixed)",
                "  Mates",
                "    m1 (fastened: plate → pin)",
                "    m2 (cylindrical: pin → plate, redundant)",
                "  pin (bracket @ working, 1 dof)",
                "  ring (instance_pattern, 1 copies)",
                "    ring:1/pin",
            ]
        );
        let row = |id: &str| rows.iter().find(|r| r.id == id).unwrap();
        let icons = |id: &str| {
            let r = row(id);
            (
                r.icon,
                r.tint,
                r.badges
                    .iter()
                    .map(|b| (b.icon, b.tint))
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(icons("part:bracket"), ("cube", Tint::Accent, vec![]));
        assert_eq!(
            icons("part:frame"),
            (
                "assembly",
                Tint::Accent,
                vec![("arrows-clockwise", Tint::Warn)]
            )
        );
        assert_eq!(
            icons("solid:bracket/base"),
            ("cube-focus", Tint::Danger, vec![("x-circle", Tint::Danger)])
        );
        assert_eq!(
            icons("feature:bracket/base"),
            ("cube-transparent", Tint::Plain, vec![])
        );
        assert_eq!(
            icons("feature:bracket/e1"),
            ("extrude-cut", Tint::Plain, vec![])
        );
        assert_eq!(
            icons("uses:bracket/e1"),
            ("link-simple", Tint::Weak, vec![])
        );
        assert_eq!(
            icons("feature:bracket/f1"),
            ("fillet", Tint::Danger, vec![("x-circle", Tint::Danger)])
        );
        assert_eq!(
            icons("feature:bracket/s1"),
            (
                "pencil-simple-line",
                Tint::Info,
                vec![("arrows-clockwise", Tint::Warn)]
            )
        );
        assert_eq!(
            icons("feature:frame/plate"),
            ("cube", Tint::Plain, vec![("push-pin", Tint::Plain)])
        );
        assert_eq!(icons("mates:frame"), ("paperclip", Tint::Weak, vec![]));
        assert_eq!(
            icons("feature:frame/m1"),
            ("lock-simple", Tint::Plain, vec![])
        );
        assert_eq!(
            icons("feature:frame/m2"),
            (
                "mate-cylindrical",
                Tint::Plain,
                vec![("warning-circle", Tint::Warn)]
            )
        );
        assert_eq!(
            icons("feature:frame/ring"),
            ("pattern-circular", Tint::Plain, vec![])
        );
        assert_eq!(icons("copy:frame/ring:1/pin"), ("cube", Tint::Weak, vec![]));
        // What rows choose, hide and offer.
        assert_eq!(
            row("feature:bracket/e1").choose,
            Some(("bracket".into(), "e1".into()))
        );
        assert!(row("feature:frame/pin").hideable && row("copy:frame/ring:1/pin").hideable);
        assert_eq!(row("feature:frame/m1").menu, Some(command::Row::Mate));
        assert_eq!(
            row("feature:frame/m1").tip.as_deref(),
            Some("a = frame/plate\nb = frame/pin")
        );
        assert_eq!(
            row("feature:bracket/f1").tip.as_deref(),
            Some("the radius is too large")
        );
        assert!(row("solid:bracket/base").solid);
    }

    #[test]
    fn a_mate_says_its_value_and_the_stop_it_rests_on() {
        let mut m = Model::default();
        m.apply_part(
            &json!({"part": "frame", "revision": "r", "kind": "assembly",
            "features": [
            {"id": "base", "type": "instance", "ok": true, "hash": "1",
             "instance": {"part": "plate", "rev": "working", "fixed": true}},
            {"id": "pin", "type": "instance", "ok": true, "hash": "2",
             "instance": {"part": "bolt", "rev": "working"}},
            {"id": "hinge", "type": "mate", "ok": true, "hash": "3",
             "mate": {"state": "well", "form": "revolute", "a": "frame/base", "b": "frame/pin",
                      "value": {"angle_deg": 3.9999999999}, "limit": {"angle": "max"},
                      "limits": {"angle_rad": {"min": -0.1, "max": 0.0698131700797732}}}},
            {"id": "tube", "type": "mate", "ok": true, "hash": "4",
             "mate": {"state": "well", "form": "cylindrical", "a": "frame/base", "b": "frame/pin",
                      "value": {"angle_deg": 4.0, "distance": "12.5000000001 mm"},
                      "limit": {"angle": "max", "distance": "min"}}},
            {"id": "glue", "type": "mate", "ok": true, "hash": "5",
             "mate": {"state": "well", "form": "fastened", "a": "frame/base", "b": "frame/pin"}}],
            "occurrences": []}),
        );
        let p = m.parts.get("frame").unwrap();
        let hinge = p.feature("hinge").unwrap().mate.clone().unwrap();
        assert_eq!(hinge.stop.as_deref(), Some("at max"));
        assert_eq!(
            hinge.limits.angle,
            Some((Some(-0.1), Some(0.0698131700797732)))
        );
        assert_eq!(hinge.limits.distance, None);
        assert_eq!(
            row_text(p.feature("hinge").unwrap(), 0, 0),
            "hinge (revolute: base → pin, 4.0°, at max)"
        );
        assert_eq!(
            row_text(p.feature("tube").unwrap(), 0, 0),
            "tube (cylindrical: base → pin, 4.0°, 12.5 mm, angle at max, distance at min)"
        );
        assert_eq!(
            row_text(p.feature("glue").unwrap(), 0, 0),
            "glue (fastened: base → pin)"
        );
    }

    #[test]
    fn datums_paths_and_helices_are_rows_that_show_and_hide() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r1", "features": [
            {"id": "d1", "type": "plane", "ok": true,
             "datum": {"kind": "plane", "origin_m": [0.0, 0.0, 0.0], "normal": [0.0, 0.0, 1.0],
                       "x": [1.0, 0.0, 0.0]}},
            {"id": "a1", "type": "axis", "ok": true,
             "datum": {"kind": "axis", "origin_m": [0.0, 0.0, 0.0], "direction": [0.0, 0.0, 1.0]}},
            {"id": "q1", "type": "point", "ok": true,
             "datum": {"kind": "point", "origin_m": [0.0, 0.0, 0.0]}},
            {"id": "h", "type": "helix", "ok": true},
            {"id": "w", "type": "path", "ok": true},
            {"id": "bad", "type": "plane", "ok": false,
             "error": {"code": "datum_degenerate", "message": "its normal vanishes"}}]}));
        m.hidden.insert("p/a1".to_owned());
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        assert_eq!(
            texts(&rows, &Tree::default()),
            [
                "p",
                "  d1 (plane)",
                "  a1 (axis)",
                "  q1 (point)",
                "  h (helix)",
                "  w (path)",
                "  bad (plane): failed (datum_degenerate)",
            ]
        );
        let row = |id: &str| rows.iter().find(|r| r.id == id).unwrap();
        let icon = |id: &str| row(id).icon;
        assert_eq!(
            ["d1", "a1", "q1", "h", "w"].map(|f| icon(&format!("feature:p/{f}"))),
            ["datum-plane", "datum-axis", "datum-point", "path", "path"]
        );
        // Each that built has a Show box, a hidden one weak with its badge;
        // the failed one is red with its code, and nothing to hide.
        assert!(row("feature:p/d1").hideable && !row("feature:p/d1").hidden);
        let a1 = row("feature:p/a1");
        assert!(a1.hideable && a1.hidden && a1.weak);
        assert!(a1.badges.iter().any(|b| b.icon == "eye-slash"));
        let bad = row("feature:p/bad");
        assert!(bad.failed && bad.tint == Tint::Danger && !bad.hideable);
        assert_eq!(row("feature:p/h").reference.as_deref(), Some("p/h"));
    }

    #[test]
    fn reference_imports_are_badged_and_say_so() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r1", "features": [
            {"id": "motor", "type": "import", "ok": true, "hash": "1", "solids": ["motor"],
             "import": {"file": "imports/m.step", "blake3": "00", "body": 0,
                        "product": {"id": "M-1", "name": "Motor"}, "mode": "reference"}},
            {"id": "bolt", "type": "import", "ok": true, "hash": "2", "solids": ["bolt"],
             "import": {"file": "imports/b.step", "blake3": "00", "body": 0}}],
            "solids": [{"solid": "motor", "ok": true, "features": ["motor"], "hash": "1",
                        "reference": true},
                       {"solid": "bolt", "ok": true, "features": ["bolt"], "hash": "2"}]}));
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let row = |id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        let badged = |id: &str| row(id).badges.iter().any(|b| b.icon == "file-lock");
        assert_eq!(row("solid:p/motor").text, "motor (solid, reference)");
        assert_eq!(
            row("feature:p/motor").text,
            "motor (import: m.step / M-1, reference)"
        );
        assert!(badged("solid:p/motor") && badged("feature:p/motor"));
        assert_eq!(row("solid:p/motor").import, Some(true));
        assert_eq!(row("feature:p/motor").import, Some(true));
        // A healed import is a plain solid: no badge, but it can be
        // made a reference.
        assert_eq!(row("solid:p/bolt").text, "bolt (solid)");
        assert!(!badged("solid:p/bolt") && !badged("feature:p/bolt"));
        assert_eq!(row("solid:p/bolt").import, Some(false));
    }

    #[test]
    fn shells_offsets_mirrors_and_splits_take_their_glyphs_and_a_split_lists_its_solid() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r1", "features": [
            {"id": "b", "type": "box", "ok": true, "hash": "1", "solids": ["b"]},
            {"id": "s", "type": "shell", "ok": true, "hash": "2", "solids": ["b"]},
            {"id": "o", "type": "offset", "ok": true, "hash": "3", "solids": ["b"]},
            {"id": "sp", "type": "split", "ok": true, "hash": "4", "solids": ["b", "sp"],
             "uses": ["d"]},
            {"id": "f", "type": "fillet", "ok": true, "hash": "5", "solids": ["sp"]},
            {"id": "m", "type": "mirror", "ok": true, "hash": "6", "solids": ["b"],
             "operation": "intersect"},
            {"id": "bad", "type": "mirror", "ok": false, "solids": ["b"], "operation": "join",
             "error": {"code": "mirror_failed", "message": "it leaves the size box"}}],
            "solids": [{"solid": "b", "ok": false, "features": ["b", "s", "o", "sp", "m", "bad"],
                        "hash": "6", "error": {"code": "mirror_failed"}},
                       {"solid": "sp", "ok": true, "features": ["sp", "f"], "hash": "5"}]}));
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let row = |id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        assert_eq!(
            ["s", "o", "sp", "m", "bad"].map(|f| row(&format!("feature:p/{f}")).icon),
            ["shell", "offset", "split", "mirror-body", "mirror-body"]
        );
        // An intersecting mirror takes the intersect badge, as an extrude.
        assert!(
            row("feature:p/m")
                .badges
                .iter()
                .any(|b| b.icon == "intersect")
        );
        let bad = row("feature:p/bad");
        assert!(bad.failed && bad.tint == Tint::Danger);
        assert!(bad.text.contains("mirror_failed"), "{}", bad.text);
        // The split's solid is listed once, under the split in its
        // target's chain, its chain after the split; every row once.
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "{ids:?}");
        assert_eq!(
            texts(&rows, &Tree::default()),
            [
                "p",
                "  b (solid): failed (mirror_failed), showing its last good state",
                "    b (box)",
                "    s (shell)",
                "    o (offset)",
                "    sp (split)",
                "      uses d",
                "      sp (solid)",
                "        f (fillet)",
                "    m (mirror, intersect)",
                "    bad (mirror, join): failed (mirror_failed)",
            ]
        );
    }

    #[test]
    fn tweaks_take_their_glyphs_and_a_varying_draft_its_warning() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r1", "features": [
            {"id": "b", "type": "box", "ok": true, "hash": "1", "solids": ["b"]},
            {"id": "m", "type": "move_face", "ok": true, "hash": "2", "solids": ["b"],
             "uses": ["b"], "move_face": {"target": "b", "by": "translate", "direction": "z"}},
            {"id": "d", "type": "draft", "ok": true, "hash": "3", "solids": ["b"],
             "uses": ["b", "m"], "draft": {"target": "b", "parting": ["p/b/k"], "pull": [0, 0, 1],
              "angles": [{"face": "p/b/f", "least": {"value": "2.5 deg"},
                          "greatest": {"value": "2.5 deg"}, "exact": true, "varies": true}]}},
            {"id": "r", "type": "replace_face", "ok": false, "solids": ["b"], "uses": ["b"],
             "replace_face": {"target": "b"},
             "error": {"code": "tweak_failed", "message": "the edge vanishes"}}],
            "solids": [{"solid": "b", "ok": false, "features": ["b", "m", "d", "r"],
                        "hash": "3", "error": {"code": "tweak_failed"}}]}));
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let row = |id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        assert_eq!(
            ["m", "d", "r"].map(|f| row(&format!("feature:p/{f}")).icon),
            ["move-face", "draft", "replace-face"]
        );
        let d = row("feature:p/d");
        assert!(
            d.badges
                .iter()
                .any(|b| b.icon == "warning" && b.tint == Tint::Warn),
            "{:?}",
            d.badges
        );
        assert!(
            !row("feature:p/m")
                .badges
                .iter()
                .any(|b| b.icon == "warning")
        );
        let bad = row("feature:p/r");
        assert!(bad.failed && bad.tint == Tint::Danger);
        assert!(bad.text.contains("tweak_failed"), "{}", bad.text);
        let shown = texts(&rows, &Tree::default());
        for line in ["    d (draft)", "      uses b, m", "    m (move_face)"] {
            assert!(shown.iter().any(|t| t == line), "{line:?} in {shown:#?}");
        }
    }

    #[test]
    fn direct_edits_take_their_glyphs_and_say_what_they_did() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r1", "features": [
            {"id": "b", "type": "box", "ok": true, "hash": "1", "solids": ["b"]},
            {"id": "x", "type": "delete_face", "ok": true, "hash": "2", "solids": ["b"],
             "uses": ["b"], "delete_face": {"target": "b", "heal": "cap",
              "seeds": [{"seed": "p/b/s1", "faces": ["p/b/s1", "p/b/s2"]}],
              "modified": {"faces": 2, "edges": 0, "vertices": 0},
              "created": {"faces": 0, "edges": 0, "vertices": 0},
              "deleted": {"faces": 2, "edges": 4, "vertices": 0}}},
            {"id": "db", "type": "delete_blend", "ok": true, "hash": "3", "solids": ["b"],
             "uses": ["b", "x"], "delete_blend": {"target": "b",
              "below": {"value": "2 mm", "m": 0.002},
              "blends": [{"kind": "fillet", "size": {"value": "1 mm", "m": 0.001},
                          "faces": ["p/b/f1"]}],
              "modified": {"faces": 2, "edges": 0, "vertices": 0},
              "created": {"faces": 0, "edges": 1, "vertices": 0},
              "deleted": {"faces": 1, "edges": 2, "vertices": 0}}},
            {"id": "m", "type": "move_face", "ok": true, "hash": "4", "solids": ["b"],
             "uses": ["b"], "move_face": {"target": "b", "by": "translate", "direction": "x",
              "topology": "change", "blends": "keep",
              "modified": {"faces": 1, "edges": 0, "vertices": 0},
              "created": {"faces": 1, "edges": 0, "vertices": 0},
              "deleted": {"faces": 0, "edges": 0, "vertices": 0}}},
            {"id": "r", "type": "resize_blend", "ok": false, "solids": ["b"], "uses": ["b"],
             "resize_blend": {"target": "b"},
             "error": {"code": "tweak_failed", "message": "not a blend"}}],
            "solids": [{"solid": "b", "ok": false, "features": ["b", "x", "db", "m", "r"],
                        "hash": "4", "error": {"code": "tweak_failed"}}]}));
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let row = |id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        assert_eq!(
            ["x", "db", "r", "m"].map(|f| row(&format!("feature:p/{f}")).icon),
            ["delete-face", "delete-blend", "resize-blend", "move-face"]
        );
        let tip = |id: &str| {
            row(&format!("feature:p/{id}"))
                .tip
                .clone()
                .unwrap_or_default()
        };
        assert!(
            tip("x").contains("faces: 2 modified, 0 made, 2 taken away"),
            "{}",
            tip("x")
        );
        assert!(tip("x").contains("seed p/b/s1: 2 faces"), "{}", tip("x"));
        assert!(
            tip("db").contains("1 blend taken: fillet 1 mm"),
            "{}",
            tip("db")
        );
        assert!(tip("m").contains("topology may change"), "{}", tip("m"));
        // A failed one is red with its code, its tip its error.
        let bad = row("feature:p/r");
        assert!(bad.failed && bad.tint == Tint::Danger);
        assert!(bad.text.contains("tweak_failed"), "{}", bad.text);
        assert!(tip("r").contains("not a blend"), "{}", tip("r"));
        let shown = texts(&rows, &Tree::default());
        for line in [
            "    x (delete_face)",
            "    db (delete_blend)",
            "      uses b, x",
        ] {
            assert!(shown.iter().any(|t| t == line), "{line:?} in {shown:#?}");
        }
    }

    #[test]
    fn sheet_metal_features_take_their_glyphs_and_list_what_they_read() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r1", "features": [
            {"id": "s", "type": "sketch", "ok": true, "sketch": {"state": "well", "dof": 0}},
            {"id": "plate", "type": "base_flange", "ok": true, "hash": "1", "solids": ["plate"],
             "uses": ["s"]},
            {"id": "f1", "type": "flange", "ok": true, "hash": "2", "solids": ["plate"],
             "uses": ["plate"]},
            {"id": "u", "type": "unfold", "ok": true, "hash": "3", "solids": ["plate"],
             "uses": ["f1", "plate"], "flat": {"bends": 1, "partial": false,
                                              "box_m": [[0, 0], [0.1, 0.06]]}},
            {"id": "r", "type": "refold", "ok": false, "solids": ["plate"], "uses": ["u"],
             "error": {"code": "sheet_metal_failed", "message": "the fold collides"}}],
            "solids": [{"solid": "plate", "ok": false, "features": ["plate", "f1", "u", "r"],
                        "hash": "3", "error": {"code": "sheet_metal_failed"}}]}));
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let row = |id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        assert_eq!(
            ["plate", "f1", "u", "r"].map(|f| row(&format!("feature:p/{f}")).icon),
            ["base-flange", "edge-flange", "unfold", "refold"]
        );
        let bad = row("feature:p/r");
        assert!(bad.failed && bad.tint == Tint::Danger);
        assert!(bad.text.contains("sheet_metal_failed"), "{}", bad.text);
        let shown = texts(&rows, &Tree::default());
        for line in [
            "    plate (base_flange)",
            "      uses s",
            "    f1 (flange)",
            "      uses plate",
            "    u (unfold)",
            "      uses f1, plate",
            "    r (refold): failed (sheet_metal_failed)",
            "      uses u",
        ] {
            assert!(shown.iter().any(|t| t == line), "{line:?} in {shown:#?}");
        }
    }

    #[test]
    fn sheets_and_surface_features_take_their_glyphs() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r1", "features": [
            {"id": "s", "type": "sketch", "ok": true, "sketch": {"state": "well", "dof": 0}},
            {"id": "w", "type": "extrude", "ok": true, "hash": "1", "solids": ["w"],
             "uses": ["s"]},
            {"id": "pl", "type": "planar", "ok": true, "hash": "2", "solids": ["pl"],
             "uses": ["s"]},
            {"id": "f", "type": "fill", "ok": true, "hash": "3", "solids": ["f"]},
            {"id": "x", "type": "extract", "ok": true, "hash": "4", "solids": ["x"]},
            {"id": "sw", "type": "sew", "ok": true, "hash": "5", "solids": ["sw"],
             "operation": "new", "consumes": ["w", "pl"], "uses": ["w", "pl"]},
            {"id": "t", "type": "thicken", "ok": true, "hash": "6", "solids": ["t"],
             "operation": "new", "consumes": ["f"], "uses": ["f"]},
            {"id": "t2", "type": "thicken", "ok": true, "hash": "7", "solids": ["t"],
             "operation": "intersect", "uses": ["x"]}],
            "solids": [
                {"solid": "x", "ok": true, "features": ["x"], "hash": "4",
                 "kind": "sheet", "boundaries": 2, "closed": false},
                {"solid": "sw", "ok": true, "features": ["sw"], "hash": "5",
                 "kind": "sheet", "boundaries": 0, "closed": true},
                {"solid": "t", "ok": true, "features": ["t", "t2"], "hash": "7",
                 "kind": "solid"}]}));
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let row = |id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        assert_eq!(
            ["x", "sw", "t"].map(|f| row(&format!("feature:p/{f}")).icon),
            ["extract", "sew", "thicken"]
        );
        // Those under consumed sheets' closed rows, by their types.
        assert_eq!(
            ["planar", "fill"].map(|t| type_icon(t, None, None)),
            ["planar-face", "fill"]
        );
        // A sheet's row: the sheet glyph and its boundaries; a solid's as
        // ever.
        assert_eq!(row("solid:p/x").icon, "sheet");
        assert_eq!(row("solid:p/x").text, "x (sheet, 2 boundaries)");
        assert_eq!(row("solid:p/sw").text, "sw (sheet, closed)");
        // A new sew founds its body: listed where it is, as `sew` alone.
        assert_eq!(row("feature:p/sw").text, "sw (sew)");
        assert_eq!(row("solid:p/t").icon, "cube-focus");
        // An intersecting thicken takes the intersect badge, as an extrude.
        assert!(
            row("feature:p/t2")
                .badges
                .iter()
                .any(|b| b.icon == "intersect")
        );
        // The sheets a sew and a thicken consumed, under them, weak.
        let shown = texts(&rows, &Tree::default());
        for line in [
            "w (sheet, consumed)",
            "pl (sheet, consumed)",
            "f (sheet, consumed)",
        ] {
            assert!(
                shown.iter().any(|t| t.trim() == line),
                "{line:?} in {shown:#?}"
            );
        }
        // A surface extrude: a badge says it makes a sheet.
        let mut m2 = Model::default();
        m2.apply_part(&json!({"part": "q", "revision": "r", "features": [
            {"id": "e", "type": "extrude", "ok": true, "hash": "1", "solids": ["e"]},
            {"id": "b", "type": "extrude", "ok": true, "hash": "2", "solids": ["b"]}],
            "solids": [
                {"solid": "e", "ok": true, "features": ["e"], "hash": "1", "kind": "sheet",
                 "boundaries": 1},
                {"solid": "b", "ok": true, "features": ["b"], "hash": "2", "kind": "solid"}]}));
        let rows2 = super::rows(&source(&m2, &states, &pdm));
        let badged = |id: &str| {
            rows2
                .iter()
                .find(|r| r.id == id)
                .unwrap()
                .badges
                .iter()
                .any(|b| b.icon == "sheet" && b.tip == SURFACE_TIP)
        };
        assert!(badged("feature:q/e"));
        assert!(!badged("feature:q/b"));
        assert_eq!(sheet_text(None), "sheet");
    }

    #[test]
    fn sweeps_and_lofts_take_their_glyphs_and_list_what_they_read() {
        let mut m = Model::default();
        m.apply_part(&json!({"part": "p", "revision": "r1", "features": [
            {"id": "s1", "type": "sketch", "ok": true, "sketch": {"state": "well", "dof": 0}},
            {"id": "s2", "type": "sketch", "ok": true, "sketch": {"state": "well", "dof": 0}},
            {"id": "w", "type": "sweep", "ok": true, "hash": "1", "solids": ["w"],
             "uses": ["s1", "s2"]},
            {"id": "l", "type": "loft", "ok": true, "hash": "2", "solids": ["w"],
             "operation": "intersect", "uses": ["s1", "s2"]},
            {"id": "bad", "type": "loft", "ok": false, "solids": ["w"], "operation": "cut",
             "error": {"code": "loft_failed", "message": "section 1 lies edge-on"}}],
            "solids": [{"solid": "w", "ok": false, "features": ["w", "l", "bad"],
                        "hash": "2", "error": {"code": "loft_failed"}}]}));
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let row = |id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        assert_eq!(row("feature:p/w").icon, "sweep");
        assert_eq!(row("feature:p/l").icon, "loft");
        assert_eq!(row("feature:p/bad").icon, "loft");
        // An intersecting loft takes the intersect badge, as an extrude.
        assert!(
            row("feature:p/l")
                .badges
                .iter()
                .any(|b| b.icon == "intersect")
        );
        let bad = row("feature:p/bad");
        assert!(bad.failed && bad.tint == Tint::Danger, "{:?}", bad.text);
        assert!(bad.text.contains("loft_failed"), "{}", bad.text);
        // What each reads is listed under it.
        let shown = texts(&rows, &Tree::default()).join("\n");
        assert!(shown.contains("uses s1, s2"), "{shown}");
    }

    #[test]
    fn the_icon_sheet_holds_every_icon_a_row_or_badge_takes() {
        // Every icon name this module writes is on the sheet, and every
        // one on the sheet is drawn.
        let src = include_str!("tree.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap()];
        let sheet: Vec<&str> = SHEET
            .iter()
            .flat_map(|(_, e)| e.iter().map(|x| x.1))
            .collect();
        let names: Vec<&str> = crate::ui::icon::Icon::all().map(|i| i.name()).collect();
        for quoted in src.split('"').skip(1).step_by(2) {
            if names.contains(&quoted) {
                assert!(sheet.contains(&quoted), "{quoted} isn't on the sheet");
            }
        }
        for name in &sheet {
            assert!(names.contains(name), "{name} isn't an icon");
        }
    }

    #[test]
    fn every_feature_type_has_an_icon_the_window_draws() {
        let names: Vec<&str> = crate::ui::icon::Icon::all().map(|i| i.name()).collect();
        let no = |_: &str| false;
        let mut cases = Vec::new();
        for ty in [
            "box",
            "cylinder",
            "cone",
            "sphere",
            "torus",
            "sketch",
            "extrude",
            "revolve",
            "boolean",
            "fillet",
            "chamfer",
            "hole",
            "pattern",
            "instance_pattern",
            "import",
            "plane",
            "axis",
            "point",
            "path",
            "helix",
            "sweep",
            "loft",
            "shell",
            "offset",
            "mirror",
            "split",
            "draft",
            "move_face",
            "replace_face",
            "delete_face",
            "delete_blend",
            "resize_blend",
            "insert",
            "delete_body",
            "mystery",
        ] {
            for op in [
                None,
                Some("join"),
                Some("cut"),
                Some("intersect"),
                Some("subtract"),
            ] {
                for form in [None, Some("linear"), Some("circular"), Some("mirror")] {
                    let mut f = feature(ty);
                    f.operation = op.map(str::to_owned);
                    f.form = form.map(str::to_owned);
                    cases.push(feature_icon(&f, no).0);
                }
            }
        }
        for form in [
            "fastened",
            "revolute",
            "slider",
            "cylindrical",
            "planar",
            "ball",
        ] {
            cases.push(mate_icon(form));
        }
        cases.extend([
            "cube",
            "assembly",
            "cube-focus",
            "paperclip",
            "minus-circle",
            "stethoscope",
            "spinner-gap",
            "link-simple",
            "files",
            "package",
        ]);
        for c in cases {
            assert!(names.contains(&c), "{c}");
        }
        // Badges.
        for b in [
            "x-circle",
            "warning",
            "arrows-clockwise",
            "eye-slash",
            "arrow-elbow-down-right",
            "push-pin",
            "warning-circle",
            "git-merge",
            "seal-check",
            "seal-warning",
            "seal-question",
            "prohibit",
            "plus-circle",
            "note-pencil",
            "arrows-down-up",
            "intersect",
        ] {
            assert!(names.contains(&b), "{b}");
        }
        let mut cut = feature("revolve");
        cut.operation = Some("cut".into());
        assert_eq!(feature_icon(&cut, no), ("revolve-cut", false));
        cut.operation = Some("intersect".into());
        assert_eq!(feature_icon(&cut, no), ("revolve", true));
        let mut b = feature("boolean");
        b.operation = Some("subtract".into());
        assert_eq!(feature_icon(&b, no).0, "subtract");
        let mut p = feature("pattern");
        p.form = Some("mirror".into());
        assert_eq!(feature_icon(&p, no).0, "flip-horizontal");
        assert_eq!(feature_icon(&feature("insert"), no).0, "arrow-square-in");
        assert_eq!(feature_icon(&feature("delete_body"), no).0, "trash");
    }

    #[test]
    fn search_keeps_matches_and_their_ancestors() {
        let m = model();
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let tree = Tree {
            query: "PIN".into(),
            ..Tree::default()
        };
        assert_eq!(
            texts(&rows, &tree),
            [
                "frame (assembly, under, 1 dof, stale)",
                "  Mates",
                "    m1 (fastened: plate → pin)",
                "    m2 (cylindrical: pin → plate, redundant)",
                "  pin (bracket @ working, 1 dof)",
                "  ring (instance_pattern, 1 copies)",
                "    ring:1/pin",
            ]
        );
        let shown = tree.shown(&rows, &|_, _| false);
        let pin = shown
            .iter()
            .find(|s| rows[s.index].id == "feature:frame/pin")
            .unwrap();
        assert_eq!(pin.mark, Some((0, 3)));
        // Rows closed by hand still show what matches under them.
        let mut tree = tree;
        tree.set_open("part:frame", false);
        assert_eq!(texts(&rows, &tree).len(), 7);
    }

    #[test]
    fn search_finds_pdm_names_and_stable_names() {
        let m = model();
        let states = States::default();
        let pdm = BTreeMap::from([("bracket".to_owned(), "BRK-100 Bracket".to_owned())]);
        let rows = rows(&source(&m, &states, &pdm));
        let tree = Tree {
            query: "brk-1".into(),
            ..Tree::default()
        };
        assert_eq!(texts(&rows, &tree), ["bracket"]);
        let tree = Tree {
            query: "4b1e".into(),
            ..Tree::default()
        };
        let names = |b: &BodyKey, q: &str| b.1 == "base" && "4b1e0c".starts_with(q);
        let shown: Vec<&str> = tree
            .shown(&rows, &names)
            .iter()
            .map(|s| rows[s.index].id.as_str())
            .collect();
        assert_eq!(shown, ["part:bracket", "solid:bracket/base"]);
        assert!(hex_query("4b1e") && !hex_query("4b1") && !hex_query("pin1"));
    }

    #[test]
    fn filters_keep_failing_changed_and_hidden_rows() {
        let mut m = model();
        m.hidden.insert("frame/pin".into());
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let failing = Tree {
            failing: true,
            ..Tree::default()
        };
        assert_eq!(
            texts(&rows, &failing),
            [
                "bracket",
                "  base (solid): failed (blend_failed), showing its last good state",
                "    f1 (fillet): failed (blend_failed)",
            ]
        );
        let hidden = Tree {
            hidden: true,
            ..Tree::default()
        };
        assert_eq!(
            texts(&rows, &hidden),
            [
                "frame (assembly, under, 1 dof, stale)",
                "  pin (bracket @ working, 1 dof)"
            ]
        );
        let mut compare = Compare {
            rev: "HEAD".into(),
            ..Compare::default()
        };
        compare.features.insert(
            ("bracket".into(), "e1".into()),
            Mark {
                change: Change::Changed,
                moved: false,
                failed_there: None,
                lines: Vec::new(),
            },
        );
        compare.removed.insert("bracket".into(), vec!["c1".into()]);
        let mut src = source(&m, &states, &pdm);
        src.compare = Some(&compare);
        let rows = super::rows(&src);
        let changed = Tree {
            changed: true,
            ..Tree::default()
        };
        assert_eq!(
            texts(&rows, &changed),
            [
                "bracket",
                "  base (solid): failed (blend_failed), showing its last good state",
                "    e1 (extrude, cut) [changed]",
                "  c1 [removed]",
            ]
        );
        let e1 = rows.iter().find(|r| r.id == "feature:bracket/e1").unwrap();
        assert_eq!(
            e1.badges,
            [Badge::new("note-pencil", Tint::Warn, "[changed]")]
        );
    }

    #[test]
    fn release_states_and_merge_conflicts_are_badges() {
        let m = model();
        let mut states = States::default();
        states.reset(&json!({"parts": [
            {"part": "bracket", "state": "released", "label": "B",
             "proposed": {"label": "C", "kind": "release"}},
            {"part": "frame", "state": "obsolete", "label": "A"}]}));
        let pdm = BTreeMap::new();
        let merge = json!({"conflicts": [
            {"concerns": {"part": "bracket", "feature": "e1"}},
            {"concerns": {"part": "frame"}}]});
        let mut src = source(&m, &states, &pdm);
        src.merge = Some(&merge);
        let rows = rows(&src);
        let row = |id: &str| rows.iter().find(|r| r.id == id).unwrap();
        let badges = |id: &str| row(id).badges.iter().map(|b| b.icon).collect::<Vec<_>>();
        assert_eq!(row("part:bracket").text, "bracket (B, C proposed)");
        assert_eq!(badges("part:bracket"), ["seal-check", "seal-question"]);
        assert_eq!(
            badges("part:frame"),
            ["prohibit", "arrows-clockwise", "git-merge"]
        );
        assert!(row("part:frame").weak);
        assert_eq!(badges("feature:bracket/e1"), ["git-merge"]);
    }

    #[test]
    fn keys_move_open_close_choose_hide_and_open_menus() {
        let m = model();
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let mut tree = Tree::default();
        let none = |_: &BodyKey, _: &str| false;
        let shown = |t: &Tree| t.shown(&rows, &none);
        let press = |t: &mut Tree, k: Key| {
            let s = shown(t);
            t.key(&rows, &s, k)
        };
        // The first key puts the cursor on the first row.
        assert_eq!(press(&mut tree, Key::Down), None);
        assert_eq!(tree.cursor.as_deref(), Some("part:bracket"));
        // Left closes a part, Right opens it again, then goes into it.
        press(&mut tree, Key::Left);
        assert_eq!(shown(&tree).len(), 9);
        press(&mut tree, Key::Right);
        assert_eq!(shown(&tree).len(), 15);
        press(&mut tree, Key::Right);
        assert_eq!(tree.cursor.as_deref(), Some("solid:bracket/base"));
        press(&mut tree, Key::Down);
        assert_eq!(
            press(&mut tree, Key::Enter),
            Some(Act::Choose {
                key: ("bracket".into(), "base".into()),
                gone: false
            })
        );
        // Left on a row with nothing under it goes to its parent.
        press(&mut tree, Key::Left);
        assert_eq!(tree.cursor.as_deref(), Some("solid:bracket/base"));
        // Space hides a solid; the menu opens on it.
        assert_eq!(
            press(&mut tree, Key::Space),
            Some(Act::Hide("bracket/base".into()))
        );
        assert_eq!(
            press(&mut tree, Key::Menu),
            Some(Act::Menu("bracket/base".into()))
        );
        // Enter on a row that chooses nothing opens or closes it.
        press(&mut tree, Key::Up);
        assert_eq!(press(&mut tree, Key::Enter), None);
        assert_eq!(shown(&tree).len(), 9);
        // Up at the top stays there.
        press(&mut tree, Key::Up);
        assert_eq!(tree.cursor.as_deref(), Some("part:bracket"));
        // A hidden instance shows again with Space.
        let mut m = model();
        m.hidden.insert("frame/pin".into());
        let rows = super::rows(&source(&m, &states, &pdm));
        let mut tree = Tree {
            cursor: Some("feature:frame/pin".into()),
            ..Tree::default()
        };
        let s = tree.shown(&rows, &none);
        assert_eq!(
            tree.key(&rows, &s, Key::Space),
            Some(Act::Show("frame/pin".into()))
        );
    }

    #[test]
    fn unloaded_parts_load_when_opened() {
        let mut m = Model::default();
        m.listed.insert(
            "big".into(),
            crate::core::model::Listed::from_json(
                &json!({"part": "big", "kind": "assembly", "failed": 2}),
            ),
        );
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        assert_eq!(rows[0].text, "big (assembly) (2 failed)");
        assert_eq!(rows[0].load.as_deref(), Some("big"));
        assert!(!rows[0].open && rows[0].failed);
        let mut tree = Tree::default();
        assert_eq!(texts(&rows, &tree), ["big (assembly) (2 failed)"]);
        // A search shows it, but doesn't load it.
        tree.query = "big".into();
        assert!(tree.narrowing());
        assert_eq!(tree.loads(&rows[0]), None);
        tree.query.clear();
        tree.set_open("part:big", true);
        assert_eq!(
            texts(&rows, &tree),
            ["big (assembly) (2 failed)", "  loading…"]
        );
        assert_eq!(tree.loads(&rows[0]), Some("big"));
    }

    #[test]
    fn expand_and_collapse_all_and_below() {
        let m = model();
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let none = |_: &BodyKey, _: &str| false;
        let mut tree = Tree::default();
        let shown = |t: &Tree| t.shown(&rows, &none);
        // Collapse all leaves the top rows alone.
        tree.collapse(&rows, None);
        let tops = rows.iter().filter(|r| r.parent.is_none()).count();
        assert_eq!(shown(&tree).len(), tops);
        assert!(tops < rows.len());
        // Expand all shows every row, whatever started closed.
        assert_eq!(tree.expand(&rows, None, true), 0);
        assert_eq!(shown(&tree).len(), rows.len());
        // Collapse below closes a part's rows and nothing else's.
        // The rows under `of`, not counting itself.
        let under = |of: &str| {
            super::subtree(&rows, Some(of))
                .iter()
                .filter(|x| **x)
                .count()
                - 1
        };
        let below = under("part:bracket");
        assert!(below > 0);
        tree.collapse(&rows, Some("part:bracket"));
        assert_eq!(shown(&tree).len(), rows.len() - below);
        // And its own rows stay closed when it opens again by hand...
        tree.set_open("part:bracket", true);
        assert!(shown(&tree).len() < rows.len());
        // ...until Expand below opens all of them.
        tree.expand(&rows, Some("part:bracket"), true);
        assert_eq!(shown(&tree).len(), rows.len());
        // A search still shows rows open past the hand-set state.
        tree.collapse(&rows, None);
        tree.query = "base".into();
        assert!(shown(&tree).len() > tops);
    }

    #[test]
    fn expand_all_at_scale_leaves_unloaded_parts_closed() {
        let mut m = Model::default();
        m.listed.insert(
            "big".into(),
            crate::core::model::Listed::from_json(&json!({"part": "big", "kind": "assembly"})),
        );
        let (states, pdm) = (States::default(), BTreeMap::new());
        let rows = rows(&source(&m, &states, &pdm));
        let mut tree = Tree::default();
        // At scale it opens nothing it would have to load, and says so.
        assert_eq!(tree.expand(&rows, None, false), 1);
        assert_eq!(tree.loads(&rows[0]), None);
        // In a small project it opens it, which loads it.
        assert_eq!(tree.expand(&rows, None, true), 0);
        assert_eq!(tree.loads(&rows[0]), Some("big"));
    }
}
