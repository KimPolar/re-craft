//! Structure-start references for a fresh chunk's structure-reference stage.
//!
//! This only indexes already generated starts. It does not generate pieces,
//! write chunks, or establish that structure generation is complete.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use native_core::beardifier::{BoundingBox, TerrainAdjustment, structure_start_bounds};
use native_resources::packs::ResourceId;
use native_resources::structure_start::SimpleStructureStart;
use native_world::loaded::ChunkPos;

/// A valid start and its complete, structure-adjusted bounding box. Invalid
/// starts must be omitted; unknown or incomplete starts must return an error.
#[derive(Clone, Debug)]
pub struct StructureStartBounds {
    pub structure: ResourceId,
    pub bounds: BoundingBox,
}

impl StructureStartBounds {
    /// Construct the complete reference bounds from all generated pieces.
    pub fn from_pieces(
        structure: ResourceId,
        pieces: &[BoundingBox],
        adjustment: TerrainAdjustment,
    ) -> Result<Self> {
        Ok(Self {
            structure,
            bounds: structure_start_bounds(pieces, adjustment).map_err(anyhow::Error::msg)?,
        })
    }

    /// Simple starts currently contain exactly one non-pool piece.
    pub fn from_simple(structure: ResourceId, start: &SimpleStructureStart) -> Result<Self> {
        Self::from_pieces(structure, &[start.bounds], start.terrain_adjustment)
    }
}

/// Packed source chunk coordinates grouped by structure registry key.
/// The sorted iteration order is not Java's hash-set iteration order; callers
/// must not use it as the generation or decoration order.
pub type StructureReferences = BTreeMap<ResourceId, BTreeSet<i64>>;

/// Collect references from all 289 source chunks in Java's 8-chunk radius.
/// `starts` must return the complete set of valid starts for each source.
/// The returned map is built privately so a failed lookup cannot publish a
/// partially constructed reference set.
pub fn collect_structure_references(
    target: ChunkPos,
    mut starts: impl FnMut(ChunkPos) -> Result<Vec<StructureStartBounds>>,
) -> Result<StructureReferences> {
    let min_x = target.x.checked_sub(8).context("chunk X underflow")?;
    let max_x = target.x.checked_add(8).context("chunk X overflow")?;
    let min_z = target.z.checked_sub(8).context("chunk Z underflow")?;
    let max_z = target.z.checked_add(8).context("chunk Z overflow")?;
    let block_x = target.x.checked_mul(16).context("block X overflow")?;
    let block_z = target.z.checked_mul(16).context("block Z overflow")?;
    let end_x = block_x.checked_add(15).context("block X overflow")?;
    let end_z = block_z.checked_add(15).context("block Z overflow")?;
    let mut references = StructureReferences::new();
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            let source = ChunkPos { x, z };
            for start in starts(source).with_context(|| format!("missing starts at {x}, {z}"))? {
                ensure!(
                    (0..3).all(|axis| start.bounds.min[axis] <= start.bounds.max[axis]),
                    "invalid structure start bounds at {x}, {z}"
                );
                if start.bounds.max[0] >= block_x
                    && start.bounds.min[0] <= end_x
                    && start.bounds.max[2] >= block_z
                    && start.bounds.min[2] <= end_z
                {
                    // ChunkPos.pack keeps X in the low 32 bits and Z in the high 32.
                    let packed = ((z as u32 as u64) << 32 | x as u32 as u64) as i64;
                    references
                        .entry(start.structure)
                        .or_default()
                        .insert(packed);
                }
            }
        }
    }
    Ok(references)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use native_resources::structure_start::SimpleStructureKind;

    #[test]
    fn simple_starts_feed_reference_bounds_with_adaptation() {
        let start = SimpleStructureStart {
            kind: SimpleStructureKind::SwampHut,
            chunk: [0, 0],
            generation_point: [0, 0, 0],
            orientation: None,
            bounds: BoundingBox::new([16, 20, 0], [20, 25, 4]).unwrap(),
            terrain_adjustment: TerrainAdjustment::BeardThin,
        };
        let reference =
            StructureStartBounds::from_simple(ResourceId::parse("example:hut").unwrap(), &start)
                .unwrap();
        let result = collect_structure_references(ChunkPos { x: 0, z: 0 }, |pos| {
            Ok(if pos == (ChunkPos { x: 1, z: 0 }) {
                vec![reference.clone()]
            } else {
                vec![]
            })
        })
        .unwrap();
        assert_eq!(
            result[&ResourceId::parse("example:hut").unwrap()],
            BTreeSet::from([1])
        );
    }

    #[test]
    fn scans_complete_neighborhood_and_rejects_missing_or_invalid_starts() {
        let target = ChunkPos { x: -1, z: 2 };
        let mut count = 0;
        let result = collect_structure_references(target, |_| {
            count += 1;
            Ok(vec![])
        })
        .unwrap();
        assert_eq!(count, 289);
        assert!(result.is_empty());
        assert!(
            collect_structure_references(target, |pos| {
                if pos == target {
                    bail!("not generated")
                } else {
                    Ok(vec![])
                }
            })
            .is_err()
        );
        let invalid = StructureStartBounds {
            structure: ResourceId::parse("minecraft:buried_treasure").unwrap(),
            bounds: BoundingBox {
                min: [0, 1, 0],
                max: [1, 0, 1],
            },
        };
        assert!(collect_structure_references(target, |_| Ok(vec![invalid.clone()])).is_err());
        assert!(
            collect_structure_references(ChunkPos { x: i32::MAX, z: 0 }, |_| Ok(vec![])).is_err()
        );
    }
}
