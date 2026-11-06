//! Incrementally execute verified farm-animal paths for one generation region.
use anyhow::{Context, Result, ensure};
use native_resources::{
    animal_variants::{FarmAnimal, FarmAnimalSpawnVariants},
    heightmap_tags::HeightmapTags,
    mob_spawn_placement::MobSpawnHeightmap,
    worldgen_mob_rules::GroundSpawnTags,
    worldgen_mob_spawner::{ChunkCreatureSpawner, CreatureSpawnProgress},
    worldgen_region_random::{
        AnimalGroupData, DonkeyGroupData, HorseGroupData, RegionRandom, SheepBiomeTags,
    },
};
use native_world::loaded::ChunkPos;
use native_world::movement::BoundingBox;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::{
    FreshTerrainBatch, GeneratedCreatureCollisionProgress, GeneratedCreatureInstanceRuleProgress,
    GeneratedCreatureObstructionProgress, GeneratedCreaturePose, GeneratedCreatureProbe,
    GeneratedCreatureProbeProgress, GeneratedCreatureRuleProgress, GeneratedMobPlacementView,
};
use crate::respawn::RespawnBorder;

/// All tags must be from the same unchanged data-pack stack as the batch.
#[derive(Clone, Copy)]
pub struct SheepSpawnInputs<'a> {
    pub border: RespawnBorder,
    pub prevent_inside: &'a BTreeSet<String>,
    pub height_tags: &'a HeightmapTags,
    pub ground_tags: &'a GroundSpawnTags,
    pub biome_tags: &'a SheepBiomeTags,
    pub has_ceiling: bool,
}

struct OwnedSheepSpawnInputs {
    border: RespawnBorder,
    prevent_inside: BTreeSet<String>,
    height_tags: HeightmapTags,
    ground_tags: GroundSpawnTags,
    biome_tags: SheepBiomeTags,
    has_ceiling: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SheepSpawnProgress {
    Pending,
    Rejected,
    Prepared([i32; 4]),
    Finished,
}

/// Values supplied by an independently constructed entity. A chicken's egg
/// timer must come from that entity's RNG, not from the world-generation RNG.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FreshCreatureConstruction {
    pub uuid: [i32; 4],
    pub chicken_egg_time: Option<i32>,
}

