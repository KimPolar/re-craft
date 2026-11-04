//! Isolated postprocessing over the actual generated block snapshot.
//!
//! More block and fluid callbacks are needed before this can produce a
//! playable or saveable chunk. Unknown callbacks fail the entire attempt.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_resources::tree_leaf_update::ShapeEdgeWorld;
use native_world::{
    generation_light::VisibleLightSnapshot,
    loaded::{BlockPosition, BlockState, ChunkPos, World},
};

use super::{
    chunk_ticks::{RegionTicks, ScheduleOutcome, TickRegistry},
    postprocessing::{
        ConvertedPostprocessingRegion, PostprocessingChunk, PostprocessingWorld,
        update_from_neighbor_shapes,
    },
};

/// An isolated attempt. The source snapshot is immutable, and no ticks are
/// returned if a callback fails or if any chunk remains unfinished.
pub struct GeneratedPostprocessing {
    world: Arc<World>,
    source: Arc<World>,
    batch_revision: Option<u64>,
    min_y: i32,
    max_y: i32,
    region: ConvertedPostprocessingRegion,
    completed: BTreeSet<ChunkPos>,
    active: Option<ChunkPos>,
    failed: bool,
    light: Option<VisibleLightSnapshot>,
    mushroom_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    edits: Vec<BlockPosition>,
    light_checks: Vec<BlockPosition>,
    heightmaps: BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps>,
    heightmap_tags: Option<Arc<native_resources::heightmap_tags::HeightmapTags>>,
}

/// Completed callbacks for supported blocks only. Other conversion, lighting,
/// entity and world-save requirements must still be validated.
pub struct PostprocessedGeneratedRegion {
    pub world: Arc<World>,
    pub ticks: RegionTicks,
    pub chunks: BTreeSet<ChunkPos>,
    /// Ordered block changes performed during postprocessing.
    pub edits: Vec<BlockPosition>,
    /// These light checks must be processed before publishing the result.
    pub light_checks: Vec<BlockPosition>,
    /// Updated FULL maps; other chunk and world validation remains necessary.
    pub heightmaps: BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps>,
    pub(super) source: Arc<World>,
    pub(super) sealed_world: Arc<World>,
    pub(super) sealed_checks: Vec<BlockPosition>,
    pub(super) sealed_chunks: BTreeSet<ChunkPos>,
    pub(super) batch_revision: Option<u64>,
    pub(super) min_y: i32,
    pub(super) max_y: i32,
}

/// Apply one explicitly chosen light-engine drain to a complete input
/// revision. Source heights already include all edits affecting direct sky.
pub(super) fn drain_postprocessing_light(
    world: &World,
    bounds: native_world::block_light::LightBounds,
    before: &VisibleLightSnapshot,
    active: &BTreeSet<ChunkPos>,
    sources: &BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    checks: &[[i32; 3]],
) -> Result<VisibleLightSnapshot> {
    let block = native_world::generation_block_light::check_generated_block_light(
        world, bounds, before, active, checks,
    )
    .context("postprocessing block-light inputs are incomplete or unsupported")?;
    if before.has_sky_channel() {
        native_world::generation_sky_light::check_generated_sky_blocks(
            world, bounds, &block, active, checks, sources,
        )
        .context("postprocessing sky-light inputs are incomplete or unsupported")
    } else {
        Ok(block)
    }
}

impl GeneratedPostprocessing {
    pub(super) fn new(
        world: Arc<World>,
        min_y: i32,
        max_y: i32,
        region: ConvertedPostprocessingRegion,
        light: Option<VisibleLightSnapshot>,
        mushroom_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
        heightmaps: BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps>,
        heightmap_tags: Option<Arc<native_resources::heightmap_tags::HeightmapTags>>,
    ) -> Self {
        Self {
            source: Arc::clone(&world),
            world,
            batch_revision: None,
            min_y,
            max_y,
            region,
            completed: BTreeSet::new(),
            active: None,
            failed: false,
            light,
            mushroom_tags,
            edits: Vec::new(),
            light_checks: Vec::new(),
            heightmaps,
            heightmap_tags,
        }
    }

