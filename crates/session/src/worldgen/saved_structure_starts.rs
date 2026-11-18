//! Restore supported saved structure starts without modifying their NBT.
use anyhow::{Context, Result, bail, ensure};
use native_core::beardifier::{
    Beardifier, BoundingBox, PieceProjection, StructurePiece, TerrainAdjustment,
};
use native_persistence::{
    jigsaw_piece::SavedJigsawPiece,
    nbt::{Tag, TagType},
};
use native_resources::{packs::ResourceId, structure_start::decode_terrain_adjustment};
use native_world::loaded::ChunkPos;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::{
    SavedMineshaftPiece, SavedSwampHutPiece,
    structure_references::{
        OrderedStructureReferences, SavedStructureReferences, StructureStartBounds,
    },
};

#[derive(Clone, Debug)]
pub enum RestoredStructurePiece {
    BuriedTreasure(BoundingBox),
    SwampHut(SavedSwampHutPiece),
    Mineshaft(SavedMineshaftPiece),
    Jigsaw(SavedJigsawPiece),
    RuinedPortal(super::ruined_portal::SavedRuinedPortalPiece),
    OceanRuin(super::ocean_ruin::SavedOceanRuinPiece),
}

impl RestoredStructurePiece {
    pub fn bounds(&self) -> BoundingBox {
        match self {
            Self::BuriedTreasure(bounds) => *bounds,
            Self::SwampHut(piece) => piece.bounds,
            Self::Mineshaft(piece) => piece.piece.bounds,
            Self::Jigsaw(piece) => piece.piece.bounds,
            Self::RuinedPortal(piece) => piece.bounds,
            Self::OceanRuin(piece) => piece.bounds,
        }
    }
}

pub enum RestoredStructureStart {
    Invalid {
        original: Tag,
    },
    Present {
        chunk: ChunkPos,
        references: i32,
        adjustment: TerrainAdjustment,
        pieces: Vec<RestoredStructurePiece>,
        original: Tag,
    },
}

impl RestoredStructureStart {
    /// Restore the piece inputs used for terrain adaptation, preserving the
    /// original piece and jigsaw junction order. The caller must still resolve
    /// referenced starts in Java's structure and reference iteration order.
    pub fn terrain_pieces(&self) -> Result<Vec<StructurePiece>> {
        let Self::Present {
            adjustment, pieces, ..
        } = self
        else {
            return Ok(Vec::new());
        };
        pieces
            .iter()
            .map(|piece| match piece {
                RestoredStructurePiece::Jigsaw(saved) => {
                    super::jigsaw::terrain_piece(&saved.piece, *adjustment)
                }
                _ => Ok(StructurePiece {
                    bounds: piece.bounds(),
                    adjustment: *adjustment,
                    ground_delta: 0,
                    projection: PieceProjection::NonPool,
                    junctions: Vec::new(),
                }),
            })
            .collect()
    }

    /// Empty starts and Java's INVALID sentinel have no reference bounds.
    pub fn reference_bounds(&self, id: &ResourceId) -> Result<Option<StructureStartBounds>> {
        match self {
            Self::Invalid { .. } => Ok(None),
            Self::Present {
                adjustment, pieces, ..
            } if !pieces.is_empty() => {
                let boxes: Vec<_> = pieces.iter().map(RestoredStructurePiece::bounds).collect();
                Ok(Some(StructureStartBounds::from_pieces(
                    id.clone(),
                    &boxes,
                    *adjustment,
                )?))
            }
            Self::Present { .. } => Ok(None),
        }
    }
}

/// Build the terrain adjustment from captured references and their starts.
/// The caller supplies the Java structure-key order; each key's saved
/// fastutil reference order is retained. Missing starts or discarded input
/// fail instead of silently generating terrain with incomplete structures.
pub fn saved_beardifier_for_chunk(
    target: ChunkPos,
    references: &SavedStructureReferences,
    structure_order: &[ResourceId],
    starts: &BTreeMap<ChunkPos, BTreeMap<ResourceId, RestoredStructureStart>>,
) -> Result<Beardifier> {
    ensure!(
        references.discarded.is_empty(),
        "saved structure references contain discarded data"
    );
    beardifier_for_ordered_references(
        target,
        &references.references,
        structure_order,
        |id, source| {
            let start = starts
                .get(&source)
                .and_then(|by_id| by_id.get(id))
                .with_context(|| {
                    format!(
                        "missing saved structure start {}:{} at {},{}",
                        id.namespace, id.path, source.x, source.z
                    )
                })?;
            if let RestoredStructureStart::Present { chunk, .. } = start {
                ensure!(
                    *chunk == source,
                    "saved structure start belongs to another chunk"
                );
            }
            start.terrain_pieces()
        },
    )
}

