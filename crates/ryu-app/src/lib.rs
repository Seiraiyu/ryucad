//! The desktop shell every ryu product's window shares (design 519, The
//! app's home): fonts, icons, looks, keys, the command registry's types,
//! the palette's matching, the tile layout, toasts, people and the status
//! line's parts, the daemon link and the 2D canvas. Nothing here names a
//! product. A product's app fills in its commands, tiles, words and
//! requests through the traits here, and names the generic types for
//! itself (`ryucad_app::core::keys::Keymap` is `Keymap<Ryucad>`), so its
//! own code keeps its paths.

pub mod fonts;
