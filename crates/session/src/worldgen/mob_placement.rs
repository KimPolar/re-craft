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
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::FreshTerrainBatch;
use crate::respawn::RespawnBorder;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeneratedCreatureProbeProgress {
    Pending,
    Rejected,
    /// Placement passed; entity-size collision and mob rules remain to check.
    NeedsCollision(IVec3),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeneratedCreatureCollisionProgress {
    Rejected,
    /// WorldGenRegion's null-entity collision query passed; check the
    /// entity type's spawn rules before attempting construction.
    NeedsSpawnRules(native_resources::worldgen_mob_geometry::CreatureSpawnGeometry),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeneratedCreatureRuleProgress {
    Rejected,
    /// The registered spawn predicate passed. Entity creation and the mob's
    /// own spawn checks must still be performed in Java's order.
    NeedsConstruction(native_resources::worldgen_mob_geometry::CreatureSpawnGeometry),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeneratedCreaturePose {
    pub geometry: native_resources::worldgen_mob_geometry::CreatureSpawnGeometry,
    pub yaw: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeneratedCreatureInstanceRuleProgress {
    Rejected,
    NeedsObstruction(GeneratedCreaturePose),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeneratedCreatureObstructionProgress {
    Rejected,
    /// The constructed entity still needs finalizeSpawn and chunk storage.
    NeedsFinalization(GeneratedCreaturePose),
}

pub struct GeneratedCreatureProbe {
    attempt: CreatureAttempt,
    top_search: native_resources::mob_spawn_placement::TopMobSpawnPosition,
    top: Option<IVec3>,
    can_summon: bool,
    rejected: bool,
    placement_passed: bool,
    collision_passed: bool,
    rules_passed: bool,
    constructed_yaw: Option<f32>,
    instance_rules_passed: bool,
    obstruction_passed: bool,
    finalized: bool,
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
            placement_passed: false,
            collision_passed: false,
            rules_passed: false,
            constructed_yaw: None,
            instance_rules_passed: false,
            obstruction_passed: false,
            finalized: false,
        }
    }

    /// Resolve the collision position and spawn AABB after placement passes.
    /// Unknown entity dimensions remain unresolved.
    pub fn collision_geometry(
        &self,
        top: IVec3,
    ) -> Result<native_resources::worldgen_mob_geometry::CreatureSpawnGeometry> {
        ensure!(
            self.placement_passed && !self.rejected && self.top == Some(top),
            "creature placement is unresolved"
        );
        let dimensions = native_resources::worldgen_mob_geometry::known_creature_spawn_dimensions(
            &self.attempt.entity,
        )
        .context("unverified creature spawn dimensions")?;
        native_resources::worldgen_mob_geometry::creature_spawn_geometry(
            self.attempt.x.div_euclid(16),
            self.attempt.z.div_euclid(16),
            self.attempt.x,
            top.y,
            self.attempt.z,
            dimensions,
        )
    }

    /// Test the WorldGenRegion null-entity collision query. Its entity query
    /// is always empty and its border check accepts a null entity. A block
    /// collision skips the random walk; missing data leaves the attempt pending.
    pub fn check_collision(
        &mut self,
        planner: &mut ChunkCreatureSpawner,
        world: &GeneratedMobPlacementView<'_>,
    ) -> Result<GeneratedCreatureCollisionProgress> {
        ensure!(
            planner.next(0)? == CreatureSpawnProgress::Attempt(self.attempt.clone()),
            "creature planner attempt changed"
        );
        let top = self.top.context("creature placement is unresolved")?;
        let geometry = self.collision_geometry(top)?;
        if !super::mob_collision::no_block_collision(geometry, |pos| world.block(pos))? {
            planner.reject(AttemptRejection::CollisionRuleOrConstruction)?;
            self.rejected = true;
            return Ok(GeneratedCreatureCollisionProgress::Rejected);
        }
        self.collision_passed = true;
        Ok(GeneratedCreatureCollisionProgress::NeedsSpawnRules(
            geometry,
        ))
    }

    /// Apply the registered chunk-generation spawn predicate. The tags must
    /// belong to this batch's unchanged data-pack stack. A failed rule skips
    /// the candidate without consuming the planner's random walk.
    pub fn check_spawn_rules(
        &mut self,
        planner: &mut ChunkCreatureSpawner,
        world: &mut GeneratedMobPlacementView<'_>,
        tags: &native_resources::worldgen_mob_rules::GroundSpawnTags,
    ) -> Result<GeneratedCreatureRuleProgress> {
        ensure!(
            self.collision_passed && !self.rejected && !self.rules_passed,
            "creature collision has not passed or rules were already checked"
        );
        ensure!(
            planner.next(0)? == CreatureSpawnProgress::Attempt(self.attempt.clone()),
            "creature planner attempt changed"
        );
        let geometry =
            self.collision_geometry(self.top.context("creature position is missing")?)?;
        let pos = geometry_block_position(geometry)?;
        let mut inputs = GeneratedGroundRuleWorld { view: world, tags };
        if !native_resources::worldgen_mob_rules::check_ground_creature_spawn(
            &self.attempt.entity,
            pos,
            &mut inputs,
        )? {
            planner.reject(AttemptRejection::CollisionRuleOrConstruction)?;
            self.rejected = true;
            return Ok(GeneratedCreatureRuleProgress::Rejected);
        }
        self.rules_passed = true;
        Ok(GeneratedCreatureRuleProgress::NeedsConstruction(geometry))
    }

    /// Call only after the selected entity's real constructor succeeds. Java
    /// draws the yaw from the chunk-generation RNG before its instance rules.
    /// This does not construct or persist an entity.
    pub fn mark_constructed(
        &mut self,
        planner: &mut ChunkCreatureSpawner,
    ) -> Result<GeneratedCreaturePose> {
        ensure!(
            self.rules_passed && !self.rejected && self.constructed_yaw.is_none(),
            "creature is not ready for construction"
        );
        ensure!(
            planner.next(0)? == CreatureSpawnProgress::Attempt(self.attempt.clone()),
            "creature planner attempt changed"
        );
        let geometry =
            self.collision_geometry(self.top.context("creature position is missing")?)?;
        let yaw = planner.entity_created()?;
        self.constructed_yaw = Some(yaw);
        Ok(GeneratedCreaturePose { geometry, yaw })
    }

    /// Check the constructed animal's instance rule before obstruction and
    /// finalization. A rejection performs Java's post-construction walk.
    pub fn check_instance_rules(
        &mut self,
        planner: &mut ChunkCreatureSpawner,
        world: &mut GeneratedMobPlacementView<'_>,
    ) -> Result<GeneratedCreatureInstanceRuleProgress> {
        let yaw = self
            .constructed_yaw
            .context("creature has not been constructed")?;
        ensure!(
            !self.rejected
                && !self.instance_rules_passed
                && planner.created_attempt() == Some(self.attempt.clone()),
            "constructed creature is no longer pending"
        );
        let geometry =
            self.collision_geometry(self.top.context("creature position is missing")?)?;
        let pos = geometry_block_position(geometry)?;
        if !native_resources::worldgen_mob_rules::check_animal_instance_spawn(
            &self.attempt.entity,
            pos,
            world,
        )? {
            planner.finish_entity(false)?;
            self.rejected = true;
            return Ok(GeneratedCreatureInstanceRuleProgress::Rejected);
        }
        self.instance_rules_passed = true;
        Ok(GeneratedCreatureInstanceRuleProgress::NeedsObstruction(
            GeneratedCreaturePose { geometry, yaw },
        ))
    }

    /// Check the constructed entity's actual bounding box. WorldGenRegion's
    /// entity obstruction query is empty; fluids are checked first by Java.
    /// The caller must supply the constructed entity's current box, not the
    /// earlier entity-type spawn box.
    pub fn check_obstruction(
        &mut self,
        planner: &mut ChunkCreatureSpawner,
        world: &mut GeneratedMobPlacementView<'_>,
        entity_bounds: native_world::movement::BoundingBox,
    ) -> Result<GeneratedCreatureObstructionProgress> {
        let yaw = self
            .constructed_yaw
            .context("creature has not been constructed")?;
        ensure!(
            self.instance_rules_passed
                && !self.obstruction_passed
                && !self.rejected
                && planner.created_attempt() == Some(self.attempt.clone()),
            "creature instance rules have not passed or entity is no longer pending"
        );
        let geometry =
            self.collision_geometry(self.top.context("creature position is missing")?)?;
        if super::mob_obstruction::contains_any_liquid(
            entity_bounds,
            world.min_y,
            world.max_y,
            world,
        )? {
            planner.finish_entity(false)?;
            self.rejected = true;
            return Ok(GeneratedCreatureObstructionProgress::Rejected);
        }
        self.obstruction_passed = true;
        Ok(GeneratedCreatureObstructionProgress::NeedsFinalization(
            GeneratedCreaturePose { geometry, yaw },
        ))
    }

    /// Record a genuinely constructed and finalized entity and its passenger
    /// traversal. The caller supplies the real final block positions and
    /// serialized results in Java traversal order, after all species-specific
    /// initialization. Failure leaves both planner and batch unchanged.
    pub fn record_finalized(
        &mut self,
        planner: &mut ChunkCreatureSpawner,
        batch: &mut FreshTerrainBatch,
        additions: Vec<super::GeneratedEntityAddition>,
    ) -> Result<()> {
        ensure!(
            self.obstruction_passed && !self.rejected && !self.finalized,
            "creature is not ready for finalization"
        );
        ensure!(
            planner.created_attempt() == Some(self.attempt.clone()),
            "constructed creature is no longer pending"
        );
        let mut next = planner.clone();
        next.finish_entity(true)?;
        batch.record_finalized_creature(&self.attempt, additions)?;
        *planner = next;
        self.finalized = true;
        Ok(())
    }

    /// Resolve one pending planner attempt against generated blocks. Missing
    /// heightmap inputs or unsupported block states return an error without
    /// advancing the planner. The caller still checks collision and mob rules.
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
        self.placement_passed = true;
        Ok(GeneratedCreatureProbeProgress::NeedsCollision(top))
    }
}

fn geometry_block_position(
    geometry: native_resources::worldgen_mob_geometry::CreatureSpawnGeometry,
) -> Result<IVec3> {
    let coords = geometry.position;
    ensure!(
        coords.iter().all(|v| v.is_finite()
            && v.floor() >= f64::from(i32::MIN)
            && v.floor() <= f64::from(i32::MAX)),
        "creature rule position overflow"
    );
    Ok(IVec3::new(
        coords[0].floor() as i32,
        coords[1].floor() as i32,
        coords[2].floor() as i32,
    ))
}

pub struct GeneratedMobPlacementView<'a> {
    batch: &'a FreshTerrainBatch,
    world: Arc<World>,
    heightmap: MobSpawnHeightmap,
    border: RespawnBorder,
    prevent_inside: &'a BTreeSet<String>,
    height_tags: &'a native_resources::heightmap_tags::HeightmapTags,
    min_y: i32,
    max_y: i32,
    void_air: BlockState,
    last_facts: Option<(IVec3, MobSpawnBlockFacts)>,
    computed_heights: BTreeMap<(i32, i32), i32>,
    rule_light: BTreeMap<ChunkPos, native_world::generation_light::VisibleLightSnapshot>,
    fluid_states: BTreeMap<native_world::loaded::BlockStateId, bool>,
}

impl<'a> GeneratedMobPlacementView<'a> {
    /// Tags must come from the same unchanged pack stack as the batch.
    pub fn new(
        batch: &'a FreshTerrainBatch,
        heightmap: MobSpawnHeightmap,
        border: RespawnBorder,
        prevent_inside: &'a BTreeSet<String>,
        height_tags: &'a native_resources::heightmap_tags::HeightmapTags,
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
            height_tags,
            min_y,
            max_y,
            void_air: BlockState {
                name: "minecraft:void_air".into(),
                properties: Default::default(),
            },
            last_facts: None,
            computed_heights: BTreeMap::new(),
            rule_light: BTreeMap::new(),
            fluid_states: BTreeMap::new(),
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

    fn rule_raw_brightness(&mut self, position: IVec3) -> Result<u8> {
        let chunk = ChunkPos {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        };
        if !self.rule_light.contains_key(&chunk) {
            let snapshot = self.batch.light_snapshot([chunk])?;
            self.rule_light.insert(chunk, snapshot);
        }
        self.rule_light[&chunk]
            .raw_brightness(position.to_array(), 0)
            .context("creature spawn light is unavailable")
    }

    #[cfg(test)]
    pub(super) fn set_rule_light_for_test(
        &mut self,
        pos: ChunkPos,
        light: native_world::generation_light::VisibleLightSnapshot,
    ) {
        self.rule_light.insert(pos, light);
    }
}

impl super::mob_obstruction::FluidScanWorld for GeneratedMobPlacementView<'_> {
    fn require_chunk(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(
            self.world.chunks.contains_key(&pos),
            "creature obstruction chunk is missing"
        );
        Ok(())
    }
    fn has_fluid(&mut self, pos: IVec3) -> Result<bool> {
        let id = self
            .world
            .block_id(pos.x, pos.y, pos.z)
            .context("creature obstruction block is missing")?;
        if let Some(&fluid) = self.fluid_states.get(&id) {
            return Ok(fluid);
        }
        let state = self
            .world
            .state(id)
            .context("creature obstruction block state is missing")?;
        ensure!(
            self.batch
                .generated_block_registry()?
                .is_canonical_state(&state.name, &state.properties),
            "unverified creature obstruction block state {state:?}"
        );
        let fluid = native_world::fluid::FluidState::from_block(state)
            .map_err(anyhow::Error::msg)?
            .is_some();
        self.fluid_states.insert(id, fluid);
        Ok(fluid)
    }
}

struct GeneratedGroundRuleWorld<'a, 'b> {
    view: &'a mut GeneratedMobPlacementView<'b>,
    tags: &'a native_resources::worldgen_mob_rules::GroundSpawnTags,
}

impl native_resources::worldgen_mob_rules::GroundCreatureSpawnWorld
    for GeneratedGroundRuleWorld<'_, '_>
{
    fn raw_brightness(&mut self, position: IVec3) -> Result<u8> {
        self.view.rule_raw_brightness(position)
    }
    fn tagged_ground(
        &mut self,
        position: IVec3,
        tag: native_resources::worldgen_mob_rules::GroundSpawnTag,
    ) -> Result<bool> {
        Ok(self.tags.contains(tag, &self.view.block(position)?.name))
    }
}

impl native_resources::worldgen_mob_rules::AnimalInstanceSpawnWorld
    for GeneratedMobPlacementView<'_>
{
    fn is_grass_block(&mut self, position: IVec3) -> Result<bool> {
        Ok(self.block(position)?.name == "minecraft:grass_block")
    }
    fn raw_brightness(&mut self, position: IVec3) -> Result<u8> {
        self.rule_raw_brightness(position)
    }
    fn ambient_light(&mut self) -> Result<f32> {
        self.batch.ambient_light()
    }
}

impl MobPlacementWorld for GeneratedMobPlacementView<'_> {
    fn height(&mut self, x: i32, z: i32) -> Result<i32> {
        if let Some(height) = self.batch.primed_mob_spawn_height(x, z, self.heightmap)? {
            return Ok(height);
        }
        if let Some(&height) = self.computed_heights.get(&(x, z)) {
            return Ok(height);
        }
        // LIGHT-complete chunks are frozen against generation writes. A
        // requested column can therefore be scanned lazily without changing
        // the externally visible heightmap or recomputing an existing one.
        let mut height = self.min_y;
        for y in (self.min_y..self.max_y).rev() {
            let state = self.block(IVec3::new(x, y, z))?;
            if unprimed_heightmap_matches(state, self.heightmap, self.height_tags)? {
                height = y + 1;
                break;
            }
        }
        self.computed_heights.insert((x, z), height);
        Ok(height)
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

fn unprimed_heightmap_matches(
    state: &BlockState,
    kind: MobSpawnHeightmap,
    tags: &native_resources::heightmap_tags::HeightmapTags,
) -> Result<bool> {
    let air = matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    );
    if air {
        ensure!(state.properties.is_empty(), "malformed air block state");
        return Ok(false);
    }
    if kind == MobSpawnHeightmap::WorldSurface {
        return Ok(true);
    }
    let motion = tags.for_block(&state.name)?;
    let blocks_motion = match kind {
        MobSpawnHeightmap::OceanFloor | MobSpawnHeightmap::MotionBlocking => motion.blocks_motion,
        MobSpawnHeightmap::MotionBlockingNoLeaves => motion.blocks_motion_no_leaves,
        MobSpawnHeightmap::WorldSurface => unreachable!(),
    };
    if blocks_motion || kind == MobSpawnHeightmap::OceanFloor {
        return Ok(blocks_motion);
    }
    let facts = common_mob_spawn_block_facts(state)
        .with_context(|| format!("unsupported fluid state in mob heightmap: {state:?}"))?;
    Ok(!facts.fluid_empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::{heightmap_tags::HeightmapTags, packs::ResourceProvider};

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json" => {
                    Some(br#"{"values":["minecraft:stone","minecraft:oak_leaves"]}"#.to_vec())
                }
                "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                    Some(br#"{"values":["minecraft:stone"]}"#.to_vec())
                }
                _ => None,
            })
        }
    }
    fn state(name: &str, property: Option<(&str, &str)>) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: property
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
    #[test]
    fn missing_heightmap_predicates_follow_active_tags_and_fluid_state() {
        let registry = ["air", "stone", "water", "short_grass", "oak_leaves"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        let tags = HeightmapTags::load(&mut Data, &registry).unwrap();
        let kinds = [
            MobSpawnHeightmap::WorldSurface,
            MobSpawnHeightmap::OceanFloor,
            MobSpawnHeightmap::MotionBlocking,
            MobSpawnHeightmap::MotionBlockingNoLeaves,
        ];
        for (state, expected) in [
            (state("air", None), [false, false, false, false]),
            (state("stone", None), [true, true, true, true]),
            (
                state("water", Some(("level", "0"))),
                [true, false, true, true],
            ),
            (state("short_grass", None), [true, false, false, false]),
        ] {
            for (kind, expected) in kinds.into_iter().zip(expected) {
                assert_eq!(
                    unprimed_heightmap_matches(&state, kind, &tags).unwrap(),
                    expected,
                    "{state:?} {kind:?}"
                );
            }
        }
        let leaves = state("oak_leaves", None);
        assert!(
            unprimed_heightmap_matches(&leaves, MobSpawnHeightmap::MotionBlocking, &tags).unwrap()
        );
        assert!(
            unprimed_heightmap_matches(&leaves, MobSpawnHeightmap::MotionBlockingNoLeaves, &tags)
                .is_err()
        );
        assert!(
            unprimed_heightmap_matches(
                &state("unknown", None),
                MobSpawnHeightmap::OceanFloor,
                &tags
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn unprimed_predicates_match_java_heightmap_types() {
        use serde_json::Value;
        use std::{path::PathBuf, process::Command};
        let registry = [
            "air",
            "stone",
            "water",
            "short_grass",
            "oak_leaves",
            "grass_block",
            "redstone_ore",
            "seagrass",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        let tags = HeightmapTags::load(&mut Data, &registry).unwrap();
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("MobUnprimedHeightOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/MobUnprimedHeightOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        let output = temp.path().join("output.json");
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("MobUnprimedHeightOracle")
            .arg(&output));
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let kinds = [
            MobSpawnHeightmap::WorldSurface,
            MobSpawnHeightmap::OceanFloor,
            MobSpawnHeightmap::MotionBlocking,
            MobSpawnHeightmap::MotionBlockingNoLeaves,
        ];
        let mut checked = 0;
        for row in rows {
            let state = BlockState {
                name: row["name"].as_str().unwrap().into(),
                properties: serde_json::from_value(row["properties"].clone()).unwrap(),
            };
            for (index, kind) in kinds.into_iter().enumerate() {
                match unprimed_heightmap_matches(&state, kind, &tags) {
                    Ok(actual) => {
                        assert_eq!(
                            actual,
                            row["matches"][index].as_bool().unwrap(),
                            "{state:?} {kind:?}"
                        );
                        checked += 1;
                    }
                    Err(_) => assert_eq!(state.name, "minecraft:oak_leaves"),
                }
            }
        }
        assert!(checked > 150, "only {checked} predicates checked");
    }
}
