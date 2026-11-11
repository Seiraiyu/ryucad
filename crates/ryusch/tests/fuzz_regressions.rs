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

/// A connector turned a quarter by a move: its pins point up or down,
/// and adjacent pins' labels stood over each other (plan 2.G's sweep,
/// nine seeds; failing since phase 1). Then the same turned by a lasting
/// instruction (seed 143), and a part of seed 150's and 258's: the first
/// label turned off its pin covered its neighbours' way out, so they
/// now stand along their pins.
#[test]
fn a_part_turned_a_quarter_keeps_its_labels_apart() {
    check_bytes(&[91, 139, 109, 1, 79, 116, 67]);
    check_bytes(&[91, 139, 109, 178, 79, 116, 65]);
    check_bytes(&[91, 139, 109, 178, 79, 116, 67, 150, 39]);
    check_bytes(&[
        11, 143, 201, 98, 87, 11, 22, 37, 44, 182, 91, 133, 1, 94, 114, 241,
    ]);
    check_bytes(&[
        222, 209, 228, 83, 87, 44, 232, 233, 167, 25, 163, 24, 201, 2, 193, 39, 218, 234,
    ]);
}
