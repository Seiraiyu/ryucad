//! The sensor breakout's board (design 732, plan 5.D): the board writes
//! agent round `board-1` gave through the CLI, in order, as
//! `scripts/board-round/fixture.py` took them from its call log (undone
//! writes left out, every ID the round was answered pinned with `--id`).
//! Replayed after `breakout::script()`, the board is DRC clean and
//! exports. Change it by running a round again, not by hand.

#![allow(dead_code)]

/// The board's ID.
pub const BOARD: &str = "q7smdsgi6u";

/// The board's commands, in the order the round gave them.
pub fn script() -> Vec<Vec<String>> {
    let commands: &[&[&str]] = &[
        &[
            "board",
            "create",
            "--design",
            "board",
            "--rect",
            "20,15",
            "breakout",
            "--id",
            "q7smdsgi6u",
        ],
        &[
            "board",
            "edit",
            "q7smdsgi6u",
            "--outline",
            "{\"start\":[\"0\",\"0\"],\"edges\":[{\"type\":\"line\",\"to\":[\"16.5\",\"0\"]},{\"type\":\"line\",\"to\":[\"16.5\",\"11\"]},{\"type\":\"line\",\"to\":[\"0\",\"11\"]},{\"type\":\"line\",\"to\":[\"0\",\"0\"]}]}",
        ],
        &[
            "board",
            "place",
            "q7smdsgi6u",
            "--ops",
            "[{\"ref\":\"J1\",\"at\":[\"1.6\",\"5.5\"],\"rotation\":180},{\"ref\":\"U2\",\"at\":[\"7\",\"2\"]},{\"ref\":\"C2\",\"at\":[\"5\",\"4.3\"],\"rotation\":180},{\"ref\":\"C3\",\"at\":[\"9.7\",\"2.47\"],\"rotation\":270},{\"ref\":\"R2\",\"at\":[\"7\",\"4.4\"],\"rotation\":270},{\"ref\":\"U1\",\"at\":[\"12\",\"7.5\"]},{\"ref\":\"R1\",\"at\":[\"9\",\"7.825\"]},{\"ref\":\"C1\",\"at\":[\"14.98\",\"9.4\"]}]",
        ],
        &[
            "board",
            "place",
            "q7smdsgi6u",
            "--ops",
            "[{\"ref\":\"J1\",\"at\":[\"1.6\",\"5.5\"],\"rotation\":\"180\"}]",
        ],
        &[
            "board",
            "place",
            "q7smdsgi6u",
            "--ops",
            "[{\"ref\":\"C2\",\"at\":[\"5\",\"4.3\"],\"rotation\":\"180\"},{\"ref\":\"C3\",\"at\":[\"9.7\",\"2.47\"],\"rotation\":\"270\"},{\"ref\":\"R2\",\"at\":[\"7\",\"4.4\"],\"rotation\":\"270\"}]",
        ],
        &[
            "board",
            "place",
            "q7smdsgi6u",
            "R2",
            "--at",
            "7,5.4",
            "--rotation",
            "270",
        ],
        // Not the round's: R2's turned designator touched U2's, a
        // `silk_ref_overlap` warning since #1095, so it goes to R2's
        // other side and the board stays clean.
        &["board", "place", "q7smdsgi6u", "R2", "--ref-at", "0,-2.2"],
        &[
            "board",
            "via",
            "add",
            "q7smdsgi6u",
            "--ops",
            "[{\"net\":\"rail:gnd\",\"at\":[\"4.85\",\"2.0\"],\"id\":\"g_u2\"},{\"net\":\"rail:gnd\",\"at\":[\"9.7\",\"1.15\"],\"id\":\"g_c3\"},{\"net\":\"rail:gnd\",\"at\":[\"12.975\",\"5.6\"],\"id\":\"g_u1_5\"},{\"net\":\"rail:gnd\",\"at\":[\"13.75\",\"7.825\"],\"id\":\"g_u1_7\"},{\"net\":\"rail:gnd\",\"at\":[\"10.2\",\"8.6\"],\"id\":\"g_u1_1\"},{\"net\":\"rail:gnd\",\"at\":[\"15.46\",\"8.5\"],\"id\":\"g_c1\"}]",
        ],
        &[
            "board",
            "track",
            "add",
            "q7smdsgi6u",
            "--ops",
            "[{\"id\":\"vin_loop\",\"net\":\"rail:vin\",\"layer\":\"top\",\"path\":[\"U2/1\",[\"4.1\",\"2.95\"],[\"4.1\",\"1.05\"],\"U2/3\"]},{\"id\":\"vin_j1\",\"net\":\"rail:vin\",\"layer\":\"top\",\"path\":[\"J1/1\",[\"4.1\",\"1.69\"]]},{\"id\":\"vin_c2\",\"net\":\"rail:vin\",\"layer\":\"top\",\"path\":[\"C2/1\",\"U2/1\"]},{\"id\":\"gnd_c2\",\"net\":\"rail:gnd\",\"layer\":\"top\",\"path\":[\"J1/2\",\"C2/2\"]},{\"id\":\"gnd_u2\",\"net\":\"rail:gnd\",\"layer\":\"top\",\"path\":[\"U2/2\",\"via/g_u2\"]},{\"id\":\"gnd_c3\",\"net\":\"rail:gnd\",\"layer\":\"top\",\"path\":[\"C3/2\",\"via/g_c3\"]},{\"id\":\"gnd_u1_5\",\"net\":\"rail:gnd\",\"layer\":\"top\",\"path\":[\"U1/5\",\"via/g_u1_5\"]},{\"id\":\"gnd_u1_7\",\"net\":\"rail:gnd\",\"layer\":\"top\",\"path\":[\"U1/7\",\"via/g_u1_7\"]},{\"id\":\"gnd_u1_1\",\"net\":\"rail:gnd\",\"layer\":\"top\",\"path\":[\"U1/1\",\"via/g_u1_1\"]},{\"id\":\"gnd_c1\",\"net\":\"rail:gnd\",\"layer\":\"top\",\"path\":[\"C1/2\",\"via/g_c1\"]},{\"id\":\"gnd_bus\",\"net\":\"rail:gnd\",\"layer\":\"bottom\",\"path\":[\"J1/2\",\"via/g_u2\",\"via/g_c3\",[\"12.975\",\"1.15\"],\"via/g_u1_5\",\"via/g_u1_7\",\"via/g_c1\"]},{\"id\":\"gnd_bus_u1_1\",\"net\":\"rail:gnd\",\"layer\":\"bottom\",\"path\":[\"via/g_u1_5\",[\"10.2\",\"5.6\"],\"via/g_u1_1\"]},{\"id\":\"v33_main\",\"net\":\"rail:v3v3\",\"layer\":\"top\",\"path\":[\"U2/5\",\"C3/1\",[\"14.5\",\"2.95\"],[\"14.5\",\"9.9\"],[\"9.48\",\"9.9\"],\"R1/2\"]},{\"id\":\"v33_u1_8\",\"net\":\"rail:v3v3\",\"layer\":\"top\",\"path\":[\"U1/8\",[\"14.5\",\"8.475\"]]},{\"id\":\"v33_u1_6\",\"net\":\"rail:v3v3\",\"layer\":\"top\",\"path\":[\"U1/6\",[\"14.5\",\"7.175\"]]},{\"id\":\"v33_u1_2\",\"net\":\"rail:v3v3\",\"layer\":\"top\",\"path\":[\"U1/2\",\"R1/2\"]},{\"id\":\"v33_r2\",\"net\":\"rail:v3v3\",\"layer\":\"top\",\"path\":[\"R2/2\",[\"8.14\",\"4.92\"],\"U2/5\"]},{\"id\":\"sda\",\"net\":\"sda\",\"layer\":\"top\",\"path\":[\"U1/3\",[\"7.7\",\"7.175\"],[\"7.7\",\"9.31\"],\"J1/4\"]},{\"id\":\"sda_r1\",\"net\":\"sda\",\"layer\":\"top\",\"path\":[\"R1/1\",[\"7.7\",\"7.825\"]]},{\"id\":\"scl\",\"net\":\"scl\",\"layer\":\"top\",\"path\":[\"U1/4\",[\"4.0\",\"6.525\"],\"J1/3\"]},{\"id\":\"scl_r2\",\"net\":\"scl\",\"layer\":\"top\",\"path\":[\"R2/1\",[\"7.0\",\"6.525\"]]}]",
        ],
        &[
            "board",
            "track",
            "edit",
            "q7smdsgi6u",
            "gnd_bus",
            "--path",
            "J1/2",
            "4.85,4.23",
            "via/g_u2",
            "4.85,1.15",
            "via/g_c3",
            "via/g_u1_5",
        ],
        &[
            "board",
            "track",
            "edit",
            "q7smdsgi6u",
            "gnd_bus",
            "--ops",
            "{\"path\":[\"J1/2\",[\"4.85\",\"4.23\"],\"via/g_u2\",[\"4.85\",\"1.15\"],\"via/g_c3\",[\"12.975\",\"1.15\"],\"via/g_u1_5\",[\"13.75\",\"5.6\"],\"via/g_u1_7\",[\"15.46\",\"7.825\"],\"via/g_c1\"]}",
        ],
        &[
            "board",
            "edit",
            "q7smdsgi6u",
            "--outline",
            "{\"start\":[\"0\",\"0\"],\"edges\":[{\"type\":\"line\",\"to\":[\"16.3\",\"0\"]},{\"type\":\"line\",\"to\":[\"16.3\",\"10.7\"]},{\"type\":\"line\",\"to\":[\"0\",\"10.7\"]},{\"type\":\"line\",\"to\":[\"0\",\"0\"]}]}",
        ],
    ];
    commands
        .iter()
        .map(|c| c.iter().map(|s| (*s).to_owned()).collect())
        .collect()
}
