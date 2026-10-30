//! The form toolkit (design 199, Form toolkit): the one way forms and
//! dialogs are drawn. A two-column grid of labels and fields; quantity
//! fields that keep their text, check its syntax as it is typed and show
//! the daemon's value beside it; read-only fields with copy buttons; a
//! dirty mark per field; the daemon's refusals on the field they name;
//! one conflict banner; and Apply and Revert at the foot. Enter in a
//! field applies and Esc reverts, and the form says so to its caller
//! ([`Keys`]). What a form may change stays the caller's.

use crate::core::edit::{Changed, Choice, Field, Flag};
use crate::ui::icon::{self, Icon};
use eframe::egui::{self, Color32, RichText};

/// What the keyboard asked of a form this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    /// Enter in one of its fields.
    pub apply: bool,
    /// Esc in one of its fields.
    pub revert: bool,
}

impl Keys {
    fn take(&mut self, ui: &egui::Ui, r: &egui::Response) {
        // egui lets go of a single-line field on Enter and Esc.
        if r.lost_focus() {
            let (enter, esc) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            self.apply |= enter;
            self.revert |= esc;
        }
    }
}

/// The accent a dirty field's mark and frame take.
fn accent(ui: &egui::Ui) -> Color32 {
    ui.visuals().selection.stroke.color
}

fn danger(ui: &egui::Ui) -> Color32 {
    ui.visuals().error_fg_color
}

/// The rows of one form's grid.
pub struct Rows<'a> {
    ui: &'a mut egui::Ui,
    /// The fields' width.
    width: f32,
    pub keys: Keys,
}

