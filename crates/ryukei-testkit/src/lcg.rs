/// A 64-bit linear congruential generator (Knuth's MMIX constants).
///
/// Pure integer arithmetic, so the sequence is identical on every platform.
/// Not for statistics; only for reproducible test inputs.
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    /// Uniform in `[lo, hi)`, built from the top 53 bits.
    pub fn next_f64(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * unit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_is_fixed() {
        let mut g = Lcg::new(1);
        assert_eq!(g.next_u64(), 7806831264735756412);
        assert_eq!(g.next_u64(), 9396908728118811419);
    }

    #[test]
    fn f64_in_range() {
        let mut g = Lcg::new(7);
        for _ in 0..1000 {
            let x = g.next_f64(-2.0, 3.0);
            assert!((-2.0..3.0).contains(&x));
        }
    }
}
