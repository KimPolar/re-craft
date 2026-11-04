//! Block-backed placement queries for the supported generated block states.
use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_resources::mob_spawn_placement::{MobPlacementWorld, MobSpawnHeightmap};
use native_resources::worldgen_mob_spawner::{
    AttemptRejection, ChunkCreatureSpawner, CreatureAttempt, CreatureSpawnProgress,
};
use native_world::{
    loaded::{BlockState, ChunkPos, World},
    mob_spawn_blocks::{MobSpawnBlockFacts, common_mob_spawn_block_facts},
};
use std::{collections::BTreeSet, sync::Arc};

use super::FreshTerrainBatch;
use crate::respawn::RespawnBorder;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeneratedCreatureProbeProgress {
    Pending,
    Rejected,
    /// Placement passed; entity-size collision and mob rules remain to check.
    NeedsCollision(IVec3),
}

pub struct GeneratedCreatureProbe {
    attempt: CreatureAttempt,
    top_search: native_resources::mob_spawn_placement::TopMobSpawnPosition,
    top: Option<IVec3>,
    can_summon: bool,
    rejected: bool,
}

impl GeneratedCreatureProbe {
    pub fn new(attempt: CreatureAttempt, min_y: i32, has_ceiling: bool, can_summon: bool) -> Self {
        Self {
            top_search: native_resources::mob_spawn_placement::TopMobSpawnPosition::new(
                attempt.x,
                attempt.z,
                min_y,
                has_ceiling,
            ),
            attempt,
            top: None,
            can_summon,
            rejected: false,
        }
    }

    /// Resolve one pending planner attempt against generated blocks. Missing
    /// heightmaps or unsupported block states return an error without
    /// advancing the planner. The caller still performs entity collision.
    pub fn poll(
        &mut self,
        planner: &mut ChunkCreatureSpawner,
        world: &mut GeneratedMobPlacementView<'_>,
        budget: usize,
    ) -> Result<GeneratedCreatureProbeProgress> {
        ensure!(!self.rejected, "creature probe was already rejected");
        ensure!(
            planner.next(0)? == CreatureSpawnProgress::Attempt(self.attempt.clone()),
            "creature planner attempt changed"
        );
        if self.top.is_none() {
            self.top = self.top_search.poll(world, budget)?;
            if self.top.is_none() {
                return Ok(GeneratedCreatureProbeProgress::Pending);
            }
        } else if budget == 0 {
            return Ok(GeneratedCreatureProbeProgress::Pending);
        }
        let top = self.top.expect("top position was resolved");
        if !self.can_summon
            || !native_resources::mob_spawn_placement::on_ground_spawn_position_ok(world, top)?
        {
            planner.reject(AttemptRejection::Placement)?;
            self.rejected = true;
            return Ok(GeneratedCreatureProbeProgress::Rejected);
        }
        Ok(GeneratedCreatureProbeProgress::NeedsCollision(top))
    }
}

pub struct GeneratedMobPlacementView<'a> {
    batch: &'a FreshTerrainBatch,
    world: Arc<World>,
    heightmap: MobSpawnHeightmap,
    border: RespawnBorder,
    prevent_inside: &'a BTreeSet<String>,
    min_y: i32,
    max_y: i32,
    void_air: BlockState,
    last_facts: Option<(IVec3, MobSpawnBlockFacts)>,
}

impl<'a> GeneratedMobPlacementView<'a> {
    /// The caller must resolve the complete active prevent-mob-spawning tag.
    pub fn new(
        batch: &'a FreshTerrainBatch,
        heightmap: MobSpawnHeightmap,
        border: RespawnBorder,
        prevent_inside: &'a BTreeSet<String>,
    ) -> Result<Self> {
        ensure!(
            batch.is_complete(),
            "finish generation before mob placement"
        );
        ensure!(border.is_valid(), "invalid mob spawn border");
        let world = batch.snapshot().context("generated terrain is missing")?;
        let (min_y, max_y) = batch.build_bounds()?;
        Ok(Self {
            batch,
            world,
            heightmap,
            border,
            prevent_inside,
            min_y,
            max_y,
            void_air: BlockState {
                name: "minecraft:void_air".into(),
                properties: Default::default(),
            },
            last_facts: None,
        })
    }
    fn block(&self, pos: IVec3) -> Result<&BlockState> {
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        ensure!(
            self.world.chunks.contains_key(&chunk),
            "mob placement chunk is missing"
        );
        if pos.y < self.min_y || pos.y >= self.max_y {
            return Ok(&self.void_air);
        }
        self.world
            .block(pos.x, pos.y, pos.z)
            .context("mob placement block is missing")
    }
    fn facts(&mut self, pos: IVec3) -> Result<MobSpawnBlockFacts> {
        if let Some((cached_pos, facts)) = self.last_facts {
            if cached_pos == pos {
                return Ok(facts);
            }
        }
        let state = self.block(pos)?;
        let facts = common_mob_spawn_block_facts(state)
            .with_context(|| format!("unsupported mob spawn block state {state:?}"))?;
        self.last_facts = Some((pos, facts));
        Ok(facts)
    }
}

impl MobPlacementWorld for GeneratedMobPlacementView<'_> {
    fn height(&mut self, x: i32, z: i32) -> Result<i32> {
        self.batch
            .primed_mob_spawn_height(x, z, self.heightmap)?
            .context("mob spawn heightmap is not primed")
    }
    fn is_air(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.air)
    }
    fn pathfindable_land(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.pathfindable_land)
    }
    fn within_border(&mut self, pos: IVec3) -> Result<bool> {
        Ok(f64::from(pos.x) >= self.border.min_x
            && f64::from(pos.x) < self.border.max_x
            && f64::from(pos.z) >= self.border.min_z
            && f64::from(pos.z) < self.border.max_z)
    }
    fn valid_spawn_support(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.valid_support)
    }
    fn full_collision(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.full_collision)
    }
    fn signal_source(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.signal_source)
    }
    fn fluid_empty(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.fluid_empty)
    }
    fn prevents_mob_spawning(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.prevent_inside.contains(&self.block(pos)?.name))
    }
    fn dangerous_to_entity(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.dangerous)
    }
}
