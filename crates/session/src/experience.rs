//! Player experience fields and the 26.3 point-to-level calculation.

/// Experience progress, level, and cumulative points are independent saved
/// fields in Java. They are not recomputed from one another on load.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Experience {
    pub progress: f32,
    pub level: i32,
    pub total: i32,
}

impl Experience {
    /// Matches Java's integer arithmetic, including overflow for invalid levels.
    pub fn needed_for_next_level(self) -> i32 {
        if self.level >= 30 {
            112_i32.wrapping_add(self.level.wrapping_sub(30).wrapping_mul(9))
        } else if self.level >= 15 {
            37_i32.wrapping_add(self.level.wrapping_sub(15).wrapping_mul(5))
        } else {
            7_i32.wrapping_add(self.level.wrapping_mul(2))
        }
    }

    /// Apply points in Java's order and precision, including its score and
    /// total-experience overflow behavior. Invalid saved values are rejected
    /// transactionally rather than allowing an unbounded level loop.
    pub fn give_points(&mut self, score: &mut i32, amount: i32) -> bool {
        let mut next = *self;
        if !next.progress.is_finite() || next.level < 0 || next.needed_for_next_level() <= 0 {
            return false;
        }
        next.progress += amount as f32 / next.needed_for_next_level() as f32;
        next.total = next.total.wrapping_add(amount).max(0);
        for _ in 0..4096 {
            if next.progress < 0.0 {
                let remainder = next.progress * next.needed_for_next_level() as f32;
                if next.level > 0 {
                    next.level -= 1;
                    if next.needed_for_next_level() <= 0 {
                        return false;
                    }
                    next.progress = 1.0 + remainder / next.needed_for_next_level() as f32;
                } else {
                    next.level = 0;
                    next.total = 0;
                    next.progress = 0.0;
                }
            } else if next.progress >= 1.0 {
                next.progress = (next.progress - 1.0) * next.needed_for_next_level() as f32;
                next.level = next.level.saturating_add(1);
                if next.needed_for_next_level() <= 0 {
                    return false;
                }
                next.progress /= next.needed_for_next_level() as f32;
            } else if next.progress.is_finite() {
                *self = next;
                *score = score.wrapping_add(amount);
                return true;
            } else {
                return false;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_values_do_not_consume_points_or_score() {
        for xp in [
            Experience {
                progress: f32::NAN,
                level: 0,
                total: 0,
            },
            Experience {
                progress: 0.0,
                level: -1,
                total: 0,
            },
            Experience {
                progress: f32::MAX,
                level: 0,
                total: 0,
            },
        ] {
            let mut xp = xp;
            let original = xp;
            let mut score = 3;
            assert!(!xp.give_points(&mut score, 7));
            assert_eq!(xp.progress.to_bits(), original.progress.to_bits());
            assert_eq!(xp.level, original.level);
            assert_eq!(xp.total, original.total);
            assert_eq!(score, 3);
        }
    }
}
