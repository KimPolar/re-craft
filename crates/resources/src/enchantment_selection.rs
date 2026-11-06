//! Ordered enchantment selection for Java 26.3's cost-based providers.
//!
//! Callers must supply the active provider's holder order, item eligibility,
//! per-level costs and exclusive sets. This module does not load data packs
//! or write item components.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};

use crate::{packs::ResourceId, worldgen_region_random::RegionRandom};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelCost {
    pub level: i32,
    pub min: i32,
    pub max: i32,
}

/// A primary-item-eligible holder, in the provider's exact holder order.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: ResourceId,
    pub weight: i32,
    /// Levels must be in strictly increasing order.
    pub levels: Vec<LevelCost>,
    pub exclusive: BTreeSet<ResourceId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    pub id: ResourceId,
    pub level: i32,
}

/// `by_cost_with_difficulty` draws from the inclusive range even when the
/// range contains only one value. Validate before consuming the RNG.
pub fn difficulty_cost(
    random: &mut RegionRandom,
    min_cost: i32,
    max_span: i32,
    special: f32,
) -> Result<i32> {
    ensure!(
        (1..=10_000).contains(&min_cost) && (0..=10_000).contains(&max_span),
        "invalid provider cost bounds"
    );
    ensure!(
        special.is_finite() && (0.0..=1.0).contains(&special),
        "invalid special difficulty multiplier"
    );
    let span = (special * max_span as f32) as i32;
    Ok(min_cost
        + random
            .next_int_bound(span + 1)
            .context("invalid provider range")?)
}

/// Run `EnchantmentHelper.selectEnchantment` using pre-resolved eligible
/// holders. An absent enchantability component returns an empty list without
/// drawing random values. Errors leave the RNG untouched.
pub fn select(
    random: &mut RegionRandom,
    cost: i32,
    enchantability: Option<i32>,
    candidates: &[Candidate],
) -> Result<Vec<Selected>> {
    let Some(enchantability) = enchantability else {
        return Ok(Vec::new());
    };
    ensure!(
        (1..=10_000).contains(&enchantability),
        "invalid enchantability"
    );
    ensure!(
        (0..=1_000_000).contains(&cost),
        "unsupported enchantment cost"
    );
    let mut ids = BTreeSet::new();
    let mut total = 0_i32;
    for candidate in candidates {
        ensure!(
            ids.insert(candidate.id.clone()),
            "duplicate enchantment holder"
        );
        ensure!(
            candidate.weight > 0 && !candidate.levels.is_empty(),
            "invalid enchantment candidate"
        );
        ensure!(
            candidate.levels.iter().all(|entry| entry.level > 0)
                && candidate
                    .levels
                    .windows(2)
                    .all(|pair| pair[0].level < pair[1].level),
            "invalid enchantment level order"
        );
        total = total
            .checked_add(candidate.weight)
            .context("enchantment weight overflow")?;
    }
    let mut staged = random.clone();
    let bound = enchantability / 4 + 1;
    let value = cost
        + 1
        + staged
            .next_int_bound(bound)
            .context("invalid enchantability bound")?
        + staged
            .next_int_bound(bound)
            .context("invalid enchantability bound")?;
    let variation = (staged.next_float() + staged.next_float() - 1.0_f32) * 0.15_f32;
    let rounded_input = value as f32 + value as f32 * variation;
    let mut power = ((f64::from(rounded_input) + 0.5).floor() as i32).max(1);
    let mut available: Vec<_> = candidates
        .iter()
        .filter_map(|candidate| {
            candidate
                .levels
                .iter()
                .rev()
                .find(|level| level.min <= power && power <= level.max)
                .map(|level| (candidate, level.level))
        })
        .collect();
    let mut selected = Vec::new();
    if !available.is_empty() {
        let choose = |random: &mut RegionRandom, entries: &[(&Candidate, i32)]| -> Result<usize> {
            let weight = entries.iter().try_fold(0_i32, |sum, (candidate, _)| {
                sum.checked_add(candidate.weight)
                    .context("enchantment weight overflow")
            })?;
            let mut value = random
                .next_int_bound(weight)
                .context("invalid enchantment weight")?;
            for (index, (candidate, _)) in entries.iter().enumerate() {
                value -= candidate.weight;
                if value < 0 {
                    return Ok(index);
                }
            }
            unreachable!("validated weighted selection")
        };
        let index = choose(&mut staged, &available)?;
        selected.push(available[index]);
        while staged.next_int_bound(50).expect("positive bound") <= power {
            let previous = selected.last().expect("initial selection").0;
            available.retain(|(candidate, _)| {
                candidate.id != previous.id
                    && !candidate.exclusive.contains(&previous.id)
                    && !previous.exclusive.contains(&candidate.id)
            });
            if available.is_empty() {
                break;
            }
            let index = choose(&mut staged, &available)?;
            selected.push(available[index]);
            power /= 2;
        }
    }
    let result = selected
        .into_iter()
        .map(|(candidate, level)| Selected {
            id: candidate.id.clone(),
            level,
        })
        .collect();
    *random = staged;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::random::LegacyRandom;

    #[test]
    fn invalid_inputs_leave_random_unchanged() -> Result<()> {
        let mut random = RegionRandom::Legacy(LegacyRandom::new(37));
        let mut before = random.clone();
        assert!(difficulty_cost(&mut random, 0, 17, 0.5).is_err());
        assert!(difficulty_cost(&mut random, 5, 17, f32::NAN).is_err());
        assert!(select(&mut random, 10, Some(0), &[]).is_err());
        let invalid = Candidate {
            id: ResourceId::parse("example:bad")?,
            weight: 0,
            levels: vec![LevelCost {
                level: 1,
                min: 0,
                max: 100,
            }],
            exclusive: BTreeSet::new(),
        };
        assert!(select(&mut random, 10, Some(1), &[invalid]).is_err());
        assert_eq!(random.next_long(), before.next_long());
        Ok(())
    }

    #[test]
    fn exclusive_holders_cannot_be_selected_together() -> Result<()> {
        let first = ResourceId::parse("example:first")?;
        let second = ResourceId::parse("example:second")?;
        let candidates = vec![
            Candidate {
                id: first.clone(),
                weight: 1,
                levels: vec![LevelCost {
                    level: 1,
                    min: 0,
                    max: 1000,
                }],
                exclusive: BTreeSet::from([second.clone()]),
            },
            Candidate {
                id: second,
                weight: 1,
                levels: vec![LevelCost {
                    level: 1,
                    min: 0,
                    max: 1000,
                }],
                exclusive: BTreeSet::new(),
            },
        ];
        for seed in 0..256 {
            let mut random = RegionRandom::Legacy(LegacyRandom::new(seed));
            assert_eq!(select(&mut random, 100, Some(1), &candidates)?.len(), 1);
        }
        let mut random = RegionRandom::Legacy(LegacyRandom::new(37));
        let mut before = random.clone();
        assert!(select(&mut random, 100, None, &candidates)?.is_empty());
        assert_eq!(random.next_long(), before.next_long());
        Ok(())
    }
}
