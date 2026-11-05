//! Java 26.3 drowned spawn draws, before enchantments and jockey handling.
use anyhow::{Result, ensure};
use native_core::random::LegacyRandom;

use crate::worldgen_region_random::RegionRandom;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MainHandRoll {
    Unchanged,
    Trident,
    FishingRod,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrownedSpawnReason {
    Natural,
    Structure,
    Other,
}

/// The caller must finalize and add the nautilus in Java order, including its
/// own random draws and passenger linkage. Return true if an entity was
/// created, even if addFreshEntity returned false; return false on null.
pub trait NautilusJockeyWorld {
    fn more_frequent_drowned_biome(&mut self) -> Result<bool>;
    fn spawn_zombie_nautilus(
        &mut self,
        random: &mut RegionRandom,
        persistent: bool,
    ) -> Result<bool>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrownedPostInitialization {
    pub give_nautilus_shell: bool,
    pub spawned_nautilus: bool,
}

/// Drowned.finalizeSpawn after Zombie.finalizeSpawn. The caller supplies the
/// current hand and baby state and must discard its draft and random on error.
pub fn finalize_after_zombie(
    random: &mut RegionRandom,
    world: &mut impl NautilusJockeyWorld,
    offhand_empty: bool,
    mainhand_trident: bool,
    baby: bool,
    reason: DrownedSpawnReason,
) -> Result<DrownedPostInitialization> {
    let give_nautilus_shell = offhand_empty && random.next_float() < 0.03_f32;
    let mut spawned_nautilus = false;
    if matches!(
        reason,
        DrownedSpawnReason::Natural | DrownedSpawnReason::Structure
    ) && mainhand_trident
        && random.next_float() < 0.5_f32
        && !baby
        && !world.more_frequent_drowned_biome()?
    {
        spawned_nautilus =
            world.spawn_zombie_nautilus(random, reason == DrownedSpawnReason::Structure)?;
    }
    Ok(DrownedPostInitialization {
        give_nautilus_shell,
        spawned_nautilus,
    })
}

/// Drowned override Zombie's equipment population and do not call its armor
/// or iron-weapon implementation. Unchanged preserves any existing main hand.
pub fn populate_main_hand(random: &mut RegionRandom) -> MainHandRoll {
    if f64::from(random.next_float()) <= 0.9 {
        return MainHandRoll::Unchanged;
    }
    if random.next_int_bound(16).expect("positive equipment bound") < 10 {
        MainHandRoll::Trident
    } else {
        MainHandRoll::FishingRod
    }
}

/// Modifiers drawn from the entity's own random source, not the region RNG.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrownedAttributes {
    pub reinforcement_base: f64,
    pub knockback_bonus: f64,
    pub zombie_follow_range_bonus: Option<f64>,
    pub leader_reinforcement_bonus: Option<f64>,
    pub leader_health_bonus: Option<f64>,
}

/// Apply the fresh zombie attribute rolls used by drowned, retaining the
/// exact Java float-to-double conversions. The caller applies the resulting
/// modifiers and sets leader health and door breaking when appropriate.
pub fn roll_attributes(
    random: &mut LegacyRandom,
    special_multiplier: f32,
) -> Result<DrownedAttributes> {
    ensure!(
        special_multiplier.is_finite() && (0.0..=1.0).contains(&special_multiplier),
        "invalid special difficulty multiplier"
    );
    let reinforcement_base = random.next_double() * f64::from(0.1_f32);
    let knockback_bonus = random.next_double() * f64::from(0.05_f32);
    let follow_range = random.next_double() * 1.5 * f64::from(special_multiplier);
    let leader = random.next_float() < special_multiplier * 0.05_f32;
    let (leader_reinforcement_bonus, leader_health_bonus) = if leader {
        (
            Some(random.next_double() * 0.25 + 0.5),
            Some(random.next_double() * 3.0 + 1.0),
        )
    } else {
        (None, None)
    };
    Ok(DrownedAttributes {
        reinforcement_base,
        knockback_bonus,
        zombie_follow_range_bonus: (follow_range > 1.0).then_some(follow_range),
        leader_reinforcement_bonus,
        leader_health_bonus,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct World {
        biome_calls: usize,
        spawn_calls: usize,
        persistent: Option<bool>,
    }
    impl NautilusJockeyWorld for World {
        fn more_frequent_drowned_biome(&mut self) -> Result<bool> {
            self.biome_calls += 1;
            Ok(false)
        }
        fn spawn_zombie_nautilus(
            &mut self,
            random: &mut RegionRandom,
            persistent: bool,
        ) -> Result<bool> {
            self.spawn_calls += 1;
            self.persistent = Some(persistent);
            random.next_int();
            Ok(true)
        }
    }
    #[test]
    fn invalid_difficulty_does_not_consume_entity_random() {
        let mut random = LegacyRandom::new(42);
        let before = random.state();
        assert!(roll_attributes(&mut random, f32::NAN).is_err());
        assert!(roll_attributes(&mut random, 1.1).is_err());
        assert_eq!(random.state(), before);
    }

    #[test]
    fn post_spawn_respects_short_circuit_and_callback_random() {
        let seed = (0..100_000)
            .find(|seed| {
                let mut random = LegacyRandom::new(*seed);
                random.next_float() < 0.03 && random.next_float() < 0.5
            })
            .unwrap();
        let make_random = || RegionRandom::Legacy(LegacyRandom::new(seed));
        let mut world = World::default();
        let mut random = make_random();
        let mut reference = make_random();
        reference.next_float();
        reference.next_float();
        reference.next_int();
        let result = finalize_after_zombie(
            &mut random,
            &mut world,
            true,
            true,
            false,
            DrownedSpawnReason::Structure,
        )
        .unwrap();
        assert!(result.give_nautilus_shell && result.spawned_nautilus);
        assert_eq!(
            (world.biome_calls, world.spawn_calls, world.persistent),
            (1, 1, Some(true))
        );
        assert_eq!(random.next_long(), reference.next_long());

        let mut world = World::default();
        let mut random = make_random();
        let mut reference = make_random();
        reference.next_float();
        reference.next_float();
        let result = finalize_after_zombie(
            &mut random,
            &mut world,
            true,
            true,
            true,
            DrownedSpawnReason::Structure,
        )
        .unwrap();
        assert!(result.give_nautilus_shell && !result.spawned_nautilus);
        assert_eq!((world.biome_calls, world.spawn_calls), (0, 0));
        assert_eq!(random.next_long(), reference.next_long());

        let mut world = World::default();
        let mut random = make_random();
        let mut reference = make_random();
        finalize_after_zombie(
            &mut random,
            &mut world,
            false,
            true,
            false,
            DrownedSpawnReason::Other,
        )
        .unwrap();
        assert_eq!((world.biome_calls, world.spawn_calls), (0, 0));
        assert_eq!(random.next_long(), reference.next_long());
    }
}
