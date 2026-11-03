//! The enclosures one operation has worked out (plan 15.C): NURBS jets
//! over boxes, by surface id and the box's bits. A value is what
//! [`NurbsSurface::jet_box`] gives, so a hit and a miss give the same
//! bits, and clearing a full shard changes nothing but the time.

use super::NurbsSurface;
use crate::interval::Interval;
use std::collections::HashMap;
use std::sync::Mutex;

type V3 = [Interval; 3];
type Key = (u64, [u64; 4]);
type Shard = Mutex<HashMap<Key, [V3; 6]>>;

/// How many locks the table is split under.
pub(crate) const SHARDS: usize = 16;

/// The most entries a shard holds before it is emptied.
pub(crate) const SHARD_LIMIT: usize = 1 << 14;

/// The table of callers with no operation: it remembers nothing.
pub(crate) static NONE: Enclosures = Enclosures::none();

/// One operation's table of NURBS jet enclosures, shared by every meter
/// of the call ([`crate::work::Meter::enclosures`]).
pub(crate) struct Enclosures {
    shards: Option<[Shard; SHARDS]>,
}

impl core::fmt::Debug for Enclosures {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(if self.shards.is_some() {
            "Enclosures"
        } else {
            "Enclosures(none)"
        })
    }
}

impl Enclosures {
    pub(crate) fn new() -> Self {
        Self {
            shards: Some(core::array::from_fn(|_| Mutex::new(HashMap::new()))),
        }
    }

    /// A table that remembers nothing, for meters with no operation.
    pub(crate) const fn none() -> Self {
        Self { shards: None }
    }

    /// [`NurbsSurface::jet_box`] over `b`, remembered.
    pub(crate) fn jet_box(&self, s: &NurbsSurface, b: [[f64; 2]; 2]) -> [V3; 6] {
        let Some(shards) = &self.shards else {
            return s.jet_box(b);
        };
        let bits = [
            b[0][0].to_bits(),
            b[0][1].to_bits(),
            b[1][0].to_bits(),
            b[1][1].to_bits(),
        ];
        let key = (s.id(), bits);
        let h = bits
            .iter()
            .fold(key.0.wrapping_mul(0x9e37_79b9_7f4a_7c15), |a, &x| {
                (a.rotate_left(13) ^ x).wrapping_mul(0x0100_0000_01b3)
            });
        let shard = &shards[(h >> 32) as usize % SHARDS];
        if let Some(v) = lock(shard).get(&key) {
            return *v;
        }
        // Worked out outside the lock: another task may work out the same
        // box meanwhile, and both store the same bits.
        let v = s.jet_box(b);
        let mut g = lock(shard);
        if g.len() >= SHARD_LIMIT {
            g.clear();
        }
        g.insert(key, v);
        v
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.shards
            .as_ref()
            .map_or(0, |s| s.iter().map(|m| lock(m).len()).sum())
    }
}

/// A shard's lock; a task that panicked holding it left whole entries
/// only, so the table is still sound.
fn lock(m: &Shard) -> std::sync::MutexGuard<'_, HashMap<Key, [V3; 6]>> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{KnotVector, Point3};

    /// A bicubic patch over `[0, 1]²`.
    fn bump() -> NurbsSurface {
        let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
        let mut pts = Vec::new();
        for i in 0..4 {
            for j in 0..4 {
                let z = crate::math::sin(i as f64) * crate::math::cos(j as f64) / 4.0;
                pts.push(Point3::new(i as f64 / 3.0, j as f64 / 3.0, z));
            }
        }
        NurbsSurface::new([k.clone(), k], pts, None).unwrap()
    }

    #[test]
    fn a_remembered_enclosure_is_the_same_bits() {
        let s = bump();
        let table = Enclosures::new();
        let boxes = [
            [[0.0, 0.5], [0.0, 0.5]],
            [[0.25, 0.75], [0.1, 0.9]],
            [[0.0, 1.0], [0.0, 1.0]],
        ];
        for b in boxes {
            let fresh = s.jet_box(b);
            let first = table.jet_box(&s, b);
            let again = table.jet_box(&s, b);
            assert_eq!(format!("{fresh:?}"), format!("{first:?}"));
            assert_eq!(format!("{fresh:?}"), format!("{again:?}"));
        }
        assert_eq!(table.len(), 3);
        // A table that remembers nothing gives the same too.
        let none = Enclosures::none();
        assert_eq!(
            format!("{:?}", none.jet_box(&s, boxes[1])),
            format!("{:?}", s.jet_box(boxes[1]))
        );
        assert_eq!(none.len(), 0);
    }

    #[test]
    fn copies_share_an_id_and_new_surfaces_dont() {
        let a = bump();
        let b = a.clone();
        let c = bump();
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), c.id());
        assert_eq!(a, c); // equality still by content
        assert_eq!(format!("{a:?}"), format!("{c:?}")); // and the text
    }

    #[test]
    fn a_full_shard_is_cleared_not_wrong() {
        let s = bump();
        let table = Enclosures::new();
        let n = SHARD_LIMIT * SHARDS + 10;
        for k in 0..n {
            let x = k as f64 / n as f64 * 0.5;
            let b = [[x, x + 0.1], [0.0, 0.5]];
            if k % 997 == 0 {
                assert_eq!(
                    format!("{:?}", table.jet_box(&s, b)),
                    format!("{:?}", s.jet_box(b))
                );
            } else {
                table.jet_box(&s, b);
            }
        }
        assert!(table.len() <= SHARD_LIMIT * SHARDS);
    }

    /// The table's own speed (plan 15.C Task 18): one surface's `jet_box`
    /// over the same 256 boxes twice, through a table and without.
    #[test]
    #[ignore = "a timing, run with --ignored --nocapture"]
    fn jet_box_hits() {
        let s = bump();
        let boxes: Vec<[[f64; 2]; 2]> = (0..256)
            .map(|k| {
                let (i, j) = ((k % 16) as f64 / 16.0, (k / 16) as f64 / 16.0);
                [[i, i + 1.0 / 16.0], [j, j + 1.0 / 16.0]]
            })
            .collect();
        let t = std::time::Instant::now();
        for _ in 0..2 {
            for &b in &boxes {
                std::hint::black_box(s.jet_box(b));
            }
        }
        let bare = t.elapsed();
        let table = Enclosures::new();
        let t = std::time::Instant::now();
        for _ in 0..2 {
            for &b in &boxes {
                std::hint::black_box(table.jet_box(&s, b));
            }
        }
        println!(
            "512 jet_box: {bare:?} bare, {:?} through a table",
            t.elapsed()
        );
    }
}
