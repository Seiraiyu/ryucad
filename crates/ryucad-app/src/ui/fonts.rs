//! The window's faces, embedded (design 199, Fonts and icons): Inter for
//! text, JetBrains Mono for references, revisions and values, and
//! Phosphor Regular and Fill as an icon font in families of their own.
//! egui's own fonts stay behind each as fallbacks. Their licences are in
//! `crates/ryucad-app/fonts/LICENSES.md` and every release archive.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

pub const INTER: &[u8] = include_bytes!("../../fonts/InterVariable.ttf");
pub const MONO: &[u8] = include_bytes!("../../fonts/JetBrainsMono-Regular.ttf");
pub const PHOSPHOR: &[u8] = include_bytes!("../../fonts/Phosphor.ttf");
pub const PHOSPHOR_FILL: &[u8] = include_bytes!("../../fonts/Phosphor-Fill.ttf");

/// The family Phosphor Regular's glyphs are drawn in.
pub const ICONS: &str = "icons";
/// Phosphor Fill: a chosen toggle.
pub const ICONS_FILL: &str = "icons-fill";

/// egui's fonts with ours first.
pub fn definitions() -> FontDefinitions {
    let mut d = FontDefinitions::default();
    for (name, bytes) in [
        ("inter", INTER),
        ("jetbrains-mono", MONO),
        ("phosphor", PHOSPHOR),
        ("phosphor-fill", PHOSPHOR_FILL),
    ] {
        d.font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    d.families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    d.families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains-mono".to_owned());
    d.families
        .insert(FontFamily::Name(ICONS.into()), vec!["phosphor".to_owned()]);
    d.families.insert(
        FontFamily::Name(ICONS_FILL.into()),
        vec!["phosphor-fill".to_owned()],
    );
    d
}

/// Gives `ctx` the window's fonts.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_faces_come_first_and_egui_s_stay_behind() {
        let d = definitions();
        let prop = &d.families[&FontFamily::Proportional];
        assert_eq!(prop[0], "inter");
        assert!(prop.len() > 1, "egui's fallbacks kept");
        assert_eq!(d.families[&FontFamily::Monospace][0], "jetbrains-mono");
        assert_eq!(
            d.families[&FontFamily::Name(ICONS.into())],
            vec!["phosphor".to_owned()]
        );
    }

    /// Inter has the arrow change lines now write.
    #[test]
    fn the_text_face_has_an_arrow() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut out = ctx.run_pass(egui::RawInput::default(), |ctx| {
            let has = ctx.fonts_mut(|f| f.has_glyphs(&egui::FontId::proportional(13.0), "→"));
            assert!(has);
        });
        out.textures_delta.clear();
    }
}
