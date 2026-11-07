//! Serialize supported fresh structure data without authorizing a world save.
use anyhow::{Context, Result, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::{packs::ResourceId, structure_start::SimpleStructureKind};
use native_world::loaded::ChunkPos;
use std::collections::{BTreeMap, BTreeSet};

use super::{
    SavedSwampHutPiece,
    structure_batch::{
        JigsawStarts, MineshaftStarts, OceanRuinStarts, RuinedPortalStarts, SimpleStarts,
        TerrainStartKey,
    },
    structure_references::{
        OrderedStructureReferences, StructureStartBounds, collect_ordered_structure_references,
    },
};

/// Verify that the saved reference index exactly reflects all starts in the
/// reference radius. Return source chunks that a later save plan must retain.
/// This is evidence for a save plan, not authorization to write a world.
#[cfg(test)]
fn referenced_start_chunks(
    pos: ChunkPos,
    all_starts: &BTreeMap<ChunkPos, SimpleStarts>,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<BTreeSet<ChunkPos>> {
    referenced_start_chunks_with_ocean(pos, all_starts, &BTreeMap::new(), references, huts)
}

#[cfg(test)]
pub(super) fn referenced_start_chunks_with_ocean(
    pos: ChunkPos,
    all_starts: &BTreeMap<ChunkPos, SimpleStarts>,
    all_ocean: &BTreeMap<ChunkPos, OceanRuinStarts>,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<BTreeSet<ChunkPos>> {
    referenced_start_chunks_with_mineshaft(
        pos,
        all_starts,
        all_ocean,
        &BTreeMap::new(),
        references,
        huts,
    )
}

/// Recompute every reference from all supported starts, including mineshafts.
#[cfg(test)]
pub(super) fn referenced_start_chunks_with_mineshaft(
    pos: ChunkPos,
    all_starts: &BTreeMap<ChunkPos, SimpleStarts>,
    all_ocean: &BTreeMap<ChunkPos, OceanRuinStarts>,
    all_mineshafts: &BTreeMap<ChunkPos, MineshaftStarts>,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<BTreeSet<ChunkPos>> {
    referenced_start_chunks_with_jigsaw(
        pos,
        all_starts,
        all_ocean,
        all_mineshafts,
        &BTreeMap::new(),
        references,
        huts,
    )
}

/// Recompute every reference, retaining jigsaw source chunks as well.
#[cfg(test)]
pub(super) fn referenced_start_chunks_with_jigsaw(
    pos: ChunkPos,
    all_starts: &BTreeMap<ChunkPos, SimpleStarts>,
    all_ocean: &BTreeMap<ChunkPos, OceanRuinStarts>,
    all_mineshafts: &BTreeMap<ChunkPos, MineshaftStarts>,
    all_jigsaw: &BTreeMap<ChunkPos, JigsawStarts>,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<BTreeSet<ChunkPos>> {
    referenced_start_chunks_with_portals(
        pos,
        all_starts,
        all_ocean,
        all_mineshafts,
        all_jigsaw,
        &BTreeMap::new(),
        references,
        huts,
    )
}

/// Recompute references from all retained starts before building a save draft.
#[allow(clippy::too_many_arguments)]
pub(super) fn referenced_start_chunks_with_portals(
    pos: ChunkPos,
    all_starts: &BTreeMap<ChunkPos, SimpleStarts>,
    all_ocean: &BTreeMap<ChunkPos, OceanRuinStarts>,
    all_mineshafts: &BTreeMap<ChunkPos, MineshaftStarts>,
    all_jigsaw: &BTreeMap<ChunkPos, JigsawStarts>,
    all_portals: &BTreeMap<ChunkPos, RuinedPortalStarts>,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<BTreeSet<ChunkPos>> {
    let recomputed = collect_ordered_structure_references(pos, |source| {
        let simple = all_starts.get(&source).with_context(|| {
            format!("structure starts are missing at {}, {}", source.x, source.z)
        })?;
        let mut bounds = simple
            .iter()
            .map(|(id, start)| {
                ensure!(
                    start.chunk == [source.x, source.z],
                    "structure start belongs to another chunk"
                );
                StructureStartBounds::from_simple(id.clone(), start)
            })
            .collect::<Result<Vec<_>>>()?;
        if let Some(ocean) = all_ocean.get(&source) {
            for (id, start) in ocean {
                ensure!(!simple.contains_key(id), "duplicate structure start");
                validate_ocean_start(source, start)?;
                bounds.push(start.reference_bounds(id.clone())?);
            }
        }
        if let Some(mineshafts) = all_mineshafts.get(&source) {
            for (id, start) in mineshafts {
                ensure!(
                    !simple.contains_key(id)
                        && !all_ocean
                            .get(&source)
                            .is_some_and(|starts| starts.contains_key(id)),
                    "duplicate structure start"
                );
                validate_mineshaft_start(source, id, start)?;
                bounds.push(start.reference_bounds(id.clone())?);
            }
        }
        if let Some(jigsaws) = all_jigsaw.get(&source) {
            for (id, start) in jigsaws {
                ensure!(
                    !simple.contains_key(id)
                        && !all_ocean
                            .get(&source)
                            .is_some_and(|starts| starts.contains_key(id))
                        && !all_mineshafts
                            .get(&source)
                            .is_some_and(|starts| starts.contains_key(id)),
                    "duplicate structure start"
                );
                validate_jigsaw_start(source, id, start)?;
                bounds.push(start.reference_bounds(id.clone())?);
            }
        }
        if let Some(portals) = all_portals.get(&source) {
            for (id, start) in portals {
                ensure!(
                    !simple.contains_key(id)
                        && !all_ocean
                            .get(&source)
                            .is_some_and(|starts| starts.contains_key(id))
                        && !all_mineshafts
                            .get(&source)
                            .is_some_and(|starts| starts.contains_key(id))
                        && !all_jigsaw
                            .get(&source)
                            .is_some_and(|starts| starts.contains_key(id)),
                    "duplicate structure start"
                );
                validate_portal_start(source, id, start)?;
                bounds.push(start.reference_bounds(id.clone())?);
            }
        }
        Ok(bounds)
    })?;
    ensure!(
        recomputed.membership() == references.membership(),
        "structure references differ from generated starts"
    );
    let mut required = BTreeSet::new();
    for (id, sources) in references.membership() {
        ensure!(
            references.order(id) == recomputed.order(id),
            "structure reference order differs from generated starts"
        );
        for &packed in sources {
            let bits = packed as u64;
            let source = ChunkPos {
                x: bits as u32 as i32,
                z: (bits >> 32) as u32 as i32,
            };
            let simple = all_starts.get(&source).and_then(|starts| starts.get(id));
            let ocean = all_ocean.get(&source).and_then(|starts| starts.get(id));
            let mineshaft = all_mineshafts
                .get(&source)
                .and_then(|starts| starts.get(id));
            let jigsaw = all_jigsaw.get(&source).and_then(|starts| starts.get(id));
            let portal = all_portals.get(&source).and_then(|starts| starts.get(id));
            match (simple, ocean, mineshaft, jigsaw, portal) {
                (Some(start), None, None, None, None) => {
                    validate_start(source, id, start, huts)?;
                }
                (None, Some(start), None, None, None) => {
                    validate_ocean_start(source, start)?;
                }
                (None, None, Some(start), None, None) => {
                    validate_mineshaft_start(source, id, start)?;
                }
                (None, None, None, Some(start), None) => {
                    validate_jigsaw_start(source, id, start)?;
                }
                (None, None, None, None, Some(start)) => {
                    validate_portal_start(source, id, start)?;
                }
                (None, None, None, None, None) => {
                    anyhow::bail!("referenced structure start is missing")
                }
                _ => anyhow::bail!("duplicate structure start"),
            }
            required.insert(source);
        }
    }
    Ok(required)
}

fn validate_portal_start(
    pos: ChunkPos,
    id: &ResourceId,
    start: &super::ruined_portal::PreparedStart,
) -> Result<Tag> {
    ensure!(
        start.chunk == [pos.x, pos.z],
        "portal belongs to another chunk"
    );
    start.to_tag(id)
}

fn validate_jigsaw_start(
    pos: ChunkPos,
    id: &ResourceId,
    start: &super::jigsaw::PreparedStart,
) -> Result<Tag> {
    ensure!(
        start.chunk == [pos.x, pos.z],
        "jigsaw belongs to another chunk"
    );
    start.to_tag(id)
}

fn validate_mineshaft_start(
    pos: ChunkPos,
    id: &ResourceId,
    start: &super::mineshaft::PreparedStart,
) -> Result<Tag> {
    ensure!(
        start.chunk == [pos.x, pos.z],
        "mineshaft belongs to another chunk"
    );
    start.to_tag(id)
}

fn validate_ocean_start(pos: ChunkPos, start: &super::ocean_ruin::PreparedStart) -> Result<()> {
    ensure!(
        start.chunk == [pos.x, pos.z],
        "ocean ruin belongs to another chunk"
    );
    ensure!(!start.pieces.is_empty(), "ocean ruin start has no pieces");
    for piece in &start.pieces {
        super::ocean_ruin::SavedOceanRuinPiece::capture(piece, start.temperature)?;
    }
    Ok(())
}

fn validate_start(
    pos: ChunkPos,
    id: &ResourceId,
    start: &native_resources::structure_start::SimpleStructureStart,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<Option<SavedSwampHutPiece>> {
    match start.kind {
        SimpleStructureKind::BuriedTreasure => {
            validate_buried_treasure_start(pos, start)?;
            Ok(None)
        }
        SimpleStructureKind::SwampHut => {
            ensure!(
                start.chunk == [pos.x, pos.z],
                "hut start belongs to another chunk"
            );
            let x = pos.x.checked_mul(16).context("hut X overflow")?;
            let z = pos.z.checked_mul(16).context("hut Z overflow")?;
            ensure!(
                start.bounds.min[0] == x && start.bounds.min[2] == z,
                "hut horizontal position differs from its start chunk"
            );
            let saved = *huts
                .get(&(id.clone(), pos))
                .context("hut piece state is missing")?;
            ensure!(
                SavedSwampHutPiece::capture(start, saved.state)? == saved,
                "hut piece state differs from start"
            );
            Ok(Some(saved))
        }
        _ => anyhow::bail!("unsupported structure start serialization"),
    }
}

fn validate_buried_treasure_start(
    pos: ChunkPos,
    start: &native_resources::structure_start::SimpleStructureStart,
) -> Result<()> {
    ensure!(
        start.kind == SimpleStructureKind::BuriedTreasure,
        "unsupported structure start serialization"
    );
    ensure!(
        start.chunk == [pos.x, pos.z],
        "structure start belongs to another chunk"
    );
    ensure!(
        start.orientation.is_none(),
        "buried treasure has an unexpected orientation"
    );
    ensure!(
        start.bounds.min == start.bounds.max,
        "buried treasure has an invalid bounding box"
    );
    let x = pos
        .x
        .checked_mul(16)
        .and_then(|v| v.checked_add(9))
        .context("structure X overflow")?;
    let z = pos
        .z
        .checked_mul(16)
        .and_then(|v| v.checked_add(9))
        .context("structure Z overflow")?;
    ensure!(
        start.bounds.min[0] == x && start.bounds.min[2] == z,
        "buried treasure has an invalid position"
    );
    Ok(())
}

#[cfg(test)]
fn pack_structure_data(
    pos: ChunkPos,
    starts: &SimpleStarts,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<Tag> {
    pack_structure_data_with_ocean(pos, starts, &OceanRuinStarts::new(), references, huts)
}

/// Serialize fresh starts and ordered references belonging to one chunk.
/// Existing-world data must not use this draft.
#[cfg(test)]
pub(super) fn pack_structure_data_with_ocean(
    pos: ChunkPos,
    starts: &SimpleStarts,
    ocean: &OceanRuinStarts,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<Tag> {
    pack_structure_data_with_mineshaft(
        pos,
        starts,
        ocean,
        &MineshaftStarts::new(),
        references,
        huts,
    )
}

/// Serialize supported fresh starts and their ordered references together.
#[cfg(test)]
pub(super) fn pack_structure_data_with_mineshaft(
    pos: ChunkPos,
    starts: &SimpleStarts,
    ocean: &OceanRuinStarts,
    mineshafts: &MineshaftStarts,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<Tag> {
    pack_structure_data_with_jigsaw(
        pos,
        starts,
        ocean,
        mineshafts,
        &JigsawStarts::new(),
        references,
        huts,
    )
}

/// Serialize supported starts, including jigsaw pieces, without writing a world.
#[cfg(test)]
pub(super) fn pack_structure_data_with_jigsaw(
    pos: ChunkPos,
    starts: &SimpleStarts,
    ocean: &OceanRuinStarts,
    mineshafts: &MineshaftStarts,
    jigsaws: &JigsawStarts,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<Tag> {
    pack_structure_data_with_portals(
        pos,
        starts,
        ocean,
        mineshafts,
        jigsaws,
        &RuinedPortalStarts::new(),
        references,
        huts,
    )
}

/// Serialize all supported fresh starts without writing an existing world.
#[allow(clippy::too_many_arguments)]
pub(super) fn pack_structure_data_with_portals(
    pos: ChunkPos,
    starts: &SimpleStarts,
    ocean: &OceanRuinStarts,
    mineshafts: &MineshaftStarts,
    jigsaws: &JigsawStarts,
    portals: &RuinedPortalStarts,
    references: &OrderedStructureReferences,
    huts: &BTreeMap<TerrainStartKey, SavedSwampHutPiece>,
) -> Result<Tag> {
    let mut packed_starts = Vec::with_capacity(starts.len());
    for (id, start) in starts {
        let hut = validate_start(pos, id, start, huts)?;
        let name = format!("{}:{}", id.namespace, id.path);
        let p = start.bounds.min;
        let piece = if let Some(hut) = hut {
            hut.to_tag()?
        } else {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:btp".into())),
                (
                    "BB".into(),
                    Tag::IntArray(vec![p[0], p[1], p[2], p[0], p[1], p[2]]),
                ),
                ("O".into(), Tag::Int(-1)),
                ("GD".into(), Tag::Int(0)),
            ])
        };
        let value = Tag::Compound(vec![
            ("id".into(), Tag::String(name.as_str().into())),
            ("ChunkX".into(), Tag::Int(pos.x)),
            ("ChunkZ".into(), Tag::Int(pos.z)),
            ("references".into(), Tag::Int(0)),
            (
                "Children".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![piece],
                },
            ),
        ]);
        packed_starts.push((name.as_str().into(), value));
    }
    for (id, start) in ocean {
        ensure!(!starts.contains_key(id), "duplicate structure start");
        validate_ocean_start(pos, start)?;
        let name = format!("{}:{}", id.namespace, id.path);
        let value = Tag::Compound(vec![
            ("id".into(), Tag::String(name.as_str().into())),
            ("ChunkX".into(), Tag::Int(pos.x)),
            ("ChunkZ".into(), Tag::Int(pos.z)),
            ("references".into(), Tag::Int(0)),
            (
                "Children".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: start.saved_piece_tags()?,
                },
            ),
        ]);
        packed_starts.push((name.as_str().into(), value));
    }
    for (id, start) in mineshafts {
        ensure!(
            !starts.contains_key(id) && !ocean.contains_key(id),
            "duplicate structure start"
        );
        let value = validate_mineshaft_start(pos, id, start)?;
        let name = format!("{}:{}", id.namespace, id.path);
        packed_starts.push((name.as_str().into(), value));
    }
    for (id, start) in jigsaws {
        ensure!(
            !starts.contains_key(id) && !ocean.contains_key(id) && !mineshafts.contains_key(id),
            "duplicate structure start"
        );
        let value = validate_jigsaw_start(pos, id, start)?;
        let name = format!("{}:{}", id.namespace, id.path);
        packed_starts.push((name.as_str().into(), value));
    }
    for (id, start) in portals {
        ensure!(
            !starts.contains_key(id)
                && !ocean.contains_key(id)
                && !mineshafts.contains_key(id)
                && !jigsaws.contains_key(id),
            "duplicate structure start"
        );
        let value = validate_portal_start(pos, id, start)?;
        let name = format!("{}:{}", id.namespace, id.path);
        packed_starts.push((name.as_str().into(), value));
    }
    let mut packed_references = Vec::with_capacity(references.membership().len());
    for (id, members) in references.membership() {
        let order = references
            .order(id)
            .context("structure reference order is missing")?;
        ensure!(
            order.iter().copied().collect::<BTreeSet<_>>() == *members
                && order.len() == members.len(),
            "structure reference order differs from membership"
        );
        let name = format!("{}:{}", id.namespace, id.path);
        packed_references.push((name.as_str().into(), Tag::LongArray(order.to_vec())));
    }
    Ok(Tag::Compound(vec![
        ("starts".into(), Tag::Compound(packed_starts)),
        ("References".into(), Tag::Compound(packed_references)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::super::structure_references::{
        StructureStartBounds, collect_ordered_structure_references,
    };
    use super::*;
    use native_core::beardifier::{BoundingBox, TerrainAdjustment};
    use native_resources::{packs::ResourceId, structure_start::SimpleStructureStart};

    #[test]
    fn portal_draft_preserves_start_and_reference_completeness() -> Result<()> {
        use native_persistence::{nbt::NamedTag, structure_template::StructureTemplate};
        use native_resources::{
            ruined_portal::{GenerationPoint, InitialSelection, Setup, VerticalPlacement},
            structure_transform::{Mirror, Rotation},
        };
        use std::sync::Arc;
        let pos = ChunkPos { x: -3, z: 3 };
        let id = ResourceId::parse("minecraft:ruined_portal")?;
        let selection = InitialSelection {
            setup_index: 0,
            air_pocket: false,
            template: ResourceId::parse("minecraft:ruined_portal/portal_1")?,
            rotation: Rotation::None,
            mirror: Mirror::None,
        };
        let setup = Setup {
            placement: VerticalPlacement::OnLandSurface,
            air_pocket_probability: 0.0,
            mossiness: 0.0,
            overgrown: false,
            vines: false,
            can_be_cold: false,
            replace_with_blackstone: false,
            weight: 1.0,
        };
        let point = GenerationPoint {
            position: [-48, 70, 48],
            pivot: [1, 0, 1],
            bounds: BoundingBox::new([-48, 70, 48], [-46, 72, 50]).unwrap(),
        };
        let piece = super::super::ruined_portal::SavedRuinedPortalPiece::capture(
            &selection,
            &setup,
            &point,
            [3, 3, 3],
            false,
        )?;
        let start = super::super::ruined_portal::PreparedStart {
            chunk: [-3, 3],
            point,
            terrain_adjustment: TerrainAdjustment::None,
            piece,
            template: Arc::new(StructureTemplate {
                document: NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                },
                data_version: Some(5023),
                size: [3, 3, 3],
                palettes: vec![],
                blocks: vec![],
                entities: vec![],
            }),
        };
        let bounds = start.reference_bounds(id.clone())?;
        let references = collect_ordered_structure_references(pos, |source| {
            Ok(if source == pos {
                vec![bounds.clone()]
            } else {
                vec![]
            })
        })?;
        let mut starts = BTreeMap::new();
        for z in pos.z - 8..=pos.z + 8 {
            for x in pos.x - 8..=pos.x + 8 {
                starts.insert(ChunkPos { x, z }, SimpleStarts::new());
            }
        }
        let portals =
            BTreeMap::from([(pos, RuinedPortalStarts::from([(id.clone(), start.clone())]))]);
        assert!(
            referenced_start_chunks_with_portals(
                pos,
                &starts,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &portals,
                &references,
                &BTreeMap::new()
            )?
            .contains(&pos)
        );
        let data = pack_structure_data_with_portals(
            pos,
            &starts[&pos],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &portals[&pos],
            &references,
            &BTreeMap::new(),
        )?;
        assert_eq!(
            data.get("starts")
                .and_then(|starts| starts.get("minecraft:ruined_portal")),
            Some(&start.to_tag(&id)?)
        );
        assert!(
            referenced_start_chunks_with_portals(
                pos,
                &starts,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &references,
                &BTreeMap::new()
            )
            .is_err()
        );
        starts.remove(&ChunkPos {
            x: pos.x - 8,
            z: pos.z - 8,
        });
        assert!(
            referenced_start_chunks_with_portals(
                pos,
                &starts,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &portals,
                &references,
                &BTreeMap::new()
            )
            .is_err()
        );
        let mut wrong = start.clone();
        wrong.chunk[0] += 1;
        assert!(
            pack_structure_data_with_portals(
                pos,
                &SimpleStarts::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &RuinedPortalStarts::from([(id, wrong)]),
                &references,
                &BTreeMap::new()
            )
            .is_err()
        );
        Ok(())
    }

    fn mineshaft_fixture(pos: ChunkPos) -> (ResourceId, super::super::mineshaft::PreparedStart) {
        use native_resources::mineshaft::{
            MineshaftElevation, MineshaftGraph, MineshaftGraphPieceKind, MineshaftVariant,
        };
        let placed = MineshaftGraph::new(42, [pos.x, pos.z], MineshaftVariant::Normal)
            .unwrap()
            .place(MineshaftElevation::Normal {
                sea_level: 63,
                min_y: -64,
            })
            .unwrap();
        let mut pieces: Vec<_> = placed
            .pieces
            .iter()
            .map(|piece| {
                super::super::SavedMineshaftPiece::capture(piece, MineshaftVariant::Normal, false)
                    .unwrap()
            })
            .collect();
        let spider = pieces
            .iter_mut()
            .find(|piece| {
                matches!(
                    piece.piece.kind,
                    MineshaftGraphPieceKind::Corridor { spiders: true, .. }
                )
            })
            .expect("fixture needs a spider corridor");
        spider.placed_spider = true;
        (
            ResourceId::parse("example:mine").unwrap(),
            super::super::mineshaft::PreparedStart {
                chunk: [pos.x, pos.z],
                generation_point: placed.generation_point,
                terrain_adjustment: TerrainAdjustment::None,
                variant: MineshaftVariant::Normal,
                pieces,
            },
        )
    }

    #[test]
    fn jigsaw_draft_preserves_sources_and_rejects_invalid_inputs() -> Result<()> {
        use native_resources::{
            jigsaw_placement::Piece,
            structure_pool::{HolderValue, LiquidSettings, PoolElement, Projection},
            structure_transform::Rotation,
        };
        let pos = ChunkPos { x: 0, z: 0 };
        let id = ResourceId::parse("example:jigsaw")?;
        let start = super::super::jigsaw::PreparedStart {
            chunk: [0, 0],
            generation_point: [8, 10, 8],
            terrain_adjustment: TerrainAdjustment::BeardThin,
            liquid_settings: LiquidSettings::ApplyWaterlogging,
            pieces: vec![Piece {
                element: PoolElement::Single {
                    legacy: false,
                    location: ResourceId::parse("example:room")?,
                    processors: HolderValue::Reference(ResourceId::parse("minecraft:empty")?),
                    projection: Projection::Rigid,
                    override_liquid_settings: None,
                },
                origin: [8, 10, 8],
                rotation: Rotation::None,
                bounds: BoundingBox::new([8, 10, 8], [10, 12, 10]).unwrap(),
                ground_level_delta: 1,
                junctions: vec![],
            }],
        };
        let mut simple = BTreeMap::new();
        for x in -8..=8 {
            for z in -8..=8 {
                simple.insert(ChunkPos { x, z }, SimpleStarts::new());
            }
        }
        let mut all = BTreeMap::from([(pos, JigsawStarts::from([(id.clone(), start.clone())]))]);
        let refs = collect_ordered_structure_references(pos, |source| {
            if source == pos {
                Ok(vec![start.reference_bounds(id.clone())?])
            } else {
                Ok(vec![])
            }
        })?;
        assert_eq!(
            referenced_start_chunks_with_jigsaw(
                pos,
                &simple,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &all,
                &refs,
                &BTreeMap::new()
            )?,
            BTreeSet::from([pos])
        );
        let draft = pack_structure_data_with_jigsaw(
            pos,
            &simple[&pos],
            &OceanRuinStarts::new(),
            &MineshaftStarts::new(),
            &all[&pos],
            &refs,
            &BTreeMap::new(),
        )?;
        assert_eq!(
            draft
                .get("starts")
                .and_then(|tag| tag.get("example:jigsaw")),
            Some(&start.to_tag(&id)?)
        );
        let empty = collect_ordered_structure_references(pos, |_| Ok(vec![]))?;
        assert!(
            referenced_start_chunks_with_jigsaw(
                pos,
                &simple,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &all,
                &empty,
                &BTreeMap::new()
            )
            .is_err()
        );
        all.get_mut(&pos).unwrap().get_mut(&id).unwrap().chunk[0] = 1;
        assert!(
            referenced_start_chunks_with_jigsaw(
                pos,
                &simple,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &all,
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(
            pack_structure_data_with_jigsaw(
                pos,
                &simple[&pos],
                &OceanRuinStarts::new(),
                &MineshaftStarts::new(),
                &all[&pos],
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        all.get_mut(&pos).unwrap().insert(id.clone(), start);
        let mut duplicate = fixture(pos, 90, true).0;
        let treasure = duplicate.pop_first().unwrap().1;
        duplicate.insert(id.clone(), treasure);
        assert!(
            pack_structure_data_with_jigsaw(
                pos,
                &duplicate,
                &OceanRuinStarts::new(),
                &MineshaftStarts::new(),
                &all[&pos],
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        simple.insert(pos, duplicate);
        assert!(
            referenced_start_chunks_with_jigsaw(
                pos,
                &simple,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &all,
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn mineshaft_starts_preserve_state_and_validate_all_references() -> Result<()> {
        let pos = ChunkPos { x: 0, z: 0 };
        let (id, start) = mineshaft_fixture(pos);
        let mut simple = BTreeMap::new();
        for x in -8..=8 {
            for z in -8..=8 {
                simple.insert(ChunkPos { x, z }, SimpleStarts::new());
            }
        }
        let mines = BTreeMap::from([(pos, MineshaftStarts::from([(id.clone(), start.clone())]))]);
        let refs = collect_ordered_structure_references(pos, |source| {
            if source == pos {
                Ok(vec![start.reference_bounds(id.clone())?])
            } else {
                Ok(vec![])
            }
        })?;
        assert_eq!(
            referenced_start_chunks_with_mineshaft(
                pos,
                &simple,
                &BTreeMap::new(),
                &mines,
                &refs,
                &BTreeMap::new()
            )?,
            BTreeSet::from([pos])
        );
        let packed = pack_structure_data_with_mineshaft(
            pos,
            &simple[&pos],
            &OceanRuinStarts::new(),
            &mines[&pos],
            &refs,
            &BTreeMap::new(),
        )?;
        assert_eq!(
            packed
                .get("starts")
                .and_then(|value| value.get("example:mine")),
            Some(&start.to_tag(&id)?)
        );
        let mut wrong = mines.clone();
        wrong.get_mut(&pos).unwrap().get_mut(&id).unwrap().chunk[0] += 1;
        assert!(
            referenced_start_chunks_with_mineshaft(
                pos,
                &simple,
                &BTreeMap::new(),
                &wrong,
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(
            pack_structure_data_with_mineshaft(
                pos,
                &simple[&pos],
                &OceanRuinStarts::new(),
                &wrong[&pos],
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        let empty = collect_ordered_structure_references(pos, |_| Ok(vec![]))?;
        assert!(
            referenced_start_chunks_with_mineshaft(
                pos,
                &simple,
                &BTreeMap::new(),
                &mines,
                &empty,
                &BTreeMap::new()
            )
            .is_err()
        );
        let mut duplicate = fixture(pos, 90, true).0;
        let treasure = duplicate.pop_first().unwrap().1;
        duplicate.insert(id.clone(), treasure);
        assert!(
            pack_structure_data_with_mineshaft(
                pos,
                &duplicate,
                &OceanRuinStarts::new(),
                &mines[&pos],
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        simple.insert(pos, duplicate);
        assert!(
            referenced_start_chunks_with_mineshaft(
                pos,
                &simple,
                &BTreeMap::new(),
                &mines,
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn mineshaft_structure_data_round_trips_java_chunk_serializer() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 JDK path is required")?
            .join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let source = temp.path().join("MineshaftStructureDataOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/MineshaftStructureDataOracle.java"),
        )?;
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compile.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let pos = ChunkPos { x: 0, z: 0 };
        let (id, start) = mineshaft_fixture(pos);
        let refs = collect_ordered_structure_references(pos, |source| {
            if source == pos {
                Ok(vec![start.reference_bounds(id.clone())?])
            } else {
                Ok(vec![])
            }
        })?;
        let tag = pack_structure_data_with_mineshaft(
            pos,
            &SimpleStarts::new(),
            &OceanRuinStarts::new(),
            &MineshaftStarts::from([(id, start)]),
            &refs,
            &BTreeMap::new(),
        )?;
        let input = temp.path().join("mineshaft.nbt");
        std::fs::write(
            &input,
            native_persistence::nbt::write(&native_persistence::nbt::NamedTag {
                name: "".into(),
                value: tag,
            })?,
        )?;
        let result = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("MineshaftStructureDataOracle")
            .arg(input)
            .output()?;
        ensure!(
            result.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(())
    }

    fn fixture(pos: ChunkPos, y: i32, present: bool) -> (SimpleStarts, OrderedStructureReferences) {
        let id = ResourceId::parse("example:treasure").unwrap();
        let p = [pos.x * 16 + 9, y, pos.z * 16 + 9];
        let start = SimpleStructureStart {
            kind: SimpleStructureKind::BuriedTreasure,
            chunk: [pos.x, pos.z],
            generation_point: [pos.x * 16 + 8, 0, pos.z * 16 + 8],
            orientation: None,
            bounds: BoundingBox::new(p, p).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let sources = [(-1, -1), (0, 0), (1, -1)];
        let references = collect_ordered_structure_references(pos, |source| {
            if present && sources.contains(&(source.x - pos.x, source.z - pos.z)) {
                Ok(vec![StructureStartBounds {
                    structure: id.clone(),
                    bounds: BoundingBox::new(p, p).unwrap(),
                }])
            } else {
                Ok(vec![])
            }
        })
        .unwrap();
        (
            if present {
                [(id, start)].into_iter().collect()
            } else {
                SimpleStarts::new()
            },
            references,
        )
    }

    fn ocean_fixture(pos: ChunkPos) -> (ResourceId, super::super::ocean_ruin::PreparedStart) {
        use native_persistence::{nbt::NamedTag, structure_template::StructureTemplate};
        use native_resources::{
            ocean_ruin::{Piece, Rotation, Temperature},
            structure_transform::{Mirror, bounding_box},
        };
        use std::sync::Arc;
        let template = Arc::new(StructureTemplate {
            document: NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![]),
            },
            data_version: Some(5023),
            size: [7, 4, 5],
            palettes: vec![],
            blocks: vec![],
            entities: vec![],
        });
        let pieces = [
            ([1, 70, 2], Rotation::None, 0.7, false),
            ([5, 71, 6], Rotation::Clockwise90, 0.5, true),
        ]
        .into_iter()
        .map(|(offset, rotation, integrity, large)| {
            let p = [pos.x * 16 + offset[0], offset[1], pos.z * 16 + offset[2]];
            super::super::ocean_ruin::PreparedPiece {
                selection: Piece {
                    template: "minecraft:underwater_ruin/warm_1".into(),
                    position: p,
                    rotation,
                    integrity,
                    large,
                },
                template: Arc::clone(&template),
                bounds: bounding_box(template.size, p, Mirror::None, rotation, [0; 3]).unwrap(),
            }
        })
        .collect();
        (
            ResourceId::parse("example:ruin").unwrap(),
            super::super::ocean_ruin::PreparedStart {
                chunk: [pos.x, pos.z],
                generation_point: [pos.x * 16 + 8, 70, pos.z * 16 + 8],
                terrain_adjustment: TerrainAdjustment::None,
                temperature: Temperature::Warm,
                pieces,
            },
        )
    }

    #[test]
    fn ocean_starts_preserve_pieces_and_validate_references() -> Result<()> {
        let pos = ChunkPos { x: 0, z: 0 };
        let (id, start) = ocean_fixture(pos);
        let mut simple = BTreeMap::new();
        for x in -8..=8 {
            for z in -8..=8 {
                simple.insert(ChunkPos { x, z }, SimpleStarts::new());
            }
        }
        let ocean = BTreeMap::from([(pos, OceanRuinStarts::from([(id.clone(), start.clone())]))]);
        let refs = collect_ordered_structure_references(pos, |source| {
            if source == pos {
                Ok(vec![start.reference_bounds(id.clone())?])
            } else {
                Ok(vec![])
            }
        })?;
        assert_eq!(
            referenced_start_chunks_with_ocean(pos, &simple, &ocean, &refs, &BTreeMap::new())?,
            BTreeSet::from([pos])
        );
        let tag = pack_structure_data_with_ocean(
            pos,
            &simple[&pos],
            &ocean[&pos],
            &refs,
            &BTreeMap::new(),
        )?;
        let Some(Tag::List { values, .. }) = tag
            .get("starts")
            .and_then(|v| v.get("example:ruin"))
            .and_then(|v| v.get("Children"))
        else {
            anyhow::bail!("missing ocean ruin pieces")
        };
        assert_eq!(values, &start.saved_piece_tags()?);
        let mut broken = ocean.clone();
        broken.get_mut(&pos).unwrap().get_mut(&id).unwrap().pieces[1]
            .bounds
            .min[1] += 1;
        assert!(
            referenced_start_chunks_with_ocean(pos, &simple, &broken, &refs, &BTreeMap::new())
                .is_err()
        );
        assert!(
            pack_structure_data_with_ocean(
                pos,
                &simple[&pos],
                &broken[&pos],
                &refs,
                &BTreeMap::new()
            )
            .is_err()
        );
        let empty = collect_ordered_structure_references(pos, |_| Ok(vec![]))?;
        assert!(
            referenced_start_chunks_with_ocean(pos, &simple, &ocean, &empty, &BTreeMap::new())
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn rejects_unrepresentable_structure_starts() {
        let pos = ChunkPos { x: -17, z: 23 };
        let (mut starts, references) = fixture(pos, -27, true);
        let id = ResourceId::parse("example:treasure").unwrap();
        assert!(pack_structure_data(pos, &starts, &references, &BTreeMap::new()).is_ok());
        starts.get_mut(&id).unwrap().chunk[0] += 1;
        assert!(pack_structure_data(pos, &starts, &references, &BTreeMap::new()).is_err());
        starts.get_mut(&id).unwrap().chunk[0] -= 1;
        starts.get_mut(&id).unwrap().kind = SimpleStructureKind::SwampHut;
        assert!(pack_structure_data(pos, &starts, &references, &BTreeMap::new()).is_err());
        starts.get_mut(&id).unwrap().kind = SimpleStructureKind::BuriedTreasure;
        starts.get_mut(&id).unwrap().bounds.max[1] += 1;
        assert!(pack_structure_data(pos, &starts, &references, &BTreeMap::new()).is_err());
    }

    #[test]
    fn hut_structure_data_preserves_piece_state_and_rejects_mismatches() {
        use native_resources::{
            structure_piece::{ScatteredGroundHeight, swamp_hut::SwampHutState},
            structure_start::HorizontalDirection,
        };
        let pos = ChunkPos { x: -17, z: 23 };
        let id = ResourceId::parse("example:hut").unwrap();
        let key = (id.clone(), pos);
        for direction in [
            HorizontalDirection::North,
            HorizontalDirection::South,
            HorizontalDirection::East,
            HorizontalDirection::West,
        ] {
            let (width, depth) = match direction {
                HorizontalDirection::North | HorizontalDirection::South => (7, 9),
                _ => (9, 7),
            };
            let [x, z] = [pos.x * 16, pos.z * 16];
            let start = SimpleStructureStart {
                kind: SimpleStructureKind::SwampHut,
                chunk: [pos.x, pos.z],
                generation_point: [x + 8, 70, z + 8],
                orientation: Some(direction),
                bounds: BoundingBox::new([x, 70, z], [x + width - 1, 76, z + depth - 1]).unwrap(),
                terrain_adjustment: TerrainAdjustment::None,
            };
            let mut all = BTreeMap::new();
            for x in pos.x - 8..=pos.x + 8 {
                for z in pos.z - 8..=pos.z + 8 {
                    all.insert(ChunkPos { x, z }, SimpleStarts::new());
                }
            }
            all.insert(pos, BTreeMap::from([(id.clone(), start.clone())]));
            let references = collect_ordered_structure_references(pos, |source| {
                all[&source]
                    .iter()
                    .map(|(id, start)| StructureStartBounds::from_simple(id.clone(), start))
                    .collect()
            })
            .unwrap();
            for witch in [false, true] {
                for cat in [false, true] {
                    let state = SwampHutState {
                        ground: ScatteredGroundHeight::from_saved(70),
                        spawned_witch: witch,
                        spawned_cat: cat,
                    };
                    let saved = SavedSwampHutPiece::capture(&start, state).unwrap();
                    let huts = BTreeMap::from([(key.clone(), saved)]);
                    assert_eq!(
                        referenced_start_chunks(pos, &all, &references, &huts).unwrap(),
                        BTreeSet::from([pos])
                    );
                    let data = pack_structure_data(pos, &all[&pos], &references, &huts).unwrap();
                    let piece = match data
                        .get("starts")
                        .unwrap()
                        .get("example:hut")
                        .unwrap()
                        .get("Children")
                        .unwrap()
                    {
                        Tag::List { values, .. } => &values[0],
                        _ => panic!("missing hut piece"),
                    };
                    assert_eq!(SavedSwampHutPiece::from_tag(piece).unwrap(), saved);
                    assert!(
                        pack_structure_data(pos, &all[&pos], &references, &BTreeMap::new())
                            .is_err()
                    );
                    let mut wrong = saved;
                    wrong.bounds.min[1] += 1;
                    assert!(
                        pack_structure_data(
                            pos,
                            &all[&pos],
                            &references,
                            &BTreeMap::from([(key.clone(), wrong)])
                        )
                        .is_err()
                    );
                }
            }
            let mut wrong = all.clone();
            wrong
                .get_mut(&pos)
                .unwrap()
                .get_mut(&id)
                .unwrap()
                .bounds
                .min[0] += 1;
            assert!(pack_structure_data(pos, &wrong[&pos], &references, &BTreeMap::new()).is_err());
        }
    }

    #[test]
    fn referenced_sources_require_complete_consistent_generated_starts() {
        let pos = ChunkPos { x: -17, z: 23 };
        let id = ResourceId::parse("example:treasure").unwrap();
        let (start, _) = fixture(pos, -27, true);
        let mut all = BTreeMap::new();
        for x in pos.x - 8..=pos.x + 8 {
            for z in pos.z - 8..=pos.z + 8 {
                all.insert(ChunkPos { x, z }, SimpleStarts::new());
            }
        }
        all.insert(pos, start);
        let references = collect_ordered_structure_references(pos, |source| {
            all[&source]
                .iter()
                .map(|(id, start)| StructureStartBounds::from_simple(id.clone(), start))
                .collect()
        })
        .unwrap();
        assert_eq!(
            referenced_start_chunks(pos, &all, &references, &BTreeMap::new()).unwrap(),
            BTreeSet::from([pos])
        );
        let mut missing = all.clone();
        missing.remove(&ChunkPos {
            x: pos.x - 8,
            z: pos.z + 8,
        });
        assert!(referenced_start_chunks(pos, &missing, &references, &BTreeMap::new()).is_err());
        let mut missing_start = all.clone();
        missing_start.get_mut(&pos).unwrap().clear();
        assert!(
            referenced_start_chunks(pos, &missing_start, &references, &BTreeMap::new()).is_err()
        );
        let empty_refs = collect_ordered_structure_references(pos, |_| Ok(vec![])).unwrap();
        assert!(referenced_start_chunks(pos, &all, &empty_refs, &BTreeMap::new()).is_err());
        assert!(
            referenced_start_chunks(pos, &missing_start, &empty_refs, &BTreeMap::new())
                .unwrap()
                .is_empty()
        );
        let mut wrong_kind = all.clone();
        wrong_kind.get_mut(&pos).unwrap().get_mut(&id).unwrap().kind =
            SimpleStructureKind::SwampHut;
        assert!(referenced_start_chunks(pos, &wrong_kind, &references, &BTreeMap::new()).is_err());
        let mut wrong_position = all.clone();
        wrong_position
            .get_mut(&pos)
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .bounds
            .min[1] -= 1;
        assert!(
            referenced_start_chunks(pos, &wrong_position, &references, &BTreeMap::new()).is_err()
        );
    }

    fn canonical(tag: Tag) -> Tag {
        match tag {
            Tag::Compound(fields) => {
                let mut fields: Vec<_> = fields
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect();
                fields.sort_by(|(a, _), (b, _)| a.0.cmp(&b.0));
                Tag::Compound(fields)
            }
            Tag::List {
                element_type,
                values,
            } => Tag::List {
                element_type,
                values: values.into_iter().map(canonical).collect(),
            },
            other => other,
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn ocean_structure_data_round_trips_java_chunk_serializer() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let source = temp.path().join("OceanRuinStructureDataOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/OceanRuinStructureDataOracle.java"),
        )?;
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compile.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let pos = ChunkPos { x: 0, z: 0 };
        let (id, start) = ocean_fixture(pos);
        let refs = collect_ordered_structure_references(pos, |source| {
            if source == pos {
                Ok(vec![start.reference_bounds(id.clone())?])
            } else {
                Ok(vec![])
            }
        })?;
        let actual = pack_structure_data_with_ocean(
            pos,
            &SimpleStarts::new(),
            &OceanRuinStarts::from([(id, start)]),
            &refs,
            &BTreeMap::new(),
        )?;
        let rust_path = temp.path().join("rust.nbt");
        let java_path = temp.path().join("java.nbt");
        std::fs::write(
            &rust_path,
            native_persistence::nbt::write(&native_persistence::nbt::NamedTag {
                name: "".into(),
                value: actual.clone(),
            })?,
        )?;
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("OceanRuinStructureDataOracle")
            .arg(&java_path)
            .arg(&rust_path)
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = native_persistence::nbt::read(&std::fs::read(java_path)?)?.value;
        assert_eq!(canonical(actual), canonical(expected));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn structure_data_matches_java_chunk_serializer() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let source = temp.path().join("StructureDataOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StructureDataOracle.java"),
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
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StructureDataOracle")
            .arg(temp.path()));
        for (index, pos, y, present) in [
            (0, ChunkPos { x: 0, z: 0 }, 90, false),
            (1, ChunkPos { x: 0, z: 0 }, 90, true),
            (2, ChunkPos { x: -17, z: 23 }, -27, true),
        ] {
            let (starts, references) = fixture(pos, y, present);
            let actual = pack_structure_data(pos, &starts, &references, &BTreeMap::new()).unwrap();
            let document = native_persistence::nbt::NamedTag {
                name: "".into(),
                value: actual.clone(),
            };
            std::fs::write(
                temp.path().join(format!("structure-rust-{index}.nbt")),
                native_persistence::nbt::write(&document).unwrap(),
            )
            .unwrap();
            let expected = native_persistence::nbt::read(
                &std::fs::read(temp.path().join(format!("structure-{index}.nbt"))).unwrap(),
            )
            .unwrap()
            .value;
            assert_eq!(canonical(actual), canonical(expected), "case {index}");
        }
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StructureDataOracle")
            .arg(temp.path())
            .arg("verify"));
        use native_resources::{
            structure_piece::{ScatteredGroundHeight, swamp_hut::SwampHutState},
            structure_start::HorizontalDirection,
        };
        let pos = ChunkPos { x: -17, z: 23 };
        let id = ResourceId::parse("example:hut").unwrap();
        let [x, z] = [pos.x * 16, pos.z * 16];
        let mut index = 0;
        for direction in [
            HorizontalDirection::North,
            HorizontalDirection::South,
            HorizontalDirection::East,
            HorizontalDirection::West,
        ] {
            let (width, depth) = match direction {
                HorizontalDirection::North | HorizontalDirection::South => (7, 9),
                _ => (9, 7),
            };
            let start = SimpleStructureStart {
                kind: SimpleStructureKind::SwampHut,
                chunk: [pos.x, pos.z],
                generation_point: [x + 8, 70, z + 8],
                orientation: Some(direction),
                bounds: BoundingBox::new([x, 70, z], [x + width - 1, 76, z + depth - 1]).unwrap(),
                terrain_adjustment: TerrainAdjustment::None,
            };
            let references = collect_ordered_structure_references(pos, |source| {
                if source == pos {
                    Ok(vec![StructureStartBounds::from_simple(id.clone(), &start)?])
                } else {
                    Ok(vec![])
                }
            })
            .unwrap();
            for witch in [false, true] {
                for cat in [false, true] {
                    let state = SwampHutState {
                        ground: ScatteredGroundHeight::from_saved(70),
                        spawned_witch: witch,
                        spawned_cat: cat,
                    };
                    let saved = SavedSwampHutPiece::capture(&start, state).unwrap();
                    let data = pack_structure_data(
                        pos,
                        &BTreeMap::from([(id.clone(), start.clone())]),
                        &references,
                        &BTreeMap::from([((id.clone(), pos), saved)]),
                    )
                    .unwrap();
                    let document = native_persistence::nbt::NamedTag {
                        name: "".into(),
                        value: data,
                    };
                    std::fs::write(
                        temp.path().join(format!("hut-rust-{index}.nbt")),
                        native_persistence::nbt::write(&document).unwrap(),
                    )
                    .unwrap();
                    index += 1;
                }
            }
        }
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StructureDataOracle")
            .arg(temp.path())
            .arg("verify_hut"));
    }
}
