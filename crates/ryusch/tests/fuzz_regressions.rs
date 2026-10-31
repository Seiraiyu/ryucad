//! What the `schematic` fuzz target found, replayed: each a layout that
//! broke a hard rule or moved what an edit didn't touch, fixed since.

use ryusch::testing::check_bytes;

/// A part asked onto `Sheet 1` while the rest flowed there too: laid out
/// twice over each other.
#[test]
fn a_named_sheet_flows_first() {
    check_bytes(&[255, 47, 255, 47]);
}

/// Labels on adjacent pins of a part alone on its sheet, at the margin.
#[test]
fn labels_fit_side_by_side_on_adjacent_pins() {
    check_bytes(&[253, 255, 91, 2]);
    check_bytes(&[251, 5, 162, 61]);
}

/// A capacitor held to the right edge rode in its IC's decoupling.
#[test]
fn an_edge_held_satellite_joins_no_idiom() {
    check_bytes(&[11, 61, 25, 11, 61, 25, 245, 245]);
}

/// An IC held to the top edge, its pull-up standing above it.
#[test]
fn an_edge_held_anchor_takes_no_idiom() {
    check_bytes(&[2, 0, 0, 99, 5, 5, 2, 2]);
}

/// A part beside one a move put on another sheet.
#[test]
fn beside_goes_onto_the_named_parts_sheet() {
    check_bytes(&[188, 1, 2, 99, 188, 1, 255, 255, 255, 255]);
}

/// An `align` that couldn't hold was laid out again on every edit.
#[test]
fn an_instruction_already_tried_is_left_as_it_lies() {
    check_bytes(&[255, 34, 34, 93, 1, 43, 0, 57, 57, 0]);
}
