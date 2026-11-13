//! The command registry (design 199, Command registry): every action the
//! window offers, once, by a namespaced id that `ui.keys` names and so is
//! never reused. The menus, the toolbar, the context menus, the palette
//! and the keymap are all built from [`COMMANDS`]. It holds no egui: a
//! command's icon is a name `ui::icon` knows, its keys are chord text
//! (`core::keys`), and whether it can run now is read from a [`State`] the
//! window fills in each frame. Running one is the window's
//! (`ui::commands`).

/// The menu a command belongs to, in the menu bar's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    File,
    Edit,
    View,
    Tools,
    Project,
    Help,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::File,
        Category::Edit,
        Category::View,
        Category::Tools,
        Category::Project,
        Category::Help,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Category::File => "File",
            Category::Edit => "Edit",
            Category::View => "View",
            Category::Tools => "Tools",
            Category::Project => "Project",
            Category::Help => "Help",
        }
    }
}

/// Where a command's keys work: anywhere in the window, or only over the
/// viewport. Two commands' keys conflict when their scopes meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Window,
    Viewport,
    /// The Markup bar's keys, only while it is open (design 423): they
    /// meet only each other, so A, F and Mod+Z there never shadow the
    /// window's or the presets' keys elsewhere.
    Markup,
}

impl Scope {
    /// Whether a key could reach both: a window key works over the
    /// viewport too.
    pub fn meets(self, other: Scope) -> bool {
        self == other
            || (self != Scope::Markup
                && other != Scope::Markup
                && (self == Scope::Window || other == Scope::Window))
    }

    pub fn title(self) -> &'static str {
        match self {
            Scope::Window => "Anywhere",
            Scope::Viewport => "Viewport (while the pointer is over it, or it has the keyboard)",
            Scope::Markup => "Markup bar (while it is open)",
        }
    }
}

/// A tree row, by what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Row {
    Part,
    Solid,
    /// A feature that is none of the kinds below.
    Feature,
    Sketch,
    Instance,
    Mate,
    /// A part's or an assembly's pattern.
    Pattern,
    /// A pattern's copy in an assembly.
    Copy,
}

impl Row {
    pub const ALL: [Row; 8] = [
        Row::Part,
        Row::Solid,
        Row::Feature,
        Row::Sketch,
        Row::Instance,
        Row::Mate,
        Row::Pattern,
        Row::Copy,
    ];
}

/// A viewport entity, by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pick {
    Face,
    Edge,
    Vertex,
    /// A sketch's point or curve.
    Sketch,
    /// A datum, path or helix.
    Datum,
}

impl Pick {
    pub const ALL: [Pick; 5] = [
        Pick::Face,
        Pick::Edge,
        Pick::Vertex,
        Pick::Sketch,
        Pick::Datum,
    ];
}

/// What a right-click was on, for context menus. A context command runs
/// with the reference of what was clicked: a row's (`<part>`,
/// `<part>/<feature>`, `<asm>/<path>`), an entity's, or a tile's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// A tree row.
    Row(Row),
    /// An entity under the pointer in the viewport.
    Entity(Pick),
    /// A compare's ghost under the pointer: gone from the working files,
    /// so it can be described and copied, never selected.
    Ghost,
    /// The viewport's background.
    Viewport,
    /// A tile's tab.
    Tab,
    /// The Tree tile's space under its rows.
    Tree,
}

const ROWS: [Context; 8] = [
    Context::Row(Row::Part),
    Context::Row(Row::Solid),
    Context::Row(Row::Feature),
    Context::Row(Row::Sketch),
    Context::Row(Row::Instance),
    Context::Row(Row::Mate),
    Context::Row(Row::Pattern),
    Context::Row(Row::Copy),
];

const ENTITIES: [Context; 5] = [
    Context::Entity(Pick::Face),
    Context::Entity(Pick::Edge),
    Context::Entity(Pick::Vertex),
    Context::Entity(Pick::Sketch),
    Context::Entity(Pick::Datum),
];

/// What can be annotated: every row and entity (a ghost is gone).
const ANNOTATABLE: [Context; 13] = [
    Context::Row(Row::Part),
    Context::Row(Row::Solid),
    Context::Row(Row::Feature),
    Context::Row(Row::Sketch),
    Context::Row(Row::Instance),
    Context::Row(Row::Mate),
    Context::Row(Row::Pattern),
    Context::Row(Row::Copy),
    Context::Entity(Pick::Face),
    Context::Entity(Pick::Edge),
    Context::Entity(Pick::Vertex),
    Context::Entity(Pick::Sketch),
    Context::Entity(Pick::Datum),
];

/// What has an instance to move or mate: an instance's row and an
/// occurrence's entities (offered on a part's entities too, where
/// [`enabled`] says why not).
const MOVABLE: [Context; 4] = [
    Context::Row(Row::Instance),
    Context::Entity(Pick::Face),
    Context::Entity(Pick::Edge),
    Context::Entity(Pick::Vertex),
];

/// Every row, every entity and a ghost: what has a reference to copy.
const REFERENCED: [Context; 14] = [
    Context::Row(Row::Part),
    Context::Row(Row::Solid),
    Context::Row(Row::Feature),
    Context::Row(Row::Sketch),
    Context::Row(Row::Instance),
    Context::Row(Row::Mate),
    Context::Row(Row::Pattern),
    Context::Row(Row::Copy),
    Context::Entity(Pick::Face),
    Context::Entity(Pick::Edge),
    Context::Entity(Pick::Vertex),
    Context::Entity(Pick::Sketch),
    Context::Entity(Pick::Datum),
    Context::Ghost,
];

/// Every row and the tree's space under them: where the whole tree
/// opens or closes.
const TREE: [Context; 9] = [
    Context::Row(Row::Part),
    Context::Row(Row::Solid),
    Context::Row(Row::Feature),
    Context::Row(Row::Sketch),
    Context::Row(Row::Instance),
    Context::Row(Row::Mate),
    Context::Row(Row::Pattern),
    Context::Row(Row::Copy),
    Context::Tree,
];

/// The rows that show a part: a part's, and an instance's for the part
/// it places (#541).
const SHOWABLE: [Context; 2] = [Context::Row(Row::Part), Context::Row(Row::Instance)];

/// The rows that can be hidden: a solid, an instance, a copy.
const HIDEABLE: [Context; 3] = [
    Context::Row(Row::Solid),
    Context::Row(Row::Instance),
    Context::Row(Row::Copy),
];

/// The rows an import's mode changes from: its feature's, and the solid
/// it founds.
const IMPORT_ROWS: [Context; 2] = [Context::Row(Row::Feature), Context::Row(Row::Solid)];

