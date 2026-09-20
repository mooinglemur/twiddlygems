//! A small deterministic PRNG.
//!
//! The engine has no dependencies, and gameplay only needs a decent
//! non-cryptographic stream, so this is a hand-rolled SplitMix64. Being
//! deterministic matters: a level seed has to reproduce the same board every
//! time so a run can be replayed, and later so a multiworld seed lines up.

#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Avoid the all-zero state producing a dull first few outputs.
        Rng { state: seed ^ 0x9e37_79b9_7f4a_7c15 }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform in `0..n`. Returns 0 when `n` is 0 rather than dividing by zero.
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        // Lemire's multiply-shift rejection method, debiased.
        let mut m = (self.next_u32() as u64) * (n as u64);
        let mut low = m as u32;
        if low < n {
            let threshold = n.wrapping_neg() % n;
            while low < threshold {
                m = (self.next_u32() as u64) * (n as u64);
                low = m as u32;
            }
        }
        (m >> 32) as u32
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u32 + 1) as usize;
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(12345);
        let mut b = Rng::new(12345);
        for _ in 0..64 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn below_stays_in_range_and_covers_it() {
        let mut rng = Rng::new(7);
        let mut seen = [false; 6];
        for _ in 0..2000 {
            let v = rng.below(6);
            assert!(v < 6);
            seen[v as usize] = true;
        }
        assert!(seen.iter().all(|s| *s), "every value in 0..6 should appear");
    }

    #[test]
    fn below_zero_is_zero() {
        let mut rng = Rng::new(7);
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn shuffle_keeps_every_element() {
        let mut rng = Rng::new(99);
        let mut items: Vec<u32> = (0..32).collect();
        rng.shuffle(&mut items);
        items.sort();
        assert_eq!(items, (0..32).collect::<Vec<u32>>());
    }
}
