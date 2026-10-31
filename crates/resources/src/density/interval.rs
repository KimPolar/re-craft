//! 26.3 density function interval arithmetic, including NaI and signed zero.

#[derive(Clone, Copy, Debug)]
pub(super) struct Interval {
    pub min: f32,
    pub max: f32,
}

pub(super) fn min(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        return a;
    }
    if b.is_nan() {
        return b;
    }
    if a == 0.0 && b == 0.0 {
        return f32::from_bits(a.to_bits() | b.to_bits());
    }
    if a <= b { a } else { b }
}
pub(super) fn max(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        return a;
    }
    if b.is_nan() {
        return b;
    }
    if a == 0.0 && b == 0.0 {
        return f32::from_bits(a.to_bits() & b.to_bits());
    }
    if a >= b { a } else { b }
}

fn mul_bound(a: f32, b: f32) -> f32 {
    if a == 0.0 || b == 0.0 { 0.0 } else { a * b }
}

impl Interval {
    pub const NAI: Self = Self {
        min: f32::NAN,
        max: f32::NAN,
    };
    pub fn exact(value: f32) -> Self {
        Self {
            min: value,
            max: value,
        }
    }
    pub fn bounds(min: f32, max: f32) -> Self {
        if min.is_nan() || max.is_nan() {
            Self::NAI
        } else {
            Self { min, max }
        }
    }
    pub fn enclosing_values(a: f32, b: f32) -> Self {
        if a.is_nan() && b.is_nan() {
            return Self::NAI;
        }
        if a.is_nan() {
            return Self::exact(b);
        }
        if b.is_nan() {
            return Self::exact(a);
        }
        Self::bounds(min(a, b), max(a, b))
    }
    pub fn enclosing(a: Self, b: Self) -> Self {
        if a.min.is_nan() {
            return b;
        }
        if b.min.is_nan() {
            return a;
        }
        Self::bounds(min(a.min, b.min), max(a.max, b.max))
    }
    pub fn add(self, rhs: Self) -> Self {
        Self::bounds(self.min + rhs.min, self.max + rhs.max)
    }
    pub fn sub(self, rhs: Self) -> Self {
        Self::bounds(self.min - rhs.max, self.max - rhs.min)
    }
    pub fn mul(self, rhs: Self) -> Self {
        if self.min.is_nan() || rhs.min.is_nan() {
            return Self::NAI;
        }
        let a = mul_bound(self.min, rhs.min);
        let b = mul_bound(self.min, rhs.max);
        let c = mul_bound(self.max, rhs.min);
        let d = mul_bound(self.max, rhs.max);
        Self::bounds(min(min(a, b), min(c, d)), max(max(a, b), max(c, d)))
    }
    pub fn minimum(self, rhs: Self) -> Self {
        Self::bounds(min(self.min, rhs.min), min(self.max, rhs.max))
    }
    pub fn maximum(self, rhs: Self) -> Self {
        Self::bounds(max(self.min, rhs.min), max(self.max, rhs.max))
    }
    pub fn clamp(self, lower: f32, upper: f32) -> Self {
        if self.min.is_nan() {
            return Self::NAI;
        }
        if self.min >= upper {
            return Self::exact(upper);
        }
        if self.max <= lower {
            return Self::exact(lower);
        }
        Self::bounds(max(self.min, lower), min(self.max, upper))
    }
    pub fn monotonic(self, operation: impl Fn(f32) -> f32) -> Self {
        if self.min.is_nan() {
            return Self::NAI;
        }
        Self::enclosing_values(operation(self.min), operation(self.max))
    }
    pub fn abs(self) -> Self {
        if self.min.is_nan() {
            return Self::NAI;
        }
        let lo = self.min.abs();
        let hi = self.max.abs();
        Self::bounds(
            if self.min <= 0.0 && self.max >= 0.0 {
                0.0
            } else {
                min(lo, hi)
            },
            max(lo, hi),
        )
    }
    pub fn square(self) -> Self {
        if self.min.is_nan() {
            return Self::NAI;
        }
        let lo = self.min * self.min;
        let hi = self.max * self.max;
        Self::bounds(
            if self.min <= 0.0 && self.max >= 0.0 {
                0.0
            } else {
                min(lo, hi)
            },
            max(lo, hi),
        )
    }
    pub fn lerp(self, first: Self, second: Self) -> Self {
        if self.min.is_nan() || first.min.is_nan() || second.min.is_nan() {
            return Self::NAI;
        }
        let mut result = Self::NAI;
        for (a, b) in [
            (first.min, second.min),
            (first.max, second.min),
            (first.min, second.max),
            (first.max, second.max),
        ] {
            result = Self::enclosing(result, self.lerp_bounds(a, b));
        }
        result
    }
    fn lerp_bounds(self, first: f32, second: f32) -> Self {
        if first.is_finite() && second.is_finite() {
            let f = |alpha| first + mul_bound(alpha, second - first);
            return Self::enclosing_values(f(self.min), f(self.max));
        }
        if first == second {
            return Self::exact(first);
        }
        let f = |alpha: f32| {
            let a = mul_bound(1.0 - alpha, first);
            let b = mul_bound(alpha, second);
            if a.is_infinite() && b.is_infinite() {
                if alpha <= 0.0 {
                    return if second > first {
                        f32::NEG_INFINITY
                    } else {
                        f32::INFINITY
                    };
                }
                if alpha >= 1.0 {
                    return if second > first {
                        f32::INFINITY
                    } else {
                        f32::NEG_INFINITY
                    };
                }
                return f32::NAN;
            }
            a + b
        };
        Self::bounds_from_pair_or_nai(f(self.min), f(self.max))
    }
    fn bounds_from_pair_or_nai(a: f32, b: f32) -> Self {
        if a.is_nan() || b.is_nan() {
            Self::NAI
        } else {
            Self::enclosing_values(a, b)
        }
    }
}
