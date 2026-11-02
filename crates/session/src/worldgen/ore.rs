//! Apply ordinary ore features to intermediate generated chunk sections.
//!
//! The surrounding decoration scheduler must still supply Java's feature order,
//! random seed, writable region and complete CARVERS-stage neighborhood.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use native_resources::{
    feature_order::{FeatureKey, FeatureSchedule},
    heightmap_tags::HeightmapTags,
    ore_feature::{OreBlockTags, OreVein, OreVeinWorld, OreWorld},
    packs::{ResourceId, ResourceProvider},
    placed_feature::{PlacedFeature, PlacementEnvironment},
};
use native_world::loaded::{BlockState, BlockStateId, ChunkPos, World};
use serde_json::Value;

use super::{CarvedTerrainWorld, DecorationPlan};

/// An unpublished generation attempt. Dropping it, or any failed placement,
/// leaves the original terrain and its snapshots untouched.
pub struct OreVeinRegion<'a> {
    target: &'a mut CarvedTerrainWorld,
    world: World,
    tags: &'a OreBlockTags,
    motion: &'a HeightmapTags,
    writable: BTreeSet<ChunkPos>,
    heights: BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    motion_cache: BTreeMap<BlockStateId, bool>,
    failed: bool,
}

/// A placed feature bound to the exact configured ore it references.
pub struct PlacedOreVein {
    placement: PlacedFeature,
    ore: OreVein<BlockState>,
}

/// A placed ore resolved from one exact entry of the decoration schedule.
pub struct ScheduledOreVein<'a> {
    key: FeatureKey,
    placed: PlacedOreVein,
    schedule: &'a FeatureSchedule,
}

impl<'a> ScheduledOreVein<'a> {
    /// Read both named and inline placed features from the same active packs
    /// and block registry that were used to prepare the schedule.
    pub fn load(
        provider: &mut impl ResourceProvider,
        schedule: &'a FeatureSchedule,
        key: &FeatureKey,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Self> {
        ensure!(
            schedule.steps().iter().flatten().any(|item| item == key),
            "feature is absent from the schedule"
        );
        let placed = match key {
            FeatureKey::Registered(id) => PlacedOreVein::load(provider, id, resolve)?,
            FeatureKey::Inline { biome, .. } => {
                let definition = schedule
                    .biome(biome)
                    .context("inline feature biome is missing")?
                    .inline
                    .get(key)
                    .context("inline feature definition is missing")?;
                PlacedOreVein::from_placement(
                    provider,
                    PlacedFeature::decode(definition)?,
                    resolve,
                )?
            }
        };
        Ok(Self {
            key: key.clone(),
            placed,
            schedule,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        self.placed.required_tags()
    }

    /// Java `BiomeGenerationSettings.hasFeature` tests value equality. Exact
    /// schedule identity is always equal; independently decoded uniform and
    /// trapezoid heights prove that any other feature object is unequal.
    pub fn biome_allows(&self, biome: &ResourceId) -> Result<bool> {
        let settings = self
            .schedule
            .biome(biome)
            .context("biome is absent from the feature schedule")?;
        if self.schedule.biome_contains_identity(biome, &self.key)? {
            return Ok(true);
        }
        if self.placed.placement.has_independent_height_provider()
            || settings.steps.iter().all(Vec::is_empty)
        {
            return Ok(false);
        }
        bail!("cannot determine Java feature equality for this biome")
    }
}

impl PlacedOreVein {
    pub fn load(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Self> {
        let placement = PlacedFeature::load(provider, id)?;
        Self::from_placement(provider, placement, resolve)
    }

    pub fn from_placement(
        provider: &mut impl ResourceProvider,
        placement: PlacedFeature,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Self> {
        let ore = if let Some(id) = placement.feature.as_str() {
            OreVein::load(provider, &ResourceId::parse(id)?, resolve)?
        } else {
            OreVein::decode(&placement.feature, resolve)?
        };
        Ok(Self { placement, ore })
    }

    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        self.ore.required_tags()
    }
}

impl CarvedTerrainWorld {
    /// Writable chunks must match the Java WorldGenRegion for this step.
    /// All reads, including neighbors and heightmaps, require loaded chunks.
    /// The tags must belong to the same registry and active data packs for the
    /// lifetime of the terrain. Other generation writes must update any primed
    /// heightmaps before this interface can be combined with those features.
    pub fn ore_vein_region<'a>(
        &'a mut self,
        tags: &'a OreBlockTags,
        motion: &'a HeightmapTags,
        writable: BTreeSet<ChunkPos>,
    ) -> Result<OreVeinRegion<'a>> {
        ensure!(
            writable.iter().all(|p| self.world.chunks.contains_key(p)),
            "writable generation chunk is missing"
        );
        let world = (*self.world).clone();
        let heights = self.ore_ocean_floor_heights.clone();
        Ok(OreVeinRegion {
            target: self,
            world,
            tags,
            motion,
            writable,
            heights,
            motion_cache: BTreeMap::new(),
            failed: false,
        })
    }
}

impl OreVeinRegion<'_> {
    /// A failed attempt must be discarded; on failure the caller's RNG is unchanged.
    /// If the region contains earlier successful placements, the caller must
    /// also restore its RNG to the beginning of the discarded region.
    pub fn place(
        &mut self,
        feature: &OreVein<BlockState>,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next = random.clone();
        self.failed = true;
        let result = feature.place(self, &mut next, origin)?;
        *random = next;
        self.failed = false;
        Ok(result)
    }

    /// Run placement modifiers and their ore feature in Java's depth-first
    /// order on one RNG. The environment must use the active biome settings,
    /// top-level placed feature, current heightmaps and level minimum Y.
    /// `generator_min_y` and `generator_height` are the placement context's
    /// generator bounds, which can differ from the level bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn place_placed(
        &mut self,
        placed: &PlacedOreVein,
        random: &mut DecorationRandom,
        origin: [i32; 3],
        generator_min_y: i32,
        generator_height: i32,
        environment: &mut impl PlacementEnvironment,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next = random.clone();
        self.failed = true;
        let mut positions =
            placed
                .placement
                .positions(origin, generator_min_y, generator_height)?;
        let mut result = false;
        while let Some(position) = positions.next(&mut next, environment)? {
            result |= placed.ore.place(self, &mut next, position)?;
        }
        *random = next;
        self.failed = false;
        Ok(result)
    }

