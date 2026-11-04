//! The independent entity RNG used during fresh animal construction.
use anyhow::{Context, Result, ensure};
use native_core::random::LegacyRandom;
use native_resources::packs::ResourceId;

use super::FreshCreatureConstruction;

/// Construction outputs and the still-live entity RNG. The region RNG and
/// the chunk spawner's decoration RNG are separate sources.
#[derive(Clone, Debug)]
pub struct FreshCreatureRandom {
    pub construction: FreshCreatureConstruction,
    pub initial_yaw: f32,
    pub random: LegacyRandom,
}

impl FreshCreatureRandom {
    /// Use independent operating-system entropy for a new entity's seed.
    /// The exact seed is not derived from the world seed in vanilla.
    pub fn from_entropy(entity: &ResourceId) -> Result<Self> {
        validate_entity(entity)?;
        let mut seed = [0_u8; 8];
        getrandom::fill(&mut seed)
            .map_err(|error| anyhow::anyhow!("cannot seed entity RNG: {error}"))?;
        Self::from_seed(entity, i64::from_ne_bytes(seed))
    }

    /// A supplied seed allows deterministic comparison against Java's
    /// LegacyRandomSource. Retain `random` when instantiating a live entity.
    pub fn from_seed(entity: &ResourceId, seed: i64) -> Result<Self> {
        Self::from_random(entity, LegacyRandom::new(seed))
    }

    pub fn from_random(entity: &ResourceId, mut random: LegacyRandom) -> Result<Self> {
        validate_entity(entity)?;
        let most = (random.next_long() as u64 & 0xffff_ffff_ffff_0fff) | 0x4000;
        let least = (random.next_long() as u64 & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
        let uuid = [
            (most >> 32) as i32,
            most as i32,
            (least >> 32) as i32,
            least as i32,
        ];
        // LivingEntity's constructor consumes the same random source before
        // Chicken's constructor draws its egg-laying delay.
        let initial_yaw = random.next_float() * std::f32::consts::TAU;
        let chicken_egg_time = if entity.path == "chicken" {
            Some(
                6000 + random
                    .next_int_bound(6000)
                    .context("invalid chicken timer bound")?,
            )
        } else {
            None
        };
        Ok(Self {
            construction: FreshCreatureConstruction {
                uuid,
                chicken_egg_time,
            },
            initial_yaw,
            random,
        })
    }
}

fn validate_entity(entity: &ResourceId) -> Result<()> {
    ensure!(
        entity.namespace == "minecraft"
            && matches!(
                entity.path.as_str(),
                "sheep" | "pig" | "cow" | "chicken" | "horse"
            ),
        "unsupported creature construction"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unverified_species_and_keeps_independent_random_state() {
        let chicken = ResourceId::parse("minecraft:chicken").unwrap();
        let first = FreshCreatureRandom::from_seed(&chicken, 42).unwrap();
        let second = FreshCreatureRandom::from_seed(&chicken, 42).unwrap();
        assert_eq!(first.construction, second.construction);
        assert_eq!(first.random.state(), second.random.state());
        assert!((6000..12000).contains(&first.construction.chicken_egg_time.unwrap()));
        assert_eq!((first.construction.uuid[1] as u32) & 0xf000, 0x4000);
        assert_eq!((first.construction.uuid[2] as u32) >> 30, 2);
        assert!(
            FreshCreatureRandom::from_seed(&ResourceId::parse("minecraft:donkey").unwrap(), 42)
                .is_err()
        );
    }
}
