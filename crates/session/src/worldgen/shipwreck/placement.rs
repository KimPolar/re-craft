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

impl PreparedStart {
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
                "minecraft:jigsaw" => {
                    anyhow::bail!("shipwreck jigsaw final-state processing is unsupported")
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
        *self = staged;
        *random = next;
        Ok(true)
    }
}