/// How a command shows in a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A button.
    Action,
    /// A check: on or off ([`checked`]).
    Toggle,
    /// A submenu of choices the window lists (recent projects, parts).
    Choices,
}

/// One command.
#[derive(Clone, Copy, Debug)]
pub struct Command {
    /// `category.name`, unique and never reused.
    pub id: &'static str,
    pub title: &'static str,
    pub category: Category,
    /// A `ui::icon` name, for the toolbar, menus and the palette.
    pub icon: Option<&'static str>,
    /// Its default chords, before the navigation preset and `ui.keys`.
    pub keys: &'static [&'static str],
    pub scope: Scope,
    /// A viewport command whose keys work only while the viewport has the
    /// keyboard (F6 or a click), not merely under the pointer: the arrows
    /// and + and -, which other tiles use too.
    pub focus: bool,
    /// Whether its keys fire while a text field has the keyboard: only
    /// chords that type nothing may.
    pub in_text: bool,
    pub toolbar: bool,
    pub context: &'static [Context],
    /// Whether the menu bar lists it (context-only and viewport-only
    /// commands aren't).
    pub menu: bool,
    pub kind: Kind,
}

const fn cmd(id: &'static str, title: &'static str, category: Category) -> Command {
    Command {
        id,
        title,
        category,
        icon: None,
        keys: &[],
        scope: Scope::Window,
        focus: false,
        in_text: false,
        toolbar: false,
        context: &[],
        menu: true,
        kind: Kind::Action,
    }
}

impl Command {
    const fn icon(self, icon: &'static str) -> Command {
        Command {
            icon: Some(icon),
            ..self
        }
    }
    const fn keys(self, keys: &'static [&'static str]) -> Command {
        Command { keys, ..self }
    }
    const fn in_text(self) -> Command {
        Command {
            in_text: true,
            ..self
        }
    }
    const fn toolbar(self) -> Command {
        Command {
            toolbar: true,
            ..self
        }
    }
    const fn context(self, context: &'static [Context]) -> Command {
        Command { context, ..self }
    }
    const fn hidden(self) -> Command {
        Command {
            menu: false,
            ..self
        }
    }
    const fn viewport(self) -> Command {
        Command {
            scope: Scope::Viewport,
            ..self
        }
    }
    const fn markup(self) -> Command {
        Command {
            scope: Scope::Markup,
            menu: false,
            ..self
        }
    }
    const fn focused(self) -> Command {
        Command {
            scope: Scope::Viewport,
            focus: true,
            ..self
        }
    }
    const fn toggle(self) -> Command {
        Command {
            kind: Kind::Toggle,
            ..self
        }
    }
    const fn choices(self) -> Command {
        Command {
            kind: Kind::Choices,
            ..self
        }
    }
}

use Category::{Edit, File, Help, Project, Tools, View};

/// The standard views the View menu and the toolbar's views button list,
/// in their order.
pub const STANDARD_VIEWS: [&str; 9] = [
    "view.front",
    "view.back",
    "view.left",
    "view.right",
    "view.top",
    "view.bottom",
    "view.front_top_right",
    "view.dimetric",
    "view.trimetric",
];

