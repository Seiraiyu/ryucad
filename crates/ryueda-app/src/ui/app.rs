//! The ryueda window (design 519): the tiles round the canvas, the menu
//! bar from the command table, the palette, the People strip, the status
//! line and toasts, over a [`Window`].

use crate::core::command::{COMMANDS, places};
use crate::core::keys::{Keymap, consume};
use crate::core::link::Daemon;
use crate::core::prefs;
use crate::core::window::Window;
use crate::ui::tiles::{Behavior, Local, SPEC};
use eframe::egui;
use ryu_app::command::{Category, Kind, menu};
use ryu_app::people::Line;
use ryu_app::shell::{self, Pane};
use ryu_app::style::{self, Contrast, Look};
use ryu_doc::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

pub enum Screen {
    Opened(Box<Window>),
    /// The project couldn't be opened: why.
    Failed(Error),
}

pub struct App {
    pub screen: Screen,
    tree: egui_tiles::Tree<Pane>,
    local: Local,
    keymap: Keymap,
    look: (Look, Contrast),
    palette: Option<String>,
    recent: Vec<&'static str>,
}

impl App {
    /// The window on the project in or above `dir`.
    pub fn open(ctx: &egui::Context, dir: &Path, daemon: &Daemon) -> App {
        ryu_app::fonts::install(ctx);
        let look = (prefs::look(), prefs::contrast());
        style::apply(ctx, look.0, look.1);
        ctx.set_theme(match prefs::theme() {
            Some(true) => egui::ThemePreference::Dark,
            Some(false) => egui::ThemePreference::Light,
            None => egui::ThemePreference::System,
        });
        let dark = ctx.global_style().visuals.dark_mode;
        let c = ctx.clone();
        let wake = Arc::new(move || c.request_repaint());
        let screen = match ryueda::project::find_root(dir)
            .and_then(|root| Window::open(&root, daemon, &whoami(), wake, dark))
        {
            Ok(w) => Screen::Opened(Box::new(w)),
            Err(e) => Screen::Failed(e),
        };
        let read = SPEC.read(&prefs::get("ui.layout"));
        App {
            screen,
            tree: shell::tree(&SPEC, &read.root),
            local: Local::default(),
            keymap: Keymap::new((), &prefs::get("ui.keys")),
            look,
            palette: None,
            recent: Vec::new(),
        }
    }

    pub fn opened(&self) -> Option<&Window> {
        match &self.screen {
            Screen::Opened(w) => Some(w),
            Screen::Failed(_) => None,
        }
    }

    pub fn opened_mut(&mut self) -> Option<&mut Window> {
        match &mut self.screen {
            Screen::Opened(w) => Some(w),
            Screen::Failed(_) => None,
        }
    }

    fn run(&mut self, ctx: &egui::Context, id: &'static str) {
        ryu_app::palette::ran(&mut self.recent, id);
        let Some(w) = self.opened_mut() else { return };
        match id {
            "view.fit" => w.camera = None,
            "view.next_sheet" => w.step_sheet(1),
            "view.prev_sheet" => w.step_sheet(-1),
            "view.go_to" => self.palette = Some("@".into()),
            "view.layer_frames" => w.layers[0] = !w.layers[0],
            "view.layer_notes" => w.layers[1] = !w.layers[1],
            "view.layer_intent" => w.layers[2] = !w.layers[2],
            "view.layer_pin_numbers" => w.layers[3] = !w.layers[3],
            "view.follow_shows" => {
                w.follow_shows = !w.follow_shows;
                let _ = prefs::set("ui.follow_shows", w.follow_shows.into());
            }
            "view.flash_changes" => {
                w.flash_on = !w.flash_on;
                w.flash.clear();
                let _ = prefs::set("ui.flash_changes", w.flash_on.into());
            }
            "edit.copy_ref" => {
                if let Some(r) = w.selection.last() {
                    ctx.copy_text(r.clone());
                }
            }
            "edit.clear_selection" => w.pick(None, false),
            "edit.undo" => w.undo(),
            "edit.redo" => w.redo(),
            "tools.palette" => self.palette = Some(String::new()),
            "tools.check" => w.run_check(),
            _ => {}
        }
    }

