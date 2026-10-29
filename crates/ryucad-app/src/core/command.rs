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
}

impl Scope {
    /// Whether a key could reach both: a window key works over the
    /// viewport too.
    pub fn meets(self, other: Scope) -> bool {
        self == other || self == Scope::Window || other == Scope::Window
    }

    pub fn title(self) -> &'static str {
        match self {
            Scope::Window => "Anywhere",
            Scope::Viewport => "Viewport (only while the pointer is over it)",
        }
    }
}

/// What a right-click was on, for context menus. A context command runs
/// with the reference of what was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// An entity under the pointer in the viewport.
    Entity,
    /// A compare's ghost under the pointer: gone from the working files,
    /// so it can be described and copied, never selected.
    Ghost,
    /// The viewport's background.
    Viewport,
    /// A tree row that can be hidden: an instance, a copy, a solid.
    Hideable,
}

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
    /// Whether its keys fire while a text field has the keyboard: only
    /// modified chords may.
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

use Category::{Edit, File, Project, Tools, View};

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
    cmd("file.import_step", "Import STEP…", File).icon("file-arrow-down"),
    cmd("file.update_import", "Update import…", File),
    cmd("file.export_step", "Export STEP…", File).icon("export"),
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
        .context(&[Context::Entity, Context::Ghost, Context::Hideable]),
    cmd("edit.select", "Select", Edit)
        .context(&[Context::Entity])
        .hidden(),
    cmd("edit.clear_selection", "Clear selection", Edit).keys(&["Escape"]),
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
    cmd("view.fit_selection", "Zoom to selection", View)
        .icon("selection")
        .toolbar(),
    cmd("view.quality", "Display quality", View).choices(),
    cmd("view.edges", "Edges", View).toggle(),
    cmd("view.sketches", "Sketches", View).toggle(),
    cmd("view.nav_cube", "Navigation cube", View).toggle(),
    cmd("view.triad", "Axis triad", View).toggle(),
    cmd("view.origin", "Origin", View).toggle(),
    cmd("view.show", "Show", View).choices(),
    cmd("view.isolate", "Isolate", View)
        .icon("eye")
        .context(&[Context::Hideable])
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
        .context(&[Context::Hideable, Context::Viewport]),
    cmd("view.compare_head", "Compare with HEAD", View)
        .icon("git-diff")
        .toolbar(),
    cmd("view.compare_with", "Compare with…", View),
    cmd("view.compare_stop", "Stop comparing", View),
    cmd("measure.tool", "Measure", Tools).icon("ruler").toggle(),
    cmd("measure.mass", "Mass properties", Tools).icon("scales"),
    cmd("measure.clearance", "Clearance", Tools).icon("arrows-left-right"),
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
];

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
    /// Display toggles and the Measure tool, as they stand.
    pub edges: bool,
    pub sketches: bool,
    pub nav_cube: bool,
    pub triad: bool,
    pub origin: bool,
    pub measuring: bool,
    /// The view selector is open.
    pub selector: bool,
}

/// Whether `id` can run now, or why not: a sentence the menus' hover, the
/// palette and the toolbar's tooltip all show.
pub fn enabled(id: &str, s: &State) -> Result<(), &'static str> {
    let need = |ok: bool, why: &'static str| if ok { Ok(()) } else { Err(why) };
    match id {
        "file.update_import" => need(
            s.chosen_import,
            "Choose an imported feature in the tree first.",
        ),
        "file.export_step" => need(
            s.one_shown,
            "Show one part or assembly first (View > Show).",
        ),
        "edit.copy_reference" | "view.fit_selection" => {
            need(s.selected > 0, "Select something first.")
        }
        // Esc also closes the view selector and leaves the Measure tool.
        "edit.clear_selection" => need(
            s.selected > 0 || s.chosen || s.selector || s.measuring,
            "Nothing is selected or chosen.",
        ),
        "view.normal_to" => need(s.planar_face, "Select a planar face first."),
        "view.compare_head" => need(
            s.comparing.as_deref() != Some("HEAD"),
            "Already comparing with HEAD.",
        ),
        "view.compare_stop" => need(s.comparing.is_some(), "Not comparing."),
        "measure.mass" => need(s.weighable, "Show one part or assembly, or select a solid."),
        "measure.clearance" | "tools.interference" => {
            need(s.assembly_shown, "Show an assembly first (View > Show).")
        }
        _ => Ok(()),
    }
}

/// A toggle's state now; `None` for commands that aren't toggles.
pub fn checked(id: &str, s: &State) -> Option<bool> {
    Some(match id {
        "view.edges" => s.edges,
        "view.sketches" => s.sketches,
        "view.nav_cube" => s.nav_cube,
        "view.triad" => s.triad,
        "view.origin" => s.origin,
        "measure.tool" => s.measuring,
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
    }

    #[test]
    fn every_command_is_titled_and_its_keys_read() {
        for c in COMMANDS {
            assert!(!c.title.is_empty(), "{}", c.id);
            assert!(c.title.starts_with(|x: char| x.is_uppercase()), "{}", c.id);
            for k in c.keys {
                let chord = Chord::parse(k).unwrap_or_else(|| panic!("{} {k}", c.id));
                // Only modified chords fire while typing.
                assert!(!c.in_text || chord.modified(), "{} {k}", c.id);
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
            ..State::default()
        };
        for c in COMMANDS {
            assert_eq!(enabled(c.id, &some), Ok(()), "{}", c.id);
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
    fn menus_come_in_the_bar_s_order() {
        let titles: Vec<&str> = Category::ALL.iter().map(|c| c.title()).collect();
        assert_eq!(titles, ["File", "Edit", "View", "Tools", "Project", "Help"]);
        for c in &Category::ALL[..5] {
            assert!(menu(*c).next().is_some(), "{c:?}");
        }
        assert_eq!(
            context(Context::Entity).map(|c| c.id).collect::<Vec<_>>(),
            ["edit.copy_reference", "edit.select"]
        );
        assert_eq!(
            context(Context::Viewport).map(|c| c.id).collect::<Vec<_>>(),
            ["view.fit", "view.show_all"]
        );
    }
}
