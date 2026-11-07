//! Place a shipwreck into an isolated generation draft.
use super::*;
use native_core::random::{DecorationRandom, LegacyRandom};
use native_persistence::{
    jigsaw_template::{
        ProcessedTemplateBlock, StructureTemplateSettings, process_structure_template_with_palette,
        resolve_template_state,
    },
    nbt::{JavaString, Tag},
    structure_processors::PreparedProcessors,
};
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    blockstates::positional_seed,
    shipwreck::{HeightWorld, MarkerWorld},
    structure_transform::{Mirror, Rotation},
};
use native_world::loaded::BlockState;

use crate::worldgen::jigsaw::{
    JigsawPlacementWorld, StructurePlacementWorld, place_structure_records,
};

/// All operations must target an unpublished draft; discard it on any error.
pub trait ShipwreckPlacementWorld: StructurePlacementWorld + HeightWorld + MarkerWorld {
    /// Template entities use the shipwreck's nonzero rotation pivot.
    fn place_shipwreck_entities(
        &mut self,
        _entities: &[Tag],
        _origin: [i32; 3],
        _rotation: Rotation,
        _pivot: [i32; 3],
        _clip: BoundingBox,
    ) -> Result<()> {
        anyhow::bail!("shipwreck template entities are not yet supported")
    }
    fn shipwreck_piece_aligned(&mut self, _bounds: BoundingBox) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataMarker {
    pub position: [i32; 3],
    pub metadata: JavaString,
}

struct JigsawMarker {
    position: [i32; 3],
    final_state: BlockState,
}

impl PreparedStart {
    fn jigsaw_markers(
        &self,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
    ) -> Result<Vec<JigsawMarker>> {
        let count =
            i32::try_from(self.template.palettes.len()).context("too many shipwreck palettes")?;
        let [x, y, z] = self.piece.position;
        let index = LegacyRandom::new(positional_seed(x, y, z))
            .next_int_bound(count)
            .context("shipwreck has no palettes")? as usize;
        let palette = &self.template.palettes[index];
        let mut markers = Vec::new();
        for block in &self.template.blocks {
            let Some(Tag::String(name)) = palette
                .get(block.state)
                .context("shipwreck palette index out of bounds")?
                .get("id")
            else {
                anyhow::bail!("template block identifier missing")
            };
            if !(name.matches("minecraft:jigsaw") || name.matches("jigsaw")) {
                continue;
            }
            let Some(nbt) = &block.nbt else { continue };
            let Tag::Compound(fields) = nbt else {
                anyhow::bail!("jigsaw marker is not a compound")
            };
            let position = native_resources::structure_transform::block_world_position(
                block.position,
                self.piece.position,
                Mirror::None,
                self.piece.rotation,
                shipwreck::PIVOT,
            );
            if !(0..3).all(|axis| (clip.min[axis]..=clip.max[axis]).contains(&position[axis])) {
                continue;
            }
            ensure!(
                fields
                    .iter()
                    .filter(|(key, _)| key.matches("final_state"))
                    .count()
                    <= 1,
                "duplicate jigsaw final_state"
            );
            let raw = match nbt.get("final_state") {
                Some(Tag::String(value)) => value.to_string_lossy(),
                _ => "minecraft:air".to_owned(),
            };
            let final_state =
                native_persistence::jigsaw_replacement::parse_final_state(registry, &raw)?
                    .unwrap_or_else(|| BlockState {
                        name: "minecraft:air".into(),
                        properties: Default::default(),
                    });
            markers.push((
                block.position,
                JigsawMarker {
                    position,
                    final_state,
                },
            ));
        }
        markers.sort_by_key(|(position, _)| (position[1], position[0], position[2]));
        Ok(markers.into_iter().map(|(_, marker)| marker).collect())
    }
    /// Prepare ordered block writes and DATA markers for one clipped chunk.
    /// State rotation happens later, when each record is placed.
    pub fn block_records(
        &self,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
        is_static_full: impl FnMut(&BlockState) -> Result<bool>,
        world: &mut impl FnMut([i32; 3]) -> Result<BlockState>,
    ) -> Result<(Vec<ProcessedTemplateBlock>, Vec<DataMarker>)> {
        let count =
            i32::try_from(self.template.palettes.len()).context("too many shipwreck palettes")?;
        let [x, y, z] = self.piece.position;
        let index = LegacyRandom::new(positional_seed(x, y, z))
            .next_int_bound(count)
            .context("shipwreck has no palettes")? as usize;
        let records = process_structure_template_with_palette(
            &self.template,
            &PreparedProcessors::empty(),
            StructureTemplateSettings {
                origin: self.piece.position,
                mirror: Mirror::None,
                rotation: self.piece.rotation,
                pivot: shipwreck::PIVOT,
                clip,
            },
            index,
            |tag| resolve_template_state(tag, registry),
            is_static_full,
            world,
        )?;
        let mut blocks = Vec::with_capacity(records.len());
        let mut markers = Vec::new();
        for record in records {
            match record.block.state.name.as_str() {
                "minecraft:air" => {}
                "minecraft:structure_block" => {
                    let Some(nbt) = &record.block.nbt else {
                        continue;
                    };
                    let Some(Tag::String(mode)) = nbt.get("mode") else {
                        anyhow::bail!("shipwreck marker mode missing or invalid")
                    };
                    if mode.matches("SAVE") || mode.matches("LOAD") || mode.matches("CORNER") {
                        continue;
                    }
                    ensure!(mode.matches("DATA"), "invalid shipwreck marker mode");
                    let metadata = match nbt.get("metadata") {
                        Some(Tag::String(value)) => value.clone(),
                        _ => "".into(),
                    };
                    markers.push(DataMarker {
                        position: record.block.position,
                        metadata,
                    });
                }
                _ => blocks.push(record),
            }
        }
        Ok((blocks, markers))
    }

