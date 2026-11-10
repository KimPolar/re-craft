//! Structure-start references for a fresh chunk's structure-reference stage.
//!
//! This only indexes already generated starts. It does not generate pieces,
//! write chunks, or establish that structure generation is complete.
use std::collections::{BTreeMap, BTreeSet};

use super::structure_hash_order::{Insertion, StructureIdentityMap};
use anyhow::{Context, Result, ensure};
use native_core::beardifier::{BoundingBox, TerrainAdjustment, structure_start_bounds};
use native_persistence::nbt::{JavaString, Tag};
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

/// Find structure IDs whose actual piece boxes contain `pos`, as required by
/// StructureManager.getStructureWithPieceAt. The caller supplies every valid
/// referenced start and its complete piece list for the target chunk.
pub fn structures_with_piece_at<'a>(
    pos: [i32; 3],
    starts: impl IntoIterator<Item = (&'a ResourceId, &'a [BoundingBox])>,
) -> Result<BTreeSet<ResourceId>> {
    let mut result = BTreeSet::new();
    for (id, pieces) in starts {
        for piece in pieces {
            ensure!(
                (0..3).all(|axis| piece.min[axis] <= piece.max[axis]),
                "invalid structure piece bounds"
            );
            if (0..3).all(|axis| (piece.min[axis]..=piece.max[axis]).contains(&pos[axis])) {
                result.insert(id.clone());
            }
        }
    }
    Ok(result)
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

/// Both membership and fastutil iteration order for each structure key.
/// The order between distinct structure keys is still caller-supplied.
#[derive(Clone)]
pub struct OrderedStructureReferences {
    membership: StructureReferences,
    ordered: BTreeMap<ResourceId, Vec<i64>>,
    structure_order: Option<Vec<ResourceId>>,
}

impl OrderedStructureReferences {
    pub fn membership(&self) -> &StructureReferences {
        &self.membership
    }

    pub fn order(&self, structure: &ResourceId) -> Option<&[i64]> {
        self.ordered.get(structure).map(Vec::as_slice)
    }

    /// Present only when the caller supplied the Java structure identity hashes.
    pub fn structure_order(&self) -> Option<&[ResourceId]> {
        self.structure_order.as_deref()
    }

    pub(super) fn into_membership(self) -> StructureReferences {
        self.membership
    }
}

/// Information Java would discard while loading saved structure references.
/// The original NBT must remain available when deciding whether to save.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscardedSavedReference {
    InvalidCompound,
    UnknownStructure(JavaString),
    InvalidIdentifier(JavaString),
    InvalidArray(JavaString),
    OutsideRadius { structure: ResourceId, packed: i64 },
}

pub struct SavedStructureReferences {
    pub references: OrderedStructureReferences,
    pub discarded: Vec<DiscardedSavedReference>,
}

