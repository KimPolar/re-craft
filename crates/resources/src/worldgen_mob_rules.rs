//! Verified animal placement rules for Java 26.3 chunk generation.
use crate::{
    heightmap_tags::resolve,
    packs::{ResourceId, ResourceProvider},
    transforms::read_block_tag,
};
use anyhow::{Context, Result, bail};
use glam::IVec3;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroundSpawnTag {
    Animals,
    Goats,
    Rabbits,
}

#[derive(Clone, Debug)]
pub struct GroundSpawnTags {
    animals: BTreeSet<String>,
    goats: BTreeSet<String>,
    rabbits: BTreeSet<String>,
}

impl GroundSpawnTags {
    /// Resolve all three tags from the same active pack stack and complete
    /// block registry as the generated world.
    pub fn load(provider: &mut impl ResourceProvider, registry: &BTreeSet<String>) -> Result<Self> {
        let ids = [
            "minecraft:animals_spawnable_on",
            "minecraft:goats_spawnable_on",
            "minecraft:rabbits_spawnable_on",
        ]
        .map(|id| ResourceId::parse(id).expect("built-in tag ID"));
        let mut raw = BTreeMap::new();
        let mut failed = BTreeSet::new();
        for id in &ids {
            read_block_tag(provider, id, &mut raw, &mut failed, 0)?;
        }
        let mut cache = BTreeMap::new();
        let mut get = |id| resolve(id, &raw, registry, &mut cache, &mut BTreeSet::new());
        Ok(Self {
            animals: get(&ids[0])?,
            goats: get(&ids[1])?,
            rabbits: get(&ids[2])?,
        })
    }
    pub fn contains(&self, tag: GroundSpawnTag, block: &str) -> bool {
        match tag {
            GroundSpawnTag::Animals => &self.animals,
            GroundSpawnTag::Goats => &self.goats,
            GroundSpawnTag::Rabbits => &self.rabbits,
        }
        .contains(block)
    }
}

pub trait GroundCreatureSpawnWorld {
    fn raw_brightness(&mut self, position: IVec3) -> Result<u8>;
    fn tagged_ground(&mut self, position: IVec3, tag: GroundSpawnTag) -> Result<bool>;
}

/// Inputs used by Animal.getWalkTargetValue after an entity is constructed.
/// Chunk-generation regions use zero sky darkening.
pub trait AnimalInstanceSpawnWorld {
    fn is_grass_block(&mut self, position: IVec3) -> Result<bool>;
    fn raw_brightness(&mut self, position: IVec3) -> Result<u8>;
    fn ambient_light(&mut self) -> Result<f32>;
}

/// `PathfinderMob.checkSpawnRules` for the verified animal classes. Java
/// checks the snapped entity's block position, not its earlier candidate.
/// Call only after successful entity construction and yaw assignment.
pub fn check_animal_instance_spawn(
    entity: &ResourceId,
    position: IVec3,
    world: &mut impl AnimalInstanceSpawnWorld,
) -> Result<bool> {
    if entity.namespace != "minecraft"
        || !matches!(
            entity.path.as_str(),
            "pig" | "cow" | "sheep" | "chicken" | "horse" | "goat" | "rabbit"
        )
    {
        bail!("unsupported animal instance spawn rule");
    }
    let below = IVec3::new(
        position.x,
        position
            .y
            .checked_sub(1)
            .context("spawn ground Y underflow")?,
        position.z,
    );
    if world.is_grass_block(below)? {
        return Ok(true);
    }
    let light = if !(-30_000_000..30_000_000).contains(&position.x)
        || !(-30_000_000..30_000_000).contains(&position.z)
    {
        15
    } else {
        world.raw_brightness(position)?
    };
    anyhow::ensure!(light <= 15, "invalid spawn brightness");
    let fraction = f32::from(light) / 15.0_f32;
    let level = fraction / (4.0_f32 - 3.0_f32 * fraction);
    let ambient = world.ambient_light()?;
    anyhow::ensure!(ambient.is_finite(), "invalid dimension ambient light");
    let magic = level + ambient * (1.0_f32 - level);
    Ok(magic - 0.5_f32 >= 0.0_f32)
}

fn bright_enough(world: &mut impl GroundCreatureSpawnWorld, position: IVec3) -> Result<bool> {
    let light = world.raw_brightness(position)?;
    anyhow::ensure!(light <= 15, "invalid spawn brightness");
    Ok(light > 8)
}

