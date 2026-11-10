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
    structure_references::{SavedStructureReferences, StructureStartBounds},
};

#[derive(Clone, Debug)]
pub enum RestoredStructurePiece {
    BuriedTreasure(BoundingBox),
    SwampHut(SavedSwampHutPiece),
    Mineshaft(SavedMineshaftPiece),
    Jigsaw(SavedJigsawPiece),
}

impl RestoredStructurePiece {
    pub fn bounds(&self) -> BoundingBox {
        match self {
            Self::BuriedTreasure(bounds) => *bounds,
            Self::SwampHut(piece) => piece.bounds,
            Self::Mineshaft(piece) => piece.piece.bounds,
            Self::Jigsaw(piece) => piece.piece.bounds,
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
    let keys: BTreeSet<_> = references.references.membership().keys().cloned().collect();
    let order: BTreeSet<_> = structure_order.iter().cloned().collect();
    ensure!(
        order.len() == structure_order.len() && order == keys,
        "structure order does not cover references exactly"
    );
    let mut pieces = Vec::new();
    for id in structure_order {
        for &packed in references
            .references
            .order(id)
            .context("missing saved reference order")?
        {
            let source = ChunkPos {
                x: packed as u32 as i32,
                z: ((packed as u64) >> 32) as u32 as i32,
            };
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
            pieces.extend(start.terrain_pieces()?);
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

/// Decode starts for one captured chunk using the active structure registry.
/// Unsupported types and lossy piece encodings fail without publishing partial
/// results. Retained NBT does not authorize modifying an existing world.
pub fn restore_saved_structure_starts(
    position: ChunkPos,
    starts_field: Option<&Tag>,
    mut definition: impl FnMut(&ResourceId) -> Result<Option<Value>>,
) -> Result<BTreeMap<ResourceId, RestoredStructureStart>> {
    let mut result = BTreeMap::new();
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
                "buried_treasure" | "swamp_hut" | "mineshaft" | "jigsaw"
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
}