/// Rebuild Java's LongOpenHashSet from a saved `References` compound. The
/// supplied registry must be the one used to open this world. A missing or
/// non-compound field has the same empty view as Java; discarded data is
/// reported and the caller must retain the original NBT for lossless saves.
pub fn restore_saved_structure_references(
    target: ChunkPos,
    field: Option<&Tag>,
    mut registered: impl FnMut(&ResourceId) -> Result<bool>,
) -> Result<SavedStructureReferences> {
    let mut membership = StructureReferences::new();
    let mut ordered = BTreeMap::new();
    let mut discarded = Vec::new();
    let Some(Tag::Compound(entries)) = field else {
        if field.is_some() {
            discarded.push(DiscardedSavedReference::InvalidCompound);
        }
        return Ok(SavedStructureReferences {
            references: OrderedStructureReferences {
                membership,
                ordered,
                structure_order: None,
            },
            discarded,
        });
    };
    let mut seen = BTreeSet::new();
    for (name, value) in entries {
        let id = match String::from_utf16(&name.0)
            .ok()
            .and_then(|name| ResourceId::parse(&name).ok())
        {
            Some(id) => id,
            None => {
                discarded.push(DiscardedSavedReference::InvalidIdentifier(name.clone()));
                continue;
            }
        };
        ensure!(
            seen.insert(id.clone()),
            "ambiguous duplicate structure reference ID"
        );
        if !registered(&id)? {
            discarded.push(DiscardedSavedReference::UnknownStructure(name.clone()));
            continue;
        }
        let Tag::LongArray(values) = value else {
            discarded.push(DiscardedSavedReference::InvalidArray(name.clone()));
            continue;
        };
        let mut filtered = Vec::with_capacity(values.len());
        for &packed in values {
            let x = packed as u32 as i32;
            let z = ((packed as u64) >> 32) as u32 as i32;
            let dx = target.x.wrapping_sub(x).wrapping_abs();
            let dz = target.z.wrapping_sub(z).wrapping_abs();
            if dx.max(dz) > 8 {
                discarded.push(DiscardedSavedReference::OutsideRadius {
                    structure: id.clone(),
                    packed,
                });
            } else {
                filtered.push(packed);
            }
        }
        let mut set = ReferenceSet::with_expected(filtered.len())?;
        for packed in filtered {
            set.insert(packed);
        }
        let values = set.ordered();
        membership.insert(id.clone(), values.iter().copied().collect());
        ordered.insert(id, values);
    }
    Ok(SavedStructureReferences {
        references: OrderedStructureReferences {
            membership,
            ordered,
            structure_order: None,
        },
        discarded,
    })
}

/// Insert-only default fastutil LongOpenHashSet, as used by ChunkAccess.
struct ReferenceSet {
    slots: Vec<Option<u64>>,
    zero: bool,
    len: usize,
}

impl Default for ReferenceSet {
    fn default() -> Self {
        Self {
            slots: vec![None; 32],
            zero: false,
            len: 0,
        }
    }
}

impl ReferenceSet {
    fn with_expected(expected: usize) -> Result<Self> {
        let needed = expected
            .checked_mul(4)
            .and_then(|n| n.checked_add(2))
            .context("structure reference capacity overflow")?
            / 3;
        let size = needed
            .max(2)
            .checked_next_power_of_two()
            .context("structure reference capacity overflow")?;
        ensure!(size <= (1 << 30), "too many saved structure references");
        Ok(Self {
            slots: vec![None; size],
            zero: false,
            len: 0,
        })
    }
    fn find(&self, key: u64) -> usize {
        let h = key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let h = h ^ (h >> 32);
        let mut slot = (h ^ (h >> 16)) as usize & (self.slots.len() - 1);
        while self.slots[slot].is_some_and(|other| other != key) {
            slot = (slot + 1) & (self.slots.len() - 1);
        }
        slot
    }

    fn insert(&mut self, value: i64) {
        let key = value as u64;
        if key == 0 {
            if self.zero {
                return;
            }
            self.zero = true;
        } else {
            let slot = self.find(key);
            if self.slots[slot].is_some() {
                return;
            }
            self.slots[slot] = Some(key);
        }
        let previous = self.len;
        self.len += 1;
        if previous >= self.slots.len() * 3 / 4 {
            let size = self.slots.len() * 2;
            let old = std::mem::replace(&mut self.slots, vec![None; size]);
            for key in old.into_iter().rev().flatten() {
                let slot = self.find(key);
                self.slots[slot] = Some(key);
            }
        }
    }

    fn ordered(&self) -> Vec<i64> {
        let mut result = Vec::with_capacity(self.len);
        if self.zero {
            result.push(0);
        }
        result.extend(self.slots.iter().rev().flatten().map(|&key| key as i64));
        result
    }
}

/// Collect references from all 289 source chunks in Java's 8-chunk radius.
/// `starts` must return the complete set of valid starts for each source.
/// The returned map is built privately so a failed lookup cannot publish a
/// partially constructed reference set.
pub fn collect_structure_references(
    target: ChunkPos,
    starts: impl FnMut(ChunkPos) -> Result<Vec<StructureStartBounds>>,
) -> Result<StructureReferences> {
    Ok(collect_ordered_structure_references(target, starts)?.into_membership())
}

