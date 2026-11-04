//! Incrementally execute the verified sheep path for one generation region.
use anyhow::{Context, Result, ensure};
use native_resources::{
    heightmap_tags::HeightmapTags,
    mob_spawn_placement::MobSpawnHeightmap,
    worldgen_mob_rules::GroundSpawnTags,
    worldgen_mob_spawner::{ChunkCreatureSpawner, CreatureSpawnProgress},
    worldgen_region_random::{AnimalGroupData, RegionRandom, SheepBiomeTags},
};
use native_world::loaded::ChunkPos;
use native_world::movement::BoundingBox;
use std::{collections::BTreeSet, sync::Arc};

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
/// An unsupported creature stops
/// with its attempt unresolved; this does not mark SPAWN complete or save.
pub struct GeneratedSheepSpawner {
    planner: ChunkCreatureSpawner,
    random: RegionRandom,
    center: ChunkPos,
    stamp: super::terrain_batch::SpawnStamp,
    inputs: Arc<OwnedSheepSpawnInputs>,
    group_id: Option<u64>,
    group: AnimalGroupData,
    stage: Stage,
    probe: Option<GeneratedCreatureProbe>,
    pose: Option<GeneratedCreaturePose>,
    uuid: Option<[i32; 4]>,
    additions: Vec<super::GeneratedEntityAddition>,
    #[cfg(test)]
    rule_light: Option<(
        ChunkPos,
        native_world::generation_light::VisibleLightSnapshot,
    )>,
}

impl GeneratedSheepSpawner {
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
            stage: Stage::Begin,
            probe: None,
            pose: None,
            uuid: None,
            additions: Vec::new(),
            #[cfg(test)]
            rule_light: None,
        })
    }

    fn rejected(&mut self) -> SheepSpawnProgress {
        self.probe = None;
        self.pose = None;
        self.uuid = None;
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
                        attempt.entity.namespace == "minecraft" && attempt.entity.path == "sheep",
                        "unsupported generated creature {}:{}",
                        attempt.entity.namespace,
                        attempt.entity.path
                    );
                    let id = self.planner.group_id().context("spawn group is missing")?;
                    if self.group_id != Some(id) {
                        self.group_id = Some(id);
                        self.group = AnimalGroupData::default();
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
                let uuid = new_uuid()?;
                let pose = self
                    .probe
                    .as_mut()
                    .context("missing creature probe")?
                    .mark_constructed(&mut self.planner)?;
                self.uuid = Some(uuid);
                self.pose = Some(pose);
                self.stage = Stage::Instance;
                Ok(SheepSpawnProgress::Pending)
            }
            Stage::Record => {
                let uuid = self.uuid.context("missing constructed sheep UUID")?;
                let addition = self
                    .probe
                    .as_mut()
                    .context("missing creature probe")?
                    .prepare_fresh_sheep(
                        &mut self.planner,
                        batch,
                        &inputs.biome_tags,
                        &mut self.random,
                        &mut self.group,
                        uuid,
                    )?;
                self.additions.push(addition);
                self.probe = None;
                self.pose = None;
                self.uuid = None;
                self.stage = Stage::Begin;
                Ok(SheepSpawnProgress::Prepared(uuid))
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
                            .context("missing constructed sheep pose")?
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
            "sheep generation is unfinished"
        );
        Ok(self.random)
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
