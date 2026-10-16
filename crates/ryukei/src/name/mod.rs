//! Stable names for faces, edges and vertices, and the journal that
//! reports how they change.
//!
//! A [`Name`] is derived, never counted: it is the hash of the caller's
//! [`OpKey`], the entity's [`Role`] in that operation and the names of its
//! parents. Running the same operations with the same keys gives the same
//! names, however the body's arrays happen to be laid out.

mod hash;
mod journal;

pub use journal::{Event, Journal};

use core::fmt;
use hash::{NAME_KEY, siphash128};

/// A stable 128-bit name. Compared only by value; see [`Name::derive`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(u128);

/// The caller's key for one operation. ryucad passes its feature ID; the
/// kernel never invents one. Reusing a key for two operations on the same
/// entities gives them the same names, which the checker reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpKey(pub u128);

impl OpKey {
    /// The key for an operation identified by `bytes`: SipHash-2-4-128 of
    /// them under an all-zero key. ryucad derives a feature's key this way
    /// from its part and feature IDs, so the key never depends on order.
    pub fn from_bytes(bytes: &[u8]) -> OpKey {
        OpKey(siphash128(&[0; 16], bytes))
    }
}

/// What an entity is to the operation that named it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Made by an Euler operator.
    NewVertex,
    NewEdge,
    NewFace,
    /// One of the two pieces of a split, in the operator's documented
    /// order.
    Part(u8),
    /// The result of merging two entities.
    Merged,
    /// A primitive's face.
    Face(FaceRole),
    /// A primitive's edge or vertex; its parents are the faces around it.
    Edge,
    Vertex,
}

/// The faces of the primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaceRole {
    PlusX,
    MinusX,
    PlusY,
    MinusY,
    PlusZ,
    MinusZ,
    Side,
    Top,
    Bottom,
}

impl Role {
    /// The role's fixed two-byte encoding inside a name.
    fn code(self) -> [u8; 2] {
        match self {
            Self::NewVertex => [1, 0],
            Self::NewEdge => [2, 0],
            Self::NewFace => [3, 0],
            Self::Part(i) => [4, i],
            Self::Merged => [5, 0],
            Self::Face(f) => [6, f as u8],
            Self::Edge => [7, 0],
            Self::Vertex => [8, 0],
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewVertex => f.write_str("new-vertex"),
            Self::NewEdge => f.write_str("new-edge"),
            Self::NewFace => f.write_str("new-face"),
            Self::Part(i) => write!(f, "part{i}"),
            Self::Merged => f.write_str("merged"),
            Self::Face(face) => f.write_str(match face {
                FaceRole::PlusX => "+X",
                FaceRole::MinusX => "-X",
                FaceRole::PlusY => "+Y",
                FaceRole::MinusY => "-Y",
                FaceRole::PlusZ => "+Z",
                FaceRole::MinusZ => "-Z",
                FaceRole::Side => "side",
                FaceRole::Top => "top",
                FaceRole::Bottom => "bottom",
            }),
            Self::Edge => f.write_str("edge"),
            Self::Vertex => f.write_str("vertex"),
        }
    }
}

impl Name {
    /// The name of the entity playing `role` in operation `key`, made from
    /// `parents` in the given order. Callers sort parents whose order means
    /// nothing.
    ///
    /// The hash input is `"RKN1"`, the key (16 bytes, little-endian), the
    /// role code (2 bytes), the parent count (4 bytes, little-endian) and
    /// each parent (16 bytes, little-endian).
    pub fn derive(key: OpKey, role: Role, parents: &[Name]) -> Self {
        let mut bytes = Vec::with_capacity(26 + 16 * parents.len());
        bytes.extend_from_slice(b"RKN1");
        bytes.extend_from_slice(&key.0.to_le_bytes());
        bytes.extend_from_slice(&role.code());
        bytes.extend_from_slice(&(parents.len() as u32).to_le_bytes());
        for p in parents {
            bytes.extend_from_slice(&p.0.to_le_bytes());
        }
        Self(siphash128(&NAME_KEY, &bytes))
    }

    pub fn to_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({:032x})", self.0)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// The readable recipe of a derived name, e.g. `box#2a/+Z` or
/// `mef#31(box#2a/+Z)/part1`. For debugging only; names compare by hash.
pub(crate) fn recipe(kind: &str, key: OpKey, role: Role, parents: &[&str]) -> String {
    if parents.is_empty() {
        format!("{kind}#{:x}/{role}", key.0)
    } else {
        format!("{kind}#{:x}({})/{role}", key.0, parents.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_key_from_bytes_is_fixed() {
        // Pins the encoding: changing it renames every feature's entities.
        assert_eq!(
            format!("{:032x}", OpKey::from_bytes(b"ryucad.feature\0p\0f").0),
            "02a896e2b8aa1b02abfba56f8c8e1046"
        );
        assert_ne!(OpKey::from_bytes(b"a"), OpKey::from_bytes(b"b"));
    }

    #[test]
    fn derive_is_fixed() {
        // Pins the encoding: changing it renames every entity everywhere.
        let n = Name::derive(OpKey(0x2a), Role::Face(FaceRole::PlusZ), &[]);
        assert_eq!(format!("{n}"), "57796150f72b59a1a8966c739eed89b2");
        let m = Name::derive(OpKey(7), Role::Part(1), &[n]);
        assert_eq!(format!("{m:?}"), "Name(230a95de0113977e565e67b9356eeaf5)");
    }

    #[test]
    fn every_input_changes_the_name() {
        let base = Name::derive(OpKey(1), Role::NewEdge, &[]);
        let p = Name::derive(OpKey(9), Role::NewFace, &[]);
        let q = Name::derive(OpKey(9), Role::NewVertex, &[]);
        let all = [
            base,
            Name::derive(OpKey(2), Role::NewEdge, &[]),
            Name::derive(OpKey(1), Role::NewVertex, &[]),
            Name::derive(OpKey(1), Role::NewEdge, &[p]),
            Name::derive(OpKey(1), Role::NewEdge, &[p, q]),
            Name::derive(OpKey(1), Role::NewEdge, &[q, p]),
            Name::derive(OpKey(1), Role::Part(0), &[]),
            Name::derive(OpKey(1), Role::Part(1), &[]),
            Name::derive(OpKey(1), Role::Face(FaceRole::Top), &[]),
            Name::derive(OpKey(1), Role::Face(FaceRole::Bottom), &[]),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(base, Name::derive(OpKey(1), Role::NewEdge, &[]));
    }

    #[test]
    fn role_codes_are_distinct() {
        let mut roles = vec![
            Role::NewVertex,
            Role::NewEdge,
            Role::NewFace,
            Role::Part(0),
            Role::Part(1),
            Role::Merged,
            Role::Edge,
            Role::Vertex,
        ];
        for f in [
            FaceRole::PlusX,
            FaceRole::MinusX,
            FaceRole::PlusY,
            FaceRole::MinusY,
            FaceRole::PlusZ,
            FaceRole::MinusZ,
            FaceRole::Side,
            FaceRole::Top,
            FaceRole::Bottom,
        ] {
            roles.push(Role::Face(f));
        }
        let mut codes: Vec<[u8; 2]> = roles.iter().map(|r| r.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), roles.len());
    }

    #[test]
    fn recipes_read_well() {
        assert_eq!(
            recipe("box", OpKey(0x2a), Role::Face(FaceRole::PlusZ), &[]),
            "box#2a/+Z"
        );
        assert_eq!(
            recipe("mef", OpKey(0x31), Role::Part(1), &["box#2a/+Z"]),
            "mef#31(box#2a/+Z)/part1"
        );
    }
}
