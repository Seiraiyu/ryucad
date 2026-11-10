//! ISO 286 limits and fits: standard tolerance grades, and the limit
//! deviations of a tolerance class ("H7", "g6") at a nominal size.
//!
//! Every value is table data, never a formula. The tables are those of
//! ISO 286-1:1988 (Table 1, standard tolerances; Table 2, fundamental
//! deviations of shafts; Table 3, fundamental deviations of holes and the
//! values of Δ; Annex A Table 5, IT01 and IT0), as reproduced word for word by
//! IS 919 (Part 1):1993, and checked against ISO 286-2:1988's limit deviation
//! tables (IS 919 (Part 2):1993). The tolerance grades were checked against
//! Wikipedia's "IT Grade" table (which reproduces ISO 286-1 Table 1) and the
//! `saucecode/ISO286` tables; A to H and Δ against `saucecode/ISO286` too.
//!
//! Where ISO 286-2 prints a class differently from ISO 286-1's rules, ISO
//! 286-2 is followed:
//! - M6 from 250 to 315 mm has ES = −9 µm (ISO 286-1 Table 3, note 4).
//! - K above IT8 is defined only up to 3 mm (ISO 286-2 Table 8, note 2),
//!   although ISO 286-1 Table 3 prints K = 0 for every grade above 500 mm.
//! - Grades IT17 and IT18 have no limit deviations up to 3 mm (ISO 286-2
//!   Tables 6, 7 and 22 leave them blank), although ISO 286-1 Table 1 gives
//!   IT17 = 1 mm and IT18 = 1.4 mm there; `it` still gives those.
//! - js and JS are exactly ±IT/2 as ISO 286-2 Table 7 prints them (JS7 from 6
//!   to 10 mm is ±7.5 µm). ISO 286-1's note that js7 to js11 *may* be rounded
//!   down to whole micrometres is not applied.
//!
//! All values are integers in tenths of a micrometre, so results are the same
//! on every platform. Sizes are integers in micrometres; a size on a step's
//! boundary belongs to the lower step ("over A up to and including B").

/// Which member a class tolerances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Member {
    Hole,
    Shaft,
}

/// A standard tolerance grade: IT01, IT0, or IT1 to IT18.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grade {
    It01,
    It0,
    /// IT1 to IT18.
    It(u8),
}

impl Grade {
    /// The column of the tolerance table: IT01 is 0, IT0 is 1, ITn is n + 1.
    fn column(self) -> Option<usize> {
        match self {
            Grade::It01 => Some(0),
            Grade::It0 => Some(1),
            Grade::It(n @ 1..=18) => Some(usize::from(n) + 1),
            Grade::It(_) => None,
        }
    }

    /// The grade as a number for comparisons: IT01 is −1, IT0 is 0.
    fn number(self) -> i32 {
        match self {
            Grade::It01 => -1,
            Grade::It0 => 0,
            Grade::It(n) => i32::from(n),
        }
    }

    fn text(self) -> String {
        match self {
            Grade::It01 => "01".to_string(),
            Grade::It0 => "0".to_string(),
            Grade::It(n) => n.to_string(),
        }
    }
}

/// A tolerance class: "H7", "g6", "JS9", "zc10". "IT01" is not a class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Class {
    pub member: Member,
    /// The fundamental deviation's letters as written: upper case for a hole,
    /// lower case for a shaft.
    pub letters: String,
    pub grade: Grade,
}

/// The fundamental deviations' letters, in lower case.
const LETTERS: [&str; 28] = [
    "a", "b", "c", "cd", "d", "e", "ef", "f", "fg", "g", "h", "js", "j", "k", "m", "n", "p", "r",
    "s", "t", "u", "v", "x", "y", "z", "za", "zb", "zc",
];

impl Class {
    /// Reads a class such as "H7" or "zc10": the letters, all upper case for
    /// a hole or all lower case for a shaft, then a grade from 01 and 0 to 18.
    pub fn parse(text: &str) -> Option<Class> {
        let split = text.find(|c: char| c.is_ascii_digit())?;
        let (letters, digits) = text.split_at(split);
        let member = if !letters.is_empty() && letters.bytes().all(|b| b.is_ascii_uppercase()) {
            Member::Hole
        } else if !letters.is_empty() && letters.bytes().all(|b| b.is_ascii_lowercase()) {
            Member::Shaft
        } else {
            return None;
        };
        if !LETTERS.contains(&letters.to_ascii_lowercase().as_str()) {
            return None;
        }
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let grade = match digits {
            "01" => Grade::It01,
            "0" => Grade::It0,
            _ if digits.starts_with('0') || digits.len() > 2 => return None,
            _ => match digits.parse::<u8>() {
                Ok(n @ 1..=18) => Grade::It(n),
                _ => return None,
            },
        };
        Some(Class {
            member,
            letters: letters.to_string(),
            grade,
        })
    }

    /// The class as written: "H7".
    pub fn text(&self) -> String {
        format!("{}{}", self.letters, self.grade.text())
    }
}

/// "H7" or "H7/g6" (a hole's class, then a shaft's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fit {
    One(Class),
    Pair(Class, Class),
}

