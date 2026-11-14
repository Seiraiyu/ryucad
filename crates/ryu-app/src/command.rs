//! The command registry's types (design 199, Command registry): every
//! action a window offers, once, by a namespaced id that `ui.keys` names
//! and so is never reused. A product's app holds its table, a
//! `&'static [Command<C>]`, where `C` is what its context menus target.
//! Menus, the toolbar, context menus, the palette and the keymap are all
//! built from that table. It holds no egui. Moved from ryucad-app by
//! ryueda 3.C.

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

/// How a command shows in a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A button.
    Action,
    /// A check: on or off (the product says which).
    Toggle,
    /// A submenu of choices the window lists (recent projects, parts).
    Choices,
}

/// One command; `C` is what its context menus are offered on.
#[derive(Clone, Copy, Debug)]
pub struct Command<C: 'static> {
    /// `category.name`, unique and never reused.
    pub id: &'static str,
    pub title: &'static str,
    pub category: Category,
    /// An [`crate::icon`] name, for the toolbar, menus and the palette.
    pub icon: Option<&'static str>,
    /// Its default chords, before a preset and `ui.keys`.
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
    pub context: &'static [C],
    /// Whether the menu bar lists it (context-only and viewport-only
    /// commands aren't).
    pub menu: bool,
    pub kind: Kind,
}

/// A command with nothing but its id, title and menu.
pub const fn cmd<C: Copy>(id: &'static str, title: &'static str, category: Category) -> Command<C> {
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

impl<C: Copy> Command<C> {
    pub const fn icon(self, icon: &'static str) -> Self {
        Command {
            icon: Some(icon),
            ..self
        }
    }
    pub const fn keys(self, keys: &'static [&'static str]) -> Self {
        Command { keys, ..self }
    }
    pub const fn in_text(self) -> Self {
        Command {
            in_text: true,
            ..self
        }
    }
    pub const fn toolbar(self) -> Self {
        Command {
            toolbar: true,
            ..self
        }
    }
    pub const fn context(self, context: &'static [C]) -> Self {
        Command { context, ..self }
    }
    pub const fn hidden(self) -> Self {
        Command {
            menu: false,
            ..self
        }
    }
    pub const fn viewport(self) -> Self {
        Command {
            scope: Scope::Viewport,
            ..self
        }
    }
    pub const fn markup(self) -> Self {
        Command {
            scope: Scope::Markup,
            menu: false,
            ..self
        }
    }
    pub const fn focused(self) -> Self {
        Command {
            scope: Scope::Viewport,
            focus: true,
            ..self
        }
    }
    pub const fn toggle(self) -> Self {
        Command {
            kind: Kind::Toggle,
            ..self
        }
    }
    pub const fn choices(self) -> Self {
        Command {
            kind: Kind::Choices,
            ..self
        }
    }
}

/// The command with this id.
pub fn get<C>(registry: &'static [Command<C>], id: &str) -> Option<&'static Command<C>> {
    registry.iter().find(|c| c.id == id)
}

/// The commands a category's menu lists, in the registry's order.
pub fn menu<C>(
    registry: &'static [Command<C>],
    c: Category,
) -> impl Iterator<Item = &'static Command<C>> {
    registry.iter().filter(move |x| x.category == c && x.menu)
}

/// The toolbar's buttons, in the registry's order.
pub fn toolbar<C>(registry: &'static [Command<C>]) -> impl Iterator<Item = &'static Command<C>> {
    registry.iter().filter(|x| x.toolbar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Here {
        Canvas,
    }

    const TOY: &[Command<Here>] = &[
        cmd("file.open", "Open…", Category::File)
            .keys(&["Mod+O"])
            .in_text(),
        cmd("view.fit", "Fit", Category::View)
            .icon("corners-out")
            .toolbar(),
        cmd("view.pan_left", "Pan left", Category::View)
            .keys(&["Left"])
            .focused()
            .hidden(),
        cmd("edit.copy_ref", "Copy reference", Category::Edit)
            .context(&[Here::Canvas])
            .hidden(),
        cmd("view.theme", "Theme", Category::View).choices(),
    ];

    #[test]
    fn a_registry_is_built_and_read_by_id_menu_and_toolbar() {
        assert_eq!(get(TOY, "view.fit").unwrap().title, "Fit");
        assert!(get(TOY, "view.nothing").is_none());
        let view: Vec<&str> = menu(TOY, Category::View).map(|c| c.id).collect();
        assert_eq!(view, ["view.fit", "view.theme"]);
        let bar: Vec<&str> = toolbar(TOY).map(|c| c.id).collect();
        assert_eq!(bar, ["view.fit"]);
        let pan = get(TOY, "view.pan_left").unwrap();
        assert!(pan.focus && pan.scope == Scope::Viewport && !pan.menu);
        assert_eq!(get(TOY, "edit.copy_ref").unwrap().context, &[Here::Canvas]);
        assert_eq!(get(TOY, "view.theme").unwrap().kind, Kind::Choices);
        for c in TOY {
            assert!(ryu_session::chords::command_id_ok(c.id), "{}", c.id);
        }
    }

    #[test]
    fn scopes_meet_as_they_always_have() {
        use Scope::*;
        assert!(Window.meets(Viewport) && Viewport.meets(Window) && Window.meets(Window));
        assert!(!Markup.meets(Window) && !Viewport.meets(Markup) && Markup.meets(Markup));
        assert_eq!(
            Category::ALL.map(Category::title),
            ["File", "Edit", "View", "Tools", "Project", "Help"]
        );
    }
}