    pub(super) fn bind_batch_revision(&mut self, revision: u64) {
        self.batch_revision = Some(revision);
    }

    /// Run a bounded number of callbacks without interleaving another chunk.
    /// Java runs each chunk's pass synchronously against its live neighbors.
    pub fn poll(&mut self, pos: ChunkPos, budget: usize) -> Result<bool> {
        ensure!(!self.failed, "generated postprocessing attempt failed");
        ensure!(
            !self.completed.contains(&pos),
            "chunk was already processed"
        );
        ensure!(
            self.active.is_none_or(|active| active == pos),
            "another chunk is being processed"
        );
        let task = self
            .region
            .tasks
            .get_mut(&pos)
            .context("chunk is not in the converted region")?;
        self.active = Some(pos);
        self.failed = true;
        let mut view = GeneratedBlockView {
            world: &mut self.world,
            min_y: self.min_y,
            max_y: self.max_y,
            ticks: &mut self.region.ticks,
            light: self.light.as_ref(),
            mushroom_tags: self.mushroom_tags.as_deref(),
            edits: &mut self.edits,
            light_checks: &mut self.light_checks,
            heightmaps: &mut self.heightmaps,
            heightmap_tags: self.heightmap_tags.as_deref(),
        };
        let done = task.poll(&mut view, budget)?;
        if done {
            self.completed.insert(pos);
            self.active = None;
        }
        self.failed = false;
        Ok(done)
    }

    pub fn finish(self) -> Result<PostprocessedGeneratedRegion> {
        ensure!(!self.failed, "generated postprocessing attempt failed");
        ensure!(
            self.completed.len() == self.region.tasks.len(),
            "generated postprocessing is incomplete"
        );
        Ok(PostprocessedGeneratedRegion {
            sealed_world: Arc::clone(&self.world),
            world: self.world,
            ticks: self.region.ticks,
            sealed_chunks: self.completed.clone(),
            chunks: self.completed,
            edits: self.edits,
            sealed_checks: self.light_checks.clone(),
            light_checks: self.light_checks,
            heightmaps: self.heightmaps,
            source: self.source,
            batch_revision: self.batch_revision,
            min_y: self.min_y,
            max_y: self.max_y,
        })
    }
}

struct GeneratedBlockView<'a> {
    world: &'a mut Arc<World>,
    min_y: i32,
    max_y: i32,
    ticks: &'a mut RegionTicks,
    light: Option<&'a VisibleLightSnapshot>,
    mushroom_tags: Option<&'a native_resources::ore_feature::OreBlockTags>,
    edits: &'a mut Vec<BlockPosition>,
    light_checks: &'a mut Vec<BlockPosition>,
    heightmaps: &'a mut BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps>,
    heightmap_tags: Option<&'a native_resources::heightmap_tags::HeightmapTags>,
}

#[derive(Clone, Copy)]
enum DryBlock {
    Plain,
    Falling,
    Mushroom,
}

fn supported_dry_block(state: &BlockState) -> Result<DryBlock> {
    ensure!(
        state.properties.is_empty(),
        "unsupported generated postprocessing block state"
    );
    match state.name.as_str() {
        "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel" => Ok(DryBlock::Falling),
        "minecraft:brown_mushroom" | "minecraft:red_mushroom" => Ok(DryBlock::Mushroom),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:stone"
        | "minecraft:dirt" => Ok(DryBlock::Plain),
        _ => bail!("unsupported generated postprocessing block {}", state.name),
    }
}

