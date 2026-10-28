//! Readable IDs for library objects, made from their names
//! (`R_0402_1005Metric` is `r_0402_1005metric`), so importing a library
//! again finds the same objects. Design objects take random IDs instead.

use crate::id::Id;

/// The longest readable ID before a collision's `-<n>`, so the whole
/// stays within an ID's 63 characters.
const MAX: usize = 56;

/// Names Windows reserves for devices: an ID equal to one gets a `_`.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The readable form of `name`: ASCII letters lowercased, digits, `_` and
/// `-` kept, every run of anything else one `_`, trimmed to start and end
/// with a letter or digit, at most 56 characters; `item` when nothing is
/// left.
pub fn readable(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let edge = |c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit());
    let mut s = out.trim_matches(edge).to_owned();
    if s.len() > MAX {
        s.truncate(MAX);
        s = s.trim_end_matches(edge).to_owned();
    }
    if s.is_empty() {
        s = "item".to_owned();
    }
    if RESERVED.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

/// The ID for an object called `name`: its readable form, or with `-2`,
/// `-3`, … when `taken` refuses that.
pub fn free(name: &str, taken: impl Fn(&str) -> bool) -> Id {
    let base = readable(name);
    let text = std::iter::once(base.clone())
        .chain((2u64..).map(|n| format!("{base}-{n}")))
        .find(|s| !taken(s))
        .expect("some n is free");
    Id::parse(&text, "library object").expect("readable IDs are IDs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_ids() {
        assert_eq!(readable("R_0402_1005Metric"), "r_0402_1005metric");
        assert_eq!(readable("Conn_01x02_Pin"), "conn_01x02_pin");
        assert_eq!(readable("LED 3mm (red)"), "led_3mm_red");
        assert_eq!(readable("  +3V3 "), "3v3");
        assert_eq!(readable("Ω µ"), "item");
        assert_eq!(readable("--a--"), "a");
        assert_eq!(readable("CON"), "con_");
        let long = readable(&"ab".repeat(40));
        assert_eq!(long.len(), 56);
        assert_eq!(readable(&format!("{}__x", "a".repeat(55))), "a".repeat(55));
    }

    #[test]
    fn collisions_take_the_next_number() {
        let taken = ["r", "r-2"];
        assert_eq!(free("R", |s| taken.contains(&s)).as_str(), "r-3");
        assert_eq!(free("C", |s| taken.contains(&s)).as_str(), "c");
        let long = free(&"x".repeat(80), |s| s.len() == 56);
        assert_eq!(long.as_str().len(), 58);
    }
}
