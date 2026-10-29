//! The Help menu's windows (design 199, Command registry > Help menu):
//! About ryucad (versions, commit, the adapter, and where preferences and
//! the cache live) and Driving ryucad from an agent (the lines to copy for
//! this project). Keyboard shortcuts is the Shortcuts tile's.

use crate::ui::dialogs::window;
use eframe::egui;
use std::path::Path;

/// What About says, one line each: (what, value).
pub fn about_lines() -> Vec<(String, String)> {
    let v = ryucad::proto::VersionInfo::current();
    let path = |p: ryucad::error::Result<std::path::PathBuf>| {
        p.map_or_else(|e| e.message, |p| p.display().to_string())
    };
    let adapter = crate::ui::platform::adapter();
    vec![
        ("ryucad".into(), v.ryucad),
        ("ryukei".into(), v.ryukei),
        ("Commit".into(), v.commit),
        ("Target".into(), v.target),
        (
            "Adapter".into(),
            if adapter.is_empty() {
                "none yet".into()
            } else {
                adapter
            },
        ),
        ("Preferences".into(), path(ryucad::config::config_dir())),
        ("Cache".into(), path(ryucad::cache::cache_dir())),
    ]
}

/// The lines an agent runs for the project at `root`.
pub fn agent_lines(root: &Path) -> String {
    let dir = root.display().to_string();
    let dir = if dir.contains(char::is_whitespace) {
        format!("\"{dir}\"")
    } else {
        dir
    };
    format!("cd {dir}\nryucad rebuild\nryucad --help")
}

pub fn about(ctx: &egui::Context, open: &mut bool, view: egui::Rect) {
    window("About ryucad", view)
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("CAD for agents to drive, and people to watch, point at and judge.");
            egui::Grid::new("about").num_columns(2).show(ui, |ui| {
                for (what, value) in about_lines() {
                    ui.weak(what);
                    ui.monospace(value);
                    ui.end_row();
                }
            });
            ui.weak(
                "Its text faces, Inter and JetBrains Mono, are under the SIL Open Font License 1.1, \
                 and its icons, Phosphor, under MIT: their texts are in the release archive's \
                 licenses folder.",
            );
        });
}

pub fn agents(ctx: &egui::Context, open: &mut bool, root: &Path, view: egui::Rect) {
    window("Driving ryucad from an agent", view)
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(
                "Agents drive ryucad through its CLI: every command prints one JSON document, \
                 and everything this window does is a command too.",
            );
            let lines = agent_lines(root);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.monospace(&lines);
            });
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(lines);
            }
            ui.weak("This window shows what agents do as they do it.");
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_names_the_build_and_where_things_live() {
        let lines = about_lines();
        let names: Vec<&str> = lines.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(
            names,
            [
                "ryucad",
                "ryukei",
                "Commit",
                "Target",
                "Adapter",
                "Preferences",
                "Cache"
            ]
        );
        assert!(lines.iter().all(|(_, v)| !v.is_empty()));
    }

    #[test]
    fn the_agent_lines_name_the_project() {
        assert_eq!(
            agent_lines(Path::new("/work/bracket")),
            "cd /work/bracket\nryucad rebuild\nryucad --help"
        );
        assert!(agent_lines(Path::new("/my work/b")).starts_with("cd \"/my work/b\""));
    }
}
