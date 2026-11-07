//! Ordered placement of a single jigsaw template into an isolated draft.
use super::*;
use native_core::{beardifier::BoundingBox, random::DecorationRandom};
use native_persistence::{jigsaw_template::ProcessedTemplateBlock, nbt::Tag};
use native_resources::structure_transform::Mirror;
use native_world::{fluid::FluidState, loaded::BlockState};

/// Implementations must use a discardable world draft. A failure may occur
/// after writes; the caller must discard that draft instead of publishing it.
pub trait JigsawPlacementWorld {
    fn block(&mut self, position: [i32; 3]) -> Result<BlockState>;
    /// First available block above WORLD_SURFACE_WG for the generation region.
    fn world_surface_height(&mut self, _x: i32, _z: i32) -> Result<i32> {
        anyhow::bail!("terrain-matching jigsaw heightmap is missing")
    }
    fn fluid_at(&mut self, position: [i32; 3]) -> Result<Option<FluidState>>;
    fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>>;
    /// Apply Java's NBT barrier and flag-18 block write; load the block entity
    /// and draw its loot seed when required. Return false for a rejected write.
    fn place_template_block(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool>;
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool>;
    fn place_liquid(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
        fluid: Option<FluidState>,
    ) -> Result<()>;
    /// Mark an existing block entity changed; do nothing if there is none.
    fn mark_block_entity_changed(&mut self, position: [i32; 3]) -> Result<()>;
    /// Spawn and finalize template entities according to the clip and rotation.
    fn place_template_entities(
        &mut self,
        entities: &[Tag],
        origin: [i32; 3],
        rotation: native_resources::structure_transform::Rotation,
        clip: BoundingBox,
    ) -> Result<()>;
}

fn inside(clip: BoundingBox, pos: [i32; 3]) -> bool {
    (0..3).all(|i| (clip.min[i]..=clip.max[i]).contains(&pos[i]))
}
fn source(fluid: Option<FluidState>) -> bool {
    fluid.is_some_and(|value| value.source)
}

fn place_records_inner<W: JigsawPlacementWorld>(
    records: &[ProcessedTemplateBlock],
    world: &mut W,
    clip: BoundingBox,
    apply_waterlogging: bool,
    random: &mut DecorationRandom,
    mut rotate: impl FnMut(&BlockState) -> Result<BlockState>,
    mut place: impl FnMut(
        &mut W,
        [i32; 3],
        &BlockState,
        Option<&Tag>,
        &mut DecorationRandom,
    ) -> Result<bool>,
) -> Result<Vec<([i32; 3], bool)>> {
    let mut unresolved = Vec::new();
    let mut template_sources = BTreeSet::new();
    let mut placed = Vec::new();
    for record in records {
        let block = &record.block;
        if !inside(clip, block.position) {
            continue;
        }
        let previous = if apply_waterlogging {
            world.fluid_at(block.position)?
        } else {
            None
        };
        let state = rotate(&block.state)?;
        if !place(world, block.position, &state, block.nbt.as_ref(), random)? {
            continue;
        }
        placed.push((block.position, block.nbt.is_some()));
        if apply_waterlogging {
            if source(world.fluid_of_state(&state)?) {
                template_sources.insert(block.position);
            } else if world.is_liquid_container(&state)? {
                world.place_liquid(block.position, &state, previous)?;
                if !source(previous) {
                    unresolved.push(block.position);
                }
            }
        }
    }
    let directions = [[0, 1, 0], [0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]];
    let mut changed = true;
    while changed && !unresolved.is_empty() {
        changed = false;
        let mut remaining = Vec::new();
        for pos in unresolved {
            let mut fluid = world.fluid_at(pos)?;
            for delta in directions {
                if source(fluid) {
                    break;
                }
                let neighbor = std::array::from_fn(|i| pos[i].wrapping_add(delta[i]));
                let candidate = world.fluid_at(neighbor)?;
                if source(candidate) && !template_sources.contains(&neighbor) {
                    fluid = candidate;
                }
            }
            if source(fluid) {
                let state = world.block(pos)?;
                if world.is_liquid_container(&state)? {
                    world.place_liquid(pos, &state, fluid)?;
                    changed = true;
                    continue;
                }
            }
            remaining.push(pos);
        }
        unresolved = remaining;
    }
    Ok(placed)
}

fn place_records<W: JigsawPlacementWorld>(
    records: &[ProcessedTemplateBlock],
    world: &mut W,
    clip: BoundingBox,
    apply_waterlogging: bool,
    random: &mut DecorationRandom,
    rotate: impl FnMut(&BlockState) -> Result<BlockState>,
) -> Result<()> {
    let placed = place_records_inner(
        records,
        world,
        clip,
        apply_waterlogging,
        random,
        rotate,
        |world, pos, state, nbt, random| world.place_template_block(pos, state, nbt, random),
    )?;
    // SinglePoolElement sets knownShape and skips the shape passes.
    for (pos, has_nbt) in placed {
        if has_nbt {
            world.mark_block_entity_changed(pos)?;
        }
    }
    Ok(())
}

/// Additional ordinary-template operations omitted by known-shape jigsaws.
pub trait StructurePlacementWorld:
    JigsawPlacementWorld + native_resources::tree_leaf_update::ShapeEdgeWorld<State = BlockState>
{
    /// Place with Java's flag 2, including NBT loading and loot-seed draws.
    fn place_structure_block(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool>;
    /// Java Block.updateFromNeighbourShapes, including scheduled effects.
    fn update_from_neighbor_shapes(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
    ) -> Result<BlockState>;
    fn update_neighbors_at(&mut self, position: [i32; 3], state: &BlockState) -> Result<()>;
    /// Apply a TemplateStructurePiece jigsaw final state with Java's flag 3.
    fn finalize_jigsaw_marker(&mut self, _position: [i32; 3], _state: &BlockState) -> Result<()> {
        anyhow::bail!("ordinary-template jigsaw finalization is unsupported")
    }
    /// Refuse publication when a temporary block entity could not be removed.
    fn finish_jigsaw_markers(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Place already-processed ordinary template records; a caller must discard
/// its world draft on error. Entity and structure marker handling is separate.
pub fn place_structure_records<W: StructurePlacementWorld>(
    records: &[ProcessedTemplateBlock],
    world: &mut W,
    registry: &native_resources::block_registry::GeneratedBlockRegistry,
    clip: BoundingBox,
    mirror: Mirror,
    rotation: native_resources::structure_transform::Rotation,
    random: &mut DecorationRandom,
) -> Result<()> {
    use native_resources::tree_leaf_update::{ShapeEdgeWorld, TreeBounds, update_shape_at_edges};
    let mut next = random.clone();
    let placed = place_records_inner(
        records,
        world,
        clip,
        true,
        &mut next,
        |state| {
            let properties = registry.transform_template_state(
                &state.name,
                &state.properties,
                mirror,
                rotation,
            )?;
            Ok(BlockState {
                name: state.name.clone(),
                properties,
            })
        },
        |world, pos, state, nbt, random| world.place_structure_block(pos, state, nbt, random),
    )?;
    if !placed.is_empty() {
        let mut min = placed[0].0;
        let mut max = min;
        let mut shape = BTreeSet::new();
        for (pos, _) in &placed {
            for axis in 0..3 {
                min[axis] = min[axis].min(pos[axis]);
                max[axis] = max[axis].max(pos[axis]);
            }
            shape.insert(*pos);
        }
        update_shape_at_edges(world, TreeBounds { min, max }, &shape, 2)?;
        for (pos, has_nbt) in placed {
            let old = ShapeEdgeWorld::block(world, pos)?;
            let updated = world.update_from_neighbor_shapes(pos, &old)?;
            if updated != old {
                ShapeEdgeWorld::set_block(world, pos, &updated, 18)?;
            }
            world.update_neighbors_at(pos, &updated)?;
            if has_nbt {
                world.mark_block_entity_changed(pos)?;
            }
        }
    }
    *random = next;
    Ok(())
}

impl PreparedTemplatePiece {
    /// Process and place a single element. The decoration RNG commits
    /// only on success; the caller must discard its unpublished world draft on
    /// every error, including failures in entity or marker processing.
    pub fn place(
        &self,
        world: &mut impl JigsawPlacementWorld,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        clip: BoundingBox,
        inherited_liquids: LiquidSettings,
        is_static_full: impl FnMut(&BlockState) -> Result<bool>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(
            (0..3).all(|i| clip.min[i] <= clip.max[i]),
            "invalid template clipping box"
        );
        if self.template.size.iter().any(|size| *size < 1)
            || self.template.blocks.is_empty() && self.template.entities.is_empty()
        {
            return Ok(false);
        }
        let mut next_random = random.clone();
        let shared = std::cell::RefCell::new(world);
        let records = self.process_blocks_with_height(
            registry,
            clip,
            is_static_full,
            &mut |position| shared.borrow_mut().block(position),
            &mut |x, z| shared.borrow_mut().world_surface_height(x, z),
        )?;
        let world = shared.into_inner();
        let liquids = self.override_liquid_settings.unwrap_or(inherited_liquids);
        place_records(
            &records,
            world,
            clip,
            liquids == LiquidSettings::ApplyWaterlogging,
            &mut next_random,
            |state| {
                let properties = registry.transform_template_state(
                    &state.name,
                    &state.properties,
                    Mirror::None,
                    self.rotation,
                )?;
                Ok(BlockState {
                    name: state.name.clone(),
                    properties,
                })
            },
        )?;
        if !self.template.entities.is_empty() {
            world.place_template_entities(
                &self.template.entities,
                self.origin,
                self.rotation,
                clip,
            )?;
        }
        // SinglePoolElement's marker callback is a no-op, but Java still parses
        // marker mode after placement and fails on an invalid value.
        self.validate_marker_modes()?;
        *random = next_random;
        Ok(true)
    }

    fn validate_marker_modes(&self) -> Result<()> {
        let [x, y, z] = self.origin;
        let count = i32::try_from(self.template.palettes.len()).context("too many palettes")?;
        let index = native_core::random::LegacyRandom::new(
            native_resources::blockstates::positional_seed(x, y, z),
        )
        .next_int_bound(count)
        .context("template has no palettes")? as usize;
        let palette = &self.template.palettes[index];
        for block in &self.template.blocks {
            if !matches!(palette[block.state].get("id"), Some(Tag::String(id)) if id.matches("minecraft:structure_block") || id.matches("structure_block"))
            {
                continue;
            }
            let Some(nbt) = &block.nbt else { continue };
            let Some(Tag::String(mode)) = nbt.get("mode") else {
                anyhow::bail!("structure marker mode missing or invalid")
            };
            ensure!(
                ["SAVE", "LOAD", "CORNER", "DATA"]
                    .iter()
                    .any(|value| mode.matches(value)),
                "invalid structure marker mode"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::jigsaw_replacement::PlacementBlock;
    use std::collections::BTreeMap;

    fn state(name: &str) -> BlockState {
        let properties = if name == "oak_stairs" {
            BTreeMap::from([("waterlogged".into(), "false".into())])
        } else {
            BTreeMap::new()
        };
        BlockState {
            name: format!("minecraft:{name}"),
            properties,
        }
    }
    fn record(pos: [i32; 3], name: &str, nbt: Option<Tag>) -> ProcessedTemplateBlock {
        ProcessedTemplateBlock {
            source_index: 0,
            block: PlacementBlock {
                position: pos,
                state: state(name),
                nbt,
            },
        }
    }
    #[derive(Default)]
    struct World {
        blocks: BTreeMap<[i32; 3], BlockState>,
        events: Vec<(&'static str, [i32; 3])>,
        fluid_reads: usize,
    }
    impl JigsawPlacementWorld for World {
        fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
            Ok(self
                .blocks
                .get(&pos)
                .cloned()
                .unwrap_or_else(|| state("air")))
        }
        fn fluid_at(&mut self, pos: [i32; 3]) -> Result<Option<FluidState>> {
            self.fluid_reads += 1;
            FluidState::from_block(&self.block(pos)?).map_err(anyhow::Error::msg)
        }
        fn fluid_of_state(&mut self, value: &BlockState) -> Result<Option<FluidState>> {
            FluidState::from_block(value).map_err(anyhow::Error::msg)
        }
        fn place_template_block(
            &mut self,
            pos: [i32; 3],
            value: &BlockState,
            _: Option<&Tag>,
            _: &mut DecorationRandom,
        ) -> Result<bool> {
            self.events.push(("write", pos));
            self.blocks.insert(pos, value.clone());
            Ok(true)
        }
        fn is_liquid_container(&mut self, value: &BlockState) -> Result<bool> {
            Ok(value.name == "minecraft:oak_stairs")
        }
        fn place_liquid(
            &mut self,
            pos: [i32; 3],
            value: &BlockState,
            fluid: Option<FluidState>,
        ) -> Result<()> {
            self.events.push(("liquid", pos));
            if source(fluid) {
                let mut wet = value.clone();
                wet.properties.insert("waterlogged".into(), "true".into());
                self.blocks.insert(pos, wet);
            }
            Ok(())
        }
        fn mark_block_entity_changed(&mut self, pos: [i32; 3]) -> Result<()> {
            self.events.push(("mark", pos));
            Ok(())
        }
        fn place_template_entities(
            &mut self,
            _: &[Tag],
            _: [i32; 3],
            _: native_resources::structure_transform::Rotation,
            _: BoundingBox,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn restores_external_fluid_in_passes_without_using_template_sources() -> Result<()> {
        let mut world = World::default();
        world.blocks.insert([3, 0, 0], state("water"));
        let blocks = vec![
            record([0, 0, 0], "water", None),
            record([1, 0, 0], "oak_stairs", None),
            record([2, 0, 0], "oak_stairs", None),
        ];
        let clip = BoundingBox::new([-10; 3], [10; 3]).map_err(anyhow::Error::msg)?;
        place_records(
            &blocks,
            &mut world,
            clip,
            true,
            &mut DecorationRandom::new(0),
            |v| Ok(v.clone()),
        )?;
        assert_eq!(world.blocks[&[1, 0, 0]].properties["waterlogged"], "true");
        assert_eq!(world.blocks[&[2, 0, 0]].properties["waterlogged"], "true");
        assert_eq!(
            world.events,
            vec![
                ("write", [0, 0, 0]),
                ("write", [1, 0, 0]),
                ("liquid", [1, 0, 0]),
                ("write", [2, 0, 0]),
                ("liquid", [2, 0, 0]),
                ("liquid", [2, 0, 0]),
                ("liquid", [1, 0, 0])
            ]
        );
        Ok(())
    }

    #[test]
    fn ignored_liquids_and_clipping_preserve_order_and_mark_nbt_last() -> Result<()> {
        let mut world = World::default();
        let blocks = vec![
            record([0, 0, 0], "stone", Some(Tag::Compound(vec![]))),
            record([1, 0, 0], "oak_stairs", None),
            record([50, 0, 0], "dirt", None),
        ];
        let clip = BoundingBox::new([-10; 3], [10; 3]).map_err(anyhow::Error::msg)?;
        place_records(
            &blocks,
            &mut world,
            clip,
            false,
            &mut DecorationRandom::new(1),
            |v| Ok(v.clone()),
        )?;
        assert_eq!(
            world.events,
            vec![
                ("write", [0, 0, 0]),
                ("write", [1, 0, 0]),
                ("mark", [0, 0, 0])
            ]
        );
        assert_eq!(world.fluid_reads, 0);
        Ok(())
    }
}