/// Every command, in each menu's order.
pub const COMMANDS: &[Command] = &[
    cmd("file.open", "Open…", File)
        .icon("folder-open")
        .keys(&["Mod+O"])
        .in_text(),
    cmd("file.open_recent", "Open recent", File).choices(),
    // The Branches tab's list in the palette (design 447); a project
    // command, in the File menu as the design has it.
    cmd("project.switch_branch", "Switch branch…", File),
    cmd("file.import_step", "Import STEP…", File).icon("file-arrow-down"),
    cmd("file.update_import", "Update import…", File),
    cmd("file.export", "Export…", File).icon("export"),
    cmd("file.close", "Close", File).keys(&["Mod+W"]).in_text(),
    cmd("file.quit", "Quit", File)
        .icon("sign-out")
        .keys(&["Mod+Q"])
        .in_text(),
    cmd("edit.undo", "Undo", Edit)
        .icon("arrow-u-up-left")
        .keys(&["Mod+Z"])
        .toolbar(),
    cmd("edit.redo", "Redo", Edit)
        .icon("arrow-u-up-right")
        .keys(&["Mod+Shift+Z", "Mod+Y"])
        .toolbar(),
    cmd("edit.copy_reference", "Copy reference", Edit)
        .icon("copy")
        .context(&REFERENCED),
    cmd("edit.select", "Select", Edit)
        .context(&ENTITIES)
        .hidden(),
    cmd("edit.select_other", "Select other…", Edit)
        .icon("selection")
        .context(&ENTITIES)
        .hidden(),
    cmd("edit.clear_selection", "Clear selection", Edit).keys(&["Escape"]),
    cmd("tree.find", "Find in tree", Edit)
        .icon("magnifying-glass")
        .keys(&["Mod+F"])
        .in_text(),
    cmd("edit.preferences", "Preferences…", Edit)
        .icon("gear")
        .keys(&["Mod+Comma"])
        .in_text(),
    cmd("view.front", "Front", View),
    cmd("view.back", "Back", View),
    cmd("view.left", "Left", View),
    cmd("view.right", "Right", View),
    cmd("view.top", "Top", View),
    cmd("view.bottom", "Bottom", View),
    cmd("view.front_top_right", "Isometric", View),
    cmd("view.dimetric", "Dimetric", View),
    cmd("view.trimetric", "Trimetric", View),
    cmd("view.normal_to", "Normal to", View),
    cmd("view.named", "Named views", View).choices(),
    cmd("view.selector", "View selector", View),
    cmd("view.fit", "Zoom to fit", View)
        .icon("arrows-out")
        .toolbar()
        .context(&[Context::Viewport]),
    cmd("view.fit_row", "Zoom to fit", View)
        .icon("arrows-out")
        .context(&ROWS)
        .hidden(),
    // View > Show for the part a row is or places, so "Show" means the
    // window's focus everywhere (#541).
    cmd("view.show_row", "Show", View)
        .icon("eye")
        .context(&SHOWABLE)
        .hidden(),
    // A row and everything under it opened or closed (#540), offered on
    // rows with rows under them.
    cmd("tree.expand_below", "Expand below", View)
        .icon("caret-down")
        .context(&ROWS)
        .hidden(),
    cmd("tree.collapse_below", "Collapse below", View)
        .icon("caret-right")
        .context(&ROWS)
        .hidden(),
    cmd("view.fit_selection", "Zoom to selection", View)
        .icon("selection")
        .toolbar(),
    // The selection filter: what a click in the viewport picks.
    cmd("select.faces", "Select faces", Edit)
        .icon("square")
        .toolbar()
        .toggle(),
    cmd("select.edges", "Select edges", Edit)
        .icon("line-segment")
        .toolbar()
        .toggle(),
    cmd("select.vertices", "Select vertices", Edit)
        .icon("dot-outline")
        .toolbar()
        .toggle(),
    cmd("select.bodies", "Select whole bodies", Edit)
        .icon("cube-focus")
        .toolbar()
        .toggle(),
    cmd("select.sketches", "Select sketch entities", Edit)
        .icon("pencil-simple-line")
        .toolbar()
        .toggle(),
    cmd("select.datums", "Select datums, paths and helices", Edit)
        .icon("datum-plane")
        .toolbar()
        .toggle(),
    cmd("view.quality", "Display quality", View).choices(),
    cmd("view.edges", "Edges", View).toggle(),
    cmd("view.sketches", "Sketches", View).toggle(),
    cmd("view.datums", "Datums, paths and helices", View).toggle(),
    cmd("view.flat", "Flat pattern", View)
        .icon("square-split-horizontal")
        .toggle(),
    cmd("view.nav_cube", "Navigation cube", View).toggle(),
    cmd("view.triad", "Axis triad", View).toggle(),
    cmd("view.origin", "Origin", View).toggle(),
    cmd("view.show", "Show", View).choices(),
    // Every row of the tree opened or closed by hand (#540): in the Edit
    // menu beside Find in tree, listed here for the rows' menus' order.
    cmd("tree.expand_all", "Expand all", Edit)
        .icon("caret-down")
        .context(&TREE),
    cmd("tree.collapse_all", "Collapse all", Edit)
        .icon("caret-right")
        .context(&TREE),
    cmd("view.isolate", "Isolate", View)
        .icon("eye")
        .context(&HIDEABLE)
        .hidden(),
    cmd("view.hide_under", "Hide what is under the pointer", View)
        .icon("eye-slash")
        .keys(&["Tab"])
        .viewport()
        .hidden(),
    cmd(
        "view.show_under",
        "Show what is hidden under the pointer",
        View,
    )
    .keys(&["Shift+Tab"])
    .viewport()
    .hidden(),
    // Ctrl+Shift+Tab everywhere; Cmd+Shift+Tab too, though macOS keeps it
    // for switching apps (#142).
    cmd("view.show_all", "Show everything", View)
        .keys(&["Ctrl+Shift+Tab", "Mod+Shift+Tab"])
        .viewport()
        .context(&[
            Context::Row(Row::Solid),
            Context::Row(Row::Instance),
            Context::Row(Row::Copy),
            Context::Viewport,
        ]),
    cmd("view.compare_head", "Compare with HEAD", View)
        .icon("git-diff")
        .toolbar(),
    cmd("view.compare_with", "Compare with…", View),
    cmd("view.compare_stop", "Stop comparing", View),
    // A compare's blink (design 447): the window's own view. Not in the
    // View menu, which has no room left (its last items would fall off a
    // 1000-point window): `B` over the viewport, the compare bar, a
    // ghost's context menu and the palette.
    cmd("view.compare_swap", "Swap compared sides", View)
        .icon("arrows-left-right")
        .keys(&["B"])
        .viewport()
        .toggle()
        .hidden()
        .context(&[Context::Ghost]),
    cmd(
        "view.compare_only_rev",
        "Show only the compared revision",
        View,
    )
    .toggle()
    .hidden()
    .context(&[Context::Ghost]),
    cmd(
        "view.compare_only_working",
        "Show only the working files",
        View,
    )
    .toggle()
    .hidden()
    .context(&[Context::Ghost]),
    cmd("section.edit", "Section view", View).icon("knife"),
    cmd("section.clear", "Remove section planes", View),
    cmd("explode.edit", "Exploded view", View).icon("arrows-out-cardinal"),
    cmd("explode.collapse", "Collapse exploded view", View),
    cmd("motion.edit", "Motion", View).icon("arrow-counter-clockwise"),
    cmd("motion.stop", "Stop showing the motion", View),
    cmd("drawing.show", "Drawings", View).icon("drawing"),
    // The viewport's keys while it has the keyboard (F6 or a click).
    cmd("view.orbit_left", "Orbit left", View)
        .keys(&["Left"])
        .focused()
        .hidden(),
    cmd("view.orbit_right", "Orbit right", View)
        .keys(&["Right"])
        .focused()
        .hidden(),
    cmd("view.orbit_up", "Orbit up", View)
        .keys(&["Up"])
        .focused()
        .hidden(),
    cmd("view.orbit_down", "Orbit down", View)
        .keys(&["Down"])
        .focused()
        .hidden(),
    cmd("view.pan_left", "Pan left", View)
        .keys(&["Shift+Left"])
        .focused()
        .hidden(),
    cmd("view.pan_right", "Pan right", View)
        .keys(&["Shift+Right"])
        .focused()
        .hidden(),
    cmd("view.pan_up", "Pan up", View)
        .keys(&["Shift+Up"])
        .focused()
        .hidden(),
    cmd("view.pan_down", "Pan down", View)
        .keys(&["Shift+Down"])
        .focused()
        .hidden(),
    cmd("view.closer", "Move closer", View)
        .keys(&["Plus", "Equals"])
        .focused()
        .hidden(),
    cmd("view.farther", "Move away", View)
        .keys(&["Minus"])
        .focused()
        .hidden(),
    // The app's size (`ui.scale`), not the camera's.
    cmd("view.scale_up", "Zoom in", View)
        .icon("magnifying-glass-plus")
        .keys(&["Mod+Equals", "Mod+Plus"])
        .in_text(),
    cmd("view.scale_down", "Zoom out", View)
        .icon("magnifying-glass-minus")
        .keys(&["Mod+Minus"])
        .in_text(),
    cmd("view.scale_reset", "Actual size", View)
        .keys(&["Mod+0"])
        .in_text(),
    cmd("view.next_tile", "Next panel", View)
        .keys(&["F6"])
        .in_text(),
    cmd("view.previous_tile", "Previous panel", View)
        .keys(&["Shift+F6"])
        .in_text(),
    cmd("view.panels", "Panels", View).icon("layout").choices(),
    cmd("view.close_tile", "Close", View)
        .icon("x")
        .context(&[Context::Tab])
        .hidden(),
    cmd("view.reset_layout", "Reset layout", View).context(&[Context::Tab]),
    cmd("measure.tool", "Measure", Tools).icon("ruler").toggle(),
    cmd("measure.mass", "Mass properties", Tools).icon("scales"),
    cmd("appearance.edit", "Appearance", Tools).icon("palette"),
    cmd("params.equations", "Equations", Tools).icon("function"),
    cmd("measure.clearance", "Clearance", Tools).icon("arrows-left-right"),
    cmd("annotations.show", "Annotations", Tools).icon("chat-circle-text"),
    cmd("view.annotations", "Annotations' pins", View).toggle(),
    cmd("annotation.add", "Annotate…", Edit)
        .icon("chat-circle-text")
        .context(&ANNOTATABLE)
        .hidden(),
    cmd("annotation.here", "Annotate here", Edit)
        .icon("chat-circle-text")
        .context(&[Context::Viewport])
        .hidden(),
    // Controlled edits (design 374, phase 5): an instance moved or turned
    // by its handles, and a mate from a dialog that previews its solve.
    cmd("edit.move", "Move", Edit)
        .icon("arrows-out-cardinal")
        .toolbar()
        .toggle()
        .context(&MOVABLE),
    cmd("edit.mate", "Mate…", Edit)
        .icon("link-simple")
        .context(&MOVABLE),
    cmd("tools.interference", "Check interference", Tools)
        .icon("intersect-square")
        .toolbar(),
    cmd("palette.open", "Command palette", Tools)
        .icon("command")
        .keys(&["Mod+K", "Mod+Shift+P"])
        .in_text()
        .toolbar(),
    cmd("project.settings", "Settings…", Project).icon("sliders-horizontal"),
    cmd("project.doctor", "Doctor…", Project).icon("stethoscope"),
    cmd("deps.show", "Dependencies", Project).icon("package"),
    cmd("deps.place", "Place from dependency…", Project).icon("plus-square"),
    // An import's mode (design 364), offered only where it changes it
    // ([`offered`]); after a row's Copy reference and Zoom to fit, which
    // every row's menu starts with.
    cmd("import.to_solid", "Convert to solid", Project)
        .context(&IMPORT_ROWS)
        .hidden(),
    cmd("import.to_reference", "Keep as reference", Project)
        .context(&IMPORT_ROWS)
        .hidden(),
    cmd("help.shortcuts", "Keyboard shortcuts", Help)
        .icon("keyboard")
        .keys(&["Mod+Slash"])
        .in_text(),
    cmd("help.agents", "Driving ryucad from an agent", Help).icon("robot"),
    cmd("help.about", "About ryucad", Help).icon("info"),
    // The window's mode (design 374), last on the toolbar, beside the
    // People strip.
    cmd("mode.view", "View only", Edit)
        .icon("eye")
        .toolbar()
        .toggle(),
    cmd("mode.annotate", "Annotate", Edit)
        .icon("note-pencil")
        .toolbar()
        .toggle(),
    cmd("mode.edit", "Direct edit", Edit)
        .icon("pencil-simple")
        .toolbar()
        .toggle(),
    // Redline marks (design 423): the Markup bar, after the modes it works in, and its
    // tools, whose keys work only while it is open.
    cmd("annotation.markup", "Markup", Tools)
        .icon("pencil-line")
        .toolbar()
        .toggle(),
    cmd("markup.arrow", "Arrow", Tools)
        .icon("arrow-up-right")
        .keys(&["A"])
        .markup()
        .toggle(),
    cmd("markup.line", "Line", Tools)
        .icon("line-segment")
        .keys(&["L"])
        .markup()
        .toggle(),
    cmd("markup.ellipse", "Ellipse", Tools)
        .icon("circle")
        .keys(&["E"])
        .markup()
        .toggle(),
    cmd("markup.rectangle", "Rectangle", Tools)
        .icon("rectangle")
        .keys(&["R"])
        .markup()
        .toggle(),
    cmd("markup.pen", "Pen", Tools)
        .icon("pen-nib")
        .keys(&["P"])
        .markup()
        .toggle(),
    cmd("markup.pen_faces", "Pen on faces", Tools)
        .icon("paint-brush")
        .keys(&["F"])
        .markup()
        .toggle(),
    cmd("markup.text", "Text", Tools)
        .icon("text-t")
        .keys(&["T"])
        .markup()
        .toggle(),
    cmd("markup.undo", "Undo mark", Tools)
        .icon("arrow-counter-clockwise")
        .keys(&["Mod+Z"])
        .markup(),
    cmd("markup.post", "Post", Tools)
        .icon("paper-plane-tilt")
        .keys(&["Mod+Enter"])
        .in_text()
        .markup(),
];