/// Infer a structure-key order only when every possible order produces the
/// same chunk-clipped terrain contributions. Per-key reference and piece
/// order remains the captured Java order; ambiguous overlap returns `None`.
pub fn unambiguous_beardifier_structure_order(
    target: ChunkPos,
    references: &OrderedStructureReferences,
    mut pieces_for_start: impl FnMut(&ResourceId, ChunkPos) -> Result<Vec<StructurePiece>>,
) -> Result<Option<Vec<ResourceId>>> {
    let mut counts = BTreeMap::<ResourceId, (usize, usize)>::new();
    let mut supports = BTreeMap::<ResourceId, Option<Vec<BoundingBox>>>::new();
    for id in references.membership().keys() {
        for &packed in references
            .order(id)
            .context("missing saved reference order")?
        {
            let source = ChunkPos {
                x: packed as u32 as i32,
                z: ((packed as u64) >> 32) as u32 as i32,
            };
            let pieces = pieces_for_start(id, source)?;
            let clipped =
                Beardifier::for_chunk([target.x, target.z], &pieces).map_err(anyhow::Error::msg)?;
            let (rigids, junctions) = clipped.contribution_counts();
            let entry = counts.entry(id.clone()).or_default();
            entry.0 = entry
                .0
                .checked_add(rigids)
                .context("too many rigid pieces")?;
            entry.1 = entry
                .1
                .checked_add(junctions)
                .context("too many junctions")?;
            let entry = supports
                .entry(id.clone())
                .or_insert_with(|| Some(Vec::new()));
            match (entry.as_mut(), clipped.contribution_supports()) {
                (Some(existing), Some(regions)) => existing.extend(regions),
                _ => *entry = None,
            }
        }
    }
    if !super::structure_batch::terrain_contributions_commute(counts.values().copied())
        && !super::structure_batch::terrain_supports_disjoint(&supports)
    {
        return Ok(None);
    }
    Ok(Some(references.membership().keys().cloned().collect()))
}

/// Build a beardifier from either saved or newly collected references.
/// The caller resolves each start from the same captured generation inputs.
pub fn beardifier_for_ordered_references(
    target: ChunkPos,
    references: &OrderedStructureReferences,
    structure_order: &[ResourceId],
    mut pieces_for_start: impl FnMut(&ResourceId, ChunkPos) -> Result<Vec<StructurePiece>>,
) -> Result<Beardifier> {
    let keys: BTreeSet<_> = references.membership().keys().cloned().collect();
    let order: BTreeSet<_> = structure_order.iter().cloned().collect();
    ensure!(
        order.len() == structure_order.len() && order == keys,
        "structure order does not cover references exactly"
    );
    let mut pieces = Vec::new();
    for id in structure_order {
        for &packed in references
            .order(id)
            .context("missing saved reference order")?
        {
            let source = ChunkPos {
                x: packed as u32 as i32,
                z: ((packed as u64) >> 32) as u32 as i32,
            };
            pieces.extend(pieces_for_start(id, source)?);
        }
    }
    Beardifier::for_chunk([target.x, target.z], &pieces).map_err(anyhow::Error::msg)
}

fn field<'a>(tag: &'a Tag, key: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(entries) = tag else {
        bail!("structure value is not a compound")
    };
    let mut matches = entries.iter().filter(|(name, _)| name.matches(key));
    let result = matches.next().map(|(_, value)| value);
    ensure!(matches.next().is_none(), "duplicate structure field {key}");
    Ok(result)
}

fn integer(tag: &Tag, key: &str) -> Result<i32> {
    match field(tag, key)? {
        Some(Tag::Int(value)) => Ok(*value),
        _ => bail!("missing or invalid structure integer {key}"),
    }
}

