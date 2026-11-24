//! The window's tiles by id, with their tab titles, in the order View >
//! Panels lists them (design 938, The window). `ui::tiles` draws them.

pub const TILES: [(&str, &str); 11] = [
    ("navigator", "Navigator"),
    ("items", "Items"),
    ("item", "Item"),
    ("bom", "BOM"),
    ("where_used", "Where used"),
    ("findings", "Findings"),
    ("changes", "Changes"),
    ("releases", "Releases"),
    ("imports", "Imports"),
    ("documents", "Documents"),
    ("activity", "Activity"),
];
