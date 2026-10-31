//! The tree's rows (design 199, Tree): every part, its solids and
//! features, an assembly's instances, its mates under one Mates row and
//! its patterns' copies, each row with its icon, its state's badges and
//! today's text (which the window tests read); and the tile's search,
//! filters, open rows and keyboard cursor. It holds no egui: icons and
//! badges are `ui::icon` names, tints are what the look colours them, and
//! `ui::tiles::tree` draws what [`Tree::shown`] keeps.

use crate::core::command;
use crate::core::compare::{Change, Compare, Mark};
use crate::core::model::{BodyKey, Feature, MateInfo, Model, Part};
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
pub fn row_text(f: &Feature, faults: usize) -> String {
    let ty = match (&f.ty, &f.operation, &f.instance) {
        // An instance says what it places, and what its mates leave it.
        (_, _, Some(i)) => {
            let freedom = match &i.solve {
                _ if i.fixed => ", fixed".to_owned(),
                Some((state, _)) if state == "failing" => ", failing".to_owned(),
                Some((_, dof)) => format!(", {dof} dof"),
                None => String::new(),
            };
            format!("{} @ {}{freedom}", i.part, i.rev)
        }
        // A mate its form and what it joins, and a redundant or
        // conflicting one says so.
        _ if f.mate.is_some() => {
            let m = f.mate.as_ref().expect("a mate");
            let state = match m.state.as_deref() {
                Some("redundant" | "conflicting") => {
                    format!(", {}", m.state.as_deref().unwrap_or_default())
                }
                _ => String::new(),
            };
            format!(
                "{}: {} → {}{state}",
                m.form,
                MateInfo::path_of(&m.a),
                MateInfo::path_of(&m.b)
            )
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
            s
        }
        // An instance pattern how many copies it places.
        (Some(t), _, None) if t == "instance_pattern" && f.error.is_none() => {
            format!("{t}, {} copies", f.copies.len())
        }
        (Some(t), Some(op), None) => format!("{t}, {op}"),
        (Some(t), None, None) => t.clone(),
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
        Some("extrude" | "revolve" | "box" | "cylinder" | "cone" | "sphere" | "torus")
    ) && op == Some("intersect");
    (icon, intersects)
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
        let key = (part.id.clone(), f.id.clone());
        let mark = self.src.compare.and_then(|c| {
            let m = c.features.get(&key)?;
            Some((
                mark_text(m, &c.rev),
                mark_colour(m),
                mark_badges(m, &mark_text(m, &c.rev)),
            ))
        });
        let mut text = row_text(f, 0);
        let (icon, intersects) = feature_icon(f, |p| self.is_assembly(p));
        let reference = format!("{}/{}", part.id, f.id);
        let mut row = Row::new(format!("feature:{reference}"), depth, icon, String::new());
        if intersects {
            row.badges
                .push(Badge::new("intersect", Tint::Plain, "intersect"));
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
        if let Some(i) = &f.instance {
            if i.fixed {
                row.badges
                    .push(Badge::new("push-pin", Tint::Plain, "fixed"));
            }
            if i.solve.as_ref().is_some_and(|(s, _)| s == "failing") {
                row.fail("failing");
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
            let mut r = Row::new(format!("placed:{path}"), depth, icon, row_text(f, 0));
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
        let mut title = if tool {
            format!("{solid} (tool, consumed)")
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
        let mut row = Row::new(format!("solid:{reference}"), depth, "cube-focus", title);
        row.strong = !tool;
        row.open = !tool;
        row.body = Some(key);
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
        let chain: Vec<&Feature> = part
            .features
            .iter()
            .filter(|f| f.solids.iter().any(|s| s == solid))
            .collect();
        for f in chain {
            self.feature(part, f, depth + 1, Some(me));
            for t in &f.consumes {
                self.solid(part, t, true, depth + 2, Some(me));
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
            match f.solids.as_slice() {
                // A sketch, an instance, or a file that doesn't load.
                [] => self.feature(part, f, 1, Some(me)),
                // A solid shows where its founder is listed; what acts on
                // solids shows in them, a consumed solid under its boolean.
                [s] if *s == f.id && f.operation.is_none() && part.solid(s).is_some() => {
                    let s = s.clone();
                    self.solid(part, &s, false, 1, Some(me));
                }
                _ => {}
            }
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

    pub fn set_open(&mut self, id: &str, open: bool) {
        self.open.insert(id.to_owned(), open);
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
        }
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
        }
    }

    fn texts(rows: &[Row], tree: &Tree) -> Vec<String> {
        tree.shown(rows, &|_, _| false)
            .iter()
            .map(|s| format!("{}{}", "  ".repeat(rows[s.index].depth), rows[s.index].text))
            .collect()
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
        tree.set_open("part:big", true);
        assert_eq!(
            texts(&rows, &tree),
            ["big (assembly) (2 failed)", "  loading…"]
        );
    }
}