impl Fit {
    /// Reads one class, or a hole's and a shaft's separated by "/".
    pub fn parse(text: &str) -> Option<Fit> {
        match text.split_once('/') {
            None => Class::parse(text.trim()).map(Fit::One),
            Some((hole, shaft)) => {
                let hole = Class::parse(hole.trim())?;
                let shaft = Class::parse(shaft.trim())?;
                (hole.member == Member::Hole && shaft.member == Member::Shaft)
                    .then_some(Fit::Pair(hole, shaft))
            }
        }
    }
}

/// The main size steps' upper ends in millimetres (ISO 286-1 Table 1).
const MAIN_MM: [u64; 21] = [
    3, 6, 10, 18, 30, 50, 80, 120, 180, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600, 2000, 2500,
    3150,
];

/// The intermediate size steps' upper ends in millimetres (ISO 286-1
/// Tables 2 and 3).
const FINE_MM: [u64; 41] = [
    3, 6, 10, 14, 18, 24, 30, 40, 50, 65, 80, 100, 120, 140, 160, 180, 200, 225, 250, 280, 315,
    355, 400, 450, 500, 560, 630, 710, 800, 900, 1000, 1120, 1250, 1400, 1600, 1800, 2000, 2240,
    2500, 2800, 3150,
];

/// The main step 400 to 500 mm: steps up to it are "up to 500 mm".
const MAIN_500: usize = 12;

/// The step holding a size, or None for 0 or over 3150 mm.
fn step_of(size_um: u64, bounds: &[u64]) -> Option<usize> {
    if size_um == 0 {
        return None;
    }
    bounds.iter().position(|&b| size_um <= b * 1000)
}

/// Standard tolerances in tenths of a micrometre, by main step, columns IT01,
/// IT0, IT1 … IT18; 0 where the tables give none (IT01 and IT0 over 500 mm).
/// ISO 286-1:1988 Table 1 and Annex A Table 5; Wikipedia "IT Grade" agrees in
/// every cell; `saucecode/ISO286` agrees in every cell but IT16 from 2000 to
/// 2500 mm, where it has 1.1 mm for 11 mm (a typing slip).
#[rustfmt::skip]
const IT: [[u32; 20]; 21] = [
    [3, 5, 8, 12, 20, 30, 40, 60, 100, 140, 250, 400, 600, 1000, 1400, 2500, 4000, 6000, 10000, 14000], // 0-3
    [4, 6, 10, 15, 25, 40, 50, 80, 120, 180, 300, 480, 750, 1200, 1800, 3000, 4800, 7500, 12000, 18000], // 3-6
    [4, 6, 10, 15, 25, 40, 60, 90, 150, 220, 360, 580, 900, 1500, 2200, 3600, 5800, 9000, 15000, 22000], // 6-10
    [5, 8, 12, 20, 30, 50, 80, 110, 180, 270, 430, 700, 1100, 1800, 2700, 4300, 7000, 11000, 18000, 27000], // 10-18
    [6, 10, 15, 25, 40, 60, 90, 130, 210, 330, 520, 840, 1300, 2100, 3300, 5200, 8400, 13000, 21000, 33000], // 18-30
    [6, 10, 15, 25, 40, 70, 110, 160, 250, 390, 620, 1000, 1600, 2500, 3900, 6200, 10000, 16000, 25000, 39000], // 30-50
    [8, 12, 20, 30, 50, 80, 130, 190, 300, 460, 740, 1200, 1900, 3000, 4600, 7400, 12000, 19000, 30000, 46000], // 50-80
    [10, 15, 25, 40, 60, 100, 150, 220, 350, 540, 870, 1400, 2200, 3500, 5400, 8700, 14000, 22000, 35000, 54000], // 80-120
    [12, 20, 35, 50, 80, 120, 180, 250, 400, 630, 1000, 1600, 2500, 4000, 6300, 10000, 16000, 25000, 40000, 63000], // 120-180
    [20, 30, 45, 70, 100, 140, 200, 290, 460, 720, 1150, 1850, 2900, 4600, 7200, 11500, 18500, 29000, 46000, 72000], // 180-250
    [25, 40, 60, 80, 120, 160, 230, 320, 520, 810, 1300, 2100, 3200, 5200, 8100, 13000, 21000, 32000, 52000, 81000], // 250-315
    [30, 50, 70, 90, 130, 180, 250, 360, 570, 890, 1400, 2300, 3600, 5700, 8900, 14000, 23000, 36000, 57000, 89000], // 315-400
    [40, 60, 80, 100, 150, 200, 270, 400, 630, 970, 1550, 2500, 4000, 6300, 9700, 15500, 25000, 40000, 63000, 97000], // 400-500
    [0, 0, 90, 110, 160, 220, 320, 440, 700, 1100, 1750, 2800, 4400, 7000, 11000, 17500, 28000, 44000, 70000, 110000], // 500-630
    [0, 0, 100, 130, 180, 250, 360, 500, 800, 1250, 2000, 3200, 5000, 8000, 12500, 20000, 32000, 50000, 80000, 125000], // 630-800
    [0, 0, 110, 150, 210, 280, 400, 560, 900, 1400, 2300, 3600, 5600, 9000, 14000, 23000, 36000, 56000, 90000, 140000], // 800-1000
    [0, 0, 130, 180, 240, 330, 470, 660, 1050, 1650, 2600, 4200, 6600, 10500, 16500, 26000, 42000, 66000, 105000, 165000], // 1000-1250
    [0, 0, 150, 210, 290, 390, 550, 780, 1250, 1950, 3100, 5000, 7800, 12500, 19500, 31000, 50000, 78000, 125000, 195000], // 1250-1600
    [0, 0, 180, 250, 350, 460, 650, 920, 1500, 2300, 3700, 6000, 9200, 15000, 23000, 37000, 60000, 92000, 150000, 230000], // 1600-2000
    [0, 0, 220, 300, 410, 550, 780, 1100, 1750, 2800, 4400, 7000, 11000, 17500, 28000, 44000, 70000, 110000, 175000, 280000], // 2000-2500
    [0, 0, 260, 360, 500, 680, 960, 1350, 2100, 3300, 5400, 8600, 13500, 21000, 33000, 54000, 86000, 135000, 210000, 330000], // 2500-3150
];