fn treasure_piece(tag: &Tag) -> Result<RestoredStructurePiece> {
    let Tag::Compound(fields) = tag else {
        bail!("treasure piece is not a compound")
    };
    ensure!(
        fields.len() == 4
            && ["id", "BB", "O", "GD"].iter().all(|name| fields
                .iter()
                .filter(|(key, _)| key.matches(name))
                .count()
                == 1),
        "unsupported treasure piece fields"
    );
    ensure!(
        matches!(field(tag, "id")?, Some(Tag::String(id)) if id.matches("minecraft:btp"))
            && integer(tag, "O")? == -1
            && integer(tag, "GD")? == 0,
        "unsupported treasure piece"
    );
    let Some(Tag::IntArray(boxes)) = field(tag, "BB")? else {
        bail!("missing treasure bounds")
    };
    ensure!(boxes.len() == 6, "invalid treasure bounds");
    let bounds = BoundingBox::new(
        [boxes[0], boxes[1], boxes[2]],
        [boxes[3], boxes[4], boxes[5]],
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(bounds.min == bounds.max, "unsupported treasure bounds");
    Ok(RestoredStructurePiece::BuriedTreasure(bounds))
}

/// Patch only the moved piece bounds, preserving the original start and all
/// unknown fields. This is an NBT draft, not permission to write a world.
pub fn draft_saved_treasure_start(
    original: &Tag,
    id: &ResourceId,
    position: ChunkPos,
    references: i32,
    placed: &native_resources::structure_start::SimpleStructureStart,
) -> Result<Tag> {
    use native_resources::structure_start::SimpleStructureKind;
    let expected_id = format!("{}:{}", id.namespace, id.path);
    ensure!(
        matches!(field(original, "id")?, Some(Tag::String(value)) if value.matches(&expected_id)),
        "saved treasure start identity differs"
    );
    ensure!(
        integer(original, "ChunkX")? == position.x && integer(original, "ChunkZ")? == position.z,
        "saved treasure start source differs"
    );
    ensure!(
        integer(original, "references")? == references,
        "saved treasure reference count differs"
    );
    ensure!(
        placed.kind == SimpleStructureKind::BuriedTreasure
            && placed.chunk == [position.x, position.z]
            && placed.orientation.is_none()
            && placed.bounds.min == placed.bounds.max,
        "invalid decorated treasure piece"
    );
    let Some(Tag::List {
        element_type: TagType::Compound,
        values,
    }) = field(original, "Children")?
    else {
        bail!("saved treasure pieces are missing");
    };
    let [piece] = values.as_slice() else {
        bail!("expected one saved treasure piece")
    };
    let RestoredStructurePiece::BuriedTreasure(before) = treasure_piece(piece)? else {
        unreachable!()
    };
    ensure!(
        before.min[0] == placed.bounds.min[0] && before.min[2] == placed.bounds.min[2],
        "treasure horizontal position changed"
    );
    let mut result = original.clone();
    let Tag::Compound(fields) = &mut result else {
        unreachable!()
    };
    let (_, Tag::List { values, .. }) = fields
        .iter_mut()
        .find(|(name, _)| name.matches("Children"))
        .expect("children field was validated")
    else {
        unreachable!()
    };
    let Tag::Compound(piece) = &mut values[0] else {
        unreachable!()
    };
    let (_, bounds) = piece
        .iter_mut()
        .find(|(name, _)| name.matches("BB"))
        .expect("piece bounds were validated");
    *bounds = Tag::IntArray(
        placed
            .bounds
            .min
            .into_iter()
            .chain(placed.bounds.max)
            .collect(),
    );
    Ok(result)
}

/// Decode starts for one captured chunk using the active structure registry.
/// Unsupported types and lossy piece encodings fail without publishing partial
/// results. Retained NBT does not authorize modifying an existing world.
pub fn restore_saved_structure_starts(
    position: ChunkPos,
    starts_field: Option<&Tag>,
    definition: impl FnMut(&ResourceId) -> Result<Option<Value>>,
) -> Result<BTreeMap<ResourceId, RestoredStructureStart>> {
    restore_saved_structure_starts_with_templates(position, starts_field, definition, |_| Ok(None))
}

/// Like `restore_saved_structure_starts`, resolving template sizes from the
/// same active resource stack for template-backed pieces.
pub fn restore_saved_structure_starts_with_templates(
    position: ChunkPos,
    starts_field: Option<&Tag>,
    mut definition: impl FnMut(&ResourceId) -> Result<Option<Value>>,
    mut template_size: impl FnMut(&ResourceId) -> Result<Option<[i32; 3]>>,
) -> Result<BTreeMap<ResourceId, RestoredStructureStart>> {
    let mut result = BTreeMap::new();
    // All pieces in this capture use the same frozen resource stack. Avoid
    // repeatedly decompressing one template when it appears in several pieces.
    let mut template_sizes = BTreeMap::<ResourceId, [i32; 3]>::new();
    let Some(starts_field) = starts_field else {
        return Ok(result);
    };
    let Tag::Compound(starts) = starts_field else {
        bail!("saved structure starts are not a compound")
    };
    for (name, tag) in starts {
        let id = ResourceId::parse(
            &String::from_utf16(&name.0).context("invalid structure identifier")?,
        )?;
        ensure!(!result.contains_key(&id), "duplicate saved structure ID");
        let config = definition(&id)?
            .with_context(|| format!("unknown saved structure {}:{}", id.namespace, id.path))?;
        let Some(Tag::String(saved_id)) = field(tag, "id")? else {
            bail!("saved structure id is missing")
        };
        if saved_id.matches("INVALID") {
            result.insert(
                id,
                RestoredStructureStart::Invalid {
                    original: tag.clone(),
                },
            );
            continue;
        }
        let saved_id =
            ResourceId::parse(&String::from_utf16(&saved_id.0).context("invalid saved start ID")?)?;
        ensure!(
            saved_id == id,
            "saved structure start ID differs from registry key"
        );
        let chunk = ChunkPos {
            x: integer(tag, "ChunkX")?,
            z: integer(tag, "ChunkZ")?,
        };
        ensure!(
            chunk == position,
            "saved structure start belongs to another chunk"
        );
        let references = integer(tag, "references")?;
        let Some(Tag::List {
            element_type,
            values,
        }) = field(tag, "Children")?
        else {
            bail!("structure children are missing")
        };
        ensure!(
            *element_type == TagType::Compound
                || (*element_type == TagType::End && values.is_empty()),
            "invalid structure children"
        );
        let kind = ResourceId::parse(
            config
                .get("type")
                .and_then(Value::as_str)
                .context("missing structure type")?,
        )?;
        ensure!(kind.namespace == "minecraft", "unsupported structure type");
        ensure!(
            matches!(
                kind.path.as_str(),
                "buried_treasure"
                    | "swamp_hut"
                    | "mineshaft"
                    | "jigsaw"
                    | "ruined_portal"
                    | "ocean_ruin"
            ),
            "unsupported saved structure type {}",
            kind.path
        );
        let adjustment = decode_terrain_adjustment(&config)?;
        let mut pieces = Vec::with_capacity(values.len());
        for value in values {
            pieces.push(match kind.path.as_str() {
                "buried_treasure" => treasure_piece(value)?,
                "swamp_hut" => {
                    RestoredStructurePiece::SwampHut(SavedSwampHutPiece::from_tag(value)?)
                }
                "mineshaft" => {
                    RestoredStructurePiece::Mineshaft(SavedMineshaftPiece::from_tag(value)?)
                }
                "jigsaw" => RestoredStructurePiece::Jigsaw(SavedJigsawPiece::from_tag(value)?),
                "ruined_portal" | "ocean_ruin" => {
                    let Some(Tag::String(name)) = field(value, "Template")? else {
                        bail!("saved {} template is missing", kind.path)
                    };
                    let name = String::from_utf16(&name.0)
                        .with_context(|| format!("invalid saved {} template", kind.path))?;
                    let template = ResourceId::parse(&name)?;
                    let size = if let Some(&size) = template_sizes.get(&template) {
                        size
                    } else {
                        let size = template_size(&template)?.with_context(|| {
                            format!("saved {} template {name} is missing", kind.path)
                        })?;
                        template_sizes.insert(template, size);
                        size
                    };
                    if kind.path == "ruined_portal" {
                        RestoredStructurePiece::RuinedPortal(
                            super::ruined_portal::SavedRuinedPortalPiece::from_tag(value, size)?,
                        )
                    } else {
                        RestoredStructurePiece::OceanRuin(
                            super::ocean_ruin::SavedOceanRuinPiece::from_tag(value, size)?,
                        )
                    }
                }
                _ => bail!("unsupported saved structure type {}", kind.path),
            });
        }
        result.insert(
            id,
            RestoredStructureStart::Present {
                chunk,
                references,
                adjustment,
                pieces,
                original: tag.clone(),
            },
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::{
        jigsaw_placement::Piece,
        mineshaft::{MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftVariant},
        structure_piece::{ScatteredGroundHeight, swamp_hut::SwampHutState},
        structure_pool::{LiquidSettings, TemplatePool},
        structure_start::HorizontalDirection,
        structure_transform::Rotation,
    };
    use serde_json::json;

    fn start(id: &str, pieces: Vec<Tag>) -> Tag {
        Tag::Compound(vec![
            ("id".into(), Tag::String(id.into())),
            ("ChunkX".into(), Tag::Int(-2)),
            ("ChunkZ".into(), Tag::Int(3)),
            ("references".into(), Tag::Int(2)),
            (
                "Children".into(),
                Tag::List {
                    element_type: if pieces.is_empty() {
                        TagType::End
                    } else {
                        TagType::Compound
                    },
                    values: pieces,
                },
            ),
        ])
    }

    #[test]
    fn treasure_draft_preserves_original_fields_and_rejects_invalid_changes() -> Result<()> {
        use native_resources::structure_start::{SimpleStructureKind, SimpleStructureStart};
        let piece = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:btp".into())),
            ("BB".into(), Tag::IntArray(vec![-23, 90, 57, -23, 90, 57])),
            ("O".into(), Tag::Int(-1)),
            ("GD".into(), Tag::Int(0)),
        ]);
        let mut original = start("example:treasure", vec![piece]);
        let Tag::Compound(fields) = &mut original else {
            unreachable!()
        };
        fields.push(("custom".into(), Tag::String("keep".into())));
        let untouched = original.clone();
        let id = ResourceId::parse("example:treasure")?;
        let pos = ChunkPos { x: -2, z: 3 };
        let placed = SimpleStructureStart {
            kind: SimpleStructureKind::BuriedTreasure,
            chunk: [-2, 3],
            generation_point: [0, 0, 0],
            orientation: None,
            bounds: BoundingBox::new([-23, 64, 57], [-23, 64, 57]).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let draft = draft_saved_treasure_start(&original, &id, pos, 2, &placed)?;
        let Tag::List { values, .. } = draft.get("Children").unwrap() else {
            panic!()
        };
        assert_eq!(
            values[0].get("BB"),
            Some(&Tag::IntArray(vec![-23, 64, 57, -23, 64, 57]))
        );
        assert_eq!(draft.get("custom"), original.get("custom"));
        assert_eq!(draft.get("references"), original.get("references"));
        assert_eq!(original, untouched);
        assert!(draft_saved_treasure_start(&original, &id, pos, 1, &placed).is_err());
        assert!(
            draft_saved_treasure_start(
                &original,
                &ResourceId::parse("example:other")?,
                pos,
                2,
                &placed
            )
            .is_err()
        );
        let mut moved = placed.clone();
        moved.bounds = BoundingBox::new([-22, 64, 57], [-22, 64, 57]).unwrap();
        assert!(draft_saved_treasure_start(&original, &id, pos, 2, &moved).is_err());
        let mut duplicate = original.clone();
        let Tag::Compound(fields) = &mut duplicate else {
            unreachable!()
        };
        fields.push(("references".into(), Tag::Int(2)));
        assert!(draft_saved_treasure_start(&duplicate, &id, pos, 2, &placed).is_err());
        Ok(())
    }

    #[test]
    fn restores_supported_pieces_bounds_and_original_nbt() -> Result<()> {
        let hut = SavedSwampHutPiece {
            bounds: BoundingBox::new([0, 70, 0], [6, 76, 8]).unwrap(),
            orientation: HorizontalDirection::South,
            state: SwampHutState {
                ground: ScatteredGroundHeight::from_saved(70),
                spawned_witch: true,
                spawned_cat: false,
            },
        };
        let mine = SavedMineshaftPiece {
            variant: MineshaftVariant::Mesa,
            placed_spider: true,
            piece: MineshaftGraphPiece {
                bounds: BoundingBox::new([10, 20, 30], [12, 22, 39]).unwrap(),
                direction: Some(HorizontalDirection::North),
                depth: 3,
                kind: MineshaftGraphPieceKind::Corridor {
                    rails: false,
                    spiders: true,
                    sections: 2,
                },
            },
        };
        let pool = TemplatePool::decode(
            &json!({"fallback":"minecraft:empty","elements":[{"weight":1,"element":{"element_type":"minecraft:single_pool_element","location":"example:house","processors":"minecraft:empty","projection":"rigid"}}]}),
        )?;
        let jigsaw = SavedJigsawPiece {
            piece: Piece {
                element: pool.entries()[0].0.clone(),
                origin: [1, 2, -3],
                rotation: Rotation::Clockwise90,
                bounds: BoundingBox::new([0, 2, -3], [1, 4, -1]).unwrap(),
                ground_level_delta: 1,
                junctions: vec![],
            },
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
        };
        let treasure = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:btp".into())),
            ("BB".into(), Tag::IntArray(vec![-23, 63, 57, -23, 63, 57])),
            ("O".into(), Tag::Int(-1)),
            ("GD".into(), Tag::Int(0)),
        ]);
        let data = Tag::Compound(vec![
            (
                "example:treasure".into(),
                start("example:treasure", vec![treasure]),
            ),
            (
                "example:hut".into(),
                start("example:hut", vec![hut.to_tag()?]),
            ),
            (
                "example:mine".into(),
                start("example:mine", vec![mine.to_tag()?]),
            ),
            (
                "example:jigsaw".into(),
                start("example:jigsaw", vec![jigsaw.to_tag()?]),
            ),
            (
                "example:invalid".into(),
                Tag::Compound(vec![("id".into(), Tag::String("INVALID".into()))]),
            ),
        ]);
        let config = |id: &ResourceId| -> Result<Option<Value>> {
            let kind = match id.path.as_str() {
                "treasure" => "buried_treasure",
                "hut" => "swamp_hut",
                "mine" => "mineshaft",
                "jigsaw" => "jigsaw",
                "invalid" => "jigsaw",
                _ => return Ok(None),
            };
            Ok(Some(
                json!({"type": format!("minecraft:{kind}"), "terrain_adaptation": "beard_thin"}),
            ))
        };
        let restored =
            restore_saved_structure_starts(ChunkPos { x: -2, z: 3 }, Some(&data), config)?;
        assert_eq!(restored.len(), 5);
        let id = ResourceId::parse("example:jigsaw")?;
        let RestoredStructureStart::Present {
            pieces,
            original,
            references,
            ..
        } = &restored[&id]
        else {
            panic!("start missing")
        };
        assert!(matches!(&pieces[0], RestoredStructurePiece::Jigsaw(piece) if piece == &jigsaw));
        assert_eq!(*references, 2);
        assert_eq!(original, data.get("example:jigsaw").unwrap());
        assert_eq!(
            restored[&id].reference_bounds(&id)?.unwrap().bounds,
            BoundingBox::new([-12, -10, -15], [13, 16, 11]).unwrap()
        );
        assert!(
            restored[&ResourceId::parse("example:invalid")?]
                .reference_bounds(&id)?
                .is_none()
        );
        assert!(
            restored[&ResourceId::parse("example:invalid")?]
                .terrain_pieces()?
                .is_empty()
        );
        for name in ["treasure", "hut", "mine"] {
            let terrain =
                restored[&ResourceId::parse(&format!("example:{name}"))?].terrain_pieces()?;
            assert_eq!(terrain.len(), 1);
            assert_eq!(terrain[0].projection, PieceProjection::NonPool);
            assert_eq!(terrain[0].ground_delta, 0);
            assert_eq!(terrain[0].adjustment, TerrainAdjustment::BeardThin);
            assert!(terrain[0].junctions.is_empty());
        }
        assert!(
            restore_saved_structure_starts(ChunkPos { x: -1, z: 3 }, Some(&data), config).is_err()
        );
        assert!(
            restore_saved_structure_starts(ChunkPos { x: -2, z: 3 }, Some(&data), |_| Ok(None))
                .is_err()
        );
        let mut unsupported = data.clone();
        let Tag::Compound(entries) = &mut unsupported else {
            unreachable!()
        };
        entries[0].1 = start("example:treasure", vec![Tag::Int(7)]);
        assert!(
            restore_saved_structure_starts(ChunkPos { x: -2, z: 3 }, Some(&unsupported), config)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn restores_ocean_ruin_pieces_in_order_and_rejects_lossy_data() -> Result<()> {
        let piece = |position: [i32; 3], bounds: [i32; 6], rotation: &str, temperature: &str| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:orp".into())),
                ("BB".into(), Tag::IntArray(bounds.into())),
                ("O".into(), Tag::Int(2)),
                ("GD".into(), Tag::Int(0)),
                ("TPX".into(), Tag::Int(position[0])),
                ("TPY".into(), Tag::Int(position[1])),
                ("TPZ".into(), Tag::Int(position[2])),
                (
                    "Template".into(),
                    Tag::String("minecraft:underwater_ruin/warm_1".into()),
                ),
                ("Rot".into(), Tag::String(rotation.into())),
                ("Integrity".into(), Tag::Float(0.7)),
                ("BiomeType".into(), Tag::String(temperature.into())),
                ("IsLarge".into(), Tag::Byte(1)),
            ])
        };
        let first = piece([-37, 90, 52], [-37, 90, 52, -31, 93, 56], "NONE", "WARM");
        let second = piece(
            [-37, 53, 52],
            [-41, 53, 52, -37, 56, 58],
            "CLOCKWISE_90",
            "COLD",
        );
        let mut original = start("example:ruin", vec![first, second]);
        let Tag::Compound(fields) = &mut original else {
            unreachable!()
        };
        fields.push(("custom".into(), Tag::String("keep".into())));
        let data = Tag::Compound(vec![("example:ruin".into(), original.clone())]);
        let id = ResourceId::parse("example:ruin")?;
        let pos = ChunkPos { x: -2, z: 3 };
        let config = |_: &ResourceId| -> Result<Option<Value>> {
            Ok(Some(json!({
                "type": "minecraft:ocean_ruin",
                "terrain_adaptation": "beard_thin"
            })))
        };
        let template = ResourceId::parse("minecraft:underwater_ruin/warm_1")?;
        let mut template_reads = 0;
        let restored =
            restore_saved_structure_starts_with_templates(pos, Some(&data), config, |id| {
                assert_eq!(id, &template);
                template_reads += 1;
                Ok(Some([7, 4, 5]))
            })?;
        assert_eq!(template_reads, 1);
        let RestoredStructureStart::Present {
            pieces,
            original: actual,
            references,
            ..
        } = &restored[&id]
        else {
            panic!("ruin start missing")
        };
        assert_eq!(actual, &original);
        assert_eq!(*references, 2);
        let [
            RestoredStructurePiece::OceanRuin(first),
            RestoredStructurePiece::OceanRuin(second),
        ] = pieces.as_slice()
        else {
            panic!("ruin piece order changed")
        };
        assert_eq!(first.selection.position, [-37, 90, 52]);
        assert_eq!(second.selection.position, [-37, 53, 52]);
        assert_eq!(
            first.temperature,
            native_resources::ocean_ruin::Temperature::Warm
        );
        assert_eq!(
            second.temperature,
            native_resources::ocean_ruin::Temperature::Cold
        );
        let terrain = restored[&id].terrain_pieces()?;
        assert_eq!(terrain.len(), 2);
        assert_eq!(terrain[0].bounds, first.bounds);
        assert_eq!(terrain[1].bounds, second.bounds);
        assert!(
            terrain
                .iter()
                .all(|piece| piece.projection == PieceProjection::NonPool
                    && piece.ground_delta == 0
                    && piece.junctions.is_empty())
        );
        assert!(
            restore_saved_structure_starts_with_templates(pos, Some(&data), config, |_| Ok(None))
                .is_err()
        );
        assert!(
            restore_saved_structure_starts_with_templates(pos, Some(&data), config, |_| Ok(Some(
                [8, 4, 5]
            )))
            .is_err()
        );
        let mut unsupported = data.clone();
        let Tag::Compound(starts) = &mut unsupported else {
            unreachable!()
        };
        let Tag::Compound(fields) = &mut starts[0].1 else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(name, _)| name.matches("Children"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(fields) = &mut values[1] else {
            unreachable!()
        };
        fields.push(("future".into(), Tag::Int(1)));
        assert!(
            restore_saved_structure_starts_with_templates(pos, Some(&unsupported), config, |_| Ok(
                Some([7, 4, 5])
            ))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn collected_references_visit_saved_starts_in_requested_order() -> Result<()> {
        use super::super::structure_references::collect_ordered_structure_references;

        let target = ChunkPos { x: 0, z: 0 };
        let a = ResourceId::parse("example:a")?;
        let b = ResourceId::parse("example:b")?;
        let bounds = BoundingBox::new([0, 0, 0], [15, 20, 15]).unwrap();
        let references = collect_ordered_structure_references(target, |source| {
            let mut found = Vec::new();
            if source == target || source == (ChunkPos { x: 1, z: 0 }) {
                found.push(StructureStartBounds {
                    structure: a.clone(),
                    bounds,
                });
            }
            if source == target {
                found.push(StructureStartBounds {
                    structure: b.clone(),
                    bounds,
                });
            }
            Ok(found)
        })?;
        let mut visited = Vec::new();
        assert!(
            beardifier_for_ordered_references(
                target,
                &references,
                std::slice::from_ref(&a),
                |_, _| {
                    panic!("incomplete structure order must be rejected before resolving starts")
                }
            )
            .is_err()
        );
        beardifier_for_ordered_references(
            target,
            &references,
            &[b.clone(), a.clone()],
            |id, source| {
                visited.push((id.clone(), source));
                Ok(Vec::new())
            },
        )?;
        let expected_a: Vec<_> = references
            .order(&a)
            .unwrap()
            .iter()
            .map(|&packed| ChunkPos {
                x: packed as u32 as i32,
                z: ((packed as u64) >> 32) as u32 as i32,
            })
            .collect();
        assert_eq!(visited[0], (b, target));
        assert_eq!(
            visited[1..].iter().map(|(_, pos)| *pos).collect::<Vec<_>>(),
            expected_a
        );
        Ok(())
    }

    #[test]
    fn inferred_beardifier_key_order_requires_order_independent_contributions() -> Result<()> {
        use super::super::structure_references::collect_ordered_structure_references;

        let target = ChunkPos { x: 0, z: 0 };
        let a = ResourceId::parse("example:a")?;
        let b = ResourceId::parse("example:b")?;
        let references = collect_ordered_structure_references(target, |source| {
            if source != target {
                return Ok(Vec::new());
            }
            let bounds = BoundingBox::new([0, 0, 0], [15, 20, 15]).unwrap();
            Ok(vec![
                StructureStartBounds {
                    structure: a.clone(),
                    bounds,
                },
                StructureStartBounds {
                    structure: b.clone(),
                    bounds,
                },
            ])
        })?;
        let piece = |y| StructurePiece {
            bounds: BoundingBox::new([2, y, 3], [5, y + 2, 7]).unwrap(),
            adjustment: TerrainAdjustment::BeardThin,
            ground_delta: 0,
            projection: PieceProjection::NonPool,
            junctions: Vec::new(),
        };
        let mut pieces = BTreeMap::from([(a.clone(), vec![piece(0)]), (b.clone(), vec![piece(4)])]);
        let infer = |pieces: &BTreeMap<ResourceId, Vec<StructurePiece>>| {
            unambiguous_beardifier_structure_order(target, &references, |id, pos| {
                ensure!(pos == target, "unexpected reference source");
                Ok(pieces.get(id).context("missing referenced start")?.clone())
            })
        };
        let order = infer(&pieces)?.context("two rigid contributions should commute")?;
        assert_eq!(order, vec![a.clone(), b.clone()]);
        let compare = |pieces: &BTreeMap<ResourceId, Vec<StructurePiece>>,
                       ys: std::ops::Range<i32>|
         -> Result<()> {
            let build = |order: &[ResourceId]| {
                beardifier_for_ordered_references(target, &references, order, |id, _| {
                    Ok(pieces.get(id).context("missing referenced start")?.clone())
                })
            };
            let forward = build(&[a.clone(), b.clone()])?;
            let reverse = build(&[b.clone(), a.clone()])?;
            for y in ys {
                for z in 0..16 {
                    for x in 0..16 {
                        assert_eq!(
                            forward.sample(x, y, z).to_bits(),
                            reverse.sample(x, y, z).to_bits(),
                            "terrain adaptation differs at {x},{y},{z}"
                        );
                    }
                }
            }
            Ok(())
        };
        compare(&pieces, -8..16)?;

        pieces.insert(a.clone(), vec![piece(0), piece(3)]);
        pieces.insert(b.clone(), vec![piece(6), piece(9)]);
        assert!(infer(&pieces)?.is_none());
        pieces.insert(b.clone(), vec![piece(100), piece(103)]);
        assert!(infer(&pieces)?.is_some());
        compare(&pieces, -8..120)?;

        pieces.remove(&b);
        assert!(infer(&pieces).is_err());
        Ok(())
    }
}