/// The size `ui.scale` takes after Zoom in (`up`) or Zoom out from
/// `now`: an eighth a step, within its bounds.
pub fn scale_step(now: f64, up: bool) -> f64 {
    let eighths = (now * 8.0).round() + if up { 1.0 } else { -1.0 };
    (eighths / 8.0).clamp(ryucad::config::MIN_SCALE, ryucad::config::MAX_SCALE)
}

/// The command with this id.
pub fn get(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// What a command's availability depends on: the window fills it in each
/// frame from its workspace and view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Entities this window has selected.
    pub selected: usize,
    /// A feature chosen in the tree.
    pub chosen: bool,
    /// The chosen feature is an import.
    pub chosen_import: bool,
    /// Exactly one part or assembly is shown.
    pub one_shown: bool,
    /// The shown model is one assembly.
    pub assembly_shown: bool,
    /// A planar face is selected.
    pub planar_face: bool,
    /// Mass properties has something to weigh.
    pub weighable: bool,
    /// The revision being compared with.
    pub comparing: Option<String>,
    /// The compare's blink: which side is drawn, and whether alone.
    pub blink: crate::core::compare::Blink,
    /// Display toggles and the Measure tool, as they stand.
    pub edges: bool,
    pub sketches: bool,
    pub datums: bool,
    pub nav_cube: bool,
    pub triad: bool,
    pub origin: bool,
    pub measuring: bool,
    /// The view selector, or Select other's list, is open.
    pub selector: bool,
    /// The window cuts with section planes.
    pub sectioned: bool,
    /// The window shows an exploded view.
    pub exploded: bool,
    /// The window shows a motion.
    pub motion_shown: bool,
    /// What a click in the viewport picks.
    pub filter: crate::core::pick::Filter,
    /// The app's size (`ui.scale`).
    pub scale: f64,
    /// The window's mode.
    pub mode: crate::core::modes::Mode,
    /// Annotations' pins are drawn (`display.annotations`).
    pub annotations: bool,
    /// The flat view is on, and whether there is a solid to lay flat (the
    /// chosen feature's, or the one part shown).
    pub flat: bool,
    pub flattenable: bool,
    /// The selection's instance and whether it can move: `None` when
    /// nothing selected is an instance's in the shown assembly.
    pub movable: Option<Result<(String, String), String>>,
    /// The Move tool is on.
    pub moving: bool,
    /// The Mate dialog is open.
    pub mating: bool,
    /// The Markup bar is open, and its tool.
    pub marking: Option<crate::core::markup::Tool>,
}