/// The standard tolerance in tenths of a micrometre, or None outside the
/// tables. IT14 to IT18 aren't used up to 1 mm (ISO 286-1 Table 1, note 3).
pub fn it(grade: Grade, size_um: u64) -> Option<u64> {
    let step = step_of(size_um, &MAIN_MM)?;
    let column = grade.column()?;
    if grade.number() >= 14 && size_um <= 1000 {
        return None;
    }
    match IT[step][column] {
        0 => None,
        t => Some(u64::from(t)),
    }
}

/// Not defined in the tables.
const NA: i32 = i32::MIN;

/// Shafts' upper deviations es in µm by intermediate step (ISO 286-1 Table
/// 2), letters a to h; holes A to H are their negatives (ISO 286-1 Table 3
/// prints the same numbers). a, b and c stop at 500 mm (ISO 286-2 Table 2,
/// note 1; `saucecode/ISO286` gives C over 500 mm from some other source,
/// not followed); cd, ef and fg stop at 10 mm.
#[rustfmt::skip]
const UPPER: [(&str, [i32; 41]); 11] = [
    ("a", [-270, -270, -280, -290, -290, -300, -300, -310, -320, -340, -360, -380, -410, -460, -520, -580, -660, -740, -820, -920, -1050, -1200, -1350, -1500, -1650, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("b", [-140, -140, -150, -150, -150, -160, -160, -170, -180, -190, -200, -220, -240, -260, -280, -310, -340, -380, -420, -480, -540, -600, -680, -760, -840, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("c", [-60, -70, -80, -95, -95, -110, -110, -120, -130, -140, -150, -170, -180, -200, -210, -230, -240, -260, -280, -300, -330, -360, -400, -440, -480, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("cd", [-34, -46, -56, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("d", [-20, -30, -40, -50, -50, -65, -65, -80, -80, -100, -100, -120, -120, -145, -145, -145, -170, -170, -170, -190, -190, -210, -210, -230, -230, -260, -260, -290, -290, -320, -320, -350, -350, -390, -390, -430, -430, -480, -480, -520, -520]),
    ("e", [-14, -20, -25, -32, -32, -40, -40, -50, -50, -60, -60, -72, -72, -85, -85, -85, -100, -100, -100, -110, -110, -125, -125, -135, -135, -145, -145, -160, -160, -170, -170, -195, -195, -220, -220, -240, -240, -260, -260, -290, -290]),
    ("ef", [-10, -14, -18, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("f", [-6, -10, -13, -16, -16, -20, -20, -25, -25, -30, -30, -36, -36, -43, -43, -43, -50, -50, -50, -56, -56, -62, -62, -68, -68, -76, -76, -80, -80, -86, -86, -98, -98, -110, -110, -120, -120, -130, -130, -145, -145]),
    ("fg", [-4, -6, -8, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("g", [-2, -4, -5, -6, -6, -7, -7, -9, -9, -10, -10, -12, -12, -14, -14, -14, -15, -15, -15, -17, -17, -18, -18, -20, -20, -22, -22, -24, -24, -26, -26, -28, -28, -30, -30, -32, -32, -34, -34, -38, -38]),
    ("h", [0; 41]),
];

/// Shafts' lower deviations ei in µm by intermediate step (ISO 286-1 Table
/// 2), letters m to zc; holes P to ZC above IT7 are their negatives (ISO
/// 286-1 Table 3). t isn't tabulated up to 24 mm (ISO 286-2 Table 29, note
/// 1), v up to 14 mm, y up to 18 mm; v to zc stop at 500 mm.
#[rustfmt::skip]
const LOWER: [(&str, [i32; 41]); 14] = [
    ("m", [2, 4, 6, 7, 7, 8, 8, 9, 9, 11, 11, 13, 13, 15, 15, 15, 17, 17, 17, 20, 20, 21, 21, 23, 23, 26, 26, 30, 30, 34, 34, 40, 40, 48, 48, 58, 58, 68, 68, 76, 76]),
    ("n", [4, 8, 10, 12, 12, 15, 15, 17, 17, 20, 20, 23, 23, 27, 27, 27, 31, 31, 31, 34, 34, 37, 37, 40, 40, 44, 44, 50, 50, 56, 56, 66, 66, 78, 78, 92, 92, 110, 110, 135, 135]),
    ("p", [6, 12, 15, 18, 18, 22, 22, 26, 26, 32, 32, 37, 37, 43, 43, 43, 50, 50, 50, 56, 56, 62, 62, 68, 68, 78, 78, 88, 88, 100, 100, 120, 120, 140, 140, 170, 170, 195, 195, 240, 240]),
    ("r", [10, 15, 19, 23, 23, 28, 28, 34, 34, 41, 43, 51, 54, 63, 65, 68, 77, 80, 84, 94, 98, 108, 114, 126, 132, 150, 155, 175, 185, 210, 220, 250, 260, 300, 330, 370, 400, 440, 460, 550, 580]),
    ("s", [14, 19, 23, 28, 28, 35, 35, 43, 43, 53, 59, 71, 79, 92, 100, 108, 122, 130, 140, 158, 170, 190, 208, 232, 252, 280, 310, 340, 380, 430, 470, 520, 580, 640, 720, 820, 920, 1000, 1100, 1250, 1400]),
    ("t", [NA, NA, NA, NA, NA, NA, 41, 48, 54, 66, 75, 91, 104, 122, 134, 146, 166, 180, 196, 218, 240, 268, 294, 330, 360, 400, 450, 500, 560, 620, 680, 780, 840, 960, 1050, 1200, 1350, 1500, 1650, 1900, 2100]),
    ("u", [18, 23, 28, 33, 33, 41, 48, 60, 70, 87, 102, 124, 144, 170, 190, 210, 236, 258, 284, 315, 350, 390, 435, 490, 540, 600, 660, 740, 840, 940, 1050, 1150, 1300, 1450, 1600, 1850, 2000, 2300, 2500, 2900, 3200]),
    ("v", [NA, NA, NA, NA, 39, 47, 55, 68, 81, 102, 120, 146, 172, 202, 228, 252, 284, 310, 340, 385, 425, 475, 530, 595, 660, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("x", [20, 28, 34, 40, 45, 54, 64, 80, 97, 122, 146, 178, 210, 248, 280, 310, 350, 385, 425, 475, 525, 590, 660, 740, 820, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("y", [NA, NA, NA, NA, NA, 63, 75, 94, 114, 144, 174, 214, 254, 300, 340, 380, 425, 470, 520, 580, 650, 730, 820, 920, 1000, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("z", [26, 35, 42, 50, 60, 73, 88, 112, 136, 172, 210, 258, 310, 365, 415, 465, 520, 575, 640, 710, 790, 900, 1000, 1100, 1250, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("za", [32, 42, 52, 64, 77, 98, 118, 148, 180, 226, 274, 335, 400, 470, 535, 600, 670, 740, 820, 920, 1000, 1150, 1300, 1450, 1600, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("zb", [40, 50, 67, 90, 108, 136, 160, 200, 242, 300, 360, 445, 525, 620, 700, 780, 880, 960, 1050, 1200, 1300, 1500, 1650, 1850, 2100, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
    ("zc", [60, 80, 97, 130, 150, 188, 218, 274, 325, 405, 480, 585, 690, 800, 900, 1000, 1150, 1250, 1350, 1550, 1700, 1900, 2100, 2400, 2600, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
];

// The tables below run by main step up to 500 mm (13 steps).

/// Shaft j's lower deviation ei in µm for IT5 and IT6, IT7 and IT8 (ISO
/// 286-1 Table 2); j8 only up to 3 mm, none over 500 mm.
#[rustfmt::skip]
const J_SHAFT: [(u8, [i32; 13]); 4] = [
    (5, [-2, -2, -2, -3, -4, -5, -7, -9, -11, -13, -16, -18, -20]),
    (6, [-2, -2, -2, -3, -4, -5, -7, -9, -11, -13, -16, -18, -20]),
    (7, [-4, -4, -5, -6, -8, -10, -12, -15, -18, -21, -26, -28, -32]),
    (8, [-6, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA, NA]),
];

/// Shaft k's lower deviation ei in µm for IT4 to IT7 (ISO 286-1 Table 2);
/// 0 for every other grade, and over 500 mm.
const K_SHAFT: [i32; 13] = [0, 1, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4, 5];

/// Hole J's upper deviation ES in µm for IT6, IT7 and IT8 (ISO 286-1 Table
/// 3, the same as ISO 286-2 Table 8); none over 500 mm.
#[rustfmt::skip]
const J_HOLE: [(u8, [i32; 13]); 3] = [
    (6, [2, 5, 5, 6, 8, 10, 13, 16, 18, 22, 25, 29, 33]),
    (7, [4, 6, 8, 10, 12, 14, 18, 22, 26, 30, 36, 39, 43]),
    (8, [6, 10, 12, 15, 20, 24, 28, 34, 41, 47, 55, 60, 66]),
];

/// Holes K, M and N up to IT8: ES = this (µm) + Δ (ISO 286-1 Table 3).
/// The first step needs no Δ (its Δ is 0). M above IT8 is the same number
/// without Δ; K and N above IT8 are 0 there (but up to 3 mm, N is −4).
const K_HOLE: [i32; 13] = [0, -1, -1, -1, -2, -2, -2, -3, -3, -4, -4, -4, -5];
const M_HOLE: [i32; 13] = [-2, -4, -6, -7, -8, -9, -11, -13, -15, -17, -20, -21, -23];
const N_HOLE: [i32; 13] = [
    -4, -8, -10, -12, -15, -17, -20, -23, -27, -31, -34, -37, -40,
];

/// Holes M and N over 500 mm, every grade, by main step from 500 to 3150 mm
/// (ISO 286-1 Table 3; ISO 286-2 Table 9 agrees for M6 to M8 and N6 to N9).
const M_BIG: [i32; 8] = [-26, -30, -34, -40, -48, -58, -68, -76];
const N_BIG: [i32; 8] = [-44, -50, -56, -66, -78, -92, -110, -135];

/// Δ in tenths of a micrometre for IT3 to IT8, by main step up to 500 mm
/// (ISO 286-1 Table 3; `saucecode/ISO286` agrees in every cell).
#[rustfmt::skip]
const DELTA: [[i64; 6]; 13] = [
    [0, 0, 0, 0, 0, 0],          // 0-3
    [10, 15, 10, 30, 40, 60],    // 3-6
    [10, 15, 20, 30, 60, 70],    // 6-10
    [10, 20, 30, 30, 70, 90],    // 10-18
    [15, 20, 30, 40, 80, 120],   // 18-30
    [15, 30, 40, 50, 90, 140],   // 30-50
    [20, 30, 50, 60, 110, 160],  // 50-80
    [20, 40, 50, 70, 130, 190],  // 80-120
    [30, 40, 60, 70, 150, 230],  // 120-180
    [30, 40, 60, 90, 170, 260],  // 180-250
    [40, 40, 70, 90, 200, 290],  // 250-315
    [40, 50, 70, 110, 210, 320], // 315-400
    [50, 50, 70, 130, 230, 340], // 400-500
];

/// Δ for a grade at a main step up to 500 mm, or None for a grade the table
/// has no column for (below IT3 or above IT8).
fn delta(main: usize, grade: i32) -> Option<i64> {
    let column = usize::try_from(grade - 3).ok()?;
    DELTA.get(main)?.get(column).copied()
}

/// A µm table value in tenths, or None where it is undefined.
fn tenths(value: i32) -> Option<i64> {
    (value != NA).then_some(i64::from(value) * 10)
}

fn row<'a, const N: usize>(table: &'a [(&str, [i32; N])], letters: &str) -> Option<&'a [i32; N]> {
    table.iter().find(|(l, _)| *l == letters).map(|(_, r)| r)
}

/// ±IT/2, or None where that isn't a whole number of tenths (JS1 to JS3 at
/// some sizes, and JS01 and JS0, which ISO 286-2 doesn't tabulate).
fn symmetric(tol: i64) -> Option<(i64, i64)> {
    (tol % 2 == 0).then_some((tol / 2, -tol / 2))
}

/// A class's upper and lower deviations in tenths of a micrometre at a
/// nominal size given in micrometres, or None where the standard doesn't
/// define it (size 0 or over 3150 mm, or a class undefined at that size and
/// grade).
pub fn deviations(class: &Class, size_um: u64) -> Option<(i64, i64)> {
    let tol = i64::try_from(it(class.grade, size_um)?).ok()?;
    let grade = class.grade.number();
    // ISO 286-2 leaves IT17 and IT18 blank up to 3 mm.
    if grade >= 17 && size_um <= 3000 {
        return None;
    }
    let fine = step_of(size_um, &FINE_MM)?;
    let main = step_of(size_um, &MAIN_MM)?;
    let letters = class.letters.to_ascii_lowercase();
    // a, b, A and B aren't used up to 1 mm (ISO 286-1 Tables 2 and 3).
    if (letters == "a" || letters == "b") && size_um <= 1000 {
        return None;
    }
    match class.member {
        Member::Shaft => shaft(&letters, grade, tol, fine, main),
        Member::Hole => hole(&letters, grade, tol, fine, main, size_um),
    }
}

fn shaft(letters: &str, grade: i32, tol: i64, fine: usize, main: usize) -> Option<(i64, i64)> {
    match letters {
        "js" => symmetric(tol),
        "j" => {
            let r = J_SHAFT.iter().find(|(g, _)| i32::from(*g) == grade)?;
            let ei = tenths(*r.1.get(main)?)?;
            Some((ei + tol, ei))
        }
        "k" => {
            let ei = match K_SHAFT.get(main) {
                Some(&k) if (4..=7).contains(&grade) => i64::from(k) * 10,
                _ => 0,
            };
            Some((ei + tol, ei))
        }
        _ => {
            if let Some(r) = row(&UPPER, letters) {
                let es = tenths(r[fine])?;
                Some((es, es - tol))
            } else {
                let ei = tenths(row(&LOWER, letters)?[fine])?;
                Some((ei + tol, ei))
            }
        }
    }
}

fn hole(
    letters: &str,
    grade: i32,
    tol: i64,
    fine: usize,
    main: usize,
    size_um: u64,
) -> Option<(i64, i64)> {
    let es = match letters {
        "js" => return symmetric(tol),
        "j" => {
            let r = J_HOLE.iter().find(|(g, _)| i32::from(*g) == grade)?;
            tenths(*r.1.get(main)?)?
        }
        "k" => {
            if main == 0 {
                0
            } else if grade > 8 {
                // ISO 286-2 Table 8, note 2.
                return None;
            } else if main <= MAIN_500 {
                i64::from(K_HOLE[main]) * 10 + delta(main, grade)?
            } else {
                0
            }
        }
        "m" => {
            if main == 0 {
                -20
            } else if main > MAIN_500 {
                i64::from(M_BIG[main - MAIN_500 - 1]) * 10
            } else if grade > 8 {
                i64::from(M_HOLE[main]) * 10
            } else if grade == 6 && main == 10 {
                // ISO 286-1 Table 3, note 4: M6 from 250 to 315 mm.
                -90
            } else {
                i64::from(M_HOLE[main]) * 10 + delta(main, grade)?
            }
        }
        "n" => {
            // ISO 286-1 Table 3, note 5: N above IT8 isn't used up to 1 mm.
            if grade > 8 && size_um <= 1000 {
                return None;
            }
            if main == 0 {
                -40
            } else if main > MAIN_500 {
                i64::from(N_BIG[main - MAIN_500 - 1]) * 10
            } else if grade > 8 {
                0
            } else {
                i64::from(N_HOLE[main]) * 10 + delta(main, grade)?
            }
        }
        _ => {
            if let Some(r) = row(&UPPER, letters) {
                let ei = -tenths(r[fine])?;
                return Some((ei + tol, ei));
            }
            let base = -tenths(row(&LOWER, letters)?[fine])?;
            if grade <= 7 && (1..=MAIN_500).contains(&main) {
                base + delta(main, grade)?
            } else {
                base
            }
        }
    };
    Some((es, es - tol))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(text: &str, size_um: u64) -> Option<(i64, i64)> {
        deviations(&Class::parse(text).expect("a class"), size_um)
    }

    /// Each main step's upper end in micrometres.
    fn ends() -> impl Iterator<Item = u64> {
        MAIN_MM.iter().map(|mm| mm * 1000)
    }

    #[test]
    fn tolerances_at_every_main_step() {
        // ISO 286-1:1988 Table 1 (IS 919 Part 1 p. 11), in µm; Wikipedia
        // "IT Grade" prints the same.
        let it6 = [
            6, 8, 9, 11, 13, 16, 19, 22, 25, 29, 32, 36, 40, 44, 50, 56, 66, 78, 92, 110, 135,
        ];
        let it7 = [
            10, 12, 15, 18, 21, 25, 30, 35, 40, 46, 52, 57, 63, 70, 80, 90, 105, 125, 150, 175, 210,
        ];
        let it8 = [
            14, 18, 22, 27, 33, 39, 46, 54, 63, 72, 81, 89, 97, 110, 125, 140, 165, 195, 230, 280,
            330,
        ];
        let it11 = [
            60, 75, 90, 110, 130, 160, 190, 220, 250, 290, 320, 360, 400, 440, 500, 560, 660, 780,
            920, 1100, 1350,
        ];
        for (i, size) in ends().enumerate() {
            assert_eq!(it(Grade::It(6), size), Some(it6[i] * 10), "IT6 at {size}");
            assert_eq!(it(Grade::It(7), size), Some(it7[i] * 10), "IT7 at {size}");
            assert_eq!(it(Grade::It(8), size), Some(it8[i] * 10), "IT8 at {size}");
            assert_eq!(
                it(Grade::It(11), size),
                Some(it11[i] * 10),
                "IT11 at {size}"
            );
        }
    }

    #[test]
    fn tolerances_at_the_tables_edges() {
        // ISO 286-1 Annex A Table 5: IT01 at 0-3 mm is 0.3 µm; none over 500 mm.
        assert_eq!(it(Grade::It01, 2000), Some(3));
        assert_eq!(it(Grade::It0, 500_000), Some(60));
        assert_eq!(it(Grade::It01, 500_001), None);
        // ISO 286-1 Table 1: IT1 from 500 to 630 mm is 9 µm (experimental).
        assert_eq!(it(Grade::It(1), 600_000), Some(90));
        // Table 1: IT18 at 1-3 mm is 1.4 mm; IT14 to IT18 not used up to 1 mm.
        assert_eq!(it(Grade::It(18), 2000), Some(14_000));
        assert_eq!(it(Grade::It(14), 1000), None);
        assert_eq!(it(Grade::It(13), 1000), Some(1400));
        // Table 1: IT16 from 2000 to 2500 mm is 11 mm.
        assert_eq!(it(Grade::It(16), 2_400_000), Some(110_000));
        assert_eq!(it(Grade::It(7), 0), None);
    }

    #[test]
    fn a_size_on_a_boundary_belongs_to_the_lower_step() {
        // 10 mm is in 6-10 mm: IT7 15 µm (Table 1), f7 -13/-28 (ISO 286-1
        // Table 2, f in 6-10 is -13 µm).
        assert_eq!(it(Grade::It(7), 10_000), Some(150));
        assert_eq!(it(Grade::It(7), 10_001), Some(180));
        assert_eq!(dev("f7", 10_000), Some((-130, -280)));
        assert_eq!(dev("f7", 10_001), Some((-160, -340)));
    }

    #[test]
    fn the_tables_end_at_3150_mm() {
        assert_eq!(it(Grade::It(7), 3_150_000), Some(2100));
        assert_eq!(it(Grade::It(7), 3_150_001), None);
        assert_eq!(dev("H7", 3_150_000), Some((2100, 0)));
        assert_eq!(dev("H7", 3_151_000), None);
        assert_eq!(dev("H7", 0), None);
    }

    #[test]
    fn deviations_of_shafts() {
        // ISO 286-2 Table 21 (g): g6 in 18-30 is -7/-20.
        assert_eq!(dev("g6", 25_000), Some((-70, -200)));
        // ISO 286-1 Table 2: p in 30-50 is +26, IT6 16.
        assert_eq!(dev("p6", 50_000), Some((420, 260)));
        // Table 2: s in 80-100 is +71, IT6 22.
        assert_eq!(dev("s6", 100_000), Some((930, 710)));
        // ISO 286-2 Table 24: js6 is ±IT6/2; IT6 in 10-18 is 11.
        assert_eq!(dev("js6", 18_000), Some((55, -55)));
        // ISO 286-2 Table 24: j6 in 18-30 is +9/-4.
        assert_eq!(dev("j6", 30_000), Some((90, -40)));
        // ISO 286-2 Table 24: j5 up to 3 mm is ±2.
        assert_eq!(dev("j5", 2000), Some((20, -20)));
        // ISO 286-2 Table 24: j7 in 315-400 is +29/-28.
        assert_eq!(dev("j7", 400_000), Some((290, -280)));
        // ISO 286-2 Table 24: j8 only up to 3 mm; no j9.
        assert_eq!(dev("j8", 2000), Some((80, -60)));
        assert_eq!(dev("j8", 5000), None);
        assert_eq!(dev("j9", 2000), None);
        // ISO 286-2 Table 24: k6 in 120-180 is +28/+3; over 500 mm +44/0.
        assert_eq!(dev("k6", 150_000), Some((280, 30)));
        assert_eq!(dev("k6", 600_000), Some((440, 0)));
        // ISO 286-2 Table 24: k3 in 3-6 is +2.5/0, k8 there +18/0.
        assert_eq!(dev("k3", 5000), Some((25, 0)));
        assert_eq!(dev("k8", 5000), Some((180, 0)));
        // ISO 286-2 Table 29: u6 in 100-120 is +166/+144.
        assert_eq!(dev("u6", 120_000), Some((1660, 1440)));
        // ISO 286-2 Table 29: u7 in 500-560 is +670/+600.
        assert_eq!(dev("u7", 600_000).map(|d| d.1), Some(6600));
        assert_eq!(dev("u7", 550_000), Some((6700, 6000)));
        // ISO 286-2 Table 29: t starts above 24 mm; t6 in 24-30 is +54/+41.
        assert_eq!(dev("t6", 20_000), None);
        assert_eq!(dev("t6", 25_000), Some((540, 410)));
        // ISO 286-1 Table 2: a in 6-10 is -280, IT11 90; a not up to 1 mm.
        assert_eq!(dev("a11", 10_000), Some((-2800, -3700)));
        assert_eq!(dev("a9", 1000), None);
        assert_eq!(dev("a9", 2000), Some((-2700, -2950)));
        // Table 2: c in 30-40 is -120, IT11 160.
        assert_eq!(dev("c11", 40_000), Some((-1200, -2800)));
        // Table 2: cd only up to 10 mm (-46 in 3-6).
        assert_eq!(dev("cd6", 5000), Some((-460, -540)));
        assert_eq!(dev("cd6", 12_000), None);
        // Table 2: v starts above 14 mm (+39 in 14-18), y above 18 mm.
        assert_eq!(dev("v6", 12_000), None);
        assert_eq!(dev("v6", 16_000), Some((500, 390)));
        assert_eq!(dev("y6", 16_000), None);
        assert_eq!(dev("y6", 20_000), Some((760, 630)));
        assert_eq!(dev("y6", 25_000), Some((880, 750)));
        // Table 2: x up to 3 mm is +20; zc in 120-140 is +800.
        assert_eq!(dev("x6", 2000), Some((260, 200)));
        assert_eq!(dev("zc7", 130_000), Some((8400, 8000)));
        assert_eq!(dev("zc7", 600_000), None);
        // ISO 286-2 Table 22: h6 in 18-30.
        assert_eq!(dev("h6", 25_000), Some((0, -130)));
        // js1 in 18-30 would be ±0.75 µm: not whole tenths.
        assert_eq!(dev("js1", 20_000), None);
    }

    #[test]
    fn deviations_of_holes() {
        // ISO 286-2 Table 6: H7 in 18-30 is +21/0; H17 is blank up to 3 mm.
        assert_eq!(dev("H7", 25_000), Some((210, 0)));
        assert_eq!(dev("H17", 2000), None);
        assert_eq!(dev("H17", 5000), Some((12_000, 0)));
        // ISO 286-1 Table 3: D in 30-50 is +80, IT10 100.
        assert_eq!(dev("D10", 50_000), Some((1800, 800)));
        // Table 3: E in 18-30 is +40, IT9 52; F +20, IT8 33; G +7, IT7 21.
        assert_eq!(dev("E9", 25_000), Some((920, 400)));
        assert_eq!(dev("F8", 25_000), Some((530, 200)));
        assert_eq!(dev("G7", 25_000), Some((280, 70)));
        // ISO 286-2 Table 2: A, B, C not over 500 mm.
        assert_eq!(dev("C11", 600_000), None);
        assert_eq!(dev("A11", 1000), None);
        // ISO 286-2 Table 7: JS7 in 6-10 is ±7.5 (no rounding); JS17 blank up to 3.
        assert_eq!(dev("JS7", 8000), Some((75, -75)));
        assert_eq!(dev("JS17", 2000), None);
        // ISO 286-2 Table 8: J7 in 3-6 is ±6, J6 in 80-120 is +16/-6.
        assert_eq!(dev("J7", 5000), Some((60, -60)));
        assert_eq!(dev("J6", 100_000), Some((160, -60)));
        assert_eq!(dev("J9", 5000), None);
        // ISO 286-2 Table 8: K7 in 18-30 is +6/-15, K3 in 3-6 0/-2.5,
        // K4 there +0.5/-3.5, K8 over 500 mm 0/-110 at 500-630.
        assert_eq!(dev("K7", 30_000), Some((60, -150)));
        assert_eq!(dev("K3", 4000), Some((0, -25)));
        assert_eq!(dev("K4", 4000), Some((5, -35)));
        assert_eq!(dev("K8", 600_000), Some((0, -1100)));
        // ISO 286-2 Table 8, note 2: K above IT8 only up to 3 mm.
        assert_eq!(dev("K9", 5000), None);
        assert_eq!(dev("K9", 2000), Some((0, -250)));
        // ISO 286-2 Table 9: M7 in 18-30 is 0/-21, M4 in 3-6 -2.5/-6.5,
        // M6 in 250-315 -9/-41 (the special case), M6 in 180-250 -8/-37,
        // M7 in 500-630 -26/-96, M9 in 6-10 -6/-42.
        assert_eq!(dev("M7", 30_000), Some((0, -210)));
        assert_eq!(dev("M4", 5000), Some((-25, -65)));
        assert_eq!(dev("M6", 300_000), Some((-90, -410)));
        assert_eq!(dev("M6", 200_000), Some((-80, -370)));
        assert_eq!(dev("M7", 600_000), Some((-260, -960)));
        assert_eq!(dev("M9", 8000), Some((-60, -420)));
        // ISO 286-2 Table 9: N7 in 18-30 is -7/-28, N9 up to 3 mm -4/-29
        // (not used up to 1 mm), N9 in 3-6 0/-30, N8 in 2500-3150 -135/-465.
        assert_eq!(dev("N7", 30_000), Some((-70, -280)));
        assert_eq!(dev("N9", 500), None);
        assert_eq!(dev("N9", 2000), Some((-40, -290)));
        assert_eq!(dev("N9", 5000), Some((0, -300)));
        assert_eq!(dev("N8", 3_000_000), Some((-1350, -4650)));
        // ISO 286-2 Table 10: P7 in 18-30 is -14/-35, P7 in 3-6 -8/-20,
        // P3 in 18-30 -20.5/-24.5, P6 in 500-630 -78/-122.
        assert_eq!(dev("P7", 30_000), Some((-140, -350)));
        assert_eq!(dev("P7", 4000), Some((-80, -200)));
        assert_eq!(dev("P3", 25_000), Some((-205, -245)));
        assert_eq!(dev("P6", 600_000), Some((-780, -1220)));
        // ISO 286-1 Table 3: R in 18-30 is -28 + Δ(IT7) 8; S -35 + 8.
        assert_eq!(dev("R7", 30_000), Some((-200, -410)));
        assert_eq!(dev("S7", 30_000), Some((-270, -480)));
        // Table 3, note 3's example: S6 in 18-30 is -35 + 4 = -31.
        assert_eq!(dev("S6", 25_000), Some((-310, -440)));
        // Above IT7 no Δ: S8 in 18-30 is -35/-68.
        assert_eq!(dev("S8", 25_000), Some((-350, -680)));
        // Δ has no column below IT3.
        assert_eq!(dev("P2", 25_000), None);
    }

    #[test]
    fn classes_parse_and_write_back() {
        for text in ["H7", "g6", "JS9", "zc10", "h01", "H0", "cd5", "ZA18"] {
            assert_eq!(Class::parse(text).map(|c| c.text()), Some(text.to_string()));
        }
        let h7 = Class::parse("H7").expect("H7");
        assert_eq!((h7.member, h7.grade), (Member::Hole, Grade::It(7)));
        for bad in [
            "h", "H0x", "Q7", "H19", "IT01", "Js7", "H07", "H", "7", "", "i7", "H 7",
        ] {
            assert_eq!(Class::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn fits_parse() {
        let pair = Fit::parse("H7/g6").expect("a fit");
        let Fit::Pair(hole, shaft) = pair else {
            panic!("a pair")
        };
        assert_eq!(
            (hole.text(), shaft.text()),
            ("H7".to_string(), "g6".to_string())
        );
        assert!(matches!(Fit::parse("H7"), Some(Fit::One(_))));
        assert_eq!(Fit::parse("g6/H7"), None);
        assert_eq!(Fit::parse("H7/"), None);
    }
}
