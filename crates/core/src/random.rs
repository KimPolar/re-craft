//! The integer and uniform floating-point methods of 26.3 random sources.
use md5::{Digest, Md5};

const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;
const SILVER: u64 = 0x6a09_e667_f3bc_c909;

/// Per-level coordinate generator used by `Level.getBlockRandomPos`.
/// This is distinct from the level's regular `RandomSource`.
#[derive(Clone, Copy, Debug)]
pub struct BlockRandomPosition {
    value: i32,
}

impl BlockRandomPosition {
    pub fn new(value: i32) -> Self {
        Self { value }
    }

    pub fn next_section_offset(&mut self) -> [i32; 3] {
        self.value = self.value.wrapping_mul(3).wrapping_add(1_013_904_223);
        let value = self.value >> 2;
        [value & 15, (value >> 16) & 15, (value >> 8) & 15]
    }

    /// Advance the affine generator without enumerating coordinates.
    pub fn skip(&mut self, mut count: u32) {
        let mut multiplier = 3_u32;
        let mut increment = 1_013_904_223_u32;
        let mut accumulated_multiplier = 1_u32;
        let mut accumulated_increment = 0_u32;
        while count != 0 {
            if count & 1 != 0 {
                accumulated_increment = accumulated_increment
                    .wrapping_mul(multiplier)
                    .wrapping_add(increment);
                accumulated_multiplier = accumulated_multiplier.wrapping_mul(multiplier);
            }
            increment = increment.wrapping_mul(multiplier.wrapping_add(1));
            multiplier = multiplier.wrapping_mul(multiplier);
            count >>= 1;
        }
        self.value = (self.value as u32)
            .wrapping_mul(accumulated_multiplier)
            .wrapping_add(accumulated_increment) as i32;
    }
}

#[derive(Clone, Debug)]
pub struct LegacyRandom {
    seed: u64,
}

impl LegacyRandom {
    pub fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64 ^ 0x5deece66d) & ((1u64 << 48) - 1),
        }
    }

    /// Capture the internal 48-bit state without advancing the generator.
    pub fn state(&self) -> u64 {
        self.seed
    }

    /// Restore a previously captured internal state, rejecting non-48-bit data.
    pub fn from_state(state: u64) -> Option<Self> {
        (state < (1u64 << 48)).then_some(Self { seed: state })
    }

    fn bits(&mut self, count: u32) -> u32 {
        self.seed = self.seed.wrapping_mul(0x5deece66d).wrapping_add(11) & ((1u64 << 48) - 1);
        (self.seed >> (48 - count)) as u32
    }

    /// Advance the 48-bit LCG as if `next_int` had been called `count` times.
    pub fn skip_ints(&mut self, mut count: u32) {
        const MASK: u64 = (1u64 << 48) - 1;
        let mut multiplier = 0x5deece66d_u64;
        let mut increment = 11_u64;
        let mut accumulated_multiplier = 1_u64;
        let mut accumulated_increment = 0_u64;
        while count != 0 {
            if count & 1 != 0 {
                accumulated_increment = accumulated_increment
                    .wrapping_mul(multiplier)
                    .wrapping_add(increment)
                    & MASK;
                accumulated_multiplier = accumulated_multiplier.wrapping_mul(multiplier) & MASK;
            }
            increment = increment.wrapping_mul(multiplier.wrapping_add(1)) & MASK;
            multiplier = multiplier.wrapping_mul(multiplier) & MASK;
            count >>= 1;
        }
        self.seed = self
            .seed
            .wrapping_mul(accumulated_multiplier)
            .wrapping_add(accumulated_increment)
            & MASK;
    }

    pub fn next_int(&mut self) -> i32 {
        self.bits(32) as i32
    }

    pub fn next_int_bound(&mut self, bound: i32) -> Option<i32> {
        if bound <= 0 {
            return None;
        }
        if (bound & -bound) == bound {
            return Some(((i64::from(bound) * i64::from(self.bits(31))) >> 31) as i32);
        }
        loop {
            let bits = self.bits(31) as i32;
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) >= 0 {
                return Some(value);
            }
        }
    }

    pub fn next_long(&mut self) -> i64 {
        let high = i64::from(self.bits(32) as i32) << 32;
        high.wrapping_add(i64::from(self.bits(32) as i32))
    }

    pub fn next_bool(&mut self) -> bool {
        self.bits(1) != 0
    }
    pub fn next_float(&mut self) -> f32 {
        self.bits(24) as f32 * (1.0 / 16_777_216.0)
    }
    pub fn next_double(&mut self) -> f64 {
        let value = ((u64::from(self.bits(26))) << 27) | u64::from(self.bits(27));
        value as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
}