/// What moves the camera, off while the Markup bar locks it (design 423:
/// every view mark lies in its one view's plane), and what shows another
/// model, which the marks aren't drawn on.
const CAMERA: [&str; 27] = [
    "view.front",
    "view.back",
    "view.left",
    "view.right",
    "view.top",
    "view.bottom",
    "view.front_top_right",
    "view.dimetric",
    "view.trimetric",
    "view.normal_to",
    "view.named",
    "view.selector",
    "view.fit",
    "view.fit_row",
    "view.fit_selection",
    "view.show_row",
    "view.show",
    "view.orbit_left",
    "view.orbit_right",
    "view.orbit_up",
    "view.orbit_down",
    "view.pan_left",
    "view.pan_right",
    "view.pan_up",
    "view.pan_down",
    "view.closer",
    "view.farther",
];

/// The commands that write, off while a compare's revision is drawn
/// (design 447): the model on screen isn't the files.
const WRITES: [&str; 12] = [
    "edit.undo",
    "edit.redo",
    "edit.move",
    "edit.mate",
    "file.import_step",
    "file.update_import",
    "import.to_solid",
    "import.to_reference",
    "annotation.add",
    "annotation.here",
    "annotation.markup",
    "deps.place",
];

/// Whether the window's mode allows `id`, or which mode would: the part
/// of [`enabled`] that holds whatever was right-clicked, so a context
/// menu's targeted entries are gated by mode too.
pub fn mode_allows(id: &str, s: &State) -> Result<(), &'static str> {
    if s.blink.draws_rev() && WRITES.contains(&id) {
        return Err(crate::core::compare::SWAPPED);
    }
    let need = |ok: bool, why: &'static str| if ok { Ok(()) } else { Err(why) };
    match id {
        "annotation.add" | "annotation.here" => need(
            s.mode != crate::core::modes::Mode::View,
            "Annotate does this: press Annotate first.",
        ),
        "edit.move" | "edit.mate" | "annotation.markup" => need(
            s.mode != crate::core::modes::Mode::View,
            "Annotate or Direct edit does this: View changes nothing.",
        ),
        "edit.undo"
        | "edit.redo"
        | "file.import_step"
        | "file.update_import"
        | "import.to_solid"
        | "import.to_reference" => need(
            s.mode == crate::core::modes::Mode::Edit,
            "Direct edit does this: it changes the project's files.",
        ),
        _ => Ok(()),
    }
}

/// Whether `id` can run now, or why not: a sentence the menus' hover, the
/// palette and the toolbar's tooltip all show.
pub fn enabled(id: &str, s: &State) -> Result<(), &'static str> {
    mode_allows(id, s)?;
    let need = |ok: bool, why: &'static str| if ok { Ok(()) } else { Err(why) };
    if s.marking.is_some() && CAMERA.contains(&id) {
        return Err("The Markup bar locks the camera: Post or Cancel first.");
    }
    match id {
        "file.update_import" if !s.chosen_import => {
            Err("Choose an imported feature in the tree first.")
        }
        "file.export" => need(
            s.one_shown,
            "Show one part or assembly first (View > Show).",
        ),
        "edit.copy_reference" | "view.fit_selection" => {
            need(s.selected > 0, "Select something first.")
        }
        // Esc also closes the view selector or Select other's list and
        // leaves the Measure tool.
        "edit.clear_selection" => need(
            s.selected > 0
                || s.chosen
                || s.selector
                || s.measuring
                || s.moving
                || s.mating
                || s.marking.is_some(),
            "Nothing is selected or chosen.",
        ),
        "view.normal_to" => need(s.planar_face, "Select a planar face first."),
        "view.flat" => need(
            s.flat || s.flattenable,
            "Show one part, or choose a feature of a sheet metal solid.",
        ),
        "view.compare_head" => need(
            s.comparing.as_deref() != Some("HEAD"),
            "Already comparing with HEAD.",
        ),
        "view.compare_swap" | "view.compare_only_rev" | "view.compare_only_working" => {
            need(s.blink.of.is_some(), "Not comparing with a revision.")
        }
        "view.compare_stop" => need(s.comparing.is_some(), "Not comparing."),
        "section.clear" => need(s.sectioned, "The view isn't cut."),
        "explode.edit" => need(s.assembly_shown, "Show an assembly first (View > Show)."),
        "explode.collapse" => need(s.exploded, "No exploded view is shown."),
        "motion.edit" => need(s.assembly_shown, "Show an assembly first (View > Show)."),
        "motion.stop" => need(s.motion_shown, "No motion is shown."),
        "drawing.show" => need(s.one_shown, "Show one part first (View > Show)."),
        "measure.mass" => need(s.weighable, "Show one part or assembly, or select a solid."),
        "measure.clearance" | "tools.interference" => {
            need(s.assembly_shown, "Show an assembly first (View > Show).")
        }
        "deps.place" => need(s.assembly_shown, "Show an assembly first (View > Show)."),
        "annotation.markup" if s.marking.is_some() => Ok(()),
        "annotation.markup" => need(
            s.one_shown,
            "Show one part or assembly first (View > Show).",
        ),
        id if id.starts_with("markup.") => need(
            s.marking.is_some(),
            "Markup does this: open Tools > Markup first.",
        ),
        "edit.move" if s.moving => Ok(()),
        "edit.move" => match &s.movable {
            Some(Ok(_)) => Ok(()),
            Some(Err(_)) => Err("This instance can't move: the status line says why."),
            None => Err("Select one instance of the shown assembly."),
        },
        "edit.mate" => need(s.assembly_shown, "Show an assembly first (View > Show)."),
        "view.scale_up" => need(
            s.scale < ryucad::config::MAX_SCALE,
            "The window is as large as it goes.",
        ),
        "view.scale_down" => need(
            s.scale > ryucad::config::MIN_SCALE,
            "The window is as small as it goes.",
        ),
        _ => Ok(()),
    }
}

/// Whether `id` can run on what was right-clicked (`target`): the
/// viewport's tab can't be closed.
pub fn enabled_on(id: &str, target: &str) -> Result<(), &'static str> {
    match (id, target) {
        ("view.close_tile", "viewport") => Err("The viewport can't be closed."),
        _ => Ok(()),
    }
}

/// Whether a command opens or closes the rows under the row it runs on:
/// a tree's context menu offers it only on a row with rows under it.
pub fn below(id: &str) -> bool {
    matches!(id, "tree.expand_below" | "tree.collapse_below")
}

/// Whether a context menu lists `id` for what was right-clicked: an
/// import's mode commands only on an import's rows (`import`: whether it
/// is a reference), each where it changes the mode; every other command
/// always.
pub fn offered(id: &str, import: Option<bool>) -> bool {
    match id {
        "import.to_solid" => import == Some(true),
        "import.to_reference" => import == Some(false),
        _ => true,
    }
}