impl Rows<'_> {
    /// A label in the first column: weak, with the accent's `•` when
    /// what it labels was changed.
    fn label(&mut self, text: &str, dirty: bool) -> egui::Response {
        label(self.ui, text, dirty)
    }

    /// A line under a field: what is wrong with it.
    pub fn note(&mut self, text: &str) {
        self.ui.label("");
        let colour = danger(self.ui);
        self.ui.horizontal(|ui| {
            icon::show(ui, Icon::X_CIRCLE, 12.0, colour);
            ui.label(RichText::new(text).small().color(colour));
        });
        self.ui.end_row();
    }

    /// A field framed as the mockup frames it: the accent when dirty, red
    /// when wrong.
    fn framed(
        ui: &mut egui::Ui,
        edit: egui::TextEdit<'_>,
        width: f32,
        dirty: bool,
        wrong: bool,
    ) -> egui::Response {
        let stroke = if wrong {
            Some(danger(ui))
        } else if dirty {
            Some(accent(ui))
        } else {
            None
        };
        ui.scope(|ui| {
            if let Some(c) = stroke {
                let w = &mut ui.visuals_mut().widgets;
                for s in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                    s.bg_stroke = egui::Stroke::new(1.0, c);
                }
                ui.visuals_mut().selection.stroke = egui::Stroke::new(1.0, c);
            }
            let height = ui.spacing().interact_size.y;
            ui.add_sized([width, height], edit)
        })
        .inner
    }

    /// A quantity or expression: kept as written, its syntax checked as
    /// typed (the fault at its byte), the daemon's value beside it when
    /// what was read isn't a literal, and the daemon's refusal under it.
    pub fn quantity(&mut self, label: &str, f: &mut Field) -> egui::Response {
        self.text_field(label, f, true)
    }

    /// Text that isn't an expression (a name, a skip list, a hex colour).
    pub fn text(&mut self, label: &str, f: &mut Field) -> egui::Response {
        self.text_field(label, f, false)
    }

    fn text_field(&mut self, label: &str, f: &mut Field, expression: bool) -> egui::Response {
        let dirty = f.dirty();
        let fault = if expression { f.syntax() } else { None };
        let wrong = fault.is_some() || f.error.is_some();
        let name = self.label(label, dirty);
        let width = self.width;
        let value = f.value.clone().filter(|_| !dirty);
        let r = self
            .ui
            .horizontal(|ui| {
                let shown = value.as_ref().map(|v| format!("= {v}"));
                let room = shown.as_ref().map_or(0.0, |_| 70.0);
                let edit = egui::TextEdit::singleline(&mut f.text).font(egui::TextStyle::Monospace);
                let r = Rows::framed(ui, edit, (width - room).max(60.0), dirty, wrong)
                    .labelled_by(name.id);
                if let Some(v) = shown {
                    ui.weak(RichText::new(v).small());
                }
                r
            })
            .inner;
        let r = if dirty {
            r.on_hover_text(format!("was {}", f.read))
        } else {
            r
        };
        self.keys.take(self.ui, &r);
        if r.changed() {
            f.error = None;
        }
        self.ui.end_row();
        if let Some(e) = fault.or_else(|| f.error.clone()) {
            self.note(&e);
        }
        r
    }

    /// One of a list, by name.
    pub fn choice(&mut self, label: &str, c: &mut Choice) {
        let dirty = c.dirty();
        self.label(label, dirty);
        let r = egui::ComboBox::from_id_salt(("choice", &c.name))
            .selected_text(&c.chosen)
            .width(self.width - 8.0)
            .show_ui(self.ui, |ui| {
                for o in &c.options {
                    ui.selectable_value(&mut c.chosen, o.clone(), o);
                }
            })
            .response;
        let name = c.name.clone();
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &name));
        if dirty {
            r.on_hover_text(format!("was {}", c.read));
        }
        self.ui.end_row();
    }

    /// A check box, its text beside it.
    pub fn flag(&mut self, text: &str, f: &mut Flag) {
        self.label("", f.dirty());
        self.ui.checkbox(&mut f.chosen, text);
        self.ui.end_row();
    }

    /// A value shown, not edited, in the same grid: monospace for
    /// references, revisions and hashes, with a copy button when `copy`
    /// gives what to copy.
    pub fn read_only(&mut self, label: &str, value: &str, mono: bool, copy: Option<&str>) {
        self.label(label, false);
        self.ui.horizontal(|ui| {
            let text = if mono {
                RichText::new(value).monospace()
            } else {
                RichText::new(value)
            };
            ui.label(text);
            if let Some(c) = copy {
                let title = format!("Copy {label}");
                if icon::button(ui, Icon::COPY, 12.0, &title, &title, false, Ok(())).clicked() {
                    ui.ctx().copy_text(c.to_owned());
                }
            }
        });
        self.ui.end_row();
    }

    /// A control of the caller's in the field's column, its label marked
    /// dirty as `dirty` says; `add` gets the label's id to be labelled by.
    pub fn custom_dirty<R>(
        &mut self,
        label: &str,
        dirty: bool,
        add: impl FnOnce(&mut egui::Ui, egui::Id) -> R,
    ) -> R {
        let name = self.label(label, dirty).id;
        let r = add(self.ui, name);
        self.ui.end_row();
        r
    }

    /// Anything else in the field's column, under `label`.
    pub fn custom<R>(&mut self, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        self.label(label, false);
        let r = add(self.ui);
        self.ui.end_row();
        r
    }
}

/// A form's grid of labels and fields, `width` the share of the room the
/// fields take. Returns what the keyboard asked of it.
pub fn grid(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    add: impl FnOnce(&mut Rows<'_>),
) -> Keys {
    let width = (ui.available_width() * 0.62).clamp(110.0, 280.0);
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([8.0, 6.0])
        .min_col_width(60.0)
        .show(ui, |ui| {
            let mut rows = Rows {
                ui,
                width,
                keys: Keys::default(),
            };
            add(&mut rows);
            rows.keys
        })
        .inner
}

/// The form's heading: its icon, what it edits as written (the window
/// tests read it), and `• EDITED` in the accent while it is dirty.
pub fn heading(ui: &mut egui::Ui, icon: Option<Icon>, title: &str, dirty: bool) {
    ui.horizontal(|ui| {
        let weak = ui.visuals().weak_text_color();
        if let Some(i) = icon {
            icon::show(ui, i, 16.0, weak);
        }
        ui.heading(title);
        if dirty {
            ui.label(RichText::new("• EDITED").small().strong().color(accent(ui)));
        }
    });
}

/// A label as the toolkit's grids draw them, for grids of their own
/// (settings that apply at once): right-aligned and weak, with the
/// accent's `•` when what it labels was changed.
pub fn label(ui: &mut egui::Ui, text: &str, dirty: bool) -> egui::Response {
    let shown = if dirty {
        format!("• {text}")
    } else {
        text.to_owned()
    };
    let colour = if dirty {
        accent(ui)
    } else {
        ui.visuals().weak_text_color()
    };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(RichText::new(shown).color(colour))
    })
    .inner
}

