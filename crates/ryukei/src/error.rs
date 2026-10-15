use core::fmt;

/// Errors from fallible kernel constructors and operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Geometry outside the size box (see [`crate::tol::SIZE_BOX_HALF`]).
    OutsideSizeBox,
    /// A zero-length direction, a singular transform, or a tolerance below
    /// resolution.
    Degenerate,
    /// A NaN or infinity reached a constructor.
    NonFinite,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OutsideSizeBox => "geometry lies outside the size box",
            Self::Degenerate => "degenerate geometry",
            Self::NonFinite => "non-finite coordinate",
        })
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays() {
        assert_eq!(Error::NonFinite.to_string(), "non-finite coordinate");
        let e: Box<dyn std::error::Error> = Box::new(Error::Degenerate);
        assert_eq!(e.to_string(), "degenerate geometry");
    }
}