    fn checked(&self, id: &str) -> Option<bool> {
        let w = self.opened()?;
        Some(match id {
            "view.layer_frames" => w.layers[0],
            "view.layer_notes" => w.layers[1],
            "view.layer_intent" => w.layers[2],
            "view.layer_pin_numbers" => w.layers[3],
            "view.follow_shows" => w.follow_shows,
            "view.flash_changes" => w.flash_on,
            _ => return None,
        })
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let mac = cfg!(target_os = "macos");
        let typing = ctx.text_edit_focused();
        let mut fired = Vec::new();
        for c in COMMANDS {
            if typing && !c.in_text {
                continue;
            }
            for chord in self.keymap.chords(c.id) {
                if ctx.input_mut(|i| consume(i, &chord.shortcut(mac))) {
                    fired.push(c.id);
                }
            }
        }
        for id in fired {
            self.run(ctx, id);
        }
    }

    fn menus(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        egui::MenuBar::new().ui(ui, |ui| {
            for cat in Category::ALL {
                let items: Vec<_> = menu(COMMANDS, cat).collect();
                if items.is_empty() {
                    continue;
                }
                ui.menu_button(cat.title(), |ui| {
                    for c in items {
                        if c.id == "view.theme" {
                            ui.menu_button("Theme", |ui| {
                                for (label, pref) in
                                    [("System", "system"), ("Light", "light"), ("Dark", "dark")]
                                {
                                    if ui.button(label).clicked() {
                                        let _ = prefs::set("ui.theme", pref.into());
                                        ui.ctx().set_theme(match pref {
                                            "dark" => egui::ThemePreference::Dark,
                                            "light" => egui::ThemePreference::Light,
                                            _ => egui::ThemePreference::System,
                                        });
                                    }
                                }
                            });
                            continue;
                        }
                        let keys = self
                            .keymap
                            .chords(c.id)
                            .first()
                            .map(|k| k.text())
                            .unwrap_or_default();
                        let clicked = match (c.kind, self.checked(c.id)) {
                            (Kind::Toggle, Some(on)) => ui
                                .add(egui::Button::new(c.title).selected(on).shortcut_text(keys))
                                .clicked(),
                            _ => ui
                                .add(egui::Button::new(c.title).shortcut_text(keys))
                                .clicked(),
                        };
                        if clicked {
                            chosen = Some(c.id);
                        }
                    }
                });
            }
        });
        if let Some(id) = chosen {
            self.run(ui.ctx(), id);
        }
    }