fn farm_index(id: &native_resources::packs::ResourceId) -> Option<usize> {
    if id.namespace != "minecraft" {
        return None;
    }
    match id.path.as_str() {
        "pig" => Some(0),
        "cow" => Some(1),
        "chicken" => Some(2),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Stage {
    Begin,
    Placement,
    Collision,
    Rules,
    Construct,
    Instance,
    Obstruction,
    Record,
    Finished,
    Scheduled,
}

/// Owns the region RNG and group state across resumable candidate checks.
/// Sheep, horses, and donkeys are enabled by default. Pigs, cows, and chickens require
/// their active variant registries; unsupported creatures remain unresolved.
pub struct GeneratedSheepSpawner {
    planner: ChunkCreatureSpawner,
    random: RegionRandom,
    center: ChunkPos,
    stamp: super::terrain_batch::SpawnStamp,
    inputs: Arc<OwnedSheepSpawnInputs>,
    group_id: Option<u64>,
    group: AnimalGroupData,
    horse_group: HorseGroupData,
    donkey_group: DonkeyGroupData,
    stage: Stage,
    probe: Option<GeneratedCreatureProbe>,
    pose: Option<GeneratedCreaturePose>,
    construction: Option<FreshCreatureConstruction>,
    farm_variants: [Option<Arc<FarmAnimalSpawnVariants>>; 3],
    farm_moon: Option<(
        Arc<native_resources::moon_phase::Environment>,
        Arc<BTreeMap<native_resources::packs::ResourceId, i64>>,
    )>,
    additions: Vec<super::GeneratedEntityAddition>,
    #[cfg(test)]
    rule_light: Option<(
        ChunkPos,
        native_world::generation_light::VisibleLightSnapshot,
    )>,
}

impl GeneratedSheepSpawner {
    /// Use categories decoded from the installed, verified client JAR.
    pub fn from_verified_environment(
        batch: &FreshTerrainBatch,
        provider: &mut impl native_resources::packs::ResourceProvider,
        center: ChunkPos,
        environment: &native_resources::creature_timeline::CreatureSpawnEnvironment,
        clock_ticks: impl FnMut(&native_resources::packs::ResourceId) -> Option<i64>,
        registry: &native_resources::entity_category::EntityCategoryRegistry,
        inputs: SheepSpawnInputs<'_>,
    ) -> Result<Self> {
        Self::from_timed_environment(
            batch,
            provider,
            center,
            environment,
            clock_ticks,
            |id| registry.is_misc(id),
            inputs,
        )
    }

    /// Load the dimension's already-resolved timeline definitions using the
    /// supplied world clock values before starting this spawn operation.
    pub fn from_timed_environment(
        batch: &FreshTerrainBatch,
        provider: &mut impl native_resources::packs::ResourceProvider,
        center: ChunkPos,
        environment: &native_resources::creature_timeline::CreatureSpawnEnvironment,
        clock_ticks: impl FnMut(&native_resources::packs::ResourceId) -> Option<i64>,
        entity_is_misc: impl FnMut(&native_resources::packs::ResourceId) -> Option<bool>,
        inputs: SheepSpawnInputs<'_>,
    ) -> Result<Self> {
        let later_layers = environment.sample_layers(clock_ticks)?;
        Self::from_environment(
            batch,
            provider,
            center,
            environment.dimension_attributes(),
            &later_layers,
            entity_is_misc,
            inputs,
        )
    }

    /// Resolve the biome-dependent attributes before creating the spawn job.
    /// The caller supplies dimension and already-sampled later layers.
    pub fn from_environment(
        batch: &FreshTerrainBatch,
        provider: &mut impl native_resources::packs::ResourceProvider,
        center: ChunkPos,
        dimension_attributes: &serde_json::Value,
        later_layers: &[serde_json::Value],
        entity_is_misc: impl FnMut(&native_resources::packs::ResourceId) -> Option<bool>,
        inputs: SheepSpawnInputs<'_>,
    ) -> Result<Self> {
        let settings = batch.resolve_creature_spawn_settings(
            provider,
            center,
            dimension_attributes,
            later_layers,
            entity_is_misc,
        )?;
        Self::new(batch, center, settings.probability, settings.spawns, inputs)
    }

    /// The spawn list and probability must contain all applicable environment
    /// layers at this chunk's spawn query position.
    pub fn new(
        batch: &FreshTerrainBatch,
        center: ChunkPos,
        probability: f32,
        spawns: native_resources::mob_spawns::CreatureSpawns,
        inputs: SheepSpawnInputs<'_>,
    ) -> Result<Self> {
        let planner = batch.plan_creature_spawning(center, true, probability, spawns)?;
        let stamp = batch.spawn_stamp()?;
        Ok(Self {
            planner,
            random: batch.spawn_region_random(center),
            center,
            stamp,
            inputs: Arc::new(OwnedSheepSpawnInputs {
                border: inputs.border,
                prevent_inside: inputs.prevent_inside.clone(),
                height_tags: inputs.height_tags.clone(),
                ground_tags: inputs.ground_tags.clone(),
                biome_tags: inputs.biome_tags.clone(),
                has_ceiling: inputs.has_ceiling,
            }),
            group_id: None,
            group: AnimalGroupData::default(),
            horse_group: HorseGroupData::default(),
            donkey_group: DonkeyGroupData::default(),
            stage: Stage::Begin,
            probe: None,
            pose: None,
            construction: None,
            farm_variants: [None, None, None],
            farm_moon: None,
            additions: Vec::new(),
            #[cfg(test)]
            rule_light: None,
        })
    }

    fn rejected(&mut self) -> SheepSpawnProgress {
        self.probe = None;
        self.pose = None;
        self.construction = None;
        self.stage = Stage::Begin;
        SheepSpawnProgress::Rejected
    }

    /// Execute one bounded state transition. `new_uuid` is called only after
    /// placement and registered spawn rules pass, at Java's construction point.
    /// It must return a unique entity UUID from an independent source.
    pub fn advance(
        &mut self,
        batch: &mut FreshTerrainBatch,
        new_uuid: &mut impl FnMut() -> Result<[i32; 4]>,
    ) -> Result<SheepSpawnProgress> {
        self.advance_with_construction(batch, &mut |id| {
            ensure!(
                id.path != "chicken" || id.namespace != "minecraft",
                "chicken construction requires its own egg timer"
            );
            Ok(FreshCreatureConstruction {
                uuid: new_uuid()?,
                chicken_egg_time: None,
            })
        })
    }

    /// Enable one farm-animal species with registries from this batch's
    /// unchanged data-pack stack. Unsupported species stay unresolved.
    pub fn enable_farm_animal(&mut self, variants: Arc<FarmAnimalSpawnVariants>) -> Result<()> {
        ensure!(
            matches!(self.stage, Stage::Begin),
            "cannot change animal registries during a candidate"
        );
        let index = match variants.species() {
            FarmAnimal::Pig => 0,
            FarmAnimal::Cow => 1,
            FarmAnimal::Chicken => 2,
        };
        ensure!(
            self.farm_variants[index].is_none(),
            "animal registry is already enabled"
        );
        self.farm_variants[index] = Some(variants);
        Ok(())
    }

    /// Use the same immutable dimension environment and clocks as generation.
    pub fn set_farm_moon_environment(
        &mut self,
        moon: Arc<native_resources::moon_phase::Environment>,
        clocks: Arc<BTreeMap<native_resources::packs::ResourceId, i64>>,
    ) -> Result<()> {
        ensure!(
            matches!(self.stage, Stage::Begin),
            "cannot change environment during a candidate"
        );
        ensure!(
            self.farm_moon.is_none(),
            "farm moon environment is already set"
        );
        self.farm_moon = Some((moon, clocks));
        Ok(())
    }

    /// Execute one transition, obtaining construction data only at Java's
    /// constructor point. The callback must produce unique independent UUIDs.
    pub fn advance_with_construction(
        &mut self,
        batch: &mut FreshTerrainBatch,
        construct: &mut impl FnMut(
            &native_resources::packs::ResourceId,
        ) -> Result<FreshCreatureConstruction>,
    ) -> Result<SheepSpawnProgress> {
        batch.validate_spawn_stamp(&self.stamp, self.center)?;
        let inputs = Arc::clone(&self.inputs);
        match self.stage {
            Stage::Finished => Ok(SheepSpawnProgress::Finished),
            Stage::Scheduled => anyhow::bail!("creature spawning was already scheduled"),
            Stage::Begin => match self.planner.next(1)? {
                CreatureSpawnProgress::Pending => Ok(SheepSpawnProgress::Pending),
                CreatureSpawnProgress::Finished => {
                    self.stage = Stage::Finished;
                    Ok(SheepSpawnProgress::Finished)
                }
                CreatureSpawnProgress::Attempt(attempt) => {
                    ensure!(
                        (attempt.entity.namespace == "minecraft"
                            && matches!(
                                attempt.entity.path.as_str(),
                                "sheep" | "horse" | "donkey"
                            ))
                            || farm_index(&attempt.entity)
                                .is_some_and(|index| self.farm_variants[index].is_some()),
                        "unsupported generated creature {}:{}",
                        attempt.entity.namespace,
                        attempt.entity.path
                    );
                    let id = self.planner.group_id().context("spawn group is missing")?;
                    if self.group_id != Some(id) {
                        self.group_id = Some(id);
                        self.group = AnimalGroupData::default();
                        self.horse_group = HorseGroupData::default();
                        self.donkey_group = DonkeyGroupData::default();
                    }
                    let (min_y, _) = batch.build_bounds()?;
                    self.probe = Some(GeneratedCreatureProbe::new(
                        attempt,
                        min_y,
                        inputs.has_ceiling,
                        true,
                    ));
                    self.stage = Stage::Placement;
                    Ok(SheepSpawnProgress::Pending)
                }
            },
            Stage::Construct => {
                let CreatureSpawnProgress::Attempt(attempt) = self.planner.next(0)? else {
                    anyhow::bail!("creature construction attempt is missing")
                };
                let construction = construct(&attempt.entity)?;
                if attempt.entity.namespace == "minecraft" && attempt.entity.path == "chicken" {
                    ensure!(
                        construction
                            .chicken_egg_time
                            .is_some_and(|time| (6000..12000).contains(&time)),
                        "invalid constructed chicken egg timer"
                    );
                } else {
                    ensure!(
                        construction.chicken_egg_time.is_none(),
                        "egg timer belongs only to a chicken"
                    );
                }
                let pose = self
                    .probe
                    .as_mut()
                    .context("missing creature probe")?
                    .mark_constructed(&mut self.planner)?;
                self.construction = Some(construction);
                self.pose = Some(pose);
                self.stage = Stage::Instance;
                Ok(SheepSpawnProgress::Pending)
            }
            Stage::Record => {
                let construction = self.construction.context("missing constructed creature")?;
                let attempt = self
                    .planner
                    .created_attempt()
                    .context("missing constructed creature attempt")?;
                let probe = self.probe.as_mut().context("missing creature probe")?;
                let addition = if attempt.entity.path == "horse"
                    && attempt.entity.namespace == "minecraft"
                {
                    probe.prepare_fresh_horse(
                        &mut self.planner,
                        &mut self.random,
                        &mut self.horse_group,
                        construction.uuid,
                    )?
                } else if attempt.entity.path == "donkey" && attempt.entity.namespace == "minecraft"
                {
                    probe.prepare_fresh_donkey(
                        &mut self.planner,
                        &mut self.random,
                        &mut self.donkey_group,
                        construction.uuid,
                    )?
                } else if let Some(index) = farm_index(&attempt.entity) {
                    let variants = self.farm_variants[index]
                        .as_ref()
                        .context("farm animal registry is missing")?;
                    probe.prepare_fresh_farm_animal(
                        &mut self.planner,
                        batch,
                        variants,
                        &mut self.random,
                        &mut self.group,
                        construction.uuid,
                        construction.chicken_egg_time,
                        self.farm_moon
                            .as_ref()
                            .map(|(moon, clocks)| (moon.as_ref(), clocks.as_ref())),
                    )?
                } else {
                    probe.prepare_fresh_sheep(
                        &mut self.planner,
                        batch,
                        &inputs.biome_tags,
                        &mut self.random,
                        &mut self.group,
                        construction.uuid,
                    )?
                };
                self.additions.push(addition);
                self.probe = None;
                self.pose = None;
                self.construction = None;
                self.stage = Stage::Begin;
                Ok(SheepSpawnProgress::Prepared(construction.uuid))
            }
            stage => {
                let mut view = GeneratedMobPlacementView::new(
                    batch,
                    MobSpawnHeightmap::MotionBlockingNoLeaves,
                    inputs.border,
                    &inputs.prevent_inside,
                    &inputs.height_tags,
                )?;
                #[cfg(test)]
                if let Some((pos, light)) = &self.rule_light {
                    view.set_rule_light_for_test(*pos, light.clone());
                }
                let probe = self.probe.as_mut().context("missing creature probe")?;
                match stage {
                    Stage::Placement => match probe.poll(&mut self.planner, &mut view, 1)? {
                        GeneratedCreatureProbeProgress::Pending => {}
                        GeneratedCreatureProbeProgress::NeedsCollision(_) => {
                            self.stage = Stage::Collision
                        }
                        GeneratedCreatureProbeProgress::Rejected => return Ok(self.rejected()),
                    },
                    Stage::Collision => match probe.check_collision(&mut self.planner, &view)? {
                        GeneratedCreatureCollisionProgress::NeedsSpawnRules(_) => {
                            self.stage = Stage::Rules
                        }
                        GeneratedCreatureCollisionProgress::Rejected => return Ok(self.rejected()),
                    },
                    Stage::Rules => match probe.check_spawn_rules(
                        &mut self.planner,
                        &mut view,
                        &inputs.ground_tags,
                    )? {
                        GeneratedCreatureRuleProgress::NeedsConstruction(_) => {
                            self.stage = Stage::Construct
                        }
                        GeneratedCreatureRuleProgress::Rejected => return Ok(self.rejected()),
                    },
                    Stage::Instance => {
                        match probe.check_instance_rules(&mut self.planner, &mut view)? {
                            GeneratedCreatureInstanceRuleProgress::NeedsObstruction(_) => {
                                self.stage = Stage::Obstruction
                            }
                            GeneratedCreatureInstanceRuleProgress::Rejected => {
                                return Ok(self.rejected());
                            }
                        }
                    }
                    Stage::Obstruction => {
                        let geometry = self
                            .pose
                            .context("missing constructed creature pose")?
                            .geometry;
                        let bounds = BoundingBox {
                            min: glam::DVec3::from_array(geometry.min),
                            max: glam::DVec3::from_array(geometry.max),
                        };
                        match probe.check_obstruction(&mut self.planner, &mut view, bounds)? {
                            GeneratedCreatureObstructionProgress::NeedsFinalization(_) => {
                                self.stage = Stage::Record
                            }
                            GeneratedCreatureObstructionProgress::Rejected => {
                                return Ok(self.rejected());
                            }
                        }
                    }
                    _ => unreachable!(),
                }
                Ok(SheepSpawnProgress::Pending)
            }
        }
    }

    /// Schedule SPAWN after every selected creature has been handled and all
    /// generation dependencies are complete. The chunk remains intermediate.
    pub fn schedule_spawn(&mut self, batch: &mut FreshTerrainBatch) -> Result<()> {
        ensure!(
            matches!(self.stage, Stage::Finished),
            "creature spawning is unfinished"
        );
        batch.validate_spawn_stamp(&self.stamp, self.center)?;
        batch.publish_completed_spawn(self.center, &self.additions)?;
        self.additions.clear();
        self.stage = Stage::Scheduled;
        Ok(())
    }

    /// The random source is returned only when all groups were processed.
    pub fn into_random(self) -> Result<RegionRandom> {
        ensure!(
            matches!(self.stage, Stage::Finished | Stage::Scheduled),
            "creature generation is unfinished"
        );
        Ok(self.random)
    }

    #[cfg(test)]
    pub(super) fn prepared_additions(&self) -> &[super::GeneratedEntityAddition] {
        &self.additions
    }

    #[cfg(test)]
    pub(super) fn set_rule_light_for_test(
        &mut self,
        pos: ChunkPos,
        light: native_world::generation_light::VisibleLightSnapshot,
    ) {
        self.rule_light = Some((pos, light));
    }
}