impl GeneratedBlockView<'_> {
    fn read(&self, pos: [i32; 3]) -> Result<BlockState> {
        if !(self.min_y..self.max_y).contains(&pos[1]) {
            return Ok(BlockState {
                name: "minecraft:void_air".into(),
                properties: Default::default(),
            });
        }
        let chunk = ChunkPos {
            x: pos[0].div_euclid(16),
            z: pos[2].div_euclid(16),
        };
        ensure!(
            self.ticks.chunk(chunk).is_some(),
            "postprocessing neighbor is not converted"
        );
        self.world
            .block(pos[0], pos[1], pos[2])
            .cloned()
            .context("generated postprocessing block is missing")
    }

    fn mushroom_survives(&self, pos: [i32; 3]) -> Result<bool> {
        let below = self.read([pos[0], pos[1].wrapping_sub(1), pos[2]])?;
        static TAG: std::sync::OnceLock<native_resources::packs::ResourceId> =
            std::sync::OnceLock::new();
        let tag = TAG.get_or_init(|| {
            native_resources::packs::ResourceId::parse(
                "minecraft:overrides_mushroom_light_requirement",
            )
            .expect("static tag identifier")
        });
        if self
            .mushroom_tags
            .context("mushroom support tags unavailable")?
            .contains(tag, &below.name)?
        {
            return Ok(true);
        }
        let brightness = self
            .light
            .context("mushroom lighting unavailable")?
            .raw_brightness(pos, 0)
            .context("mushroom light column unavailable")?;
        if brightness >= 13 {
            return Ok(false);
        }
        if matches!(
            below.name.as_str(),
            "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
        ) {
            ensure!(
                below.properties.len() == 1
                    && matches!(
                        below.properties.get("snowy").map(String::as_str),
                        Some("true" | "false")
                    ),
                "invalid mushroom support state"
            );
            return Ok(true);
        }
        if matches!(
            below.name.as_str(),
            "minecraft:crimson_nylium" | "minecraft:warped_nylium"
        ) {
            ensure!(
                below.properties.is_empty(),
                "invalid mushroom support state"
            );
            return Ok(true);
        }
        Ok(super::ore::monster_block_flags(&below)?.1)
    }

    fn replace_mushroom(&mut self, pos: [i32; 3], next: &BlockState, flags: i32) -> Result<()> {
        ensure!(
            flags == 276 && next.name == "minecraft:air" && next.properties.is_empty(),
            "unsupported generated postprocessing write"
        );
        let old = self.read(pos)?;
        ensure!(
            matches!(supported_dry_block(&old)?, DryBlock::Mushroom),
            "postprocessing source is not a mushroom"
        );
        let same_light = native_world::block_light::same_light_behavior(&old, next)
            .context("mushroom light behavior unavailable")?;
        ensure!(
            native_world::sky_light::same_sky_behavior(&old, next) == Some(true),
            "mushroom removal changes direct sky sources"
        );
        if !same_light {
            let change = native_world::block_light::light_change(&old, next)
                .context("mushroom light change unavailable")?;
            ensure!(
                change.radius() == 0,
                "mushroom light change requires propagation during postprocessing"
            );
        }
        let world = Arc::make_mut(self.world);
        let id = world.intern_state(next.clone());
        ensure!(
            world
                .set_block_id(pos[0], pos[1], pos[2], id)
                .map_err(|error| anyhow::anyhow!("cannot update generated mushroom: {error:?}"))?,
            "generated mushroom write was unchanged"
        );
        let tags = self
            .heightmap_tags
            .context("generated heightmap tags unavailable")?;
        let chunk = ChunkPos {
            x: pos[0].div_euclid(16),
            z: pos[2].div_euclid(16),
        };
        let maps = self
            .heightmaps
            .get_mut(&chunk)
            .context("generated FULL heightmaps unavailable")?;
        let base_x = pos[0] - pos[0].rem_euclid(16);
        let base_z = pos[2] - pos[2].rem_euclid(16);
        maps.update_after_change(
            pos[0],
            pos[1],
            pos[2],
            crate::spawn_heightmaps::classify(next, tags)?,
            |x, y, z| {
                let block = world
                    .block(base_x + x, y, base_z + z)
                    .context("heightmap scan block unavailable")?;
                crate::spawn_heightmaps::classify(block, tags)
            },
        )?;
        let position = BlockPosition {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        };
        self.edits.push(position);
        if !same_light {
            self.light_checks.push(position);
        }
        Ok(())
    }
}

