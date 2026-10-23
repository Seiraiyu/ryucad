//! Revision labels. A project picks one scheme, and every label is derived
//! from it, never typed: a part's first release takes the first label and
//! each later one the label after its latest record.
//!
//! | Scheme | Sequence |
//! |---|---|
//! | `letters` | A, B, …, Y, AA, AB, …, YY, AAA, …, skipping I, O, Q, S, X and Z (ASME Y14.35) |
//! | `numeric` | 1, 2, 3, … |
//! | `numeric2` | 01, 02, …, 99, 100, … |
//!
//! Labels are numbered from 1 in their scheme's order ([`index`]), so two
//! labels of one scheme compare by their numbers.

use crate::config::Scheme;

/// The letters ASME Y14.35 allows in a revision label, in order.
const LETTERS: &[u8; 20] = b"ABCDEFGHJKLMNPRTUVWY";

/// The `n`th label of `scheme`, counting from 1.
pub fn label(scheme: Scheme, n: u64) -> String {
    assert!(n >= 1, "labels count from 1");
    match scheme {
        Scheme::Numeric => n.to_string(),
        Scheme::Numeric2 => format!("{n:02}"),
        Scheme::Letters => {
            // Bijective base 20: A is 1, Y is 20, AA is 21.
            let mut out = Vec::new();
            let mut n = n;
            while n > 0 {
                n -= 1;
                out.push(LETTERS[(n % 20) as usize]);
                n /= 20;
            }
            out.reverse();
            String::from_utf8(out).expect("ASCII")
        }
    }
}

/// The first label of `scheme`.
pub fn first(scheme: Scheme) -> String {
    label(scheme, 1)
}

/// Where `label` falls in `scheme`'s sequence, from 1; `None` when it isn't
/// one of its labels as the scheme writes them (`B` in `numeric`, `I` or
/// `01` in `letters`, `1` or `007` in `numeric2`).
pub fn index(scheme: Scheme, label: &str) -> Option<u64> {
    let b = label.as_bytes();
    // Longer labels than these would be past any part's history and past
    // what a u64 counts.
    if b.is_empty() || b.len() > 12 {
        return None;
    }
    match scheme {
        Scheme::Numeric | Scheme::Numeric2 => {
            if !b.iter().all(u8::is_ascii_digit) {
                return None;
            }
            let n: u64 = label.parse().ok()?;
            let written = match scheme {
                Scheme::Numeric => n.to_string(),
                _ => format!("{n:02}"),
            };
            (n >= 1 && written == label).then_some(n)
        }
        Scheme::Letters => b.iter().try_fold(0u64, |n, c| {
            let digit = LETTERS.iter().position(|l| l == c)? as u64 + 1;
            Some(n * 20 + digit)
        }),
    }
}

/// The label after `label` in `scheme`, if `label` is one of its labels.
pub fn next(scheme: Scheme, label: &str) -> Option<String> {
    index(scheme, label).map(|n| self::label(scheme, n + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_skip_the_letters_drawings_confuse() {
        let first: Vec<String> = (1..=20).map(|n| label(Scheme::Letters, n)).collect();
        assert_eq!(first.concat(), "ABCDEFGHJKLMNPRTUVWY");
        for skipped in ["I", "O", "Q", "S", "X", "Z", "AI", "ZA"] {
            assert_eq!(index(Scheme::Letters, skipped), None, "{skipped}");
        }
        assert_eq!(next(Scheme::Letters, "H").as_deref(), Some("J"));
        assert_eq!(next(Scheme::Letters, "W").as_deref(), Some("Y"));
    }

    #[test]
    fn letters_roll_over_to_more_letters() {
        assert_eq!(next(Scheme::Letters, "Y").as_deref(), Some("AA"));
        assert_eq!(next(Scheme::Letters, "AY").as_deref(), Some("BA"));
        assert_eq!(next(Scheme::Letters, "YY").as_deref(), Some("AAA"));
        assert_eq!(label(Scheme::Letters, 21), "AA");
        assert_eq!(label(Scheme::Letters, 420), "YY");
        assert_eq!(label(Scheme::Letters, 421), "AAA");
    }

    #[test]
    fn numbers_are_written_one_way() {
        assert_eq!(first(Scheme::Numeric), "1");
        assert_eq!(first(Scheme::Numeric2), "01");
        assert_eq!(next(Scheme::Numeric, "9").as_deref(), Some("10"));
        assert_eq!(next(Scheme::Numeric2, "09").as_deref(), Some("10"));
        assert_eq!(next(Scheme::Numeric2, "99").as_deref(), Some("100"));
        for (scheme, bad) in [
            (Scheme::Numeric, "0"),
            (Scheme::Numeric, "01"),
            (Scheme::Numeric, "+1"),
            (Scheme::Numeric, "A"),
            (Scheme::Numeric2, "1"),
            (Scheme::Numeric2, "00"),
            (Scheme::Numeric2, "007"),
            (Scheme::Numeric2, "0100"),
            (Scheme::Letters, "a"),
            (Scheme::Letters, "1"),
            (Scheme::Letters, ""),
        ] {
            assert_eq!(index(scheme, bad), None, "{bad} in {}", scheme.as_str());
        }
    }

    #[test]
    fn every_label_reads_back_as_its_number() {
        for scheme in Scheme::ALL {
            for n in (1..2000).chain([8_000, 160_000, 1 << 36]) {
                let l = label(scheme, n);
                assert_eq!(index(scheme, &l), Some(n), "{l}");
            }
        }
        // Labels too long to count are not labels.
        assert_eq!(index(Scheme::Letters, &"A".repeat(13)), None);
        assert_eq!(index(Scheme::Numeric, &"9".repeat(13)), None);
    }
}
