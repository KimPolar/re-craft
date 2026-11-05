//! Java 26.3 initial-spawn bonus chest placement.
//!
//! The world and random stream must be isolated until the caller commits the
//! result. A failed callback may have already changed both.
use anyhow::{Context, Result, ensure};
use native_core::random::{DecorationRandom, LegacyRandom, XoroshiroRandom};

use crate::packs::{ResourceId, ResourceProvider};

/// Loot table assigned by the built-in feature; its contents come from the
/// active data packs when the container is opened.
pub const BONUS_CHEST_LOOT_TABLE: &str = "minecraft:chests/spawn_bonus_chest";

/// The active data pack may replace or remove the initial-spawn feature.
pub struct BonusChestFeature;

impl BonusChestFeature {
    pub fn load(resources: &mut impl ResourceProvider) -> Result<Option<Self>> {
        const PATH: &str = "data/minecraft/worldgen/feature/bonus_chest.json";
        let Some(bytes) = resources.read_data(PATH)? else {
            return Ok(None);
        };
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("invalid bonus chest feature")?;
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .context("bonus chest feature type is missing")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "bonus_chest",
            "unsupported initial-spawn feature type"
        );
        Ok(Some(Self))
    }

    pub fn place(
        &self,
        world: &mut impl BonusChestWorld,
        random: &mut impl BonusChestRandom,
        origin: [i32; 3],
    ) -> Result<Option<[i32; 3]>> {
        place_bonus_chest(world, random, origin)
    }
}

/// The level random source used by the caller when initial spawn is selected.
pub trait BonusChestRandom {
    fn bounded(&mut self, bound: i32) -> i32;
    fn loot_seed(&mut self) -> i64;
}

macro_rules! random_impl {
    ($type:ty) => {
        impl BonusChestRandom for $type {
            fn bounded(&mut self, bound: i32) -> i32 {
                self.next_int_bound(bound).expect("positive shuffle bound")
            }
            fn loot_seed(&mut self) -> i64 {
                self.next_long()
            }
        }
    };
}
random_impl!(LegacyRandom);
random_impl!(XoroshiroRandom);
random_impl!(DecorationRandom);

/// World callbacks must use the active registry and world state. In
/// particular, torch survival does not require the destination to be air.
pub trait BonusChestWorld {
    fn motion_blocking_no_leaves(&mut self, x: i32, z: i32) -> Result<[i32; 3]>;
    fn is_empty_block(&mut self, pos: [i32; 3]) -> Result<bool>;
    fn collision_shape_empty(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// Apply Java's flags=2 write; its boolean result is ignored by the feature.
    fn set_chest(&mut self, pos: [i32; 3]) -> Result<()>;
    fn has_randomizable_container(&mut self, pos: [i32; 3]) -> Result<bool>;
    fn set_bonus_chest_loot(&mut self, pos: [i32; 3], seed: i64) -> Result<()>;
    fn torch_can_survive(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// Apply Java's flags=2 write; the previous block need not be empty.
    fn set_torch(&mut self, pos: [i32; 3]) -> Result<()>;
}

fn shuffle(values: &mut [i32; 16], random: &mut impl BonusChestRandom) {
    for end in (2..=values.len()).rev() {
        values.swap(end - 1, random.bounded(end as i32) as usize);
    }
}

/// Returns the selected chest position, or None when every column is blocked.
pub fn place_bonus_chest(
    world: &mut impl BonusChestWorld,
    random: &mut impl BonusChestRandom,
    origin: [i32; 3],
) -> Result<Option<[i32; 3]>> {
    let min_x = origin[0] & !15;
    let min_z = origin[2] & !15;
    let mut xs = std::array::from_fn(|i| min_x.wrapping_add(i as i32));
    let mut zs = std::array::from_fn(|i| min_z.wrapping_add(i as i32));
    shuffle(&mut xs, random);
    shuffle(&mut zs, random);
    for x in xs {
        for &z in &zs {
            let pos = world.motion_blocking_no_leaves(x, z)?;
            if !world.is_empty_block(pos)? && !world.collision_shape_empty(pos)? {
                continue;
            }
            world.set_chest(pos)?;
            if world.has_randomizable_container(pos)? {
                world.set_bonus_chest_loot(pos, random.loot_seed())?;
            }
            for [dx, dz] in [[0i32, -1i32], [1, 0], [0, 1], [-1, 0]] {
                let target = [pos[0].wrapping_add(dx), pos[1], pos[2].wrapping_add(dz)];
                if world.torch_can_survive(target)? {
                    world.set_torch(target)?;
                }
            }
            return Ok(Some(pos));
        }
    }
    Ok(None)
}