    /// Align, place and process markers as one staged piece. On error the
    /// caller must discard the world draft; this piece and RNG stay unchanged.
    pub fn post_process(
        &mut self,
        world: &mut impl ShipwreckPlacementWorld,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
        is_static_full: impl FnMut(&BlockState) -> Result<bool>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        let mut staged = self.clone();
        let mut next = random.clone();
        shipwreck::align_during_decoration(
            &mut staged.piece,
            staged.template.size,
            &mut staged.height_adjusted,
            world,
            || next.next_int_bound(3).unwrap(),
        )?;
        staged.bounds = staged.piece.bounds(staged.template.size)?;
        world.shipwreck_piece_aligned(staged.bounds)?;
        if staged.template.palettes.is_empty()
            || staged.template.size.iter().any(|v| *v < 1)
            || (staged.template.blocks.is_empty() && staged.template.entities.is_empty())
        {
            *self = staged;
            *random = next;
            return Ok(false);
        }
        let jigsaws = staged.jigsaw_markers(registry, clip)?;
        let shared = std::cell::RefCell::new(world);
        let (blocks, markers) =
            staged.block_records(registry, clip, is_static_full, &mut |position| {
                JigsawPlacementWorld::block(&mut **shared.borrow_mut(), position)
            })?;
        let world = shared.into_inner();
        place_structure_records(
            &blocks,
            world,
            registry,
            clip,
            Mirror::None,
            staged.piece.rotation,
            &mut next,
        )?;
        if !staged.template.entities.is_empty() {
            world.place_shipwreck_entities(
                &staged.template.entities,
                staged.piece.position,
                staged.piece.rotation,
                shipwreck::PIVOT,
                clip,
            )?;
        }
        for marker in markers {
            let name = String::from_utf16(&marker.metadata.0)
                .context("invalid shipwreck marker metadata")?;
            shipwreck::handle_data_marker(world, &mut next, &name, marker.position)?;
        }
        for marker in jigsaws {
            world.finalize_jigsaw_marker(marker.position, &marker.final_state)?;
        }
        world.finish_jigsaw_markers()?;
        *self = staged;
        *random = next;
        Ok(true)
    }
}