#[derive(Clone, Debug)]
pub struct XoroshiroRandom {
    low: u64,
    high: u64,
}

impl XoroshiroRandom {
    pub fn new(seed: i64) -> Self {
        let low = seed as u64 ^ SILVER;
        Self::from_state(
            mix_stafford13(low),
            mix_stafford13(low.wrapping_add(GOLDEN)),
        )
    }

    pub fn from_state(low: u64, high: u64) -> Self {
        if low | high == 0 {
            Self {
                low: GOLDEN,
                high: SILVER,
            }
        } else {
            Self { low, high }
        }
    }

    /// Initialize the named sequence used by Java's `RandomSequences`.
    /// `id`, when supplied, must be the canonical namespace-qualified ID.
    pub fn for_world_sequence(
        world_seed: i64,
        salt: i32,
        include_world_seed: bool,
        id: Option<&str>,
    ) -> Self {
        let seed = (if include_world_seed { world_seed } else { 0 }) ^ i64::from(salt);
        let mut low = seed as u64 ^ SILVER;
        let mut high = low.wrapping_add(GOLDEN);
        if let Some(id) = id {
            let hash = Md5::digest(id.as_bytes());
            low ^= u64::from_be_bytes(hash[..8].try_into().expect("MD5 has 16 bytes"));
            high ^= u64::from_be_bytes(hash[8..].try_into().expect("MD5 has 16 bytes"));
        }
        Self::from_state(mix_stafford13(low), mix_stafford13(high))
    }

    /// State words in the order used by Java's xoroshiro codec.
    pub fn state(&self) -> (u64, u64) {
        (self.low, self.high)
    }

    fn next_u64(&mut self) -> u64 {
        let low = self.low;
        let mut high = self.high;
        let result = low.wrapping_add(high).rotate_left(17).wrapping_add(low);
        high ^= low;
        self.low = low.rotate_left(49) ^ high ^ (high << 21);
        self.high = high.rotate_left(28);
        result
    }

    pub fn next_long(&mut self) -> i64 {
        self.next_u64() as i64
    }
    pub fn next_int(&mut self) -> i32 {
        self.next_u64() as i32
    }
    pub fn next_int_bound(&mut self, bound: i32) -> Option<i32> {
        if bound <= 0 {
            return None;
        }
        let bound = bound as u32;
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let value = self.next_int() as u32;
            let product = u64::from(value) * u64::from(bound);
            if product as u32 >= threshold {
                return Some((product >> 32) as i32);
            }
        }
    }

    pub fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 != 0
    }
    pub fn next_float(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }
    pub fn next_double(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
}

/// Java 26.3's `WorldgenRandom` over a xoroshiro source, used during decoration.
/// Its inherited legacy methods consume the high bits of separate xoroshiro
/// draws; they differ from calling methods on `XoroshiroRandom` directly.
#[derive(Clone, Debug)]
pub struct DecorationRandom {
    source: XoroshiroRandom,
}
impl DecorationRandom {
    pub fn new(seed: i64) -> Self {
        Self {
            source: XoroshiroRandom::new(seed),
        }
    }
    pub fn set_seed(&mut self, seed: i64) {
        self.source = XoroshiroRandom::new(seed);
    }
    fn bits(&mut self, bits: u32) -> u32 {
        ((self.source.next_long() as u64) >> (64 - bits)) as u32
    }
    pub fn next_long(&mut self) -> i64 {
        let high = i64::from(self.bits(32) as i32) << 32;
        high.wrapping_add(i64::from(self.bits(32) as i32))
    }
    pub fn next_int(&mut self) -> i32 {
        self.bits(32) as i32
    }
    pub fn next_int_bound(&mut self, bound: i32) -> Option<i32> {
        if bound <= 0 {
            return None;
        }
        if (bound & -bound) == bound {
            return Some(((i64::from(bound) * i64::from(self.bits(31))) >> 31) as i32);
        }
        loop {
            let bits = self.bits(31) as i32;
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) >= 0 {
                return Some(value);
            }
        }
    }
    pub fn next_bool(&mut self) -> bool {
        self.bits(1) != 0
    }
    pub fn next_float(&mut self) -> f32 {
        self.bits(24) as f32 * (1.0 / 16_777_216.0)
    }
    pub fn next_double(&mut self) -> f64 {
        let value = (u64::from(self.bits(26)) << 27) | u64::from(self.bits(27));
        value as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
    pub fn set_decoration_seed(&mut self, seed: i64, block_x: i32, block_z: i32) -> i64 {
        self.set_seed(seed);
        let x_scale = self.next_long() | 1;
        let z_scale = self.next_long() | 1;
        let result = i64::from(block_x)
            .wrapping_mul(x_scale)
            .wrapping_add(i64::from(block_z).wrapping_mul(z_scale))
            ^ seed;
        self.set_seed(result);
        result
    }
    pub fn set_feature_seed(&mut self, decoration_seed: i64, index: i32, step: i32) {
        self.set_seed(
            decoration_seed
                .wrapping_add(i64::from(index))
                .wrapping_add(i64::from(10_000i32.wrapping_mul(step))),
        );
    }
}