/// A toggle's state now; `None` for commands that aren't toggles.
pub fn checked(id: &str, s: &State) -> Option<bool> {
    Some(match id {
        "view.edges" => s.edges,
        "view.sketches" => s.sketches,
        "view.datums" => s.datums,
        "view.nav_cube" => s.nav_cube,
        "view.triad" => s.triad,
        "view.origin" => s.origin,
        "measure.tool" => s.measuring,
        "select.faces" => s.filter.faces,
        "select.edges" => s.filter.edges,
        "select.vertices" => s.filter.vertices,
        "select.bodies" => s.filter.bodies,
        "select.sketches" => s.filter.sketches,
        "select.datums" => s.filter.datums,
        "mode.view" => s.mode == crate::core::modes::Mode::View,
        "mode.annotate" => s.mode == crate::core::modes::Mode::Annotate,
        "mode.edit" => s.mode == crate::core::modes::Mode::Edit,
        "view.annotations" => s.annotations,
        "view.flat" => s.flat,
        "edit.move" => s.moving,
        "annotation.markup" => s.marking.is_some(),
        id if id.starts_with("markup.") => {
            let t = crate::core::markup::Tool::of_command(id)?;
            s.marking == Some(t)
        }
        "view.compare_swap" => s.blink.front == crate::core::compare::Front::Rev,
        "view.compare_only_rev" => {
            s.blink.alone && s.blink.front == crate::core::compare::Front::Rev
        }
        "view.compare_only_working" => {
            s.blink.alone && s.blink.front == crate::core::compare::Front::Working
        }
        _ => return None,
    })
}

/// The commands a category's menu lists, in order.
pub fn menu(c: Category) -> impl Iterator<Item = &'static Command> {
    COMMANDS.iter().filter(move |x| x.category == c && x.menu)
}

/// The commands a right-click on `what` offers, in order.
pub fn context(what: Context) -> impl Iterator<Item = &'static Command> {
    COMMANDS.iter().filter(move |c| c.context.contains(&what))
}