impl ShapeEdgeWorld for GeneratedBlockView<'_> {
    type State = BlockState;

    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        self.read(pos)
    }

    fn update_shape(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        _: [i32; 3],
        _: [i32; 3],
        _: &BlockState,
    ) -> Result<BlockState> {
        match supported_dry_block(state)? {
            DryBlock::Falling => {
                ensure!(
                    self.ticks
                        .schedule(TickRegistry::Block, &state.name, pos, 2, 0)?
                        != ScheduleOutcome::MissingChunk,
                    "falling block tick container is missing"
                );
            }
            DryBlock::Mushroom if !self.mushroom_survives(pos)? => {
                return Ok(BlockState {
                    name: "minecraft:air".into(),
                    properties: Default::default(),
                });
            }
            DryBlock::Plain | DryBlock::Mushroom => {}
        }
        Ok(state.clone())
    }

    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        self.replace_mushroom(pos, state, flags)
    }
}

impl PostprocessingWorld for GeneratedBlockView<'_> {
    type State = BlockState;
    type Fluid = ();

    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        self.read(pos)
    }
    fn fluid(&mut self, state: &BlockState) -> Result<Option<()>> {
        supported_dry_block(state)?;
        Ok(None)
    }
    fn tick_fluid(&mut self, _: [i32; 3], _: &BlockState, _: &()) -> Result<()> {
        bail!("generated fluid postprocessing is not implemented")
    }
    fn is_liquid_block(&mut self, _: &BlockState) -> Result<bool> {
        Ok(false)
    }
    fn tick_liquid_block(&mut self, _: [i32; 3], _: &BlockState) -> Result<()> {
        bail!("generated liquid postprocessing is not implemented")
    }
    fn update_neighbor_shapes(&mut self, pos: [i32; 3], state: &BlockState) -> Result<BlockState> {
        update_from_neighbor_shapes(self, pos, state)
    }
    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        self.replace_mushroom(pos, state, flags)
    }
    fn clear_processed_section(&mut self, _: usize) -> Result<()> {
        Ok(())
    }
}