/// A grid laid out as the toolkit's are, for `columns` columns.
pub fn plain_grid(id: impl std::hash::Hash + std::fmt::Debug, columns: usize) -> egui::Grid {
    egui::Grid::new(id)
        .num_columns(columns)
        .spacing([8.0, 6.0])
        .min_col_width(60.0)
}

/// What the conflict banner's buttons asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    /// Keep the values read again (the banner goes).
    Reload,
    /// Put what this window typed back over them.
    KeepMine,
}

/// The one conflict banner: who changed what while this window edited it,
/// each change as `name: theirs (you typed yours)`, Reload and Keep mine.
pub fn banner(ui: &mut egui::Ui, what: &str, changed: &[Changed]) -> Option<Banner> {
    if changed.is_empty() {
        return None;
    }
    let warn = ui.visuals().warn_fg_color;
    let mut out = None;
    egui::Frame::new()
        .fill(warn.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, warn.gamma_multiply(0.6)))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icon::show(ui, Icon::WARNING, 14.0, warn);
                ui.label(
                    RichText::new(format!("Someone changed {what} while you edited it.")).strong(),
                );
            });
            ui.label("It now reads:");
            for c in changed {
                ui.label(format!("{}: {} (you typed {})", c.name, c.theirs, c.yours));
            }
            ui.horizontal(|ui| {
                if ui
                    .button("Reload")
                    .on_hover_text("Keep what it reads now")
                    .clicked()
                {
                    out = Some(Banner::Reload);
                }
                if ui
                    .button("Keep mine")
                    .on_hover_text("Put what you typed back, to apply over it")
                    .clicked()
                {
                    out = Some(Banner::KeepMine);
                }
            });
        });
    ui.add_space(6.0);
    out
}

/// A refusal no field holds: a card at the form's top.
pub fn error(ui: &mut egui::Ui, e: &ryucad::error::Error) {
    let red = danger(ui);
    egui::Frame::new()
        .fill(red.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, red.gamma_multiply(0.6)))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                icon::show(ui, Icon::X_CIRCLE, 14.0, red);
                ui.label(
                    RichText::new(crate::core::feedback::title(e.code))
                        .strong()
                        .color(red),
                );
            });
            ui.label(&e.message);
        });
    ui.add_space(6.0);
}

/// What the foot's buttons asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Foot {
    pub apply: bool,
    pub revert: bool,
}

/// Revert and Apply at the form's foot, with their keys, disabled while
/// nothing changed or a save is on its way. `names` are their labels
/// (`Apply`, `Revert`; a tile with several forms names them apart).
pub fn foot(ui: &mut egui::Ui, dirty: bool, saving: bool, names: (&str, &str)) -> Foot {
    let mut out = Foot::default();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let ok = dirty && !saving;
        // The keys are said on hover: shortcut text would join the
        // buttons' names.
        out.revert = ui
            .add_enabled(ok, egui::Button::new(names.1))
            .on_hover_text("Esc in a field")
            .clicked();
        let apply = egui::Button::new(names.0);
        let apply = if ok {
            apply.fill(ui.visuals().selection.bg_fill)
        } else {
            apply
        };
        out.apply = ui
            .add_enabled(ok, apply)
            .on_hover_text("Enter in a field")
            .clicked();
        if saving {
            ui.spinner();
        }
    });
    out
}
