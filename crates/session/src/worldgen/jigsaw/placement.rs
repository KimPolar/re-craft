//! Ordered placement of a rigid single jigsaw template into an isolated draft.
use super::*;
use native_core::{beardifier::BoundingBox, random::DecorationRandom};
use native_persistence::{jigsaw_template::ProcessedTemplateBlock, nbt::Tag};
use native_resources::structure_transform::Mirror;
use native_world::{fluid::FluidState, loaded::BlockState};

/// Implementations must use a discardable world draft. A failure may occur
/// after writes; the caller must discard that draft instead of publishing it.
pub trait JigsawPlacementWorld {
    fn block(&mut self, position: [i32; 3]) -> Result<BlockState>;
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

fn place_records(
    records: &[ProcessedTemplateBlock],
    world: &mut impl JigsawPlacementWorld,
    clip: BoundingBox,
    apply_waterlogging: bool,
    random: &mut DecorationRandom,
    mut rotate: impl FnMut(&BlockState) -> Result<BlockState>,
) -> Result<()> {
    let mut unresolved = Vec::new();
    let mut template_sources = BTreeSet::new();
    let mut placed_with_nbt = Vec::new();
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
        if !world.place_template_block(block.position, &state, block.nbt.as_ref(), random)? {
            continue;
        }
        if block.nbt.is_some() {
            placed_with_nbt.push(block.position);
        }
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
    // SinglePoolElement sets knownShape, so it skips neighbor shape updates.
    for pos in placed_with_nbt {
        world.mark_block_entity_changed(pos)?;
    }
    Ok(())
}

impl PreparedTemplatePiece {
    /// Process and place a rigid single element. The decoration RNG commits
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
        let records = self.process_blocks(registry, clip, is_static_full, &mut |position| {
            world.block(position)
        })?;
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