fn mix_stafford13(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_state_round_trips_and_rejects_invalid_bits() {
        for seed in [0, 1, -1, i64::MIN, i64::MAX] {
            let mut original = LegacyRandom::new(seed);
            for _ in 0..32 {
                original.next_int();
                let mut restored = LegacyRandom::from_state(original.state()).unwrap();
                assert_eq!(restored.next_long(), original.clone().next_long());
            }
        }
        assert!(LegacyRandom::from_state(1u64 << 48).is_none());
    }

    #[test]
    fn skipped_block_positions_match_iterated_and_composed_sequences() {
        for seed in [0, 1, -1, i32::MIN, i32::MAX] {
            for count in [0, 1, 2, 3, 63, 4096, 65537] {
                let mut iterated = BlockRandomPosition::new(seed);
                for _ in 0..count {
                    iterated.next_section_offset();
                }
                let mut skipped = BlockRandomPosition::new(seed);
                skipped.skip(count);
                assert_eq!(
                    skipped.next_section_offset(),
                    iterated.next_section_offset(),
                    "{seed} {count}"
                );
            }
            let mut combined = BlockRandomPosition::new(seed);
            combined.skip(i32::MAX as u32);
            let mut split = BlockRandomPosition::new(seed);
            split.skip(1_000_000_000);
            split.skip(i32::MAX as u32 - 1_000_000_000);
            assert_eq!(combined.next_section_offset(), split.next_section_offset());
        }
    }

    #[test]
    fn deterministic_and_bounded_for_both_generators() {
        let mut legacy = LegacyRandom::new(-42);
        let mut same_legacy = legacy.clone();
        let mut xor = XoroshiroRandom::new(-42);
        let mut same_xor = xor.clone();
        for _ in 0..1000 {
            assert_eq!(legacy.next_long(), same_legacy.next_long());
            assert_eq!(xor.next_long(), same_xor.next_long());
            assert!((0..17).contains(&legacy.next_int_bound(17).unwrap()));
            assert!((0..17).contains(&same_legacy.next_int_bound(17).unwrap()));
            assert!((0..17).contains(&xor.next_int_bound(17).unwrap()));
            assert!((0..17).contains(&same_xor.next_int_bound(17).unwrap()));
        }
        assert_eq!(legacy.next_int_bound(0), None);
        assert_eq!(xor.next_int_bound(-1), None);
    }

    #[test]
    fn skipped_legacy_ints_match_iteration_and_composition() {
        for seed in [0, 1, -1, i64::MIN, i64::MAX] {
            for count in [0, 1, 2, 262, 524, 1_000, 65_537] {
                let mut iterated = LegacyRandom::new(seed);
                for _ in 0..count {
                    iterated.next_int();
                }
                let mut skipped = LegacyRandom::new(seed);
                skipped.skip_ints(count);
                assert_eq!(
                    skipped.next_long(),
                    iterated.next_long(),
                    "seed {seed} count {count}"
                );
            }
            let mut combined = LegacyRandom::new(seed);
            combined.skip_ints(u32::MAX);
            let mut split = LegacyRandom::new(seed);
            split.skip_ints(u32::MAX - 262);
            split.skip_ints(262);
            assert_eq!(combined.next_long(), split.next_long());
        }
    }
}