/// Preserve the per-structure reference order used by Java's fastutil set.
pub fn collect_ordered_structure_references(
    target: ChunkPos,
    starts: impl FnMut(ChunkPos) -> Result<Vec<StructureStartBounds>>,
) -> Result<OrderedStructureReferences> {
    collect_with_hashes(target, starts, None)
}

/// Reproduce Java's structure-key map order with hashes observed from the same
/// Java process. Starts for each source must be supplied in insertion order.
pub fn collect_references_with_identity_hashes(
    target: ChunkPos,
    starts: impl FnMut(ChunkPos) -> Result<Vec<StructureStartBounds>>,
    hashes: &BTreeMap<ResourceId, i32>,
) -> Result<OrderedStructureReferences> {
    collect_with_hashes(target, starts, Some(hashes))
}

fn collect_with_hashes(
    target: ChunkPos,
    mut starts: impl FnMut(ChunkPos) -> Result<Vec<StructureStartBounds>>,
    hashes: Option<&BTreeMap<ResourceId, i32>>,
) -> Result<OrderedStructureReferences> {
    let min_x = target.x.checked_sub(8).context("chunk X underflow")?;
    let max_x = target.x.checked_add(8).context("chunk X overflow")?;
    let min_z = target.z.checked_sub(8).context("chunk Z underflow")?;
    let max_z = target.z.checked_add(8).context("chunk Z overflow")?;
    let block_x = target.x.checked_mul(16).context("block X overflow")?;
    let block_z = target.z.checked_mul(16).context("block Z overflow")?;
    let end_x = block_x.checked_add(15).context("block X overflow")?;
    let end_z = block_z.checked_add(15).context("block Z overflow")?;
    let mut references = BTreeMap::<ResourceId, ReferenceSet>::new();
    let mut identities = hashes.map(StructureIdentityMap::new);
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            let source = ChunkPos { x, z };
            let source_starts =
                starts(source).with_context(|| format!("missing starts at {x}, {z}"))?;
            let source_starts = if let Some(hashes) = hashes {
                let mut index = StructureIdentityMap::new(hashes);
                let mut by_key = BTreeMap::new();
                for start in source_starts {
                    index.insert(&start.structure, Insertion::Put)?;
                    ensure!(
                        by_key.insert(start.structure.clone(), start).is_none(),
                        "duplicate structure start for one source chunk"
                    );
                }
                index
                    .order()
                    .into_iter()
                    .map(|id| by_key.remove(&id).expect("indexed structure start"))
                    .collect()
            } else {
                source_starts
            };
            for start in source_starts {
                ensure!(
                    (0..3).all(|axis| start.bounds.min[axis] <= start.bounds.max[axis]),
                    "invalid structure start bounds at {x}, {z}"
                );
                if start.bounds.max[0] >= block_x
                    && start.bounds.min[0] <= end_x
                    && start.bounds.max[2] >= block_z
                    && start.bounds.min[2] <= end_z
                {
                    if let Some(identities) = &mut identities {
                        identities.insert(&start.structure, Insertion::ComputeIfAbsent)?;
                    }
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
    let mut membership = StructureReferences::new();
    let mut ordered = BTreeMap::new();
    for (structure, references) in references {
        let values = references.ordered();
        membership.insert(structure.clone(), values.iter().copied().collect());
        ordered.insert(structure, values);
    }
    Ok(OrderedStructureReferences {
        membership,
        ordered,
        structure_order: identities.map(|map| map.order()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_references_report_discarded_data_and_rebuild_sets() {
        let target = ChunkPos { x: 0, z: 0 };
        let id = ResourceId::parse("minecraft:buried_treasure").unwrap();
        let data = Tag::Compound(vec![
            (
                "minecraft:buried_treasure".into(),
                Tag::LongArray(vec![0, 1, 1, -1, 9]),
            ),
            ("example:unknown".into(), Tag::LongArray(vec![0])),
            ("minecraft:swamp_hut".into(), Tag::Int(7)),
        ]);
        let restored = restore_saved_structure_references(target, Some(&data), |id| {
            Ok(id.namespace == "minecraft")
        })
        .unwrap();
        assert_eq!(
            restored.references.membership()[&id],
            BTreeSet::from([0, 1, -1])
        );
        assert_eq!(restored.references.order(&id).unwrap().len(), 3);
        assert!(restored.references.structure_order().is_none());
        assert_eq!(restored.discarded.len(), 3);
        assert!(matches!(
            restored.discarded[0],
            DiscardedSavedReference::OutsideRadius { packed: 9, .. }
        ));
        assert!(matches!(
            restored.discarded[1],
            DiscardedSavedReference::UnknownStructure(_)
        ));
        assert!(matches!(
            restored.discarded[2],
            DiscardedSavedReference::InvalidArray(_)
        ));
        let aliases = Tag::Compound(vec![
            ("buried_treasure".into(), Tag::LongArray(vec![])),
            ("minecraft:buried_treasure".into(), Tag::LongArray(vec![])),
        ]);
        assert!(restore_saved_structure_references(target, Some(&aliases), |_| Ok(true)).is_err());
        assert!(
            restore_saved_structure_references(target, Some(&data), |_| anyhow::bail!(
                "registry error"
            ))
            .is_err()
        );
        assert!(
            restore_saved_structure_references(target, None, |_| Ok(true))
                .unwrap()
                .references
                .membership()
                .is_empty()
        );
        assert_eq!(
            restore_saved_structure_references(target, Some(&Tag::Int(7)), |_| Ok(true))
                .unwrap()
                .discarded,
            vec![DiscardedSavedReference::InvalidCompound]
        );
    }

    #[test]
    fn piece_membership_checks_full_piece_bounds_and_validates_every_box() {
        let id = ResourceId::parse("minecraft:swamp_hut").unwrap();
        let boxes = [BoundingBox::new([3, 10, -3], [9, 16, 5]).unwrap()];
        assert!(
            structures_with_piece_at([3, 10, -3], [(&id, boxes.as_slice())])
                .unwrap()
                .contains(&id)
        );
        assert!(
            structures_with_piece_at([9, 16, 5], [(&id, boxes.as_slice())])
                .unwrap()
                .contains(&id)
        );
        assert!(
            structures_with_piece_at([10, 16, 5], [(&id, boxes.as_slice())])
                .unwrap()
                .is_empty()
        );
        let invalid = [BoundingBox {
            min: [0, 0, 0],
            max: [-1, 2, 2],
        }];
        assert!(structures_with_piece_at([100, 100, 100], [(&id, invalid.as_slice())]).is_err());
    }
    use anyhow::bail;
    use native_resources::structure_start::SimpleStructureKind;

    #[test]
    fn ordered_references_deduplicate_zero_and_keep_membership() {
        let id = ResourceId::parse("minecraft:buried_treasure").unwrap();
        let start = StructureStartBounds {
            structure: id.clone(),
            bounds: BoundingBox::new([0, 0, 0], [15, 1, 15]).unwrap(),
        };
        let refs = collect_ordered_structure_references(ChunkPos { x: 0, z: 0 }, |pos| {
            Ok(if pos == (ChunkPos { x: 0, z: 0 }) {
                vec![start.clone(), start.clone()]
            } else {
                vec![]
            })
        })
        .unwrap();
        assert_eq!(refs.order(&id), Some([0].as_slice()));
        assert_eq!(refs.membership()[&id], BTreeSet::from([0]));
        assert_eq!(
            refs.order(&ResourceId::parse("minecraft:stronghold").unwrap()),
            None
        );
    }

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
