//! A compare with a Git revision as the window shows it (design 688, The
//! compare view): the revision its participant compares against, which
//! the window follows from `participant.changed` (an agent's `ryueda
//! compare HEAD` and the window's menu go the same way), and which side
//! is drawn: the window's own view state, never the session's.

/// Which side of a compare the window draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    /// The working sheet marked against the revision: the compare scene.
    #[default]
    Both,
    /// The working sheet alone, plain.
    Working,
    /// The revision's sheet alone, plain and read only.
    Rev,
}

/// What scene the window asks for its blink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    Plain,
    Compare(String),
    Rev(String),
}

/// The participant's compare (`of`, the revision as given) and the side
/// drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blink {
    pub of: Option<String>,
    pub side: Side,
    /// The compared commit, from the last compare or revision scene
    /// (#1106): the bar names it on every side.
    pub commit: Option<String>,
}

impl Blink {
    /// Follows the participant's compare: another revision, or none,
    /// starts again from Both. Whether anything changed.
    pub fn follow(&mut self, rev: Option<&str>) -> bool {
        if self.of.as_deref() == rev {
            return false;
        }
        *self = Blink {
            of: rev.map(str::to_owned),
            side: Side::Both,
            commit: None,
        };
        true
    }

    /// `B`: the revision alone, then the working sheet alone, and back.
    pub fn flip(&mut self) {
        if self.of.is_none() {
            return;
        }
        self.side = match self.side {
            Side::Both | Side::Working => Side::Rev,
            Side::Rev => Side::Working,
        };
    }

    /// The compare bar's Both, Working and `<rev>`.
    pub fn set(&mut self, side: Side) {
        if self.of.is_some() {
            self.side = side;
        }
    }

    pub fn ask(&self) -> Ask {
        match (&self.of, self.side) {
            (None, _) | (Some(_), Side::Working) => Ask::Plain,
            (Some(r), Side::Both) => Ask::Compare(r.clone()),
            (Some(r), Side::Rev) => Ask::Rev(r.clone()),
        }
    }

    /// Whether what is drawn is the revision's, where picks are described
    /// and never selected.
    pub fn read_only(&self) -> bool {
        self.of.is_some() && self.side == Side::Rev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compare_starts_from_both_and_b_flips_the_lone_sides() {
        let mut b = Blink::default();
        assert_eq!(b.ask(), Ask::Plain);
        b.flip();
        assert_eq!(b.side, Side::Both, "no compare: B does nothing");
        assert!(b.follow(Some("HEAD")));
        assert!(
            !b.follow(Some("HEAD")),
            "the same compare again moves nothing"
        );
        assert_eq!(b.ask(), Ask::Compare("HEAD".into()));
        b.flip();
        assert_eq!((b.side, b.ask()), (Side::Rev, Ask::Rev("HEAD".into())));
        assert!(b.read_only());
        b.flip();
        assert_eq!((b.side, b.ask()), (Side::Working, Ask::Plain));
        b.flip();
        assert_eq!(b.side, Side::Rev);
        b.set(Side::Both);
        assert_eq!(b.ask(), Ask::Compare("HEAD".into()));
        b.set(Side::Rev);
        assert!(b.follow(Some("main")), "another compare");
        assert_eq!(b.side, Side::Both, "starts from Both");
        assert!(b.follow(None));
        assert_eq!((b.of.as_deref(), b.ask()), (None, Ask::Plain));
        assert!(!b.read_only());
    }
}