/// The toolbar's buttons, in order.
pub fn toolbar() -> impl Iterator<Item = &'static Command> {
    COMMANDS.iter().filter(|c| c.toolbar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keys::Chord;

    #[test]
    fn switch_branch_is_in_the_file_menu() {
        let ids: Vec<&str> = menu(Category::File).map(|c| c.id).collect();
        assert_eq!(
            ids[..3],
            ["file.open", "file.open_recent", "project.switch_branch"]
        );
        assert_eq!(
            get("project.switch_branch").unwrap().title,
            "Switch branch…"
        );
        assert!(enabled("project.switch_branch", &State::default()).is_ok());
    }

    #[test]
    fn swap_and_one_side_need_a_revision_and_say_whether_they_are_on() {
        use crate::core::compare::{Blink, Front};
        let mut s = State::default();
        for id in [
            "view.compare_swap",
            "view.compare_only_rev",
            "view.compare_only_working",
        ] {
            assert_eq!(
                enabled(id, &s),
                Err("Not comparing with a revision."),
                "{id}"
            );
            assert_eq!(get(id).unwrap().category, Category::View, "{id}");
        }
        let swap = get("view.compare_swap").unwrap();
        assert_eq!(swap.keys, ["B"]);
        assert_eq!(swap.scope, Scope::Viewport);
        s.blink = Blink {
            of: Some("HEAD".into()),
            ..Blink::default()
        };
        assert!(enabled("view.compare_swap", &s).is_ok());
        assert_eq!(checked("view.compare_swap", &s), Some(false));
        s.blink.front = Front::Rev;
        assert_eq!(checked("view.compare_swap", &s), Some(true));
        assert_eq!(checked("view.compare_only_rev", &s), Some(false));
        s.blink.alone = true;
        assert_eq!(checked("view.compare_only_rev", &s), Some(true));
        assert_eq!(checked("view.compare_only_working", &s), Some(false));
    }

    #[test]
    fn writes_wait_while_the_compared_revision_is_drawn() {
        use crate::core::compare::{Blink, Front, SWAPPED};
        use crate::core::modes::Mode;
        let mut s = State {
            mode: Mode::Edit,
            ..State::default()
        };
        assert!(enabled("edit.undo", &s).is_ok());
        s.blink = Blink {
            of: Some("HEAD".into()),
            front: Front::Rev,
            alone: false,
        };
        for id in WRITES {
            assert_eq!(enabled(id, &s), Err(SWAPPED), "{id}");
            assert_eq!(mode_allows(id, &s), Err(SWAPPED), "{id}");
        }
        // Looking, measuring and cutting stay on.
        for id in [
            "view.fit",
            "measure.tool",
            "section.edit",
            "view.compare_stop",
        ] {
            assert!(
                enabled(
                    id,
                    &State {
                        comparing: Some("HEAD".into()),
                        ..s.clone()
                    }
                )
                .is_ok(),
                "{id}"
            );
        }
        // The working files in front: writes are back.
        s.blink.front = Front::Working;
        assert!(enabled("edit.undo", &s).is_ok());
    }

    #[test]
    fn move_needs_one_movable_instance_and_a_mode_that_edits() {
        use crate::core::modes::Mode;
        let mut s = State {
            mode: Mode::Edit,
            ..State::default()
        };
        assert!(enabled("edit.move", &s).is_err(), "nothing selected");
        s.movable = Some(Ok(("frame".into(), "b2".into())));
        assert_eq!(enabled("edit.move", &s), Ok(()));
        s.movable = Some(Err("b1 is fixed".into()));
        assert!(enabled("edit.move", &s).is_err());
        s.moving = true;
        assert_eq!(enabled("edit.move", &s), Ok(()), "on: it turns off");
        assert_eq!(checked("edit.move", &s), Some(true));
        s.moving = false;
        s.movable = Some(Ok(("frame".into(), "b2".into())));
        s.mode = Mode::View;
        assert!(enabled("edit.move", &s).unwrap_err().contains("Annotate"));
        let c = get("edit.move").unwrap();
        assert_eq!(c.icon, Some("arrows-out-cardinal"));
        assert!(c.toolbar);
    }

    #[test]
    fn mate_needs_an_assembly_and_a_mode_that_edits() {
        use crate::core::modes::Mode;
        let mut s = State {
            mode: Mode::Annotate,
            ..State::default()
        };
        assert!(enabled("edit.mate", &s).is_err());
        s.assembly_shown = true;
        assert_eq!(enabled("edit.mate", &s), Ok(()));
        s.mode = Mode::View;
        assert!(enabled("edit.mate", &s).is_err());
        let c = get("edit.mate").unwrap();
        assert_eq!((c.title, c.icon), ("Mate…", Some("link-simple")));
    }

    #[test]
    fn modes_are_toolbar_toggles_and_annotating_needs_a_mode() {
        use crate::core::modes::Mode;
        let bar: Vec<&str> = toolbar().map(|c| c.id).collect();
        // Last on the toolbar but Markup, before the People strip, in order.
        assert_eq!(
            &bar[bar.len() - 4..],
            [
                "mode.view",
                "mode.annotate",
                "mode.edit",
                "annotation.markup"
            ]
        );
        for m in ["mode.view", "mode.annotate", "mode.edit"] {
            assert_eq!(get(m).unwrap().kind, Kind::Toggle, "{m}");
        }
        let s = |mode| State {
            mode,
            ..State::default()
        };
        assert_eq!(checked("mode.annotate", &s(Mode::Annotate)), Some(true));
        assert_eq!(checked("mode.edit", &s(Mode::Annotate)), Some(false));
        assert!(enabled("annotation.add", &s(Mode::View)).is_err());
        assert!(enabled("annotation.add", &s(Mode::Annotate)).is_ok());
        assert!(enabled("edit.undo", &s(Mode::Annotate)).is_err());
        assert!(enabled("edit.undo", &s(Mode::Edit)).is_ok());
        // `import.convert` isn't proposable: converting an import needs
        // Direct edit, as the daemon judges it.
        for id in ["import.to_solid", "import.to_reference"] {
            assert!(enabled(id, &s(Mode::View)).is_err(), "{id} in View");
            assert!(enabled(id, &s(Mode::Annotate)).is_err(), "{id} in Annotate");
            assert!(enabled(id, &s(Mode::Edit)).is_ok(), "{id} in Direct edit");
            // A row's context menu runs them with a target, still by mode.
            assert!(mode_allows(id, &s(Mode::Annotate)).is_err(), "{id}");
            assert!(mode_allows(id, &s(Mode::Edit)).is_ok(), "{id}");
        }
        assert!(context(Context::Entity(Pick::Face)).any(|c| c.id == "annotation.add"));
        assert!(context(Context::Viewport).any(|c| c.id == "annotation.here"));
        assert!(!context(Context::Ghost).any(|c| c.id == "annotation.add"));
        assert!(menu(Category::Tools).any(|c| c.id == "annotations.show"));
    }

    #[test]
    fn dependencies_are_in_the_project_menu() {
        let ids: Vec<&str> = menu(Category::Project).map(|c| c.id).collect();
        assert!(
            ids.contains(&"deps.show") && ids.contains(&"deps.place"),
            "{ids:?}"
        );
        let s = State::default();
        assert!(enabled("deps.place", &s).is_err());
        let s = State {
            assembly_shown: true,
            ..State::default()
        };
        assert!(enabled("deps.place", &s).is_ok());
        assert!(enabled("deps.show", &State::default()).is_ok());
    }

    #[test]
    fn ids_are_unique_and_namespaced() {
        for (i, a) in COMMANDS.iter().enumerate() {
            assert!(ryucad::config::command_id_ok(a.id), "{}", a.id);
            for b in &COMMANDS[i + 1..] {
                assert_ne!(a.id, b.id);
            }
        }
        for v in STANDARD_VIEWS {
            assert!(get(v).is_some(), "{v}");
        }
        // Stage 10's commands, moved into the registry by 11.B.
        assert!(get("appearance.edit").is_some());
        assert!(get("params.equations").is_some());
        for id in [
            "section.edit",
            "section.clear",
            "explode.edit",
            "explode.collapse",
            "motion.edit",
            "motion.stop",
        ] {
            assert!(get(id).is_some(), "{id}");
        }
    }

    #[test]
    fn every_command_is_titled_and_its_keys_read() {
        for c in COMMANDS {
            assert!(!c.title.is_empty(), "{}", c.id);
            assert!(c.title.starts_with(|x: char| x.is_uppercase()), "{}", c.id);
            for k in c.keys {
                let chord = Chord::parse(k).unwrap_or_else(|| panic!("{} {k}", c.id));
                // Only chords that type nothing fire while typing.
                assert!(!c.in_text || chord.types_nothing(), "{} {k}", c.id);
                // Keys that need the viewport's keyboard are the viewport's.
                assert!(!c.focus || c.scope == Scope::Viewport, "{}", c.id);
            }
            // Something shows it: the menu, the toolbar or a context menu.
            assert!(
                c.menu || c.toolbar || !c.context.is_empty() || !c.keys.is_empty(),
                "{}",
                c.id
            );
        }
    }

    #[test]
    fn every_icon_named_is_drawn() {
        for c in COMMANDS {
            if let Some(name) = c.icon {
                assert!(
                    crate::ui::icon::Icon::all().any(|i| i.name() == name),
                    "{}: {name}",
                    c.id
                );
            }
        }
        // The toolbar shows icons only.
        assert!(toolbar().all(|c| c.icon.is_some()));
    }

    #[test]
    fn reasons_are_sentences() {
        let nothing = State::default();
        for c in COMMANDS {
            if let Err(why) = enabled(c.id, &nothing) {
                assert!(why.starts_with(|x: char| x.is_uppercase()), "{}", c.id);
                assert!(why.ends_with('.'), "{}: {why}", c.id);
            }
        }
        let some = State {
            selected: 1,
            chosen_import: true,
            one_shown: true,
            assembly_shown: true,
            planar_face: true,
            weighable: true,
            comparing: Some("main".into()),
            blink: crate::core::compare::Blink {
                of: Some("main".into()),
                ..Default::default()
            },
            sectioned: true,
            exploded: true,
            motion_shown: true,
            scale: 1.0,
            mode: crate::core::modes::Mode::Edit,
            flattenable: true,
            movable: Some(Ok(("frame".into(), "b2".into()))),
            ..State::default()
        };
        // The Markup bar's own need it open, and are judged apart.
        for c in COMMANDS.iter().filter(|c| c.scope != Scope::Markup) {
            assert_eq!(enabled(c.id, &some), Ok(()), "{}", c.id);
        }
    }

    #[test]
    fn markup_locks_the_camera_and_its_keys_meet_only_each_other() {
        use crate::core::markup::Tool;
        use crate::core::modes::Mode;
        let mut s = State {
            one_shown: true,
            mode: Mode::Annotate,
            ..State::default()
        };
        assert_eq!(enabled("annotation.markup", &s), Ok(()));
        assert!(enabled("markup.arrow", &s).unwrap_err().contains("Markup"));
        assert_eq!(enabled("view.front", &s), Ok(()));
        s.marking = Some(Tool::Pen);
        for id in [
            "markup.arrow",
            "markup.undo",
            "markup.post",
            "annotation.markup",
        ] {
            assert_eq!(enabled(id, &s), Ok(()), "{id}");
        }
        for id in [
            "view.front",
            "view.fit",
            "view.orbit_left",
            "view.closer",
            "view.show",
        ] {
            assert!(
                enabled(id, &s).unwrap_err().contains("locks the camera"),
                "{id}"
            );
        }
        assert_eq!(checked("markup.pen", &s), Some(true));
        assert_eq!(checked("markup.arrow", &s), Some(false));
        assert_eq!(checked("annotation.markup", &s), Some(true));
        assert_eq!(enabled("edit.clear_selection", &s), Ok(()), "Esc leaves it");
        // In View, nothing marks.
        s = State {
            one_shown: true,
            mode: Mode::View,
            ..State::default()
        };
        assert!(enabled("annotation.markup", &s).is_err());
        // Its keys never meet the window's: F is Zoom to fit, Mod+Z Undo.
        assert!(!Scope::Markup.meets(Scope::Window));
        assert!(!Scope::Window.meets(Scope::Markup));
        assert!(!Scope::Markup.meets(Scope::Viewport));
        assert!(Scope::Markup.meets(Scope::Markup));
        let tools: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| c.scope == Scope::Markup)
            .map(|c| c.id)
            .collect();
        assert_eq!(tools.len(), 9);
        assert!(tools.iter().all(|id| !get(id).unwrap().menu));
        for t in Tool::ALL {
            assert!(tools.contains(&t.command()), "{t:?}");
        }
    }

    #[test]
    fn toggles_say_whether_they_are_on() {
        let s = State {
            edges: true,
            measuring: true,
            ..State::default()
        };
        for c in COMMANDS {
            assert_eq!(
                checked(c.id, &s).is_some(),
                c.kind == Kind::Toggle,
                "{}",
                c.id
            );
        }
        assert_eq!(checked("view.edges", &s), Some(true));
        assert_eq!(checked("view.origin", &s), Some(false));
    }

    #[test]
    fn zoom_in_and_out_step_by_eighths_within_bounds() {
        assert_eq!(scale_step(1.0, true), 1.125);
        assert_eq!(scale_step(1.0, false), 0.875);
        assert_eq!(scale_step(1.3, true), 1.375);
        assert_eq!(scale_step(2.0, true), 2.0);
        assert_eq!(scale_step(0.75, false), 0.75);
        let at = |scale| State {
            scale,
            ..State::default()
        };
        assert!(enabled("view.scale_up", &at(2.0)).is_err());
        assert!(enabled("view.scale_down", &at(0.75)).is_err());
        assert_eq!(enabled("view.scale_up", &at(1.0)), Ok(()));
    }

    #[test]
    fn menus_come_in_the_bar_s_order() {
        let titles: Vec<&str> = Category::ALL.iter().map(|c| c.title()).collect();
        assert_eq!(titles, ["File", "Edit", "View", "Tools", "Project", "Help"]);
        for c in Category::ALL {
            assert!(menu(c).next().is_some(), "{c:?}");
        }
    }

    #[test]
    fn context_menus_by_what_was_clicked() {
        let ids = |what| context(what).map(|c| c.id).collect::<Vec<_>>();
        // Every row has its reference and frames; the hideable ones isolate.
        for r in Row::ALL {
            let got = ids(Context::Row(r));
            assert_eq!(got[..2], ["edit.copy_reference", "view.fit_row"], "{r:?}");
            let hideable = matches!(r, Row::Solid | Row::Instance | Row::Copy);
            assert_eq!(got.contains(&"view.isolate"), hideable, "{r:?}");
            assert_eq!(got.contains(&"view.show_all"), hideable, "{r:?}");
        }
        assert_eq!(
            ids(Context::Row(Row::Part)),
            [
                "edit.copy_reference",
                "view.fit_row",
                "view.show_row",
                "tree.expand_below",
                "tree.collapse_below",
                "tree.expand_all",
                "tree.collapse_all",
                "annotation.add"
            ]
        );
        // A part's or an instance's row shows its part; every row opens
        // and closes the tree, and so does the space under the rows.
        for r in Row::ALL {
            let got = ids(Context::Row(r));
            let showable = matches!(r, Row::Part | Row::Instance);
            assert_eq!(got.contains(&"view.show_row"), showable, "{r:?}");
            for id in [
                "tree.expand_below",
                "tree.collapse_below",
                "tree.expand_all",
                "tree.collapse_all",
            ] {
                assert!(got.contains(&id), "{r:?} {id}");
            }
        }
        assert_eq!(ids(Context::Tree), ["tree.expand_all", "tree.collapse_all"]);
        assert!(below("tree.expand_below") && !below("tree.expand_all"));

        for k in Pick::ALL {
            let mut want = vec![
                "edit.copy_reference",
                "edit.select",
                "edit.select_other",
                "annotation.add",
            ];
            if matches!(k, Pick::Face | Pick::Edge | Pick::Vertex) {
                want.extend(["edit.move", "edit.mate"]);
            }
            assert_eq!(ids(Context::Entity(k)), want, "{k:?}");
        }
        assert_eq!(
            ids(Context::Ghost),
            [
                "edit.copy_reference",
                "view.compare_swap",
                "view.compare_only_rev",
                "view.compare_only_working"
            ]
        );
        assert_eq!(
            ids(Context::Viewport),
            ["view.fit", "view.show_all", "annotation.here"]
        );
        assert_eq!(ids(Context::Tab), ["view.close_tile", "view.reset_layout"]);
        assert!(enabled_on("view.close_tile", "viewport").is_err());
        assert_eq!(enabled_on("view.close_tile", "history"), Ok(()));
    }

    #[test]
    fn an_imports_mode_is_offered_only_where_it_changes() {
        for r in [Row::Feature, Row::Solid] {
            let ids: Vec<&str> = context(Context::Row(r)).map(|c| c.id).collect();
            assert!(ids.contains(&"import.to_solid"), "{r:?}");
            assert!(ids.contains(&"import.to_reference"), "{r:?}");
        }
        let listed = |import| {
            context(Context::Row(Row::Solid))
                .filter(|c| offered(c.id, import))
                .filter(|c| c.id.starts_with("import."))
                .map(|c| c.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(listed(Some(true)), ["import.to_solid"]);
        assert_eq!(listed(Some(false)), ["import.to_reference"]);
        assert!(listed(None).is_empty());
        // Context only: the menu bar and the palette don't list them.
        assert!(!get("import.to_solid").unwrap().menu);
        assert!(offered("edit.copy_reference", None));
    }
}