    fn palette(&mut self, ctx: &egui::Context) {
        let Some(mut q) = self.palette.take() else {
            return;
        };
        let mut open = true;
        let mut chosen: Option<Result<&'static str, String>> = None;
        egui::Window::new("Command palette")
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut q)
                        .hint_text("A command, or @ a designator, net or block"),
                );
                r.request_focus();
                if let Some(at) = q.strip_prefix('@') {
                    if let Some(w) = self.opened() {
                        let design = w.shown.design.clone().unwrap_or_default();
                        let block = w.shown.block.clone().unwrap_or_default();
                        for p in places(&w.nav, &design, &block, at).into_iter().take(12) {
                            if ui.button(&p.title).clicked() {
                                chosen = Some(Err(p.reference));
                            }
                        }
                    }
                } else {
                    for c in ryu_app::palette::commands(COMMANDS, &q, &self.recent)
                        .into_iter()
                        .take(12)
                    {
                        if ui.button(c.title).clicked() {
                            chosen = Some(Ok(c.id));
                        }
                    }
                }
            });
        match chosen {
            Some(Ok(id)) => self.run(ctx, id),
            Some(Err(r)) => {
                if let Some(w) = self.opened_mut() {
                    w.go_to(&r);
                }
            }
            None if open && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) => {
                self.palette = Some(q)
            }
            None => {}
        }
    }

    fn people(&mut self, ui: &mut egui::Ui) {
        let Some(w) = self.opened_mut() else { return };
        let me = w.me().to_owned();
        let lines: Vec<Line> = w
            .people
            .all
            .values()
            .map(|p| Line {
                id: p.id.clone(),
                name: p.name.clone(),
                agent: p.kind == ryu_session::live::Kind::Agent,
                you: p.id == me,
                mark: None,
                status: p.status.as_ref().map(|(t, _)| t.clone()),
                busy: None,
                followable: ryu_app::people::View::followable(&p.view),
            })
            .collect();
        if let Some(f) = ryu_app::people::strip(ui, &lines, w.following.as_deref()) {
            w.follow(f);
        }
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        let Some(w) = self.opened() else { return };
        ui.horizontal(|ui| {
            if let Some(sc) = &w.scene {
                ui.label(format!(
                    "{} · {} ({}/{})",
                    sc.block,
                    sc.name,
                    sc.sheets
                        .iter()
                        .position(|(s, _)| *s == sc.sheet)
                        .map_or(0, |i| i + 1),
                    sc.sheets.len()
                ));
                if let Some(layer) = &sc.layer {
                    ui.weak(format!("layer {layer}"));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let version = ryueda::proto::VersionInfo::current().ryueda;
                ryu_app::status::connection(ui, w.lost, &version);
                ryu_app::status::look(ui, look.0, look.1);
            });
        });
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let Some(w) = self.opened_mut() else { return };
        let shown = w.toasts.shown().to_vec();
        let mut act: Option<ryu_app::toast::Action> = None;
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -36.0))
            .show(ctx, |ui| {
                for t in &shown {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if let Some(who) = &t.who {
                                ui.strong(who);
                            }
                            ui.label(&t.text);
                            for a in &t.actions {
                                if ui.small_button(a.title()).clicked() {
                                    act = Some(a.clone());
                                }
                            }
                            if ui.small_button("✕").on_hover_text("Close").clicked() {
                                w.toasts.close(t.id);
                            }
                        });
                        if let Some(d) = &t.detail {
                            ui.weak(d);
                        }
                    });
                }
            });
        match act {
            Some(ryu_app::toast::Action::Show(refs)) => {
                if let Some(r) = refs.first() {
                    w.go_to(r);
                }
            }
            Some(ryu_app::toast::Action::Undo(_)) => w.undo(),
            _ => {}
        }
    }

    /// One frame: what eframe's `ui` and the tests' harness call.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let dark = ctx.global_style().visuals.dark_mode;
        if let Some(w) = self.opened_mut() {
            w.pump();
            w.set_dark(dark);
            if let Some(after) = w.tick(now) {
                ctx.request_repaint_after(after);
            }
        }
        self.keys(&ctx);
        egui::Panel::top("menus").show(ui, |ui| {
            self.menus(ui);
            self.people(ui);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status(ui));
        egui::CentralPanel::default().show(ui, |ui| match &mut self.screen {
            Screen::Failed(e) => {
                ui.heading("ryueda can't open this project");
                ui.label(format!(
                    "{} ({})",
                    e.message,
                    crate::ui::canvas::code_word(e.code)
                ));
            }
            Screen::Opened(w) => {
                let mut copy = None;
                let mut b = Behavior {
                    w,
                    local: &mut self.local,
                    now,
                    copy: &mut copy,
                };
                self.tree.ui(&mut b, ui);
                if let Some(r) = copy {
                    ctx.copy_text(r);
                }
            }
        });
        self.palette(&ctx);
        self.toasts(&ctx);
    }

    /// Keeps the layout for the next window.
    pub fn save_prefs(&self) {
        if let Some(n) = shell::node(&self.tree) {
            let _ = prefs::set("ui.layout", n.to_value());
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }

    fn on_exit(&mut self) {
        self.save_prefs();
    }
}

/// The person at the keyboard, by their login name.
pub fn whoami() -> String {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "person".to_owned())
}
