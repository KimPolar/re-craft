use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use native_core::random::LegacyRandom;
use native_persistence::nbt::Tag;
use native_resources::{
    animal_variants::ZombieNautilusSpawnVariants,
    drowned_spawn::MainHandRoll,
    packs::{ResourceId, ResourceProvider},
    worldgen_region_random::RegionRandom,
};
use native_session::worldgen::{
    BiomeNeighborhood, FreshCreatureRandom, QuartBiomeChunk,
    ocean_ruin::{DrownedNautilusEffects, DrownedSpawnEffects},
};
use native_world::loaded::World;

struct Resources;
impl ResourceProvider for Resources {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(
            (path == "data/minecraft/zombie_nautilus_variant/temperate.json")
                .then(|| br#"{"spawn_conditions":[{"priority":0}]}"#.to_vec()),
        )
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        let name = match folder {
            "worldgen/biome" => "minecraft:ocean",
            "zombie_nautilus_variant" => "minecraft:temperate",
            _ => bail!("unexpected registry {folder}"),
        };
        Ok(vec![ResourceId::parse(name)?])
    }
}

#[derive(Clone)]
struct NoOtherEffects;
impl DrownedSpawnEffects for NoOtherEffects {
    fn try_existing_chicken(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
        bail!("unexpected chicken")
    }
    fn spawn_chicken_jockey(
        &mut self,
        _: [i32; 3],
        _: &World,
        _: &mut RegionRandom,
        _: &BiomeNeighborhood,
    ) -> Result<bool> {
        bail!("unexpected chicken")
    }
    fn enchant_weapon(
        &mut self,
        _: [i32; 3],
        _: &World,
        _: &mut RegionRandom,
        _: MainHandRoll,
        _: f32,
    ) -> Result<Tag> {
        bail!("unexpected enchantment")
    }
    fn spawn_zombie_nautilus(
        &mut self,
        _: [i32; 3],
        _: &World,
        _: &mut RegionRandom,
        _: bool,
    ) -> Result<bool> {
        bail!("unexpected delegation")
    }
}

#[test]
fn stages_nautilus_and_rolls_back_failed_attempts() -> Result<()> {
    let variants = Arc::new(ZombieNautilusSpawnVariants::load_active(&mut Resources)?);
    let id = ResourceId::parse("minecraft:zombie_nautilus")?;
    let creature = FreshCreatureRandom::from_seed(&id, 42)?;
    let mut effects =
        DrownedNautilusEffects::new(variants, move || Ok(Some(creature.clone())), NoOtherEffects);
    let mut biomes = BiomeNeighborhood::new(87, 0, 16)?;
    let ocean = ResourceId::parse("minecraft:ocean")?;
    for z in -1..=1 {
        for x in -1..=1 {
            biomes.insert(Arc::new(QuartBiomeChunk::from_values(
                [x, z],
                87,
                0,
                16,
                vec![ocean.clone(); 64],
            )?))?;
        }
    }
    let world = World::new("minecraft:overworld");
    let position = [3, 10, -2];
    let mut random = RegionRandom::Legacy(LegacyRandom::new(19));
    let mut before = random.clone();
    assert!(
        effects
            .spawn_zombie_nautilus_in_biome(position, &world, &mut random, false, &biomes)
            .is_err()
    );
    assert_eq!(random.next_long(), before.next_long());
    assert!(effects.spawn_zombie_nautilus_in_biome(
        position,
        &world,
        &mut random,
        true,
        &biomes
    )?);
    let mut cloned = effects.clone();
    let mut before = random.clone();
    assert!(
        effects
            .spawn_zombie_nautilus_in_biome(position, &world, &mut random, true, &biomes)
            .is_err()
    );
    assert_eq!(random.next_long(), before.next_long());
    let tag = effects.take_created_nautilus()?.expect("staged nautilus");
    ensure!(
        matches!(tag.get("id"), Some(Tag::String(value)) if value.matches("minecraft:zombie_nautilus")),
        "wrong entity"
    );
    assert_eq!(tag.get("PersistenceRequired"), Some(&Tag::Byte(1)));
    assert_eq!(cloned.take_created_nautilus()?, Some(tag));
    assert!(effects.take_created_nautilus()?.is_none());
    Ok(())
}