impl PostprocessingChunk for GeneratedBlockView<'_> {
    // Fresh generation tracks its materialized block entities separately; the
    // caller validates the generated entity data before constructing this view.
    fn pending_block_entity_positions(&mut self) -> Result<Vec<[i32; 3]>> {
        Ok(Vec::new())
    }
    fn promote_pending_block_entity(&mut self, _: [i32; 3]) -> Result<()> {
        bail!("unexpected pending generated block entity")
    }
    fn clear_pending_block_entities(&mut self) -> Result<()> {
        Ok(())
    }
    fn apply_upgrade_data(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::{
        chunk_ticks::{ChunkTicks, ConvertedChunkTicks, SavedTick},
        postprocessing::PostprocessingTask,
    };
    use native_core::postprocessing::PostProcessing;
    use native_world::loaded::{BLOCK_COUNT, Chunk, Section};
    use std::collections::BTreeMap;

    fn fixture_maps(surface: i32) -> native_persistence::heightmap::FullHeightmaps {
        use native_persistence::heightmap::{
            FullHeightmaps, GeneratedHeightmaps, HeightBounds, encode_generated_heightmaps,
            generated_full_heightmaps,
        };
        let bounds = HeightBounds {
            min_y: 0,
            max_y: 16,
        };
        let mut heights = [0; 256];
        heights[8 + 8 * 16] = surface;
        let source = encode_generated_heightmaps(
            bounds,
            &GeneratedHeightmaps {
                world_surface: Some(&heights),
                ..Default::default()
            },
        )
        .unwrap();
        FullHeightmaps::from_converted(bounds, &generated_full_heightmaps(bounds, &source).unwrap())
            .unwrap()
    }

    fn fixture(unsupported: bool) -> (GeneratedPostprocessing, Arc<World>, ChunkPos) {
        let pos = ChunkPos { x: 0, z: 0 };
        let mut world = World::new("minecraft:overworld");
        let state = |name: &str| BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        };
        let stone = world.intern_state(state("stone"));
        let sand = world.intern_state(state("sand"));
        let red = world.intern_state(state("red_sand"));
        let gravel = world.intern_state(state("gravel"));
        let unknown = world.intern_state(state("oak_log"));
        let mut blocks = Box::new([stone; BLOCK_COUNT]);
        let index = |x: usize| x + 8 * 16 + 8 * 256;
        blocks[index(8)] = sand;
        blocks[index(9)] = if unsupported { unknown } else { red };
        blocks[index(10)] = gravel;
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:spawn".into()),
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
        let world = Arc::new(world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        for x in [8, 8, 9, 10] {
            marks.mark(x, 8, 8);
        }
        let task = PostprocessingTask::new(Arc::new(marks), pos);
        let ticks = ConvertedChunkTicks {
            blocks: ChunkTicks::from_saved(
                pos,
                vec![SavedTick {
                    kind: "minecraft:sand".into(),
                    position: [8, 8, 8],
                    delay: 0,
                    priority: 0,
                }],
            )
            .unwrap(),
            fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
        };
        let region = ConvertedPostprocessingRegion {
            tasks: BTreeMap::from([(pos, task)]),
            ticks: RegionTicks::new(BTreeMap::from([(pos, ticks)]), 100, 2).unwrap(),
        };
        (
            GeneratedPostprocessing::new(
                Arc::clone(&world),
                0,
                16,
                region,
                None,
                None,
                BTreeMap::from([(pos, fixture_maps(0))]),
                None,
            ),
            world,
            pos,
        )
    }

    fn result_trace(budget: usize) -> Vec<String> {
        let (mut stage, source, pos) = fixture(false);
        while !stage.poll(pos, budget).unwrap() {}
        let result = stage.finish().unwrap();
        assert!(Arc::ptr_eq(&source, &result.world));
        let mut trace = Vec::new();
        for tick in result.ticks.chunk(pos).unwrap().blocks.pack(100) {
            trace.push(format!(
                "{} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            ));
        }
        trace.push(format!("counter {}", result.ticks.next_sub_tick()));
        trace
    }

    fn mushroom_case(mushroom: &str, support: &str, tagged: bool, brightness: u8) -> String {
        use native_resources::{
            ore_feature::OreBlockTags,
            packs::{ResourceId, ResourceProvider},
        };
        use native_world::generation_light::VisibleLightColumn;
        struct TagSource {
            support: String,
            tagged: bool,
        }
        impl ResourceProvider for TagSource {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if matches!(
                    path,
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                        | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
                ) {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                if path != "data/minecraft/tags/block/overrides_mushroom_light_requirement.json" {
                    return Ok(None);
                }
                Ok(Some(serde_json::to_vec(
                    &serde_json::json!({"values": if self.tagged { vec![self.support.clone()] } else { vec![] }}),
                )?))
            }
        }
        let pos = ChunkPos { x: 0, z: 0 };
        let id = |name: &str| format!("minecraft:{name}");
        let registry = BTreeSet::from([id("air"), id("void_air"), id(mushroom), id(support)]);
        let tags = OreBlockTags::load(
            &mut TagSource {
                support: id(support),
                tagged,
            },
            &registry,
            &BTreeSet::from([
                ResourceId::parse("minecraft:overrides_mushroom_light_requirement").unwrap(),
            ]),
        )
        .unwrap();
        let heightmap_tags = native_resources::heightmap_tags::HeightmapTags::load(
            &mut TagSource {
                support: id(support),
                tagged,
            },
            &registry,
        )
        .unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: id("air"),
            properties: BTreeMap::new(),
        });
        let plant = world.intern_state(BlockState {
            name: id(mushroom),
            properties: BTreeMap::new(),
        });
        let properties = match support {
            "grass_block" | "podzol" | "mycelium" => {
                BTreeMap::from([("snowy".into(), "false".into())])
            }
            "water" => BTreeMap::from([("level".into(), "0".into())]),
            "chest" => BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
            _ => BTreeMap::new(),
        };
        let support_id = world.intern_state(BlockState {
            name: id(support),
            properties,
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[8 + 8 * 16 + 8 * 256] = plant;
        blocks[8 + 8 * 16 + 7 * 256] = support_id;
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:spawn".into()),
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
        let source = Arc::new(world);
        let mut light = VisibleLightSnapshot::new(true, false);
        light.insert_complete_column(
            pos,
            VisibleLightColumn::new(
                BTreeMap::from([(
                    0,
                    Arc::new([brightness | brightness << 4; native_world::loaded::LIGHT_BYTES]),
                )]),
                BTreeMap::new(),
            ),
        );
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        let region = ConvertedPostprocessingRegion {
            tasks: BTreeMap::from([(pos, PostprocessingTask::new(Arc::new(marks), pos))]),
            ticks: RegionTicks::new(
                BTreeMap::from([(
                    pos,
                    ConvertedChunkTicks {
                        blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                        fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                    },
                )]),
                100,
                0,
            )
            .unwrap(),
        };
        let mut stage = GeneratedPostprocessing::new(
            Arc::clone(&source),
            0,
            16,
            region,
            Some(light),
            Some(Arc::new(tags)),
            BTreeMap::from([(pos, fixture_maps(9))]),
            Some(Arc::new(heightmap_tags)),
        );
        while !stage.poll(pos, 1).unwrap() {}
        let result = stage.finish().unwrap();
        assert_eq!(source.block(8, 8, 8).unwrap().name, id(mushroom));
        let state = &result.world.block(8, 8, 8).unwrap().name;
        let removed = state == "minecraft:air";
        assert_eq!(result.edits.len(), usize::from(removed));
        assert_eq!(
            result.light_checks.len(),
            usize::from(removed && mushroom == "brown_mushroom")
        );
        let maps = result.heightmaps[&pos].to_tag().unwrap();
        let document = native_persistence::nbt::NamedTag {
            name: "".into(),
            value: native_persistence::nbt::Tag::Compound(vec![("Heightmaps".into(), maps)]),
        };
        let heights = native_persistence::heightmap::SpawnHeightmaps::read(
            &document,
            native_persistence::heightmap::HeightBounds {
                min_y: 0,
                max_y: 16,
            },
        )
        .unwrap();
        assert_eq!(
            heights
                .highest(
                    native_persistence::heightmap::SpawnHeightmapKind::WorldSurface,
                    8,
                    8
                )
                .unwrap()
                + 1,
            if removed {
                if support == "air" { 0 } else { 8 }
            } else {
                9
            }
        );
        format!(
            "{} {} {} {} {} {} {}",
            id(mushroom),
            id(support),
            tagged,
            brightness,
            state,
            usize::from(removed),
            if removed { 276 } else { -1 }
        )
    }

    #[test]
    fn mushroom_removal_preserves_source_and_records_light_and_heightmap_work() {
        assert_eq!(
            mushroom_case("brown_mushroom", "stone", false, 12),
            "minecraft:brown_mushroom minecraft:stone false 12 minecraft:brown_mushroom 0 -1"
        );
        assert_eq!(
            mushroom_case("brown_mushroom", "stone", false, 13),
            "minecraft:brown_mushroom minecraft:stone false 13 minecraft:air 1 276"
        );
        assert_eq!(
            mushroom_case("red_mushroom", "stone", true, 15),
            "minecraft:red_mushroom minecraft:stone true 15 minecraft:red_mushroom 0 -1"
        );
        assert_eq!(
            mushroom_case("red_mushroom", "air", false, 0),
            "minecraft:red_mushroom minecraft:air false 0 minecraft:air 1 276"
        );
    }

    #[test]
    fn drained_mushroom_light_preserves_prior_revision_and_requires_both_channels() {
        use native_world::{
            block_light::LightBounds, generation_light::VisibleLightColumn, loaded::LIGHT_BYTES,
        };
        let pos = ChunkPos { x: 0, z: 0 };
        let p = [8, 8, 8];
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:spawn".into()),
                block_light_correct: false,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([air; BLOCK_COUNT])),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let mut layer = [0; LIGHT_BYTES];
        let index = 8 + 8 * 16 + 8 * 256;
        layer[index / 2] = 1;
        let column =
            VisibleLightColumn::new(BTreeMap::from([(0, Arc::new(layer))]), BTreeMap::new());
        let mut before = VisibleLightSnapshot::new(true, false);
        before.insert_complete_column(pos, column.clone());
        let bounds = LightBounds::new(0, 16).unwrap();
        let active = BTreeSet::from([pos]);
        let after =
            drain_postprocessing_light(&world, bounds, &before, &active, &BTreeMap::new(), &[p, p])
                .unwrap();
        assert_eq!(before.block_light(p), Some(1));
        assert_eq!(after.block_light(p), Some(0));
        assert_eq!(after.block_section_allocated(pos, 0), Some(true));

        let mut both = VisibleLightSnapshot::new(true, true);
        both.insert_complete_column(
            pos,
            VisibleLightColumn::new(
                BTreeMap::from([(0, Arc::new(layer))]),
                BTreeMap::from([(0, Arc::new(layer))]),
            ),
        );
        let after_both = drain_postprocessing_light(
            &world,
            bounds,
            &both,
            &BTreeSet::new(),
            &BTreeMap::new(),
            &[p],
        )
        .unwrap();
        assert_eq!(after_both.block_light(p), Some(0));
        assert_eq!(after_both.sky_light(p), Some(0));
        assert_eq!(both.block_light(p), Some(1));
        assert_eq!(both.sky_light(p), Some(1));

        let mut missing_sky = VisibleLightSnapshot::new(true, true);
        missing_sky.insert_complete_column(pos, column);
        assert!(
            drain_postprocessing_light(
                &world,
                bounds,
                &missing_sky,
                &active,
                &BTreeMap::new(),
                &[p],
            )
            .is_err()
        );
        assert_eq!(missing_sky.block_light(p), Some(1));
    }

    #[test]
    fn generated_falling_blocks_schedule_real_ticks_and_fail_atomically() {
        let expected = vec![
            "minecraft:sand 8,8,8 0 0",
            "minecraft:red_sand 9,8,8 2 0",
            "minecraft:gravel 10,8,8 2 0",
            "counter 26",
        ];
        assert_eq!(result_trace(1), expected);
        assert_eq!(result_trace(100), expected);
        let (mut stage, source, pos) = fixture(true);
        assert!(stage.poll(pos, 3).is_err());
        assert!(stage.poll(pos, 1).is_err());
        assert!(stage.finish().is_err());
        assert_eq!(source.block(9, 8, 8).unwrap().name, "minecraft:oak_log");
        assert!(source.saved_block_ticks.is_empty());
        let (stage, _, _) = fixture(false);
        assert!(stage.finish().is_err());
    }

    fn java_results(class_name: &str, java_source: &str) -> Vec<String> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let source = temp.path().join(format!("{class_name}.java"));
        std::fs::write(&source, java_source).unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg(class_name)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let output = String::from_utf8(run.stdout).unwrap();
        let lines: Vec<_> = output.lines().collect();
        let start = lines
            .iter()
            .position(|line| *line == "BEGIN_RESULT")
            .unwrap();
        let end = lines.iter().position(|line| *line == "END_RESULT").unwrap();
        lines[start + 1..end]
            .iter()
            .map(|line| (*line).to_owned())
            .collect()
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_falling_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedFallingPostprocessingOracle",
            include_str!("../../tests/java/GeneratedFallingPostprocessingOracle.java"),
        );
        assert_eq!(actual, result_trace(1));
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_mushroom_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedMushroomPostprocessingOracle",
            include_str!("../../tests/java/GeneratedMushroomPostprocessingOracle.java"),
        );
        let mut expected = Vec::new();
        for mushroom in ["brown_mushroom", "red_mushroom"] {
            for support in [
                "stone",
                "dirt",
                "grass_block",
                "podzol",
                "mycelium",
                "crimson_nylium",
                "warped_nylium",
                "water",
                "air",
                "chest",
                "spawner",
                "deepslate",
            ] {
                for tagged in [false, true] {
                    for brightness in [0, 12, 13, 15] {
                        expected.push(mushroom_case(mushroom, support, tagged, brightness));
                    }
                }
            }
        }
        assert_eq!(actual, expected);
    }
}
