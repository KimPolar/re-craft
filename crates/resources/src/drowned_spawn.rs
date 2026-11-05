//! Java 26.3 drowned spawn draws and structure initialization order.
use anyhow::{Result, ensure};
use native_core::random::LegacyRandom;

use crate::worldgen_region_random::{MobBaseInitialization, RegionRandom, finalize_mob_base};

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
    /// Java equips the shell and marks its offhand drop as guaranteed.
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

/// The caller resolves live chicken availability, initializes a newly created
/// jockey, and applies the active data pack's mob-spawn enchantment provider.
pub trait StructureDrownedWorld: NautilusJockeyWorld {
    /// Search in Java order and attempt riding the first unmounted chicken.
    fn try_existing_chicken(&mut self) -> Result<bool>;
    /// Create, initialize, mount and add a chicken jockey when available.
    fn spawn_chicken_jockey(&mut self, random: &mut RegionRandom) -> Result<bool>;
    /// Apply MOB_SPAWN_EQUIPMENT to the current main-hand item.
    fn enchant_weapon(
        &mut self,
        random: &mut RegionRandom,
        weapon: MainHandRoll,
        special: f32,
    ) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChickenJockey {
    None,
    Existing,
    Created,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HalloweenHelmet {
    CarvedPumpkin,
    JackOLantern,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StructureDrownedInitialization {
    pub mob: MobBaseInitialization,
    pub can_pick_up_loot: bool,
    pub baby: bool,
    pub chicken_jockey: ChickenJockey,
    pub can_break_doors: bool,
    pub main_hand: MainHandRoll,
    /// A holiday helmet has a zero equipment drop chance.
    pub helmet: Option<HalloweenHelmet>,
    pub attributes: DrownedAttributes,
    pub post: DrownedPostInitialization,
}

/// Initialize a newly constructed structure drowned with empty equipment and
/// no group data. This composes both independent random streams in Java order.
/// The world draft, callback-owned state, and both RNGs must be discarded on
/// any error; the caller creates the entity and applies/serializes these fields.
pub fn initialize_structure_drowned(
    world: &mut impl StructureDrownedWorld,
    region_random: &mut RegionRandom,
    entity_random: &mut LegacyRandom,
    special_multiplier: f32,
    halloween: bool,
) -> Result<StructureDrownedInitialization> {
    ensure!(
        special_multiplier.is_finite() && (0.0..=1.0).contains(&special_multiplier),
        "invalid special difficulty multiplier"
    );
    let mob = finalize_mob_base(region_random, false);
    let can_pick_up_loot = region_random.next_float() < 0.55_f32 * special_multiplier;
    let baby = region_random.next_float() < 0.05_f32;
    let mut chicken_jockey = ChickenJockey::None;
    if baby {
        if f64::from(region_random.next_float()) < 0.05_f64 {
            if world.try_existing_chicken()? {
                chicken_jockey = ChickenJockey::Existing;
            }
        } else if f64::from(region_random.next_float()) < 0.05_f64
            && world.spawn_chicken_jockey(region_random)?
        {
            chicken_jockey = ChickenJockey::Created;
        }
    }
    let mut can_break_doors = region_random.next_float() < special_multiplier * 0.1_f32;
    let main_hand = populate_main_hand(region_random);
    if main_hand != MainHandRoll::Unchanged
        && region_random.next_float() < 0.25_f32 * special_multiplier
    {
        world.enchant_weapon(region_random, main_hand, special_multiplier)?;
    }
    let helmet = if halloween && region_random.next_float() < 0.25_f32 {
        Some(if region_random.next_float() < 0.1_f32 {
            HalloweenHelmet::JackOLantern
        } else {
            HalloweenHelmet::CarvedPumpkin
        })
    } else {
        None
    };
    let attributes = roll_attributes(entity_random, special_multiplier)?;
    if attributes.leader_health_bonus.is_some() {
        can_break_doors = true;
    }
    let post = finalize_after_zombie(
        region_random,
        world,
        true,
        main_hand == MainHandRoll::Trident,
        baby,
        DrownedSpawnReason::Structure,
    )?;
    Ok(StructureDrownedInitialization {
        mob,
        can_pick_up_loot,
        baby,
        chicken_jockey,
        can_break_doors,
        main_hand,
        helmet,
        attributes,
        post,
    })
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
    impl StructureDrownedWorld for World {
        fn try_existing_chicken(&mut self) -> Result<bool> {
            Ok(false)
        }
        fn spawn_chicken_jockey(&mut self, random: &mut RegionRandom) -> Result<bool> {
            random.next_int();
            Ok(true)
        }
        fn enchant_weapon(
            &mut self,
            random: &mut RegionRandom,
            _: MainHandRoll,
            _: f32,
        ) -> Result<()> {
            random.next_int();
            Ok(())
        }
    }
    #[test]
    fn invalid_difficulty_does_not_consume_entity_random() {
        let mut random = LegacyRandom::new(42);
        let before = random.state();
        assert!(roll_attributes(&mut random, f32::NAN).is_err());
        assert!(roll_attributes(&mut random, 1.1).is_err());
        assert_eq!(random.state(), before);
        let mut region = RegionRandom::Legacy(LegacyRandom::new(77));
        let mut reference = region.clone();
        let mut world = World::default();
        assert!(
            initialize_structure_drowned(&mut world, &mut region, &mut random, f32::NAN, false)
                .is_err()
        );
        assert_eq!(random.state(), before);
        assert_eq!(region.next_long(), reference.next_long());
    }

    #[test]
    fn structure_drowned_handles_successful_chicken_and_halloween() {
        let mut found_chicken = false;
        let mut found_pumpkin = false;
        for seed in 0..100_000 {
            let mut region = RegionRandom::Legacy(LegacyRandom::new(seed));
            let mut entity = LegacyRandom::new(seed.wrapping_add(100));
            let mut world = World::default();
            let initialized =
                initialize_structure_drowned(&mut world, &mut region, &mut entity, 1.0, true)
                    .unwrap();
            if initialized.chicken_jockey == ChickenJockey::Created {
                assert!(initialized.baby);
                found_chicken = true;
            }
            found_pumpkin |= initialized.helmet.is_some();
            if found_chicken && found_pumpkin {
                break;
            }
        }
        assert!(found_chicken && found_pumpkin);
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