    /// Place one planned ore using its own Java feature seed and the plan's
    /// biome neighborhood. Callers must execute every preceding feature and
    /// structure in order, and provide Java `hasFeature` semantics from the
    /// active biome settings; unknown equivalence must be an error.
    pub fn place_planned(
        &mut self,
        plan: &DecorationPlan<'_>,
        index: usize,
        ore: &ScheduledOreVein<'_>,
        biome_allows: &mut impl FnMut(&FeatureKey, &ResourceId) -> Result<bool>,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        let selected = plan
            .features
            .get(index)
            .context("planned feature index is out of range")?;
        ensure!(
            *selected.feature == ore.key,
            "planned feature does not match loaded ore"
        );
        ensure!(
            self.target.seed == plan.biomes.seed
                && self.target.min_y == plan.biomes.min_y
                && self.target.height == plan.biomes.height,
            "planned biome neighborhood does not match generated terrain"
        );
        let mut random = plan.feature_random(selected);
        let mut positions = ore.placed.placement.positions(
            plan.origin,
            plan.generator_min_y,
            plan.generator_height,
        )?;
        self.failed = true;
        let mut result = false;
        loop {
            let next = {
                let mut env = PlannedOreEnvironment {
                    region: self,
                    plan,
                    key: &ore.key,
                    biome_allows,
                };
                positions.next(&mut random, &mut env)?
            };
            let Some(position) = next else { break };
            result |= ore.placed.ore.place(self, &mut random, position)?;
        }
        self.failed = false;
        Ok(result)
    }

    /// Resolve biome membership from the active schedule for ordinary ores.
    /// Distinct placed features with uniform/trapezoid height providers are
    /// never equal in Java because each decoded provider has object identity.
    /// Ambiguous data-pack feature equality and unknown biomes are errors.
    pub fn place_planned_auto(
        &mut self,
        plan: &DecorationPlan<'_>,
        index: usize,
        ore: &ScheduledOreVein<'_>,
    ) -> Result<bool> {
        let selected = plan
            .features
            .get(index)
            .context("planned feature index is out of range")?;
        let step = usize::try_from(selected.step).context("invalid feature step")?;
        let feature_index = usize::try_from(selected.index).context("invalid feature index")?;
        let canonical = ore
            .schedule
            .steps()
            .get(step)
            .and_then(|features| features.get(feature_index))
            .context("feature is absent from the loaded schedule")?;
        ensure!(
            std::ptr::eq(selected.feature, canonical),
            "plan and ore use different feature schedules"
        );
        self.place_planned(plan, index, ore, &mut |_, biome| ore.biome_allows(biome))
    }

    /// Publish this intermediate state; it is not a finished or saveable chunk.
    pub fn finish(self) -> Result<()> {
        ensure!(!self.failed, "cannot publish a failed generation attempt");
        self.target.world = Arc::new(self.world);
        self.target.ore_ocean_floor_heights = self.heights;
        Ok(())
    }

    fn read(&self, p: [i32; 3]) -> Result<BlockState> {
        self.world
            .block(p[0], p[1], p[2])
            .cloned()
            .context("generation block or section is missing")
    }

    fn is_outside(&self, y: i32) -> bool {
        y < self.target.min_y || y >= self.target.min_y + self.target.height
    }

    // Java primes the entire heightmap on first access. Direct ore section
    // writes do not update it, including for data-pack replacement states.
    fn prime_heightmap(&mut self, pos: ChunkPos) -> Result<()> {
        let base_x = pos.x.checked_mul(16).context("chunk X overflow")?;
        let base_z = pos.z.checked_mul(16).context("chunk Z overflow")?;
        ensure!(
            base_x.checked_add(15).is_some() && base_z.checked_add(15).is_some(),
            "chunk coordinate overflow"
        );
        ensure!(
            self.world.chunks.contains_key(&pos),
            "generation heightmap chunk is missing"
        );
        let mut values = Box::new([self.target.min_y; 256]);
        for z in 0..16 {
            for x in 0..16 {
                for y in (self.target.min_y..self.target.min_y + self.target.height).rev() {
                    let id = self
                        .world
                        .block_id(base_x + x, y, base_z + z)
                        .context("generation heightmap block is missing")?;
                    let matches = if let Some(value) = self.motion_cache.get(&id) {
                        *value
                    } else {
                        let state = self
                            .world
                            .state(id)
                            .context("generation block state is missing")?;
                        let value =
                            !is_air(state) && self.motion.for_block(&state.name)?.blocks_motion;
                        self.motion_cache.insert(id, value);
                        value
                    };
                    if matches {
                        values[(x + z * 16) as usize] = y + 1;
                        break;
                    }
                }
            }
        }
        self.heights.insert(pos, Arc::from(values));
        Ok(())
    }
}

