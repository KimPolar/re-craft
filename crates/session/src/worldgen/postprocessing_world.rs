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
    leaf_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    bubble_tags: Option<Arc<native_resources::fluid_tags::BubbleColumnTags>>,
    washed_blocks: Option<Arc<BTreeSet<String>>>,
    lava_fluids: Option<Arc<BTreeSet<String>>>,
    water_source_conversion: Option<bool>,
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
    water_source_conversion: Option<bool>,
}

impl PostprocessedGeneratedRegion {
    /// The explicit rule used for this pass, if supplied by its caller.
    pub fn water_source_conversion(&self) -> Option<bool> {
        self.water_source_conversion
    }
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
            leaf_tags: None,
            bubble_tags: None,
            washed_blocks: None,
            lava_fluids: None,
            water_source_conversion: None,
            edits: Vec::new(),
            light_checks: Vec::new(),
            heightmaps,
            heightmap_tags,
        }
    }

    pub(super) fn bind_batch_revision(&mut self, revision: u64) {
        self.batch_revision = Some(revision);
    }

    pub(super) fn set_leaf_tags(
        &mut self,
        tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    ) {
        self.leaf_tags = tags;
    }

    pub(super) fn set_bubble_tags(
        &mut self,
        tags: Option<Arc<native_resources::fluid_tags::BubbleColumnTags>>,
    ) {
        self.bubble_tags = tags;
    }

    pub(super) fn set_fluid_tags(
        &mut self,
        washed: Option<Arc<BTreeSet<String>>>,
        lava: Option<Arc<BTreeSet<String>>>,
    ) {
        self.washed_blocks = washed;
        self.lava_fluids = lava;
    }

    /// Supply the level's actual gamerule before executing any chunk. The
    /// caller must keep it fixed while this isolated generation pass runs.
    pub fn set_water_source_conversion(&mut self, enabled: bool) -> Result<()> {
        ensure!(
            !self.failed && self.active.is_none() && self.completed.is_empty(),
            "water source conversion must be set before postprocessing begins"
        );
        self.water_source_conversion = Some(enabled);
        Ok(())
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
            leaf_tags: self.leaf_tags.as_deref(),
            bubble_tags: self.bubble_tags.as_deref(),
            washed_blocks: self.washed_blocks.as_deref(),
            lava_fluids: self.lava_fluids.as_deref(),
            water_source_conversion: self.water_source_conversion,
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
            water_source_conversion: self.water_source_conversion,
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
    leaf_tags: Option<&'a native_resources::ore_feature::OreBlockTags>,
    bubble_tags: Option<&'a native_resources::fluid_tags::BubbleColumnTags>,
    washed_blocks: Option<&'a BTreeSet<String>>,
    lava_fluids: Option<&'a BTreeSet<String>>,
    water_source_conversion: Option<bool>,
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
    Leaves,
}

fn leaf_distance(state: &BlockState) -> Result<Option<u8>> {
    if !matches!(
        state.name.as_str(),
        "minecraft:oak_leaves"
            | "minecraft:spruce_leaves"
            | "minecraft:birch_leaves"
            | "minecraft:jungle_leaves"
            | "minecraft:acacia_leaves"
            | "minecraft:dark_oak_leaves"
            | "minecraft:mangrove_leaves"
            | "minecraft:cherry_leaves"
            | "minecraft:pale_oak_leaves"
            | "minecraft:azalea_leaves"
            | "minecraft:flowering_azalea_leaves"
    ) {
        return Ok(None);
    }
    ensure!(
        state.properties.len() == 3
            && matches!(
                state.properties.get("persistent").map(String::as_str),
                Some("true" | "false")
            )
            && matches!(
                state.properties.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            ),
        "invalid generated leaf state"
    );
    let raw = state
        .properties
        .get("distance")
        .context("missing leaf distance")?;
    let distance = raw
        .parse::<u8>()
        .ok()
        .filter(|distance| (1..=7).contains(distance) && distance.to_string() == *raw)
        .context("invalid leaf distance")?;
    Ok(Some(distance))
}

