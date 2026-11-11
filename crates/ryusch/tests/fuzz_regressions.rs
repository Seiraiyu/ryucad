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

/// Regions and papers from plan 2.A's bytes (#665): a sheet pinned to
/// A4 by `paper` too small for what it holds (refused), a label stuck
/// where a neighbour's detour took its pin's way out, and an `align` of
/// parts of two named regions putting their frames over each other
/// (refused).
#[test]
fn regions_and_papers_keep_every_hard_rule() {
    check_bytes(&[
        162, 90, 73, 163, 246, 247, 15, 12, 25, 255, 45, 35, 124, 126, 117, 232, 14, 168, 24, 14,
        46, 3, 218, 175, 91, 52, 200, 159, 254, 226, 102, 75, 43, 238, 127, 36, 192, 66, 31, 105,
        160, 180, 10, 177, 60, 167, 41, 162, 21, 79, 220, 45, 192, 254,
    ]);
    check_bytes(&[
        154, 251, 160, 163, 219, 91, 248, 85, 79, 91, 122, 24, 120, 63, 226, 34, 20, 165, 96, 82,
        62, 99, 234, 49, 209, 138, 179, 53, 7, 50, 144, 106, 180, 79, 245, 57, 88, 34, 113, 213,
        238, 195, 138, 203, 205, 115, 198, 130, 108, 227, 47, 169, 93, 230,
    ]);
    check_bytes(&[
        154, 251, 160, 163, 219, 91, 248, 85, 79, 91, 122, 24, 120, 63, 226, 34, 20, 165, 96, 82,
        62, 99, 234, 49, 209, 138, 179, 53, 7, 50, 144, 106, 180, 79, 245, 57, 88, 34, 113, 213,
        238, 195, 138, 203, 205, 115, 198, 130, 108, 227, 47, 169, 93, 230, 45, 125, 114, 136,
    ]);
}

/// Two `region` instructions giving one title: over a previous layout
/// both regions took the second's parts (plan 2.G's sweep, seed 232).
#[test]
fn two_regions_of_one_title_keep_their_own_parts() {
    check_bytes(&[
        189, 95, 203, 238, 218, 215, 236, 107, 237, 165, 26, 166, 182, 163, 24, 40, 211, 35, 203,
        4, 16, 107, 28, 59, 12, 66, 58, 67, 186, 8, 150, 174, 237, 42, 196, 154, 40, 67, 203, 196,
        205, 7, 19, 111, 39, 138, 194, 248, 176, 176, 205, 142, 29, 240, 96, 179, 200, 245,
    ]);
}