struct PlannedOreEnvironment<'a, 'b, F> {
    region: &'a mut OreVeinRegion<'b>,
    plan: &'a DecorationPlan<'a>,
    key: &'a FeatureKey,
    biome_allows: &'a mut F,
}
impl<F: FnMut(&FeatureKey, &ResourceId) -> Result<bool>> PlacementEnvironment
    for PlannedOreEnvironment<'_, '_, F>
{
    fn min_y(&self) -> i32 {
        self.region.target.min_y
    }
    fn heightmap(&mut self, kind: &str, x: i32, z: i32) -> Result<i32> {
        ensure!(
            kind == "OCEAN_FLOOR_WG",
            "unsupported generation heightmap {kind}"
        );
        self.region.ocean_floor_height(x, z)
    }
    fn biome_allows(&mut self, pos: [i32; 3]) -> Result<bool> {
        let biome = self.plan.biomes.biome_at_block(pos[0], pos[1], pos[2])?;
        (self.biome_allows)(self.key, biome)
    }
}

fn is_air(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}
fn chunk(p: [i32; 3]) -> ChunkPos {
    ChunkPos {
        x: p[0].div_euclid(16),
        z: p[2].div_euclid(16),
    }
}

impl OreWorld for OreVeinRegion<'_> {
    type State = BlockState;
    fn block(&mut self, p: [i32; 3]) -> Result<BlockState> {
        self.read(p)
    }
    fn block_is(&mut self, state: &BlockState, id: &ResourceId) -> Result<bool> {
        Ok(state.name == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, state: &BlockState, tag: &ResourceId) -> Result<bool> {
        self.tags.contains(tag, &state.name)
    }
    fn is_air(&mut self, state: &BlockState) -> Result<bool> {
        Ok(is_air(state))
    }
    fn set_block(&mut self, _: [i32; 3], _: &BlockState, _: i32) -> Result<bool> {
        bail!("WorldGenLevel.setBlock effects are not implemented by the ore vein region")
    }
}
impl OreVeinWorld for OreVeinRegion<'_> {
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        let pos = chunk([x, 0, z]);
        if !self.heights.contains_key(&pos) {
            self.prime_heightmap(pos)?;
        }
        Ok(self.heights[&pos][(x.rem_euclid(16) + z.rem_euclid(16) * 16) as usize])
    }
    fn outside_build_height(&self, y: i32) -> bool {
        self.is_outside(y)
    }
    fn ensure_can_write(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(self.writable.contains(&chunk(p)))
    }
    fn section_block(&mut self, p: [i32; 3]) -> Result<Option<BlockState>> {
        if self.is_outside(p[1]) {
            return Ok(None);
        }
        self.read(p).map(Some)
    }
    fn section_neighbor(&mut self, p: [i32; 3]) -> Result<BlockState> {
        if self.is_outside(p[1]) {
            return Ok(BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            });
        }
        self.read(p)
    }
    fn set_section_block(&mut self, p: [i32; 3], state: &BlockState) -> Result<()> {
        ensure!(!self.is_outside(p[1]), "ore write outside build height");
        let id = self.world.intern_state(state.clone());
        let section_y = i8::try_from(p[1].div_euclid(16))?;
        let chunk = Arc::make_mut(
            self.world
                .chunks
                .get_mut(&chunk(p))
                .context("ore write chunk is missing")?,
        );
        let section = Arc::make_mut(
            chunk
                .sections
                .get_mut(&section_y)
                .context("ore write section is missing")?,
        );
        let blocks = section
            .blocks
            .as_mut()
            .context("ore write block data is missing")?;
        let index = p[0].rem_euclid(16) as usize
            + p[2].rem_euclid(16) as usize * 16
            + p[1].rem_euclid(16) as usize * 256;
        blocks[index] = id;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::packs::ResourceProvider;
    use native_world::loaded::{BLOCK_COUNT, BlockStateId, Chunk, Section};
    use serde_json::{Value, json};

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                    Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore"]}"#.to_vec())
                }
                "data/minecraft/tags/block/base_stone_overworld.json" => {
                    Some(br#"{"values":["minecraft:stone"]}"#.to_vec())
                }
                _ => None,
            })
        }
    }
    fn state(name: &str) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        }
    }
    fn fixture() -> (
        CarvedTerrainWorld,
        OreBlockTags,
        HeightmapTags,
        OreVein<BlockState>,
    ) {
        let registry = ["minecraft:air", "minecraft:stone", "minecraft:diamond_ore"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut data = Data;
        let feature = OreVein::decode(&json!({"type":"minecraft:ore", "size": 33, "discard_chance_on_air_exposure": 0.0, "targets": [{"target":{"predicate_type":"minecraft:tag_match", "tag":"minecraft:base_stone_overworld"}, "state":"minecraft:diamond_ore"}]}), &mut |v: &Value| Ok(state(v.as_str().unwrap().strip_prefix("minecraft:").unwrap()))).unwrap();
        let tags = OreBlockTags::load(&mut data, &registry, &feature.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut data, &registry).unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air"));
        let stone = world.intern_state(state("stone"));
        for z in -1..=1 {
            for x in -1..=1 {
                let position = ChunkPos { x, z };
                let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            blocks[x + z * 16 + y * 256] = if y < 8 { stone } else { air };
                        }
                    }
                }
                world.chunks.insert(
                    position,
                    Arc::new(Chunk {
                        position,
                        status: Some("minecraft:carvers".into()),
                        block_light_correct: false,
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let result = CarvedTerrainWorld {
            world: Arc::new(world),
            seed: 0,
            min_y: 0,
            height: 16,
            ore_ocean_floor_heights: BTreeMap::new(),
            postprocessing: BTreeMap::new(),
            noise_postprocessing: BTreeMap::new(),
            surface_postprocessing: BTreeMap::new(),
            carver_postprocessing: BTreeMap::new(),
        };
        (result, tags, motion, feature)
    }

    #[test]
    fn writes_real_sections_and_preserves_snapshots_without_session_edits() {
        let (mut terrain, tags, motion, feature) = fixture();
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(123);
        assert!(region.place(&feature, &mut random, [15, 4, 7]).unwrap());
        region.finish().unwrap();
        let current = terrain.snapshot();
        let count = (0..32)
            .flat_map(|x| (0..16).flat_map(move |z| (0..8).map(move |y| (x, y, z))))
            .filter(|&(x, y, z)| current.block(x, y, z).unwrap().name == "minecraft:diamond_ore")
            .count();
        assert!(count > 0);
        assert_eq!(previous.block(15, 4, 7).unwrap().name, "minecraft:stone");
        assert_eq!(current.block_edits().count(), 0);
        let untouched = ChunkPos { x: -1, z: -1 };
        assert!(Arc::ptr_eq(
            &previous.chunks[&untouched],
            &current.chunks[&untouched]
        ));
    }

    #[test]
    fn primes_whole_heightmap_and_keeps_direct_section_writes_out_of_it() {
        let (mut terrain, tags, motion, _) = fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        assert_eq!(region.ocean_floor_height(0, 0).unwrap(), 8);
        region
            .set_section_block([1, 12, 1], &state("stone"))
            .unwrap();
        assert_eq!(region.ocean_floor_height(1, 1).unwrap(), 8);
        assert_eq!(region.section_neighbor([1, -1, 1]).unwrap(), state("air"));
        region.finish().unwrap();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut next = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        assert_eq!(next.ocean_floor_height(1, 1).unwrap(), 8);
    }

    #[test]
    fn missing_neighbors_reject_and_discard_attempt_without_consuming_rng() {
        let (mut terrain, tags, motion, feature) = fixture();
        Arc::make_mut(&mut terrain.world)
            .chunks
            .remove(&ChunkPos { x: 0, z: -1 });
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(123);
        let mut expected = random.clone();
        assert!(region.place(&feature, &mut random, [15, 4, 7]).is_err());
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&previous, &terrain.snapshot()));
    }

    struct PlacementTestEnv {
        allow: bool,
        fail_after: Option<usize>,
        calls: usize,
    }
    impl PlacementEnvironment for PlacementTestEnv {
        fn min_y(&self) -> i32 {
            0
        }
        fn heightmap(&mut self, _: &str, _: i32, _: i32) -> Result<i32> {
            bail!("missing heightmap")
        }
        fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
            self.calls += 1;
            ensure!(self.fail_after != Some(self.calls), "missing biome data");
            Ok(self.allow)
        }
    }

    fn inline_placed_ore() -> PlacedOreVein {
        let placement = PlacedFeature::decode(&json!({"feature": {
            "type":"minecraft:ore", "size":9, "discard_chance_on_air_exposure":0.0,
            "targets":[{"target":{"predicate_type":"minecraft:block_match", "block":"minecraft:stone"}, "state":"minecraft:diamond_ore"}]
        }, "placement":[{"type":"minecraft:count", "count":4},{"type":"minecraft:biome"}]})).unwrap();
        PlacedOreVein::from_placement(&mut Data, placement, &mut |v| {
            Ok(state(
                v.as_str().unwrap().strip_prefix("minecraft:").unwrap(),
            ))
        })
        .unwrap()
    }

    #[test]
    fn placed_vein_filters_biomes_and_executes_all_successful_positions() {
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(7);
        let mut env = PlacementTestEnv {
            allow: false,
            fail_after: None,
            calls: 0,
        };
        assert!(
            !region
                .place_placed(
                    &inline_placed_ore(),
                    &mut random,
                    [7, 4, 7],
                    0,
                    16,
                    &mut env
                )
                .unwrap()
        );
        assert_eq!(env.calls, 4);
        env.allow = true;
        assert!(
            region
                .place_placed(
                    &inline_placed_ore(),
                    &mut random,
                    [7, 4, 7],
                    0,
                    16,
                    &mut env
                )
                .unwrap()
        );
        assert_eq!(
            env.calls, 8,
            "success must not stop later feature placements"
        );
        region.finish().unwrap();
        let after = terrain.snapshot();
        assert!((0..16).any(|x| (0..16).any(|z| {
            (0..8).any(|y| after.block(x, y, z).unwrap() != before.block(x, y, z).unwrap())
        })));
    }

    #[test]
    fn placement_failure_discards_earlier_writes_and_random_changes() {
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(7);
        let mut expected = random.clone();
        let mut env = PlacementTestEnv {
            allow: true,
            fail_after: Some(2),
            calls: 0,
        };
        assert!(
            region
                .place_placed(
                    &inline_placed_ore(),
                    &mut random,
                    [7, 4, 7],
                    0,
                    16,
                    &mut env
                )
                .is_err()
        );
        assert_eq!(env.calls, 2);
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    struct ScheduledData {
        heightmap: Option<&'static str>,
    }
    impl ScheduledData {
        fn definition(&self) -> Value {
            let mut placement = vec![
                json!({"type":"minecraft:count","count":4}),
                json!({"type":"minecraft:in_square"}),
            ];
            if self.heightmap == Some("no_height") {
                // Exercise the equality case where a distinct feature can match.
            } else if let Some(kind) = self.heightmap {
                placement.push(json!({"type":"minecraft:heightmap","heightmap":kind}));
            } else {
                placement.push(json!({"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":4},"max_inclusive":{"absolute":4}}}));
            }
            placement.push(json!({"type":"minecraft:biome"}));
            json!({"feature":{"type":"minecraft:ore","size":33,"discard_chance_on_air_exposure":0.0,"targets":[{"target":{"predicate_type":"minecraft:block_match","block":"minecraft:stone"},"state":"minecraft:diamond_ore"}]},"placement":placement})
        }
    }
    impl ResourceProvider for ScheduledData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            let value = match path {
                "data/minecraft/worldgen/biome/plains.json" => {
                    Some(json!({"features":[["test:ore",self.definition()]]}))
                }
                "data/minecraft/worldgen/biome/desert.json" => {
                    Some(json!({"features":[[self.definition()]]}))
                }
                "data/minecraft/worldgen/biome/forest.json" => Some(json!({"features":[[]]})),
                "data/test/worldgen/placed_feature/ore.json" => Some(self.definition()),
                _ => None,
            };
            match value {
                Some(value) => Ok(Some(serde_json::to_vec(&value)?)),
                None => Data.read_data(path),
            }
        }
    }
    fn scheduled_fixture(
        kind: Option<&'static str>,
    ) -> (
        ScheduledData,
        FeatureSchedule,
        super::super::BiomeNeighborhood,
    ) {
        let mut data = ScheduledData { heightmap: kind };
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let schedule =
            FeatureSchedule::load(&mut data, std::slice::from_ref(&plains), &registry).unwrap();
        let mut neighborhood = super::super::BiomeNeighborhood::new(0, 0, 16).unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                neighborhood
                    .insert(Arc::new(
                        super::super::QuartBiomeChunk::from_values(
                            [x, z],
                            0,
                            0,
                            16,
                            vec![plains.clone(); 64],
                        )
                        .unwrap(),
                    ))
                    .unwrap();
            }
        }
        (data, schedule, neighborhood)
    }
    fn fixture_plan<'a>(
        schedule: &'a FeatureSchedule,
        neighborhood: &'a super::super::BiomeNeighborhood,
    ) -> DecorationPlan<'a> {
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let mut random = DecorationRandom::new(0);
        DecorationPlan {
            origin: [0, 0, 0],
            decoration_seed: random.set_decoration_seed(0, 0, 0),
            features: schedule.select([&plains]),
            biomes: neighborhood,
            generator_min_y: 0,
            generator_height: 16,
        }
    }
    fn resolve_test(v: &Value) -> Result<BlockState> {
        Ok(state(
            v.as_str()
                .context("expected test block")?
                .strip_prefix("minecraft:")
                .context("expected minecraft namespace")?,
        ))
    }

    #[test]
    fn scheduled_ores_use_plan_biomes_and_reject_mismatched_entries() {
        let (mut data, schedule, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        assert_eq!(plan.features.len(), 2);
        let named = ScheduledOreVein::load(
            &mut data,
            &schedule,
            plan.features[0].feature,
            &mut resolve_test,
        )
        .unwrap();
        let inline = ScheduledOreVein::load(
            &mut data,
            &schedule,
            plan.features[1].feature,
            &mut resolve_test,
        )
        .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        let mut calls = 0;
        let mut allows = |_: &FeatureKey, biome: &ResourceId| {
            calls += 1;
            Ok(*biome == ResourceId::parse("minecraft:plains")?)
        };
        assert!(
            region
                .place_planned(&plan, 0, &inline, &mut allows)
                .is_err()
        );
        assert!(region.place_planned(&plan, 0, &named, &mut allows).unwrap());
        region
            .place_planned(&plan, 1, &inline, &mut allows)
            .unwrap();
        assert_eq!(calls, 8);
        region.finish().unwrap();
        let current = terrain.snapshot();
        assert!((0..16).any(|x| {
            (0..16).any(|z| (0..8).any(|y| current.block(x, y, z) != previous.block(x, y, z)))
        }));
    }

    #[test]
    fn scheduled_biome_failure_discards_prior_writes_and_unsupported_heightmaps_fail() {
        let (mut data, schedule, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let ore = ScheduledOreVein::load(
            &mut data,
            &schedule,
            plan.features[0].feature,
            &mut resolve_test,
        )
        .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .ore_vein_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        let mut calls = 0;
        assert!(
            region
                .place_planned(&plan, 0, &ore, &mut |_, _| {
                    calls += 1;
                    ensure!(calls < 2, "unresolved biome feature equivalence");
                    Ok(true)
                })
                .is_err()
        );
        assert_eq!(calls, 2);
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));

        let (mut data, schedule, neighborhood) = scheduled_fixture(Some("WORLD_SURFACE_WG"));
        let plan = fixture_plan(&schedule, &neighborhood);
        let ore = ScheduledOreVein::load(
            &mut data,
            &schedule,
            plan.features[0].feature,
            &mut resolve_test,
        )
        .unwrap();
        let mut region = terrain
            .ore_vein_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        assert!(
            region
                .place_planned(&plan, 0, &ore, &mut |_, _| Ok(true))
                .is_err()
        );
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    }

    #[test]
    fn scheduled_ore_uses_live_ocean_floor_heightmap() {
        let (mut data, schedule, neighborhood) = scheduled_fixture(Some("OCEAN_FLOOR_WG"));
        let plan = fixture_plan(&schedule, &neighborhood);
        let ore = ScheduledOreVein::load(
            &mut data,
            &schedule,
            plan.features[0].feature,
            &mut resolve_test,
        )
        .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .ore_vein_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        assert!(
            region
                .place_planned(&plan, 0, &ore, &mut |_, _| Ok(true))
                .unwrap()
        );
        region.finish().unwrap();
    }

    #[test]
    fn scheduled_ore_resolves_biome_membership_and_rejects_ambiguity() {
        let plains = ResourceId::parse("minecraft:plains").unwrap();
        let desert = ResourceId::parse("minecraft:desert").unwrap();
        let forest = ResourceId::parse("minecraft:forest").unwrap();
        let missing = ResourceId::parse("example:missing").unwrap();
        let registry = [ResourceId::parse("test:ore").unwrap()]
            .into_iter()
            .collect();
        let mut data = ScheduledData { heightmap: None };
        let schedule = FeatureSchedule::load(
            &mut data,
            &[plains.clone(), desert.clone(), forest.clone()],
            &registry,
        )
        .unwrap();
        let key = FeatureKey::Registered(ResourceId::parse("test:ore").unwrap());
        let ore = ScheduledOreVein::load(&mut data, &schedule, &key, &mut resolve_test).unwrap();
        assert!(ore.biome_allows(&plains).unwrap());
        assert!(
            !ore.biome_allows(&desert).unwrap(),
            "distinct uniform providers are not equal"
        );
        assert!(!ore.biome_allows(&forest).unwrap());
        assert!(ore.biome_allows(&missing).is_err());

        let (_, _, neighborhood) = scheduled_fixture(None);
        let plan = fixture_plan(&schedule, &neighborhood);
        let index = plan
            .features
            .iter()
            .position(|item| *item.feature == key)
            .unwrap();
        let (mut terrain, tags, motion, _) = fixture();
        let before = terrain.snapshot();
        let mut region = terrain
            .ore_vein_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        assert!(region.place_planned_auto(&plan, index, &ore).unwrap());
        region.finish().unwrap();

        let mut other_data = ScheduledData { heightmap: None };
        let other_schedule = FeatureSchedule::load(
            &mut other_data,
            &[plains.clone(), desert.clone(), forest.clone()],
            &registry,
        )
        .unwrap();
        let other_ore =
            ScheduledOreVein::load(&mut other_data, &other_schedule, &key, &mut resolve_test)
                .unwrap();
        let before = terrain.snapshot();
        let mut region = terrain
            .ore_vein_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        assert!(region.place_planned_auto(&plan, index, &other_ore).is_err());
        region.finish().unwrap();
        assert!(Arc::ptr_eq(
            &before.chunks[&native_world::loaded::ChunkPos { x: 0, z: 0 }],
            &terrain.snapshot().chunks[&native_world::loaded::ChunkPos { x: 0, z: 0 }]
        ));

        let mut ambiguous = ScheduledData {
            heightmap: Some("no_height"),
        };
        let ambiguous_schedule = FeatureSchedule::load(
            &mut ambiguous,
            &[plains.clone(), desert.clone(), forest.clone()],
            &registry,
        )
        .unwrap();
        let ambiguous_ore =
            ScheduledOreVein::load(&mut ambiguous, &ambiguous_schedule, &key, &mut resolve_test)
                .unwrap();
        assert!(ambiguous_ore.biome_allows(&plains).unwrap());
        assert!(ambiguous_ore.biome_allows(&desert).is_err());
        assert!(!ambiguous_ore.biome_allows(&forest).unwrap());
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn generated_chunk_veins_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut installed = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = installed.vanilla_generated_block_registry().unwrap();
        for name in [
            "ore_diamond",
            "ore_coal_upper",
            "ore_iron_middle",
            "ore_dirt",
        ] {
            PlacedOreVein::load(
                &mut installed,
                &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
                &mut |v| super::super::resolve_generated_terrain_state(v, &registry),
            )
            .unwrap();
        }
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("OreVeinOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../resources/tests/java/OreVeinOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("OreVeinOracle"));
        let mut expected =
            BTreeMap::<(i32, String, i64), (BTreeMap<[i32; 3], String>, bool, i64)>::new();
        let mut integrated = BTreeMap::<i64, (BTreeMap<[i32; 3], String>, i64)>::new();
        let mut key = None;
        let mut integrated_key = None;
        let mut writes = BTreeMap::new();
        for line in output.lines() {
            let words: Vec<_> = line.split_whitespace().collect();
            match words.as_slice() {
                ["C", size, chance, seed] => {
                    key = Some((
                        size.parse().unwrap(),
                        (*chance).to_owned(),
                        seed.parse().unwrap(),
                    ));
                    writes.clear();
                }
                ["I", seed] => {
                    integrated_key = Some(seed.parse().unwrap());
                    writes.clear();
                }
                ["W", x, y, z, name] if key.is_some() || integrated_key.is_some() => {
                    writes.insert(
                        [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()],
                        (*name).to_owned(),
                    );
                }
                ["R", result, next] if key.is_some() => {
                    expected.insert(
                        key.take().unwrap(),
                        (
                            std::mem::take(&mut writes),
                            result.parse().unwrap(),
                            next.parse().unwrap(),
                        ),
                    );
                }
                ["T", next] if integrated_key.is_some() => {
                    integrated.insert(
                        integrated_key.take().unwrap(),
                        (std::mem::take(&mut writes), next.parse().unwrap()),
                    );
                }
                _ => {}
            }
        }
        struct OracleData;
        impl ResourceProvider for OracleData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/tags/block/base_stone_overworld.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        Some(br#"{"values":["minecraft:stone","minecraft:deepslate"]}"#.to_vec())
                    }
                    _ => None,
                })
            }
        }
        let registry = ["air", "stone", "deepslate", "diamond_ore", "gold_ore"]
            .into_iter()
            .map(|x| format!("minecraft:{x}"))
            .collect();
        let mut data = OracleData;
        let motion = HeightmapTags::load(&mut data, &registry).unwrap();
        fn oracle_terrain() -> CarvedTerrainWorld {
            let (mut terrain, _, _, _) = fixture();
            let world = Arc::make_mut(&mut terrain.world);
            world.chunks.clear();
            let ids = [
                world.intern_state(state("air")),
                world.intern_state(state("stone")),
                world.intern_state(state("deepslate")),
            ];
            for cz in 1..=4i32 {
                for cx in -3..=0i32 {
                    let pos = ChunkPos { x: cx, z: cz };
                    let mut sections = BTreeMap::new();
                    for sy in -1..=0i32 {
                        let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
                        for y in 0..16i32 {
                            for z in 0..16i32 {
                                for x in 0..16i32 {
                                    let (wx, wy, wz) = (cx * 16 + x, sy * 16 + y, cz * 16 + z);
                                    let v = wx * 31 + wy * 17 + wz * 13;
                                    blocks[(x + z * 16 + y * 256) as usize] = ids[if v & 7 == 0 {
                                        0
                                    } else if wy < 0 {
                                        2
                                    } else {
                                        1
                                    }];
                                }
                            }
                        }
                        sections.insert(
                            sy as i8,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        );
                    }
                    world.chunks.insert(
                        pos,
                        Arc::new(Chunk {
                            position: pos,
                            status: Some("minecraft:carvers".into()),
                            block_light_correct: false,
                            sections,
                        }),
                    );
                }
            }
            terrain.min_y = -16;
            terrain.height = 32;
            terrain
        }
        let mut checked = 0;
        for size in [3, 17, 64] {
            for chance in [0.0f32, 0.5, 1.0] {
                let feature = OreVein::decode(&json!({"type":"minecraft:ore","size":size,"discard_chance_on_air_exposure":chance,"targets":[
                    {"state":"minecraft:diamond_ore","target":{"predicate_type":"minecraft:all_of","rules":[{"predicate_type":"minecraft:tag_match","tag":"minecraft:base_stone_overworld"},{"predicate_type":"minecraft:height_match","min_inclusive":-2,"max_inclusive":2},{"predicate_type":"minecraft:not","rule":{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.25}}]}},
                    {"state":"minecraft:gold_ore","target":{"predicate_type":"minecraft:any_of","rules":[{"predicate_type":"minecraft:block_match","block":"minecraft:deepslate"},{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.5}]}}
                ]}), &mut |v: &Value| Ok(state(v.as_str().unwrap().strip_prefix("minecraft:").unwrap()))).unwrap();
                let tags =
                    OreBlockTags::load(&mut data, &registry, &feature.required_tags()).unwrap();
                for seed in [1i64, 2, 5, 7] {
                    let mut terrain = oracle_terrain();
                    let previous = terrain.snapshot();
                    let writable = previous.chunks.keys().copied().collect();
                    let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
                    let mut random = DecorationRandom::new(0);
                    let deco = random.set_decoration_seed(seed, -32, 48);
                    random.set_feature_seed(deco, 7, 3);
                    let result = region
                        .place(
                            &feature,
                            &mut random,
                            [-16, if seed % 2 == 0 { -1 } else { 3 }, 32],
                        )
                        .unwrap();
                    region.finish().unwrap();
                    let world = terrain.snapshot();
                    let mut actual = BTreeMap::new();
                    for z in 16..80 {
                        for x in -48..16 {
                            for y in -16..16 {
                                let after = world.block(x, y, z).unwrap();
                                if previous.block(x, y, z).unwrap() != after {
                                    actual.insert([x, y, z], after.name.clone());
                                }
                            }
                        }
                    }
                    let key = (size, format!("{chance:.1}"), seed);
                    let (java_writes, java_result, java_next) = &expected[&key];
                    assert_eq!(&actual, java_writes, "block writes {key:?}");
                    assert_eq!(result, *java_result, "feature result {key:?}");
                    assert_eq!(random.next_long(), *java_next, "random stream {key:?}");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 36);

        let ore = json!({"type":"minecraft:ore","size":9,"discard_chance_on_air_exposure":0.5,"targets":[
            {"state":"minecraft:diamond_ore","target":{"predicate_type":"minecraft:all_of","rules":[{"predicate_type":"minecraft:tag_match","tag":"minecraft:base_stone_overworld"},{"predicate_type":"minecraft:height_match","min_inclusive":-2,"max_inclusive":2},{"predicate_type":"minecraft:not","rule":{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.25}}]}},
            {"state":"minecraft:gold_ore","target":{"predicate_type":"minecraft:any_of","rules":[{"predicate_type":"minecraft:block_match","block":"minecraft:deepslate"},{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.5}]}}
        ]});
        let placement = PlacedFeature::decode(&json!({"feature":ore,"placement":[
            {"type":"minecraft:count","count":4},{"type":"minecraft:in_square"},
            {"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":-5},"max_inclusive":{"absolute":5}}}
        ]})).unwrap();
        let placed = PlacedOreVein::from_placement(&mut data, placement, &mut |v| {
            Ok(state(
                v.as_str().unwrap().strip_prefix("minecraft:").unwrap(),
            ))
        })
        .unwrap();
        let tags = OreBlockTags::load(&mut data, &registry, &placed.required_tags()).unwrap();
        for seed in 0..8i64 {
            let mut terrain = oracle_terrain();
            let previous = terrain.snapshot();
            let writable = previous.chunks.keys().copied().collect();
            let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
            let mut random = DecorationRandom::new(0);
            let deco = random.set_decoration_seed(seed, -32, 48);
            random.set_feature_seed(deco, 7, 3);
            let mut env = PlacementTestEnv {
                allow: true,
                fail_after: None,
                calls: 0,
            };
            region
                .place_placed(&placed, &mut random, [-32, 0, 48], -16, 32, &mut env)
                .unwrap();
            region.finish().unwrap();
            let world = terrain.snapshot();
            let mut actual = BTreeMap::new();
            for z in 16..80 {
                for x in -48..16 {
                    for y in -16..16 {
                        let after = world.block(x, y, z).unwrap();
                        if previous.block(x, y, z).unwrap() != after {
                            actual.insert([x, y, z], after.name.clone());
                        }
                    }
                }
            }
            let (writes, next) = &integrated[&seed];
            assert_eq!(&actual, writes, "integrated blocks {seed}");
            assert_eq!(random.next_long(), *next, "integrated random stream {seed}");
        }
        let key = FeatureKey::Registered(ResourceId::parse("example:ore").unwrap());
        let (_, java_schedule, _) = scheduled_fixture(None);
        let scheduled = ScheduledOreVein {
            key: key.clone(),
            placed,
            schedule: &java_schedule,
        };
        for seed in 0..8i64 {
            let mut terrain = oracle_terrain();
            terrain.seed = seed;
            let previous = terrain.snapshot();
            let mut neighborhood = super::super::BiomeNeighborhood::new(seed, -16, 32).unwrap();
            for z in 2..=4 {
                for x in -3..=-1 {
                    let biomes = vec![ResourceId::parse("minecraft:plains").unwrap(); 128];
                    neighborhood
                        .insert(Arc::new(
                            super::super::QuartBiomeChunk::from_values(
                                [x, z],
                                seed,
                                -16,
                                32,
                                biomes,
                            )
                            .unwrap(),
                        ))
                        .unwrap();
                }
            }
            let mut random = DecorationRandom::new(0);
            let decoration_seed = random.set_decoration_seed(seed, -32, 48);
            let plan = DecorationPlan {
                origin: [-32, -16, 48],
                decoration_seed,
                features: vec![native_resources::feature_order::SelectedFeature {
                    step: 3,
                    index: 7,
                    feature: &key,
                }],
                biomes: &neighborhood,
                generator_min_y: -16,
                generator_height: 32,
            };
            let mut region = terrain
                .ore_vein_region(&tags, &motion, previous.chunks.keys().copied().collect())
                .unwrap();
            region
                .place_planned(&plan, 0, &scheduled, &mut |_, _| {
                    bail!("fixture contains no biome filter")
                })
                .unwrap();
            region.finish().unwrap();
            let world = terrain.snapshot();
            let mut actual = BTreeMap::new();
            for z in 16..80 {
                for x in -48..16 {
                    for y in -16..16 {
                        let after = world.block(x, y, z).unwrap();
                        if previous.block(x, y, z).unwrap() != after {
                            actual.insert([x, y, z], after.name.clone());
                        }
                    }
                }
            }
            assert_eq!(
                &actual, &integrated[&seed].0,
                "planned Java ore blocks {seed}"
            );
        }
    }
}