fn supported_dry_block(state: &BlockState) -> Result<DryBlock> {
    if leaf_distance(state)?.is_some() {
        ensure!(
            state.properties["waterlogged"] == "false",
            "waterlogged leaf fluid tick is unsupported"
        );
        return Ok(DryBlock::Leaves);
    }
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

fn liquid_state(state: &BlockState) -> Result<Option<native_world::fluid::FluidState>> {
    if !matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
        return Ok(None);
    }
    let raw = state
        .properties
        .get("level")
        .context("liquid level missing")?;
    ensure!(
        state.properties.len() == 1
            && raw
                .parse::<u8>()
                .ok()
                .is_some_and(|level| level <= 15 && level.to_string() == *raw),
        "invalid generated liquid state"
    );
    native_world::fluid::FluidState::from_block(state).map_err(anyhow::Error::msg)
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

    fn fluid_cell(&self, pos: glam::IVec3) -> native_world::fluid_sim::Cell {
        use native_world::fluid_sim::Cell;
        let Ok(state) = self.read(pos.to_array()) else {
            return Cell::Unknown;
        };
        if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            return liquid_state(&state)
                .ok()
                .flatten()
                .map_or(Cell::Unknown, Cell::Fluid);
        }
        if !state.properties.is_empty() {
            return Cell::Unknown;
        }
        match state.name.as_str() {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => Cell::Air,
            "minecraft:stone"
            | "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:rooted_dirt"
            | "minecraft:deepslate"
            | "minecraft:tuff"
            | "minecraft:granite"
            | "minecraft:diorite"
            | "minecraft:andesite"
            | "minecraft:bedrock"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:gravel"
            | "minecraft:clay" => Cell::Solid,
            _ => Cell::Unknown,
        }
    }

    fn fluid_cell_for_spread(&self, pos: glam::IVec3) -> native_world::fluid_sim::Cell {
        use native_world::{fluid::FluidKind, fluid_sim::Cell};
        let cell = self.fluid_cell(pos);
        match cell {
            Cell::Air | Cell::Fluid(_) => {
                let Ok(state) = self.read(pos.to_array()) else {
                    return Cell::Unknown;
                };
                if !self
                    .washed_blocks
                    .is_some_and(|tags| tags.contains(&state.name))
                {
                    return Cell::Unknown;
                }
                if matches!(cell, Cell::Fluid(fluid) if fluid.kind != FluidKind::Water) {
                    return Cell::Unknown;
                }
                cell
            }
            _ => cell,
        }
    }

    fn tick_generated_fluid(
        &mut self,
        pos: [i32; 3],
        fluid: native_world::fluid::FluidState,
    ) -> Result<()> {
        use native_world::{
            fluid::FluidKind,
            fluid_sim::{Cell, Settings, try_tick},
        };
        // A source enclosed on the bottom and all four sides cannot flow,
        // regardless of gamerules or the dimension's lava speed.
        if fluid.source
            && pos
                .iter()
                .all(|value| value.checked_sub(6).is_some() && value.checked_add(6).is_some())
            && self.fluid_cell(glam::IVec3::from_array(pos)) == Cell::Fluid(fluid)
            && [[0, -1, 0], [-1, 0, 0], [1, 0, 0], [0, 0, -1], [0, 0, 1]]
                .into_iter()
                .all(|delta| {
                    self.fluid_cell(glam::IVec3::new(
                        pos[0] + delta[0],
                        pos[1] + delta[1],
                        pos[2] + delta[2],
                    )) == Cell::Solid
                })
        {
            return Ok(());
        }
        // Unknown rules must yield the same plan for both possible values.
        // Water's delay is fixed and never samples lava's delay RNG.
        let mut planned = None;
        for converts in [false, true] {
            if fluid.kind == FluidKind::Water
                && self
                    .water_source_conversion
                    .is_some_and(|rule| rule != converts)
            {
                continue;
            }
            for fast_lava in [false, true] {
                if fluid.kind == FluidKind::Water && fast_lava {
                    continue;
                }
                let settings = Settings {
                    water_source_conversion: converts,
                    lava_source_conversion: converts,
                    fast_lava,
                };
                let changes = try_tick(
                    &|p| self.fluid_cell(p),
                    glam::IVec3::from_array(pos),
                    fluid,
                    settings,
                )
                .map_err(|error| {
                    anyhow::anyhow!("generated fluid tick is incomplete: {error:?}")
                })?;
                if let Some(previous) = &planned {
                    ensure!(
                        *previous == changes,
                        "generated fluid tick depends on unavailable settings"
                    );
                } else {
                    planned = Some(changes);
                }
            }
        }
        let planned = planned.context("fluid settings were not evaluated")?;
        if planned.is_empty() {
            return Ok(());
        }
        use native_world::fluid_sim::ChangedBlock;
        ensure!(
            fluid.kind == FluidKind::Water,
            "generated lava callbacks are unsupported"
        );
        let origin = glam::IVec3::from_array(pos);
        for converts in [false, true] {
            if self
                .water_source_conversion
                .is_some_and(|rule| rule != converts)
            {
                continue;
            }
            let settings = Settings {
                water_source_conversion: converts,
                ..Settings::default()
            };
            let strict = try_tick(&|p| self.fluid_cell_for_spread(p), origin, fluid, settings)
                .map_err(|error| {
                    anyhow::anyhow!("generated water path is incomplete: {error:?}")
                })?;
            ensure!(
                strict == planned,
                "water path differs under active replacement tags"
            );
        }
        for change in &planned {
            let direction = change.position - origin;
            if direction == glam::IVec3::ZERO {
                ensure!(
                    !fluid.source
                        && (matches!(change.block, ChangedBlock::Air)
                            || matches!(change.block, ChangedBlock::Fluid(next)
                            if next.kind == FluidKind::Water && (1..=8).contains(&next.amount)
                                && (!next.source || (next.amount == 8 && !next.falling)))),
                    "unsupported generated water origin update"
                );
                continue;
            }
            let valid_direction = direction == glam::IVec3::NEG_Y
                || [
                    glam::IVec3::NEG_X,
                    glam::IVec3::X,
                    glam::IVec3::NEG_Z,
                    glam::IVec3::Z,
                ]
                .contains(&direction);
            ensure!(
                valid_direction
                    && matches!(change.block, ChangedBlock::Fluid(next)
                    if next.kind == FluidKind::Water
                        && ((direction == glam::IVec3::NEG_Y && !next.source && next.falling && next.amount == 8)
                            || (direction != glam::IVec3::NEG_Y && !next.falling
                                && ((next.source && next.amount == 8)
                                    || (!next.source && (1..=7).contains(&next.amount)))))),
                "generated fluid tick requires unsupported block callbacks"
            );
            ensure!(
                matches!(self.fluid_cell_for_spread(change.position), Cell::Air),
                "water spread cannot replace the existing block yet"
            );
        }
        let settings = Settings {
            water_source_conversion: self.water_source_conversion.unwrap_or(true),
            ..Settings::default()
        };
        let mut live = GeneratedWaterTick {
            view: self,
            origin: glam::IVec3::from_array(pos),
        };
        native_world::fluid_sim::tick_live(
            &mut live,
            glam::IVec3::from_array(pos),
            fluid,
            settings,
        )
        .map_err(|error| anyhow::anyhow!("generated water tick failed: {error:?}"))?;
        Ok(())
    }

    fn schedule_water(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        let fluid = liquid_state(state)?.context("water callback has no fluid")?;
        ensure!(
            fluid.kind == native_world::fluid::FluidKind::Water,
            "unsupported fluid callback"
        );
        let id = if fluid.source {
            "minecraft:water"
        } else {
            "minecraft:flowing_water"
        };
        ensure!(
            self.ticks.schedule(TickRegistry::Fluid, id, pos, 5, 0)?
                != ScheduleOutcome::MissingChunk,
            "water tick container is missing"
        );
        Ok(())
    }

    fn schedule_water_bubble(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        let fluid = liquid_state(state)?.context("water callback has no fluid")?;
        let tags = self.bubble_tags.context("bubble-column tags unavailable")?;
        if fluid.source && tags.can_occupy_fluid("minecraft:water") {
            let support = self.read([pos[0], pos[1].wrapping_sub(1), pos[2]])?;
            if tags.enables(&support.name) {
                ensure!(
                    self.ticks
                        .schedule(TickRegistry::Block, "minecraft:water", pos, 20, 0)?
                        != ScheduleOutcome::MissingChunk,
                    "bubble tick container is missing"
                );
            }
        }
        Ok(())
    }

    fn spread_water(
        &mut self,
        target: glam::IVec3,
        fluid: native_world::fluid::FluidState,
    ) -> Result<()> {
        use native_world::fluid_sim::Cell;
        let old = self.read(target.to_array())?;
        ensure!(
            matches!(self.fluid_cell(target), Cell::Air),
            "water target is not air"
        );
        ensure!(
            self.washed_blocks
                .context("fluid replacement tags unavailable")?
                .contains(&old.name),
            "water target is not tagged as washable"
        );
        let next = BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([(
                "level".into(),
                (8 - fluid.amount + if fluid.falling { 8 } else { 0 }).to_string(),
            )]),
        };
        self.apply_water_change(target, &old, &next)
    }

    fn apply_water_change(
        &mut self,
        target: glam::IVec3,
        old: &BlockState,
        next: &BlockState,
    ) -> Result<()> {
        use native_world::fluid_sim::Cell;
        let lava_tags = self.lava_fluids.context("lava fluid tags unavailable")?;
        ensure!(
            !lava_tags.contains("minecraft:water")
                && !lava_tags.contains("minecraft:flowing_water"),
            "water uses unsupported lava callbacks"
        );
        let next_fluid = liquid_state(next)?;
        ensure!(
            next_fluid.is_some_and(|fluid| fluid.kind == native_world::fluid::FluidKind::Water)
                || (next.name == "minecraft:air" && next.properties.is_empty()),
            "unsupported water replacement state"
        );
        let old_air = matches!(
            old.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        );
        ensure!(
            old_air
                || liquid_state(old)?
                    .is_some_and(|fluid| fluid.kind == native_world::fluid::FluidKind::Water),
            "unsupported original water state"
        );
        let next_air = next_fluid.is_none();
        let chunk_pos = ChunkPos {
            x: target.x.div_euclid(16),
            z: target.z.div_euclid(16),
        };
        let section_y =
            i8::try_from(target.y.div_euclid(16)).context("water section out of range")?;
        let blocks = self
            .world
            .chunks
            .get(&chunk_pos)
            .and_then(|chunk| chunk.sections.get(&section_y))
            .and_then(|section| section.blocks.as_ref())
            .context("water section unavailable")?;
        if old_air != next_air {
            let mut nonempty_count = 0;
            for id in blocks.iter() {
                let state = self
                    .world
                    .state(*id)
                    .context("water section state missing")?;
                if !matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ) {
                    nonempty_count += 1;
                    if nonempty_count >= if old_air { 1 } else { 2 } {
                        break;
                    }
                }
            }
            ensure!(
                nonempty_count >= if old_air { 1 } else { 2 },
                "water change requires light section allocation update"
            );
        }
        let directions = [
            [-1, 0, 0],
            [1, 0, 0],
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
        ];
        for d in directions {
            let p = target + glam::IVec3::from_array(d);
            let state = self.read(p.to_array())?;
            ensure!(
                matches!(self.fluid_cell(p), Cell::Air | Cell::Solid | Cell::Fluid(_)),
                "unsupported water neighbor callback"
            );
            if let Some(fluid) = liquid_state(&state)? {
                ensure!(
                    fluid.kind == native_world::fluid::FluidKind::Water,
                    "water touches unsupported liquid"
                );
            }
        }
        self.write_postprocessed_state(target.to_array(), old, next)?;
        if let Some(fluid) = next_fluid {
            self.schedule_water(target.to_array(), next)?;
            if fluid.source {
                self.schedule_water_bubble(target.to_array(), next)?;
            }
        }
        // Redstone neighbor callbacks precede shape updates.
        for d in directions {
            let p = target + glam::IVec3::from_array(d);
            let state = self.read(p.to_array())?;
            if liquid_state(&state)?.is_some() {
                self.schedule_water(p.to_array(), &state)?;
                self.schedule_water_bubble(p.to_array(), &state)?;
            }
        }
        for d in [
            [-1, 0, 0],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let p = target + glam::IVec3::from_array(d);
            let state = self.read(p.to_array())?;
            if let Some(neighbor_fluid) = liquid_state(&state)? {
                if neighbor_fluid.source || next_fluid.is_some_and(|fluid| fluid.source) {
                    self.schedule_water(p.to_array(), &state)?;
                }
                if neighbor_fluid.source {
                    let tags = self.bubble_tags.context("bubble-column tags unavailable")?;
                    if d == [0, 1, 0]
                        && tags.can_occupy_fluid("minecraft:water")
                        && tags.enables(&next.name)
                    {
                        ensure!(
                            self.ticks.schedule(
                                TickRegistry::Block,
                                "minecraft:water",
                                p.to_array(),
                                20,
                                0
                            )? != ScheduleOutcome::MissingChunk,
                            "bubble tick container is missing"
                        );
                    }
                }
            } else if matches!(
                state.name.as_str(),
                "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
            ) {
                ensure!(
                    self.ticks
                        .schedule(TickRegistry::Block, &state.name, p.to_array(), 2, 0)?
                        != ScheduleOutcome::MissingChunk,
                    "falling tick container is missing"
                );
            }
        }
        Ok(())
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
        self.write_postprocessed_state(pos, &old, next)
    }

    fn write_postprocessed_state(
        &mut self,
        pos: [i32; 3],
        old: &BlockState,
        next: &BlockState,
    ) -> Result<()> {
        let same_light = native_world::block_light::same_light_behavior(old, next)
            .context("postprocessing light behavior unavailable")?;
        let world = Arc::make_mut(self.world);
        let id = world.intern_state(next.clone());
        ensure!(
            world
                .set_block_id(pos[0], pos[1], pos[2], id)
                .map_err(|error| anyhow::anyhow!("cannot update generated block: {error:?}"))?,
            "generated block write was unchanged"
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

struct GeneratedWaterTick<'a, 'b> {
    view: &'a mut GeneratedBlockView<'b>,
    origin: glam::IVec3,
}

impl native_world::fluid_sim::LiveFluidWorld for GeneratedWaterTick<'_, '_> {
    type Error = anyhow::Error;

    fn cell(&self, pos: glam::IVec3) -> native_world::fluid_sim::Cell {
        self.view.fluid_cell_for_spread(pos)
    }

    fn delay(
        &mut self,
        _: glam::IVec3,
        old: native_world::fluid::FluidState,
        _: Option<native_world::fluid::FluidState>,
    ) -> Result<u64> {
        ensure!(
            old.kind == native_world::fluid::FluidKind::Water,
            "unsupported fluid delay"
        );
        Ok(5)
    }

    fn update_origin(
        &mut self,
        pos: glam::IVec3,
        next: Option<native_world::fluid::FluidState>,
    ) -> Result<()> {
        ensure!(pos == self.origin, "water origin changed unexpectedly");
        let old = self.view.read(pos.to_array())?;
        ensure!(
            liquid_state(&old)?.is_some_and(|fluid| fluid.kind
                == native_world::fluid::FluidKind::Water
                && !fluid.source),
            "water origin is no longer flowing"
        );
        let replacement = if let Some(fluid) = next {
            ensure!(
                fluid.kind == native_world::fluid::FluidKind::Water
                    && (1..=8).contains(&fluid.amount)
                    && (!fluid.source || (fluid.amount == 8 && !fluid.falling)),
                "invalid recalculated water"
            );
            BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([(
                    "level".into(),
                    (8 - fluid.amount + if fluid.falling { 8 } else { 0 }).to_string(),
                )]),
            }
        } else {
            BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            }
        };
        self.view.apply_water_change(pos, &old, &replacement)
    }

    fn schedule(
        &mut self,
        pos: glam::IVec3,
        fluid: native_world::fluid::FluidState,
        delay: u64,
    ) -> Result<()> {
        ensure!(
            pos == self.origin && delay == 5,
            "unsupported flowing water schedule"
        );
        let current = self.view.read(pos.to_array())?;
        ensure!(
            liquid_state(&current)? == Some(fluid),
            "flowing water changed before scheduling"
        );
        self.view.schedule_water(pos.to_array(), &current)
    }

    fn spread_to(
        &mut self,
        pos: glam::IVec3,
        old: native_world::fluid_sim::Cell,
        fluid: native_world::fluid::FluidState,
        direction: glam::IVec3,
    ) -> Result<()> {
        use native_world::{fluid::FluidKind, fluid_sim::Cell};
        let horizontal = [
            glam::IVec3::NEG_X,
            glam::IVec3::X,
            glam::IVec3::NEG_Z,
            glam::IVec3::Z,
        ]
        .contains(&direction);
        ensure!(
            old == Cell::Air
                && pos == self.origin + direction
                && fluid.kind == FluidKind::Water
                && ((direction == glam::IVec3::NEG_Y
                    && !fluid.source
                    && fluid.falling
                    && fluid.amount == 8)
                    || (horizontal
                        && !fluid.falling
                        && ((fluid.source && fluid.amount == 8)
                            || (!fluid.source && (1..=7).contains(&fluid.amount))))),
            "unsupported generated water spread"
        );
        self.view.spread_water(pos, fluid)
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
        neighbor: &BlockState,
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
            DryBlock::Leaves => {
                static TAG: std::sync::OnceLock<native_resources::packs::ResourceId> =
                    std::sync::OnceLock::new();
                let tag = TAG.get_or_init(|| {
                    native_resources::packs::ResourceId::parse(
                        "minecraft:prevents_nearby_leaf_decay",
                    )
                    .expect("static tag identifier")
                });
                let tagged = self
                    .leaf_tags
                    .context("leaf support tags unavailable")?
                    .contains(tag, &neighbor.name)?;
                // A non-tagged neighbor's leaf distance is at least one;
                // only a tagged neighbor can produce distance zero.
                if !tagged || state.properties["distance"] != "1" {
                    ensure!(
                        self.ticks
                            .schedule(TickRegistry::Block, &state.name, pos, 1, 0)?
                            != ScheduleOutcome::MissingChunk,
                        "leaf tick container is missing"
                    );
                }
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
    type Fluid = native_world::fluid::FluidState;

    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        self.read(pos)
    }
    fn fluid(&mut self, state: &BlockState) -> Result<Option<Self::Fluid>> {
        if let Some(fluid) = liquid_state(state)? {
            return Ok(Some(fluid));
        }
        supported_dry_block(state)?;
        Ok(None)
    }
    fn tick_fluid(&mut self, pos: [i32; 3], _: &BlockState, fluid: &Self::Fluid) -> Result<()> {
        self.tick_generated_fluid(pos, *fluid)
    }
    fn is_liquid_block(&mut self, state: &BlockState) -> Result<bool> {
        Ok(liquid_state(state)?.is_some())
    }
    fn tick_liquid_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        let tags = self.bubble_tags.context("bubble-column tags unavailable")?;
        crate::bubble_column::tick_liquid_block(self, glam::IVec3::from_array(pos), state, tags)
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

impl crate::bubble_column::ColumnWorld for GeneratedBlockView<'_> {
    fn block(&mut self, pos: glam::IVec3) -> Result<BlockState> {
        self.read(pos.to_array())
    }

    fn set_block(&mut self, pos: glam::IVec3, state: &BlockState) -> Result<bool> {
        ensure!(
            self.read(pos.to_array())? == *state,
            "bubble-column update requires block callbacks"
        );
        Ok(false)
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

    fn leaf_case(name: &str, distance: u8, tagged: bool, budget: usize) -> String {
        use native_resources::{
            ore_feature::OreBlockTags,
            packs::{ResourceId, ResourceProvider},
        };
        struct TagSource(bool);
        impl ResourceProvider for TagSource {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/prevents_nearby_leaf_decay.json" {
                    return Ok(Some(serde_json::to_vec(&serde_json::json!({
                        "values": if self.0 { vec!["minecraft:stone"] } else { vec![] }
                    }))?));
                }
                Ok(None)
            }
        }
        let (mut stage, _, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let leaf = world.intern_state(BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::from([
                ("distance".into(), distance.to_string()),
                ("persistent".into(), "false".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        });
        let section = Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&pos).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        );
        section.blocks.as_mut().unwrap().fill(stone);
        world.set_block_id(8, 8, 8, leaf).unwrap();
        stage.source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        marks.mark(8, 8, 8);
        stage.region.tasks = BTreeMap::from([(pos, PostprocessingTask::new(Arc::new(marks), pos))]);
        stage.region.ticks = RegionTicks::new(
            BTreeMap::from([(
                pos,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                    fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                },
            )]),
            100,
            2,
        )
        .unwrap();
        let tag = ResourceId::parse("minecraft:prevents_nearby_leaf_decay").unwrap();
        let registry = BTreeSet::from(["minecraft:stone".to_owned(), format!("minecraft:{name}")]);
        stage.set_leaf_tags(Some(Arc::new(
            OreBlockTags::load(&mut TagSource(tagged), &registry, &BTreeSet::from([tag])).unwrap(),
        )));
        while !stage.poll(pos, budget).unwrap() {}
        let result = stage.finish().unwrap();
        let ticks = result.ticks.chunk(pos).unwrap().blocks.pack(100);
        assert!(result.edits.is_empty());
        assert_eq!(
            result.world.block(8, 8, 8).unwrap().name,
            format!("minecraft:{name}")
        );
        format!(
            "minecraft:{name} {distance} {tagged} {} {} {}",
            ticks.len(),
            ticks.first().map_or(-1, |tick| tick.delay),
            result.ticks.next_sub_tick()
        )
    }

    #[test]
    fn leaf_postprocessing_schedules_once_and_accounts_for_each_attempt() {
        assert_eq!(
            leaf_case("oak_leaves", 1, true, 1),
            "minecraft:oak_leaves 1 true 0 -1 2"
        );
        assert_eq!(
            leaf_case("oak_leaves", 1, false, 1),
            "minecraft:oak_leaves 1 false 1 1 14"
        );
        assert_eq!(
            leaf_case("cherry_leaves", 7, true, 100),
            "minecraft:cherry_leaves 7 true 1 1 14"
        );
    }

    #[test]
    fn leaves_reject_missing_tags_and_unimplemented_waterlogged_ticks() {
        let state = |distance: &str, waterlogged: &str| BlockState {
            name: "minecraft:oak_leaves".into(),
            properties: BTreeMap::from([
                ("distance".into(), distance.into()),
                ("persistent".into(), "true".into()),
                ("waterlogged".into(), waterlogged.into()),
            ]),
        };
        assert!(supported_dry_block(&state("1", "true")).is_err());
        assert!(supported_dry_block(&state("01", "false")).is_err());
        assert!(supported_dry_block(&state("8", "false")).is_err());
        let (mut stage, _, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let leaf = world.intern_state(state("1", "false"));
        world.set_block_id(8, 8, 8, leaf).unwrap();
        let before = Arc::clone(&stage.world);
        assert!(stage.poll(pos, 1).is_err());
        assert!(stage.finish().is_err());
        assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:oak_leaves");
    }

    fn contained_liquid_case(name: &str, level: u8, occupy: bool) -> String {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let liquid = world.intern_state(BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::from([("level".into(), level.to_string())]),
        });
        let section = Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        );
        section.blocks.as_mut().unwrap().fill(stone);
        world.set_block_id(8, 8, 8, liquid).unwrap();
        stage.source = Arc::clone(&stage.world);
        let source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        stage.region.tasks =
            BTreeMap::from([(chunk, PostprocessingTask::new(Arc::new(marks), chunk))]);
        stage.set_bubble_tags(Some(Arc::new(
            native_resources::fluid_tags::BubbleColumnTags::with_fluids(
                BTreeSet::new(),
                BTreeSet::new(),
                if occupy {
                    BTreeSet::from([format!("minecraft:{name}")])
                } else {
                    BTreeSet::new()
                },
            ),
        )));
        while !stage.poll(chunk, 1).unwrap() {}
        let result = stage.finish().unwrap();
        assert!(Arc::ptr_eq(&source, &result.world));
        assert!(result.edits.is_empty());
        format!(
            "minecraft:{name} {level} {occupy} {}",
            result.world.block(8, 8, 8).unwrap().name
        )
    }

    #[test]
    fn enclosed_liquid_ticks_preserve_world_and_reject_flow_or_bubbles() {
        assert_eq!(
            contained_liquid_case("water", 0, true),
            "minecraft:water 0 true minecraft:water"
        );
        assert_eq!(
            contained_liquid_case("lava", 0, false),
            "minecraft:lava 0 false minecraft:lava"
        );
        let (mut stage, source, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        world.set_block_id(8, 8, 8, water).unwrap();
        world.set_block_id(8, 7, 8, air).unwrap();
        let before = Arc::clone(&stage.world);
        assert!(stage.poll(pos, 1).is_err());
        assert!(stage.finish().is_err());
        assert_eq!(before.block(8, 7, 8).unwrap().name, "minecraft:air");
        assert_eq!(source.block(8, 8, 8).unwrap().name, "minecraft:sand");

        let (mut stage, _, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&pos).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        world.set_block_id(8, 8, 8, water).unwrap();
        let before = Arc::clone(&stage.world);
        stage.set_bubble_tags(Some(Arc::new(
            native_resources::fluid_tags::BubbleColumnTags::new(
                BTreeSet::from(["minecraft:stone".into()]),
                BTreeSet::new(),
            ),
        )));
        assert!(stage.poll(pos, 1).is_err());
        assert!(stage.finish().is_err());
        assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:water");
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn contained_liquid_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedContainedLiquidPostprocessingOracle",
            include_str!("../../tests/java/GeneratedContainedLiquidPostprocessingOracle.java"),
        );
        let mut expected = Vec::new();
        for liquid in ["water", "lava"] {
            for occupy in [false, true] {
                for fast in [false, true] {
                    expected.push(format!(
                        "{} {fast}",
                        contained_liquid_case(liquid, 0, occupy)
                    ));
                }
            }
        }
        assert_eq!(actual, expected);
    }

    fn water_spread_trace(case: &str) -> Vec<String> {
        use native_resources::{heightmap_tags::HeightmapTags, packs::ResourceProvider};
        struct Tags;
        impl ResourceProvider for Tags {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path.ends_with("blocks_motion_in_heightmap.json")
                    || path.ends_with("blocks_motion_in_heightmap_no_leaves.json")
                {
                    return Ok(Some(
                        br#"{"values":["minecraft:stone","minecraft:sand"]}"#.to_vec(),
                    ));
                }
                Ok(None)
            }
        }
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let sand = world.intern_state(BlockState {
            name: "minecraft:sand".into(),
            properties: BTreeMap::new(),
        });
        let dirt = world.intern_state(BlockState {
            name: "minecraft:dirt".into(),
            properties: BTreeMap::new(),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        let origin_x = if case == "edge" { 15 } else { 8 };
        let east_chunk = ChunkPos { x: 1, z: 0 };
        if case == "edge" {
            let mut east = world.chunks[&chunk].as_ref().clone();
            east.position = east_chunk;
            world.chunks.insert(east_chunk, Arc::new(east));
        }
        world.set_block_id(origin_x, 8, 8, water).unwrap();
        match case {
            "down" | "down_sand" => {
                world.set_block_id(8, 7, 8, air).unwrap();
                if case == "down_sand" {
                    world.set_block_id(9, 7, 8, sand).unwrap();
                }
            }
            "sides" => {
                for [x, y, z] in [[8, 8, 7], [8, 8, 9], [7, 8, 8], [9, 8, 8]] {
                    world.set_block_id(x, y, z, air).unwrap();
                }
            }
            "mixed" => {
                world.set_block_id(8, 7, 8, air).unwrap();
                for [x, y, z] in [[8, 8, 7], [8, 8, 9], [7, 8, 8]] {
                    world.set_block_id(x, y, z, water).unwrap();
                }
                world.set_block_id(9, 8, 8, air).unwrap();
            }
            "edge" => {
                world.set_block_id(16, 8, 8, air).unwrap();
            }
            "convert_on" | "convert_off" | "convert_bubble" => {
                world.set_block_id(9, 8, 8, air).unwrap();
                world.set_block_id(9, 8, 7, water).unwrap();
                if case == "convert_bubble" {
                    world.set_block_id(9, 7, 8, dirt).unwrap();
                }
            }
            "flow_drain" | "flow_drain_sand" => {
                let flowing = world.intern_state(BlockState {
                    name: "minecraft:water".into(),
                    properties: BTreeMap::from([("level".into(), "1".into())]),
                });
                world.set_block_id(8, 8, 8, flowing).unwrap();
                if case == "flow_drain_sand" {
                    world.set_block_id(9, 8, 8, sand).unwrap();
                }
            }
            "flow_weaken" | "flow_falling" | "flow_down" | "flow_sides" | "origin_source_on"
            | "origin_source_off" => {
                let flowing = world.intern_state(BlockState {
                    name: "minecraft:water".into(),
                    properties: BTreeMap::from([("level".into(), "4".into())]),
                });
                world.set_block_id(8, 8, 8, flowing).unwrap();
                if case == "flow_falling" {
                    world.set_block_id(8, 9, 8, water).unwrap();
                } else {
                    world.set_block_id(7, 8, 8, water).unwrap();
                    if case == "flow_down" {
                        world.set_block_id(8, 7, 8, air).unwrap();
                    }
                    if case == "flow_sides" {
                        for [x, y, z] in [[8, 8, 7], [8, 8, 9], [9, 8, 8]] {
                            world.set_block_id(x, y, z, air).unwrap();
                        }
                    }
                    if case.starts_with("origin_source_") {
                        world.set_block_id(8, 8, 7, water).unwrap();
                    }
                }
            }
            _ => panic!("unknown water fixture"),
        }
        stage.source = Arc::clone(&stage.world);
        let source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(origin_x, 8, 8);
        stage.region.tasks =
            BTreeMap::from([(chunk, PostprocessingTask::new(Arc::new(marks), chunk))]);
        let mut chunks = BTreeMap::from([(
            chunk,
            ConvertedChunkTicks {
                blocks: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                fluids: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
            },
        )]);
        if case == "edge" {
            let empty = PostProcessing::new(0, 16).unwrap();
            stage.region.tasks.insert(
                east_chunk,
                PostprocessingTask::new(Arc::new(empty), east_chunk),
            );
            chunks.insert(
                east_chunk,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(east_chunk, vec![]).unwrap(),
                    fluids: ChunkTicks::from_saved(east_chunk, vec![]).unwrap(),
                },
            );
        }
        stage.region.ticks = RegionTicks::new(chunks, 100, 2).unwrap();
        let registry = ["air", "water", "stone", "sand", "dirt"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        stage.heightmap_tags = Some(Arc::new(HeightmapTags::load(&mut Tags, &registry).unwrap()));
        stage.heightmaps.insert(chunk, fixture_maps(16));
        if case == "edge" {
            stage.heightmaps.insert(east_chunk, fixture_maps(16));
        }
        stage.set_bubble_tags(Some(Arc::new(
            native_resources::fluid_tags::BubbleColumnTags::new(
                if case == "convert_bubble" {
                    BTreeSet::from(["minecraft:dirt".into()])
                } else {
                    BTreeSet::new()
                },
                BTreeSet::new(),
            ),
        )));
        stage.set_fluid_tags(
            Some(Arc::new(BTreeSet::from([
                "minecraft:air".into(),
                "minecraft:water".into(),
            ]))),
            Some(Arc::new(BTreeSet::from([
                "minecraft:lava".into(),
                "minecraft:flowing_lava".into(),
            ]))),
        );
        if matches!(
            case,
            "convert_on"
                | "convert_off"
                | "convert_bubble"
                | "origin_source_on"
                | "origin_source_off"
        ) {
            stage
                .set_water_source_conversion(!matches!(case, "convert_off" | "origin_source_off"))
                .unwrap();
        }
        while !stage.poll(chunk, 1).unwrap() {}
        if case == "edge" {
            while !stage.poll(east_chunk, 1).unwrap() {}
        }
        let result = stage.finish().unwrap();
        assert_eq!(
            result.water_source_conversion(),
            match case {
                "convert_off" | "origin_source_off" => Some(false),
                "convert_on" | "convert_bubble" | "origin_source_on" => Some(true),
                _ => None,
            }
        );
        let expected_checks: Vec<_> = result
            .edits
            .iter()
            .copied()
            .filter(|p| {
                let old = source.block(p.x, p.y, p.z).unwrap();
                let new = result.world.block(p.x, p.y, p.z).unwrap();
                !native_world::block_light::same_light_behavior(old, new).unwrap()
            })
            .collect();
        assert_eq!(result.light_checks, expected_checks);
        let mut trace = vec![format!("case {case}")];
        for [x, y, z] in [
            [origin_x, 8, 8],
            [origin_x, 7, 8],
            [origin_x, 8, 7],
            [origin_x, 8, 9],
            [origin_x - 1, 8, 8],
            [origin_x + 1, 8, 8],
        ] {
            let state = result.world.block(x, y, z).unwrap();
            trace.push(format!(
                "state {x},{y},{z} {} {}",
                state.name,
                state.properties.get("level").map_or("-", String::as_str)
            ));
        }
        for change in &result.edits {
            assert_ne!(
                source.block(change.x, change.y, change.z),
                result.world.block(change.x, change.y, change.z)
            );
        }
        for kind in [TickRegistry::Block, TickRegistry::Fluid] {
            for tick_chunk in if case == "edge" {
                vec![chunk, east_chunk]
            } else {
                vec![chunk]
            } {
                let ticks = result.ticks.chunk(tick_chunk).unwrap();
                let (label, saved) = match kind {
                    TickRegistry::Block => ("block", ticks.blocks.pack(100)),
                    TickRegistry::Fluid => ("fluid", ticks.fluids.pack(100)),
                };
                for tick in saved {
                    trace.push(format!(
                        "{label} {} {},{},{} {}",
                        tick.kind, tick.position[0], tick.position[1], tick.position[2], tick.delay
                    ));
                }
            }
        }
        trace.push(format!("counter {}", result.ticks.next_sub_tick()));
        trace
    }

    #[test]
    fn downward_water_spread_updates_world_ticks_and_light_checks() {
        let plain = water_spread_trace("down");
        assert!(
            plain
                .iter()
                .any(|line| line == "fluid minecraft:flowing_water 8,7,8 5")
        );
        assert!(
            plain
                .iter()
                .any(|line| line == "fluid minecraft:water 8,8,8 5")
        );
        let sand = water_spread_trace("down_sand");
        assert!(
            sand.iter()
                .any(|line| line == "block minecraft:sand 9,7,8 2")
        );
    }

    #[test]
    fn water_spreads_to_sides_and_after_a_downward_write() {
        let sides = water_spread_trace("sides");
        for p in ["8,8,7", "8,8,9", "7,8,8", "9,8,8"] {
            assert!(
                sides
                    .iter()
                    .any(|line| line == &format!("state {p} minecraft:water 1"))
            );
        }
        let mixed = water_spread_trace("mixed");
        assert!(
            mixed
                .iter()
                .any(|line| line == "state 8,7,8 minecraft:water 8")
        );
        assert!(
            mixed
                .iter()
                .any(|line| line == "state 9,8,8 minecraft:water 1")
        );
        let edge = water_spread_trace("edge");
        assert!(
            edge.iter()
                .any(|line| line == "state 16,8,8 minecraft:water 1")
        );
        assert!(
            edge.iter()
                .any(|line| line == "fluid minecraft:flowing_water 16,8,8 5")
        );
        let source = water_spread_trace("convert_on");
        assert!(
            source
                .iter()
                .any(|line| line == "state 9,8,8 minecraft:water 0")
        );
        assert!(
            source
                .iter()
                .any(|line| line == "fluid minecraft:water 9,8,8 5")
        );
        let flow = water_spread_trace("convert_off");
        assert!(
            flow.iter()
                .any(|line| line == "state 9,8,8 minecraft:water 1")
        );
        assert!(
            flow.iter()
                .any(|line| line == "fluid minecraft:flowing_water 9,8,8 5")
        );
        let bubble = water_spread_trace("convert_bubble");
        assert!(
            bubble
                .iter()
                .any(|line| line == "block minecraft:water 9,8,8 20")
        );
    }

    #[test]
    fn water_spread_rejects_unverified_datapack_replacement_tags() {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        world.set_block_id(8, 8, 8, water).unwrap();
        world.set_block_id(8, 7, 8, air).unwrap();
        let original = Arc::clone(&stage.world);
        stage.set_fluid_tags(
            Some(Arc::new(BTreeSet::from(["minecraft:air".into()]))),
            Some(Arc::new(BTreeSet::from(["minecraft:lava".into()]))),
        );
        let error = stage.poll(chunk, 1).unwrap_err();
        assert!(format!("{error:#}").contains("generated water path is incomplete"));
        assert!(stage.finish().is_err());
        assert_eq!(original.block(8, 7, 8).unwrap().name, "minecraft:air");
    }

    #[test]
    fn water_spread_rejects_unknown_source_conversion_rule() {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        world.set_block_id(8, 8, 8, water).unwrap();
        world.set_block_id(9, 8, 8, air).unwrap();
        world.set_block_id(9, 8, 7, water).unwrap();
        let original = Arc::clone(&stage.world);
        let error = stage.poll(chunk, 1).unwrap_err();
        assert!(format!("{error:#}").contains("depends on unavailable settings"));
        assert!(stage.finish().is_err());
        assert_eq!(original.block(9, 8, 8).unwrap().name, "minecraft:air");
    }

    #[test]
    fn source_conversion_rule_is_fixed_before_chunk_processing() {
        let (mut stage, _, pos) = fixture(false);
        stage.set_water_source_conversion(false).unwrap();
        assert!(!stage.poll(pos, 0).unwrap());
        assert!(stage.set_water_source_conversion(true).is_err());
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn water_source_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedWaterSpreadPostprocessingOracle",
            include_str!("../../tests/java/GeneratedWaterSpreadPostprocessingOracle.java"),
        );
        let mut expected = Vec::new();
        for case in [
            "down",
            "down_sand",
            "sides",
            "mixed",
            "edge",
            "convert_off",
            "convert_on",
            "convert_bubble",
            "flow_drain",
            "flow_drain_sand",
            "flow_weaken",
            "flow_falling",
            "flow_down",
            "flow_sides",
            "origin_source_off",
            "origin_source_on",
        ] {
            expected.extend(water_spread_trace(case));
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn flowing_water_recalculates_origin_and_schedules_callbacks() {
        let drained = water_spread_trace("flow_drain");
        assert!(
            drained
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:air -")
        );
        let sand = water_spread_trace("flow_drain_sand");
        assert!(
            sand.iter()
                .any(|line| line == "block minecraft:sand 9,8,8 2")
        );
        let weaker = water_spread_trace("flow_weaken");
        assert!(
            weaker
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:water 1")
        );
        let falling = water_spread_trace("flow_falling");
        assert!(
            falling
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:water 8")
        );
        let down = water_spread_trace("flow_down");
        assert!(
            down.iter()
                .any(|line| line == "state 8,7,8 minecraft:water 8")
        );
        let sides = water_spread_trace("flow_sides");
        for pos in ["8,8,7", "8,8,9", "9,8,8"] {
            assert!(
                sides
                    .iter()
                    .any(|line| line == &format!("state {pos} minecraft:water 2"))
            );
        }
        let source = water_spread_trace("origin_source_on");
        assert!(
            source
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:water 0")
        );
        let flowing = water_spread_trace("origin_source_off");
        assert!(
            flowing
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:water 1")
        );
    }

    #[test]
    fn flowing_water_rejects_unavailable_rules_and_section_deallocation() {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let source = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let flowing = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "4".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        world.set_block_id(8, 8, 8, flowing).unwrap();
        world.set_block_id(7, 8, 8, source).unwrap();
        world.set_block_id(8, 8, 7, source).unwrap();
        let original = Arc::clone(&stage.world);
        let error = stage.poll(chunk, 1).unwrap_err();
        assert!(format!("{error:#}").contains("depends on unavailable settings"));
        assert!(stage.finish().is_err());
        assert_eq!(original.block(8, 8, 8).unwrap().properties["level"], "4");

        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let flowing = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "1".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(air);
        world.set_block_id(8, 8, 8, flowing).unwrap();
        let original = Arc::clone(&stage.world);
        stage.set_fluid_tags(
            Some(Arc::new(BTreeSet::from([
                "minecraft:air".into(),
                "minecraft:water".into(),
            ]))),
            Some(Arc::new(BTreeSet::from(["minecraft:lava".into()]))),
        );
        let error = stage.poll(chunk, 1).unwrap_err();
        assert!(format!("{error:#}").contains("light section allocation update"));
        assert!(stage.finish().is_err());
        assert_eq!(original.block(8, 8, 8).unwrap().properties["level"], "1");
    }

    #[test]
    fn water_edits_refresh_direct_sky_sources_without_touching_the_input() {
        use native_world::{block_light::LightBounds, sky_light::generated_chunk_sky_sources};
        let (mut stage, _, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "8".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&pos).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(air);
        let before = Arc::clone(&stage.world);
        let bounds = LightBounds::new(0, 16).unwrap();
        let original_heights =
            Arc::from(generated_chunk_sky_sources(&before, bounds, pos).unwrap());
        let original = BTreeMap::from([(pos, original_heights)]);
        Arc::make_mut(&mut stage.world)
            .set_block_id(8, 7, 8, water)
            .unwrap();
        let checks = [BlockPosition { x: 8, y: 7, z: 8 }];
        let updated = super::super::terrain_batch::updated_postprocessing_sky_sources(
            &before,
            &stage.world,
            &checks,
            bounds,
            &original,
        )
        .unwrap()
        .unwrap();
        let expected = generated_chunk_sky_sources(&stage.world, bounds, pos).unwrap();
        assert_eq!(updated[&pos].as_ref(), expected.as_ref());
        assert_ne!(updated[&pos][8 + 8 * 16], original[&pos][8 + 8 * 16]);
        assert!(
            super::super::terrain_batch::updated_postprocessing_sky_sources(
                &before,
                &stage.world,
                &checks,
                bounds,
                &BTreeMap::new(),
            )
            .is_err()
        );
        assert_eq!(before.block(8, 7, 8).unwrap().name, "minecraft:air");
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_leaf_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedLeafPostprocessingOracle",
            include_str!("../../tests/java/GeneratedLeafPostprocessingOracle.java"),
        );
        let mut expected = Vec::new();
        for name in [
            "oak_leaves",
            "spruce_leaves",
            "birch_leaves",
            "jungle_leaves",
            "acacia_leaves",
            "dark_oak_leaves",
            "mangrove_leaves",
            "cherry_leaves",
            "pale_oak_leaves",
            "azalea_leaves",
            "flowering_azalea_leaves",
        ] {
            for distance in [1, 4, 7] {
                for tagged in [false, true] {
                    expected.push(leaf_case(name, distance, tagged, 1));
                }
            }
        }
        assert_eq!(actual, expected);
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