/// The recognized rules consume no random numbers. Other entity types are
/// intentionally unresolved rather than using the animal rule by default.
pub fn check_ground_creature_spawn(
    entity: &ResourceId,
    position: IVec3,
    world: &mut impl GroundCreatureSpawnWorld,
) -> Result<bool> {
    if entity.namespace != "minecraft" {
        bail!("unsupported creature spawn rule");
    }
    let (tag, light_first) = match entity.path.as_str() {
        "pig" | "cow" | "sheep" | "chicken" | "horse" => (GroundSpawnTag::Animals, true),
        "goat" => (GroundSpawnTag::Goats, false),
        "rabbit" => (GroundSpawnTag::Rabbits, false),
        _ => bail!("unsupported creature spawn rule"),
    };
    let below = IVec3::new(
        position.x,
        position
            .y
            .checked_sub(1)
            .context("spawn ground Y underflow")?,
        position.z,
    );
    if light_first {
        let light = bright_enough(world, position)?;
        let ground = world.tagged_ground(below, tag)?;
        Ok(ground && light)
    } else if world.tagged_ground(below, tag)? {
        bright_enough(world, position)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/block/animals_spawnable_on.json" => {
                    Some(br##"{"values":["#example:ground"]}"##.to_vec())
                }
                "data/minecraft/tags/block/goats_spawnable_on.json" => {
                    Some(br#"{"values":["minecraft:stone"]}"#.to_vec())
                }
                "data/minecraft/tags/block/rabbits_spawnable_on.json" => {
                    Some(br#"{"values":[]}"#.to_vec())
                }
                "data/example/tags/block/ground.json" => {
                    Some(br#"{"values":["minecraft:dirt"]}"#.to_vec())
                }
                _ => None,
            })
        }
    }
    #[test]
    fn loads_active_nested_ground_tags_without_mixing_species() {
        let registry = BTreeSet::from(["minecraft:dirt".into(), "minecraft:stone".into()]);
        let tags = GroundSpawnTags::load(&mut Data, &registry).unwrap();
        assert!(tags.contains(GroundSpawnTag::Animals, "minecraft:dirt"));
        assert!(!tags.contains(GroundSpawnTag::Goats, "minecraft:dirt"));
        assert!(tags.contains(GroundSpawnTag::Goats, "minecraft:stone"));
        assert!(!tags.contains(GroundSpawnTag::Rabbits, "minecraft:stone"));
        assert!(GroundSpawnTags::load(&mut Data, &BTreeSet::new()).is_err());
    }

    #[test]
    fn instance_rule_uses_grass_ambient_and_world_bounds() {
        struct World {
            grass: bool,
            light: u8,
            ambient: f32,
            light_calls: usize,
            ambient_calls: usize,
        }
        impl AnimalInstanceSpawnWorld for World {
            fn is_grass_block(&mut self, _: IVec3) -> Result<bool> {
                Ok(self.grass)
            }
            fn raw_brightness(&mut self, _: IVec3) -> Result<u8> {
                self.light_calls += 1;
                Ok(self.light)
            }
            fn ambient_light(&mut self) -> Result<f32> {
                self.ambient_calls += 1;
                Ok(self.ambient)
            }
        }
        let pig = ResourceId::parse("minecraft:pig").unwrap();
        let mut world = World {
            grass: true,
            light: 0,
            ambient: 0.0,
            light_calls: 0,
            ambient_calls: 0,
        };
        assert!(check_animal_instance_spawn(&pig, IVec3::new(1, 64, 1), &mut world).unwrap());
        assert_eq!((world.light_calls, world.ambient_calls), (0, 0));
        world.grass = false;
        world.light = 9;
        assert!(!check_animal_instance_spawn(&pig, IVec3::new(1, 64, 1), &mut world).unwrap());
        world.ambient = 0.5;
        assert!(check_animal_instance_spawn(&pig, IVec3::new(1, 64, 1), &mut world).unwrap());
        world.ambient = 0.0;
        let before = world.light_calls;
        assert!(
            check_animal_instance_spawn(&pig, IVec3::new(30_000_000, 64, 1), &mut world).unwrap()
        );
        assert_eq!(world.light_calls, before);
        assert!(
            check_animal_instance_spawn(
                &ResourceId::parse("minecraft:slime").unwrap(),
                IVec3::ZERO,
                &mut world
            )
            .is_err()
        );
    }
}
