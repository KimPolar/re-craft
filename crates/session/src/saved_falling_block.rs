//! Decode saved falling-block entities without discarding unknown NBT.

use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use glam::DVec3;
use native_persistence::nbt::{NamedTag, Tag, TagType};
use native_resources::packs::ResourceId;
use native_world::loaded::BlockState;

use crate::{player_data, saved_item, saved_orb};

#[derive(Clone, Debug)]
pub struct SavedFallingBlock {
    pub source: Arc<Tag>,
    pub uuid: Option<[i32; 4]>,
    pub position: DVec3,
    pub velocity: DVec3,
    pub on_ground: bool,
    pub no_gravity: bool,
    pub fall_distance: f64,
    pub block: BlockState,
    /// String encoding denotes the registry's default state; resolve defaults
    /// before using properties for a block that has any.
    pub block_uses_default: bool,
    pub time: i32,
    pub drop_item: bool,
    pub cancel_drop: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FallingSaveState {
    pub position: DVec3,
    pub velocity: DVec3,
    pub fall_distance: f64,
    pub on_ground: bool,
    pub no_gravity: bool,
    pub time: i32,
    pub drop_item: bool,
    pub cancel_drop: bool,
}

pub struct FallingBlockDraft {
    source_nbt: Vec<u8>,
    document: NamedTag,
}

pub struct CreatedFallingBlockDraft {
    document: NamedTag,
}

impl CreatedFallingBlockDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
}

/// Construct an entity from the known ordinary falling-block state. Its source
/// block removal, movement and other effects must be saved together.
pub fn draft_created(
    uuid: [i32; 4],
    block: &str,
    state: FallingSaveState,
) -> Result<CreatedFallingBlockDraft> {
    ensure!(
        matches!(
            block,
            "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
        ),
        "unsupported falling block"
    );
    ensure!(
        state.position.is_finite()
            && state.position.x.abs() <= 30_000_512.0
            && state.position.z.abs() <= 30_000_512.0
            && state.position.y.abs() <= 20_000_000.0,
        "invalid position"
    );
    ensure!(
        state.velocity.is_finite() && state.velocity.abs().max_element() <= 10.0,
        "invalid velocity"
    );
    ensure!(
        state.fall_distance.is_finite() && state.fall_distance >= 0.0,
        "invalid fall distance"
    );
    let vector = |v: DVec3| Tag::List {
        element_type: TagType::Double,
        values: v.to_array().into_iter().map(Tag::Double).collect(),
    };
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:falling_block".into())),
            ("UUID".into(), Tag::IntArray(uuid.to_vec())),
            ("Pos".into(), vector(state.position)),
            ("Motion".into(), vector(state.velocity)),
            (
                "Rotation".into(),
                Tag::List {
                    element_type: TagType::Float,
                    values: vec![Tag::Float(0.0), Tag::Float(0.0)],
                },
            ),
            ("fall_distance".into(), Tag::Double(state.fall_distance)),
            ("Air".into(), Tag::Short(300)),
            ("Fire".into(), Tag::Short(0)),
            ("PortalCooldown".into(), Tag::Int(0)),
            ("OnGround".into(), Tag::Byte(i8::from(state.on_ground))),
            ("NoGravity".into(), Tag::Byte(i8::from(state.no_gravity))),
            ("BlockState".into(), Tag::String(block.into())),
            ("Time".into(), Tag::Int(state.time)),
            ("DropItem".into(), Tag::Byte(i8::from(state.drop_item))),
            ("CancelDrop".into(), Tag::Byte(i8::from(state.cancel_drop))),
            ("HurtEntities".into(), Tag::Byte(0)),
            ("FallHurtAmount".into(), Tag::Float(0.0)),
            ("FallHurtMax".into(), Tag::Int(40)),
        ]),
    };
    ensure!(
        read(&document.value)?.is_some(),
        "cannot decode created falling block"
    );
    native_persistence::nbt::write(&document)?;
    Ok(CreatedFallingBlockDraft { document })
}

impl FallingBlockDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn matches_source(&self, current: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(current)? == self.source_nbt)
    }
}

pub struct FallingEntityChunkDraft {
    source_nbt: Option<Vec<u8>>,
    document: NamedTag,
}

impl FallingEntityChunkDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn matches_source(&self, current: &NamedTag) -> Result<bool> {
        match &self.source_nbt {
            Some(source) => Ok(native_persistence::nbt::write(current)? == *source),
            None => Ok(false),
        }
    }
    /// The source chunk did not exist; callers must check it is still absent.
    pub fn source_was_absent(&self) -> bool {
        self.source_nbt.is_none()
    }

    /// Add this prepared chunk to the partial world transaction. All source and
    /// destination chunks and other world changes still require coordination.
    pub fn add_to_save_plan(
        &self,
        plan: &mut native_persistence::save::WorldSavePlan,
        dimension: native_persistence::world::DimensionId,
        position: native_persistence::world::ChunkPos,
        source: Option<&NamedTag>,
        timestamp: u32,
    ) -> Result<()> {
        plan.add_entity_chunk(
            dimension,
            position,
            source,
            &self.document,
            timestamp,
            |source, _| {
                match source {
                    Some(source) => ensure!(
                        self.matches_source(source)?,
                        "entity chunk draft source differs"
                    ),
                    None => ensure!(
                        self.source_was_absent(),
                        "entity chunk draft source differs"
                    ),
                }
                Ok(())
            },
        )
    }
}

pub enum EntityReplacement<'a> {
    Falling(&'a FallingBlockDraft),
    Item(&'a saved_item::ItemDraft),
    Orb(&'a saved_orb::OrbDraft),
}

impl EntityReplacement<'_> {
    fn document(&self) -> &NamedTag {
        match self {
            Self::Falling(draft) => draft.document(),
            Self::Item(draft) => draft.document(),
            Self::Orb(draft) => draft.document(),
        }
    }
    fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        match self {
            Self::Falling(draft) => draft.matches_source(source),
            Self::Item(draft) => draft.matches_source(source),
            Self::Orb(draft) => draft.matches_source(source),
        }
    }
    fn position(&self, tag: &Tag) -> Result<DVec3> {
        match self {
            Self::Falling(_) => Ok(read(tag)?
                .context("entity is not a falling block")?
                .position),
            Self::Item(_) => Ok(saved_item::read(tag)?
                .context("entity is not an item")?
                .position),
            Self::Orb(_) => Ok(saved_orb::read(tag)?
                .context("entity is not an experience orb")?
                .position),
        }
    }
}

/// Apply saved-entity replacements inside one existing entity chunk. Moving,
/// adding or removing entities requires a coordinated multi-chunk save.
pub fn draft_entity_chunk(
    original: &NamedTag,
    position: native_world::loaded::ChunkPos,
    replacements: &[FallingBlockDraft],
) -> Result<FallingEntityChunkDraft> {
    let replacements: Vec<_> = replacements
        .iter()
        .map(EntityReplacement::Falling)
        .collect();
    draft_entity_chunk_with_items(original, position, &replacements)
}

/// Apply falling-block and item replacements together. This still does not
/// authorize writing: other entities and cross-chunk changes need coordination.
pub fn draft_entity_chunk_with_items(
    original: &NamedTag,
    position: native_world::loaded::ChunkPos,
    replacements: &[EntityReplacement<'_>],
) -> Result<FallingEntityChunkDraft> {
    let Tag::Compound(fields) = &original.value else {
        anyhow::bail!("entity chunk must be a compound")
    };
    for key in ["DataVersion", "Position", "Entities"] {
        ensure!(
            fields.iter().filter(|(name, _)| name.matches(key)).count() <= 1,
            "duplicate entity chunk {key}"
        );
    }
    ensure!(
        matches!(original.value.get("DataVersion"), Some(Tag::Int(5023))),
        "unsupported entity chunk data version"
    );
    ensure!(
        matches!(original.value.get("Position"), Some(Tag::IntArray(values)) if values.as_slice() == [position.x, position.z]),
        "entity chunk position is unsupported or mismatched"
    );
    let Some(Tag::List {
        element_type: TagType::Compound,
        values,
    }) = original.value.get("Entities")
    else {
        anyhow::bail!("entity chunk must contain a compound entity list");
    };
    let mut requested = BTreeMap::new();
    for replacement in replacements {
        let uuid = saved_item::optional_uuid(&replacement.document().value, "UUID")?
            .context("replacement has no UUID")?;
        ensure!(
            requested.insert(uuid, replacement).is_none(),
            "duplicate replacement UUID"
        );
        let entity_pos = replacement.position(&replacement.document().value)?;
        let x = (entity_pos.x.floor() as i32).div_euclid(16);
        let z = (entity_pos.z.floor() as i32).div_euclid(16);
        ensure!(
            x == position.x && z == position.z,
            "entity moved to a different chunk"
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut matched = std::collections::BTreeSet::new();
    let mut entities = Vec::with_capacity(values.len());
    for entity in values {
        let Tag::Compound(entity_fields) = entity else {
            anyhow::bail!("entity list contains a non-compound")
        };
        ensure!(
            entity_fields
                .iter()
                .filter(|(name, _)| name.matches("UUID"))
                .count()
                <= 1,
            "duplicate entity UUID fields"
        );
        let uuid = saved_item::optional_uuid(entity, "UUID")?;
        if let Some(uuid) = uuid {
            ensure!(seen.insert(uuid), "duplicate entity UUID in chunk");
            if let Some(replacement) = requested.get(&uuid) {
                let named = NamedTag {
                    name: "".into(),
                    value: entity.clone(),
                };
                ensure!(replacement.matches_source(&named)?, "entity source changed");
                let previous = replacement.position(entity)?;
                ensure!(
                    (previous.x.floor() as i32).div_euclid(16) == position.x
                        && (previous.z.floor() as i32).div_euclid(16) == position.z,
                    "source entity position is outside its chunk"
                );
                entities.push(replacement.document().value.clone());
                matched.insert(uuid);
                continue;
            }
        }
        entities.push(entity.clone());
    }
    ensure!(
        matched.len() == requested.len(),
        "replacement entity is missing from the source chunk"
    );
    let source_nbt = native_persistence::nbt::write(original)?;
    let mut document = original.clone();
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    let (_, list) = fields
        .iter_mut()
        .find(|(name, _)| name.matches("Entities"))
        .unwrap();
    *list = Tag::List {
        element_type: TagType::Compound,
        values: entities,
    };
    Ok(FallingEntityChunkDraft {
        source_nbt: Some(source_nbt),
        document,
    })
}

/// Also allow a known-absent source chunk as a destination for moved entities.
/// An absent source must still be absent when a future transaction is committed.
/// A new empty result represents no entity chunk, as in Java's entity storage.
pub fn draft_entity_chunks_with_optional_sources(
    originals: &BTreeMap<native_world::loaded::ChunkPos, Option<&NamedTag>>,
    replacements: &[EntityReplacement<'_>],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    draft_entity_chunks_with_removals(originals, replacements, &[])
}

/// Remove explicitly tracked saved items while coordinating all source and
/// destination chunks. The caller must also save the pickup or merge effects.
pub fn draft_entity_chunks_with_removals(
    originals: &BTreeMap<native_world::loaded::ChunkPos, Option<&NamedTag>>,
    replacements: &[EntityReplacement<'_>],
    removed_items: &[saved_item::ItemDraft],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    draft_entity_chunks_with_item_changes(originals, replacements, removed_items, &[])
}

/// Coordinate verified item creations, removals and existing entity updates.
/// The caller must also include the player, block and other related changes.
pub fn draft_entity_chunks_with_item_changes(
    originals: &BTreeMap<native_world::loaded::ChunkPos, Option<&NamedTag>>,
    replacements: &[EntityReplacement<'_>],
    removed_items: &[saved_item::ItemDraft],
    created_items: &[saved_item::CreatedItemDraft],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    draft_entity_chunks_with_item_and_orb_changes(
        originals,
        replacements,
        removed_items,
        &[],
        created_items,
    )
}

/// Coordinate tracked saved-orb pickups with item and entity changes. The
/// caller must include player experience and equipment effects in the same save.
pub fn draft_entity_chunks_with_item_and_orb_changes(
    originals: &BTreeMap<native_world::loaded::ChunkPos, Option<&NamedTag>>,
    replacements: &[EntityReplacement<'_>],
    removed_items: &[saved_item::ItemDraft],
    removed_orbs: &[saved_orb::OrbDraft],
    created_items: &[saved_item::CreatedItemDraft],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    draft_entity_chunks_with_created_orbs(
        originals,
        replacements,
        removed_items,
        removed_orbs,
        created_items,
        &[],
    )
}

pub fn draft_entity_chunks_with_created_orbs(
    originals: &BTreeMap<native_world::loaded::ChunkPos, Option<&NamedTag>>,
    replacements: &[EntityReplacement<'_>],
    removed_items: &[saved_item::ItemDraft],
    removed_orbs: &[saved_orb::OrbDraft],
    created_items: &[saved_item::CreatedItemDraft],
    created_orbs: &[saved_orb::CreatedOrbDraft],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    draft_entity_chunks_with_falling_removals(
        originals,
        replacements,
        &[],
        removed_items,
        removed_orbs,
        created_items,
        created_orbs,
    )
}

/// The caller must also include landing blocks, drops and all related changes.
pub fn draft_entity_chunks_with_falling_removals(
    originals: &BTreeMap<native_world::loaded::ChunkPos, Option<&NamedTag>>,
    replacements: &[EntityReplacement<'_>],
    removed_falling: &[FallingBlockDraft],
    removed_items: &[saved_item::ItemDraft],
    removed_orbs: &[saved_orb::OrbDraft],
    created_items: &[saved_item::CreatedItemDraft],
    created_orbs: &[saved_orb::CreatedOrbDraft],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    draft_entity_chunks_with_created_falling(
        originals,
        replacements,
        removed_falling,
        removed_items,
        removed_orbs,
        created_items,
        created_orbs,
        &[],
    )
}

/// Include newly spawned falling entities with all their related world changes.
pub fn draft_entity_chunks_with_created_falling(
    originals: &BTreeMap<native_world::loaded::ChunkPos, Option<&NamedTag>>,
    replacements: &[EntityReplacement<'_>],
    removed_falling: &[FallingBlockDraft],
    removed_items: &[saved_item::ItemDraft],
    removed_orbs: &[saved_orb::OrbDraft],
    created_items: &[saved_item::CreatedItemDraft],
    created_orbs: &[saved_orb::CreatedOrbDraft],
    created_falling: &[CreatedFallingBlockDraft],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    let empty: BTreeMap<_, _> = originals
        .iter()
        .filter(|(_, source)| source.is_none())
        .map(|(&pos, _)| {
            (
                pos,
                NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![
                        (
                            "DataVersion".into(),
                            Tag::Int(native_persistence::world::TARGET_DATA_VERSION),
                        ),
                        ("Position".into(), Tag::IntArray(vec![pos.x, pos.z])),
                        (
                            "Entities".into(),
                            Tag::List {
                                element_type: TagType::Compound,
                                values: Vec::new(),
                            },
                        ),
                    ]),
                },
            )
        })
        .collect();
    let complete = originals
        .iter()
        .map(|(&pos, source)| (pos, source.unwrap_or_else(|| &empty[&pos])))
        .collect();
    let removals: Vec<_> = removed_falling
        .iter()
        .map(EntityReplacement::Falling)
        .chain(removed_items.iter().map(EntityReplacement::Item))
        .chain(removed_orbs.iter().map(EntityReplacement::Orb))
        .collect();
    let created: Vec<_> = created_items
        .iter()
        .map(CreatedEntity::Item)
        .chain(created_orbs.iter().map(CreatedEntity::Orb))
        .chain(created_falling.iter().map(CreatedEntity::Falling))
        .collect();
    let mut drafts =
        draft_entity_chunks_with_moves_and_removals(&complete, replacements, &removals, &created)?;
    for pos in empty.keys() {
        drafts
            .get_mut(pos)
            .expect("empty chunk was included")
            .source_nbt = None;
    }
    Ok(drafts)
}

/// Prepare existing entity chunks together, allowing supported entities to move
/// between them. Every source and destination must be supplied; the caller must
/// also account for creations, removals, and all other world state before saving.
pub fn draft_entity_chunks_with_moves(
    originals: &BTreeMap<native_world::loaded::ChunkPos, &NamedTag>,
    replacements: &[EntityReplacement<'_>],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    draft_entity_chunks_with_moves_and_removals(originals, replacements, &[], &[])
}

enum CreatedEntity<'a> {
    Item(&'a saved_item::CreatedItemDraft),
    Orb(&'a saved_orb::CreatedOrbDraft),
    Falling(&'a CreatedFallingBlockDraft),
}

impl CreatedEntity<'_> {
    fn document(&self) -> &NamedTag {
        match self {
            Self::Item(value) => value.document(),
            Self::Orb(value) => value.document(),
            Self::Falling(value) => value.document(),
        }
    }
    fn position(&self) -> Result<DVec3> {
        match self {
            Self::Item(value) => Ok(saved_item::read(&value.document().value)?
                .context("created entity is not an item")?
                .position),
            Self::Orb(value) => Ok(saved_orb::read(&value.document().value)?
                .context("created entity is not an orb")?
                .position),
            Self::Falling(value) => Ok(read(&value.document().value)?
                .context("created entity is not a falling block")?
                .position),
        }
    }
}

fn draft_entity_chunks_with_moves_and_removals(
    originals: &BTreeMap<native_world::loaded::ChunkPos, &NamedTag>,
    replacements: &[EntityReplacement<'_>],
    removed_entities: &[EntityReplacement<'_>],
    created_entities: &[CreatedEntity<'_>],
) -> Result<BTreeMap<native_world::loaded::ChunkPos, FallingEntityChunkDraft>> {
    use std::collections::BTreeSet;

    let mut drafts = BTreeMap::new();
    let mut owners = BTreeMap::new();
    for (&position, original) in originals {
        let draft = draft_entity_chunk_with_items(original, position, &[])?;
        let Some(Tag::List { values, .. }) = original.value.get("Entities") else {
            unreachable!("validated entity chunk")
        };
        for entity in values {
            if let Some(uuid) = saved_item::optional_uuid(entity, "UUID")? {
                ensure!(
                    owners.insert(uuid, position).is_none(),
                    "duplicate entity UUID across chunks"
                );
            } else {
                ensure!(
                    created_entities.is_empty(),
                    "source entity has no stable UUID"
                );
            }
        }
        drafts.insert(position, draft);
    }
    let mut requested = BTreeMap::new();
    for replacement in replacements {
        let uuid = saved_item::optional_uuid(&replacement.document().value, "UUID")?
            .context("replacement has no UUID")?;
        ensure!(
            requested.insert(uuid, replacement).is_none(),
            "duplicate replacement UUID"
        );
        ensure!(
            owners.contains_key(&uuid),
            "replacement entity is missing from source chunks"
        );
        let pos = replacement.position(&replacement.document().value)?;
        let destination = native_world::loaded::ChunkPos {
            x: (pos.x.floor() as i32).div_euclid(16),
            z: (pos.z.floor() as i32).div_euclid(16),
        };
        ensure!(
            originals.contains_key(&destination),
            "entity destination chunk is unavailable"
        );
    }
    let mut removals = BTreeMap::new();
    for removal in removed_entities {
        let uuid = saved_item::optional_uuid(&removal.document().value, "UUID")?
            .context("removed entity has no UUID")?;
        ensure!(
            !requested.contains_key(&uuid) && removals.insert(uuid, removal).is_none(),
            "duplicate entity change UUID"
        );
        ensure!(
            owners.contains_key(&uuid),
            "removed entity is missing from source chunks"
        );
        let position = removal.position(&removal.document().value)?;
        let destination = native_world::loaded::ChunkPos {
            x: (position.x.floor() as i32).div_euclid(16),
            z: (position.z.floor() as i32).div_euclid(16),
        };
        ensure!(
            originals.contains_key(&destination),
            "removed entity's final chunk is unavailable"
        );
    }

    let mut matched = BTreeSet::new();
    let mut matched_removals = BTreeSet::new();
    let mut incoming: BTreeMap<_, Vec<Tag>> = BTreeMap::new();
    let mut created = BTreeSet::new();
    for item in created_entities {
        let entity = &item.document().value;
        let uuid =
            saved_item::optional_uuid(entity, "UUID")?.context("created item has no UUID")?;
        ensure!(
            !owners.contains_key(&uuid) && created.insert(uuid),
            "created item UUID conflicts with another entity"
        );
        let position = item.position()?;
        let destination = native_world::loaded::ChunkPos {
            x: (position.x.floor() as i32).div_euclid(16),
            z: (position.z.floor() as i32).div_euclid(16),
        };
        ensure!(
            originals.contains_key(&destination),
            "created item destination chunk is unavailable"
        );
        incoming
            .entry(destination)
            .or_default()
            .push(entity.clone());
    }
    for (&position, original) in originals {
        let Some(Tag::List { values, .. }) = original.value.get("Entities") else {
            unreachable!("validated entity chunk")
        };
        let mut entities = Vec::with_capacity(values.len());
        for entity in values {
            if let Some((uuid, removal)) = saved_item::optional_uuid(entity, "UUID")?
                .and_then(|uuid| removals.get(&uuid).map(|removal| (uuid, *removal)))
            {
                let source = NamedTag {
                    name: "".into(),
                    value: entity.clone(),
                };
                ensure!(
                    removal.matches_source(&source)?,
                    "removed item source changed"
                );
                let position_in_source = removal.position(entity)?;
                ensure!(
                    (position_in_source.x.floor() as i32).div_euclid(16) == position.x
                        && (position_in_source.z.floor() as i32).div_euclid(16) == position.z,
                    "removed source item is outside its chunk"
                );
                matched_removals.insert(uuid);
                continue;
            }
            let replacement = saved_item::optional_uuid(entity, "UUID")?
                .and_then(|uuid| requested.get(&uuid).map(|replacement| (uuid, *replacement)));
            if let Some((uuid, replacement)) = replacement {
                let source = NamedTag {
                    name: "".into(),
                    value: entity.clone(),
                };
                ensure!(
                    replacement.matches_source(&source)?,
                    "entity source changed"
                );
                let previous = replacement.position(entity)?;
                ensure!(
                    (previous.x.floor() as i32).div_euclid(16) == position.x
                        && (previous.z.floor() as i32).div_euclid(16) == position.z,
                    "source entity position is outside its chunk"
                );
                let next = replacement.position(&replacement.document().value)?;
                let destination = native_world::loaded::ChunkPos {
                    x: (next.x.floor() as i32).div_euclid(16),
                    z: (next.z.floor() as i32).div_euclid(16),
                };
                let updated = replacement.document().value.clone();
                if destination == position {
                    entities.push(updated);
                } else {
                    incoming.entry(destination).or_default().push(updated);
                }
                matched.insert(uuid);
            } else {
                entities.push(entity.clone());
            }
        }
        let Tag::Compound(fields) = &mut drafts.get_mut(&position).unwrap().document.value else {
            unreachable!()
        };
        let (_, list) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("Entities"))
            .unwrap();
        *list = Tag::List {
            element_type: TagType::Compound,
            values: entities,
        };
    }
    ensure!(
        matched.len() == requested.len(),
        "replacement entity is missing from source chunks"
    );
    ensure!(
        matched_removals.len() == removals.len(),
        "removed item is missing from source chunks"
    );
    for (position, moved) in incoming {
        let Tag::Compound(fields) = &mut drafts.get_mut(&position).unwrap().document.value else {
            unreachable!()
        };
        let Some((_, Tag::List { values, .. })) =
            fields.iter_mut().find(|(name, _)| name.matches("Entities"))
        else {
            unreachable!()
        };
        values.extend(moved);
    }
    Ok(drafts)
}

/// Patch a saved entity in memory. This alone does not authorize a world write:
/// entity ownership, movement across chunks and other simulated state must be
/// coordinated before committing it.
pub fn draft(original: &NamedTag, state: FallingSaveState) -> Result<FallingBlockDraft> {
    let Tag::Compound(fields) = &original.value else {
        anyhow::bail!("entity must be a compound")
    };
    for key in [
        "id",
        "UUID",
        "Pos",
        "Motion",
        "fall_distance",
        "OnGround",
        "NoGravity",
        "BlockState",
        "Time",
        "DropItem",
        "CancelDrop",
    ] {
        ensure!(
            fields.iter().filter(|(name, _)| name.matches(key)).count() <= 1,
            "duplicate {key} fields cannot be saved safely"
        );
    }
    let saved = read(&original.value)?.context("entity is not a falling block")?;
    ensure!(
        matches!(
            saved.block.name.as_str(),
            "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
        ) && saved.block.properties.is_empty(),
        "unsupported falling block state"
    );
    ensure!(
        original.value.get("TileEntityData").is_none()
            && original.value.get("Passengers").is_none(),
        "unsupported falling block entity data"
    );
    ensure!(
        !original
            .value
            .get("HurtEntities")
            .and_then(player_data::numeric_byte)
            .is_some_and(|value| value != 0),
        "fall damage is not simulated"
    );
    for key in ["Pos", "Motion"] {
        if let Some(value) = original.value.get(key) {
            ensure!(
                matches!(value, Tag::List { element_type: TagType::Double, values } if values.len() == 3 && values.iter().all(|v| matches!(v, Tag::Double(x) if x.is_finite()))),
                "{key} cannot be preserved safely"
            );
        }
    }
    for key in ["OnGround", "NoGravity", "DropItem", "CancelDrop"] {
        if let Some(value) = original.value.get(key) {
            ensure!(
                matches!(value, Tag::Byte(0 | 1)),
                "{key} cannot be preserved safely"
            );
        }
    }
    if let Some(value) = original.value.get("Time") {
        ensure!(
            matches!(value, Tag::Int(_)),
            "Time cannot be preserved safely"
        );
    }
    if let Some(value) = original.value.get("fall_distance") {
        ensure!(
            matches!(value, Tag::Double(v) if v.is_finite()),
            "fall distance cannot be preserved safely"
        );
    }
    let valid_position = |value: DVec3| {
        value.is_finite()
            && value.x.abs() <= 30_000_512.0
            && value.z.abs() <= 30_000_512.0
            && value.y.abs() <= 20_000_000.0
    };
    let original_vector = |key: &str| -> Option<DVec3> {
        let Tag::List { values, .. } = original.value.get(key)? else {
            return None;
        };
        let [Tag::Double(x), Tag::Double(y), Tag::Double(z)] = values.as_slice() else {
            return None;
        };
        Some(DVec3::new(*x, *y, *z))
    };
    if let Some(position) = original_vector("Pos") {
        ensure!(
            valid_position(position),
            "original entity position would be clamped"
        );
    }
    if let Some(velocity) = original_vector("Motion") {
        ensure!(
            velocity.abs().max_element() <= 10.0,
            "original entity velocity would be discarded"
        );
    }
    ensure!(
        valid_position(state.position),
        "entity position cannot be saved safely"
    );
    ensure!(
        state.velocity.is_finite() && state.velocity.abs().max_element() <= 10.0,
        "entity velocity cannot be saved safely"
    );
    ensure!(
        state.fall_distance.is_finite(),
        "fall distance cannot be saved safely"
    );
    let source_nbt = native_persistence::nbt::write(original)?;
    let mut document = original.clone();
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    let vector = |v: DVec3| Tag::List {
        element_type: TagType::Double,
        values: v.to_array().into_iter().map(Tag::Double).collect(),
    };
    for (name, value) in [
        ("Pos", vector(state.position)),
        ("Motion", vector(state.velocity)),
        ("fall_distance", Tag::Double(state.fall_distance)),
        ("OnGround", Tag::Byte(i8::from(state.on_ground))),
        ("NoGravity", Tag::Byte(i8::from(state.no_gravity))),
        ("Time", Tag::Int(state.time)),
        ("DropItem", Tag::Byte(i8::from(state.drop_item))),
        ("CancelDrop", Tag::Byte(i8::from(state.cancel_drop))),
    ] {
        if let Some((_, existing)) = fields.iter_mut().find(|(key, _)| key.matches(name)) {
            *existing = value;
        } else {
            fields.push((name.into(), value));
        }
    }
    Ok(FallingBlockDraft {
        source_nbt,
        document,
    })
}

/// Unknown entity types return None; malformed falling blocks return an error.
pub fn read(entity: &Tag) -> Result<Option<SavedFallingBlock>> {
    ensure!(
        matches!(entity, Tag::Compound(_)),
        "entity must be a compound"
    );
    let Some(Tag::String(id)) = entity.get("id") else {
        anyhow::bail!("entity has no valid id")
    };
    let id = ResourceId::parse(&String::from_utf16(&id.0).context("invalid entity id")?)?;
    if id.namespace != "minecraft" || id.path != "falling_block" {
        return Ok(None);
    }
    let named = NamedTag {
        name: "".into(),
        value: entity.clone(),
    };
    let position = player_data::read_saved_position(&named)?
        .context("falling block is missing its position")?;
    let motion = player_data::read_saved_motion(&named)?;
    let block_uses_default = matches!(entity.get("BlockState"), None | Some(Tag::String(_)));
    let block = match entity.get("BlockState") {
        None => BlockState {
            name: "minecraft:sand".into(),
            properties: BTreeMap::new(),
        },
        Some(Tag::String(name)) => {
            let name = String::from_utf16(&name.0).context("invalid block name")?;
            let id = ResourceId::parse(&name)?;
            BlockState {
                name: format!("{}:{}", id.namespace, id.path),
                properties: BTreeMap::new(),
            }
        }
        Some(value) => {
            ensure!(
                matches!(value, Tag::Compound(_)),
                "BlockState must be a compound"
            );
            let Some(Tag::String(name)) = value.get("id") else {
                anyhow::bail!("BlockState has no id")
            };
            let name = String::from_utf16(&name.0).context("invalid block name")?;
            let id = ResourceId::parse(&name)?;
            let properties = match value.get("properties") {
                None => BTreeMap::new(),
                Some(Tag::Compound(entries)) => entries
                    .iter()
                    .map(|(key, value)| {
                        let Tag::String(value) = value else {
                            anyhow::bail!("block property is not a string")
                        };
                        Ok((String::from_utf16(&key.0)?, String::from_utf16(&value.0)?))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()?,
                _ => anyhow::bail!("block properties must be a compound"),
            };
            BlockState {
                name: format!("{}:{}", id.namespace, id.path),
                properties,
            }
        }
    };
    let flag = |key: &str, default: bool| {
        entity
            .get(key)
            .and_then(player_data::numeric_byte)
            .map_or(default, |v| v != 0)
    };
    Ok(Some(SavedFallingBlock {
        source: Arc::new(named.value),
        uuid: saved_item::optional_uuid(entity, "UUID")?,
        position,
        velocity: motion.velocity,
        on_ground: motion.on_ground,
        no_gravity: flag("NoGravity", false),
        fall_distance: motion.fall_distance,
        block,
        block_uses_default,
        time: entity
            .get("Time")
            .and_then(player_data::numeric_int)
            .unwrap_or(0),
        drop_item: flag("DropItem", true),
        cancel_drop: flag("CancelDrop", false),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::TagType;

    fn base() -> Tag {
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:falling_block".into())),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![Tag::Double(1.0), Tag::Double(2.0), Tag::Double(3.0)],
                },
            ),
        ])
    }

    #[test]
    fn falling_removal_requires_matching_source_and_final_chunk() {
        use native_world::loaded::ChunkPos;
        let home = ChunkPos { x: 0, z: 0 };
        let target = ChunkPos { x: 1, z: 0 };
        let mut entity = base();
        let Tag::Compound(fields) = &mut entity else {
            panic!()
        };
        fields.push(("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])));
        fields.push(("BlockState".into(), Tag::String("minecraft:sand".into())));
        let pig = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("UUID".into(), Tag::IntArray(vec![5, 6, 7, 8])),
        ]);
        let make = |entity: Tag| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![0, 0])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![entity, pig.clone()],
                    },
                ),
            ]),
        };
        let original = NamedTag {
            name: "".into(),
            value: entity.clone(),
        };
        let chunk = make(entity.clone());
        let removal = draft(
            &original,
            FallingSaveState {
                position: DVec3::new(17.0, 2.0, 3.0),
                velocity: DVec3::ZERO,
                fall_distance: 0.0,
                on_ground: true,
                no_gravity: false,
                time: 12,
                drop_item: true,
                cancel_drop: false,
            },
        )
        .unwrap();
        assert!(
            draft_entity_chunks_with_falling_removals(
                &BTreeMap::from([(home, Some(&chunk))]),
                &[],
                &[removal],
                &[],
                &[],
                &[],
                &[]
            )
            .is_err()
        );
        let removal = draft(
            &original,
            FallingSaveState {
                position: DVec3::new(17.0, 2.0, 3.0),
                velocity: DVec3::ZERO,
                fall_distance: 0.0,
                on_ground: true,
                no_gravity: false,
                time: 12,
                drop_item: true,
                cancel_drop: false,
            },
        )
        .unwrap();
        let sources = BTreeMap::from([(home, Some(&chunk)), (target, None)]);
        let drafts = draft_entity_chunks_with_falling_removals(
            &sources,
            &[],
            &[removal],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap();
        let Tag::List { values, .. } = drafts[&home].document().value.get("Entities").unwrap()
        else {
            panic!()
        };
        assert_eq!(values, &[pig.clone()]);
        assert!(drafts[&target].source_was_absent());
        let mut changed = entity;
        let Tag::Compound(fields) = &mut changed else {
            panic!()
        };
        fields.push(("extra".into(), Tag::Int(1)));
        let stale = make(changed);
        let removal = draft(
            &original,
            FallingSaveState {
                position: DVec3::new(17.0, 2.0, 3.0),
                velocity: DVec3::ZERO,
                fall_distance: 0.0,
                on_ground: true,
                no_gravity: false,
                time: 12,
                drop_item: true,
                cancel_drop: false,
            },
        )
        .unwrap();
        assert!(
            draft_entity_chunks_with_falling_removals(
                &BTreeMap::from([(home, Some(&stale)), (target, None)]),
                &[],
                &[removal],
                &[],
                &[],
                &[],
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn created_item_requires_unique_uuid_and_available_destination() {
        use native_world::loaded::ChunkPos;
        let position = ChunkPos { x: 0, z: 0 };
        let missing = ChunkPos { x: 1, z: 0 };
        let stack = crate::inventory::ItemStack::new(
            "minecraft:stone".into(),
            2,
            64,
            Tag::Compound(vec![]),
        )
        .unwrap();
        let make = |uuid, x| {
            saved_item::draft_created(
                uuid,
                None,
                None,
                saved_item::CreatedItemBase {
                    rotation: [0.0, 0.0],
                    fall_distance: 0.0,
                    air: 300,
                    portal_cooldown: 0,
                },
                saved_item::ItemSaveState {
                    position: DVec3::new(x, 2.0, 3.0),
                    velocity: DVec3::ZERO,
                    on_ground: false,
                    no_gravity: false,
                    age: 0,
                    pickup_delay: 10,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                    stack: &stack,
                },
            )
            .unwrap()
        };
        let pig = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
        ]);
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![0, 0])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![pig.clone()],
                    },
                ),
            ]),
        };
        let sources = BTreeMap::from([(position, Some(&original))]);
        assert!(
            draft_entity_chunks_with_item_changes(&sources, &[], &[], &[make([1, 2, 3, 4], 1.5)])
                .is_err()
        );
        assert!(
            draft_entity_chunks_with_item_changes(&sources, &[], &[], &[make([5, 6, 7, 8], 17.5)])
                .is_err()
        );
        assert!(
            draft_entity_chunks_with_item_changes(
                &sources,
                &[],
                &[],
                &[make([5, 6, 7, 8], 1.5), make([5, 6, 7, 8], 1.5)]
            )
            .is_err()
        );
        let mut without_uuid = original.clone();
        let Tag::Compound(fields) = &mut without_uuid.value else {
            unreachable!()
        };
        let (_, Tag::List { values, .. }) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("Entities"))
            .unwrap()
        else {
            unreachable!()
        };
        let Tag::Compound(entity) = &mut values[0] else {
            unreachable!()
        };
        entity.retain(|(name, _)| !name.matches("UUID"));
        assert!(
            draft_entity_chunks_with_item_changes(
                &BTreeMap::from([(position, Some(&without_uuid))]),
                &[],
                &[],
                &[make([5, 6, 7, 8], 1.5)]
            )
            .is_err()
        );
        let drafts = draft_entity_chunks_with_item_changes(
            &BTreeMap::from([(position, Some(&original)), (missing, None)]),
            &[],
            &[],
            &[make([5, 6, 7, 8], 17.5)],
        )
        .unwrap();
        let Some(Tag::List { values, .. }) = drafts[&position].document().value.get("Entities")
        else {
            panic!()
        };
        assert_eq!(values, &vec![pig]);
        let Some(Tag::List { values, .. }) = drafts[&missing].document().value.get("Entities")
        else {
            panic!()
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            saved_item::read(&values[0]).unwrap().unwrap().stack.count(),
            2
        );
        assert!(drafts[&missing].source_was_absent());
    }

    #[test]
    fn removed_item_requires_its_source_and_final_chunks_and_preserves_others() {
        use native_world::loaded::ChunkPos;
        let origin = ChunkPos { x: 0, z: 0 };
        let destination = ChunkPos { x: 1, z: 0 };
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![Tag::Double(1.5), Tag::Double(2.0), Tag::Double(3.0)],
                },
            ),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(3)),
                ]),
            ),
        ]);
        let other = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("UUID".into(), Tag::IntArray(vec![5, 6, 7, 8])),
            ("future".into(), Tag::Int(7)),
        ]);
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![0, 0])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![item.clone(), other.clone()],
                    },
                ),
            ]),
        };
        let saved = saved_item::read(&item).unwrap().unwrap();
        let make_removal = || {
            saved_item::draft(
                &NamedTag {
                    name: "".into(),
                    value: item.clone(),
                },
                saved_item::ItemSaveState {
                    position: DVec3::new(17.5, 2.0, 3.0),
                    velocity: saved.velocity,
                    on_ground: saved.on_ground,
                    no_gravity: saved.no_gravity,
                    age: saved.age,
                    pickup_delay: saved.pickup_delay,
                    health: saved.health,
                    fire_ticks: saved.fire_ticks,
                    invulnerable: saved.invulnerable,
                    invulnerable_time: saved.invulnerable_time,
                    stack: &saved.stack,
                },
            )
            .unwrap()
        };
        let removal = make_removal();
        assert!(
            draft_entity_chunks_with_removals(
                &BTreeMap::from([(origin, Some(&original))]),
                &[],
                &[make_removal()]
            )
            .is_err()
        );
        let sources = BTreeMap::from([(origin, Some(&original)), (destination, None)]);
        assert!(
            draft_entity_chunks_with_removals(&sources, &[], &[make_removal(), make_removal()])
                .is_err()
        );
        let drafts = draft_entity_chunks_with_removals(&sources, &[], &[removal]).unwrap();
        let Some(Tag::List { values, .. }) = drafts[&origin].document().value.get("Entities")
        else {
            panic!()
        };
        assert_eq!(values, &vec![other]);
        assert!(drafts[&destination].source_was_absent());
        let created = saved_item::draft_created(
            [9, 10, 11, 12],
            None,
            None,
            saved_item::CreatedItemBase {
                rotation: [0.0, 0.0],
                fall_distance: 0.0,
                air: 300,
                portal_cooldown: 0,
            },
            saved_item::ItemSaveState {
                position: DVec3::new(17.5, 2.0, 3.0),
                velocity: DVec3::ZERO,
                on_ground: false,
                no_gravity: false,
                age: 0,
                pickup_delay: 10,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
                stack: &saved.stack,
            },
        )
        .unwrap();
        let coordinated =
            draft_entity_chunks_with_item_changes(&sources, &[], &[make_removal()], &[created])
                .unwrap();
        let Some(Tag::List { values, .. }) =
            coordinated[&destination].document().value.get("Entities")
        else {
            panic!()
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            saved_item::read(&values[0]).unwrap().unwrap().uuid,
            Some([9, 10, 11, 12])
        );
        let mut changed = original.clone();
        let Tag::Compound(fields) = &mut changed.value else {
            unreachable!()
        };
        let (_, Tag::List { values, .. }) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("Entities"))
            .unwrap()
        else {
            unreachable!()
        };
        let Tag::Compound(entity_fields) = &mut values[0] else {
            unreachable!()
        };
        entity_fields.push(("changed".into(), Tag::Int(1)));
        assert!(
            draft_entity_chunks_with_removals(
                &BTreeMap::from([(origin, Some(&changed)), (destination, None)]),
                &[],
                &[make_removal()]
            )
            .is_err()
        );
    }

    #[test]
    fn missing_optional_fields_use_java_defaults_and_preserve_source() {
        let entity = base();
        let saved = read(&entity).unwrap().unwrap();
        assert_eq!(saved.block.name, "minecraft:sand");
        assert_eq!(saved.time, 0);
        assert!(saved.drop_item);
        assert!(!saved.cancel_drop);
        assert_eq!(*saved.source, entity);
    }

    #[test]
    fn malformed_block_state_is_not_silently_destroyed() {
        let mut entity = base();
        let Tag::Compound(entries) = &mut entity else {
            unreachable!()
        };
        entries.push(("BlockState".into(), Tag::Int(5)));
        assert!(read(&entity).is_err());
    }

    #[test]
    fn draft_preserves_unknown_fields_and_updates_motion() {
        let mut original = NamedTag {
            name: "".into(),
            value: base(),
        };
        let Tag::Compound(fields) = &mut original.value else {
            unreachable!()
        };
        fields.push(("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])));
        fields.push(("unknown".into(), Tag::Long(42)));
        let state = FallingSaveState {
            position: DVec3::new(-1.5, 3.0, 2.5),
            velocity: DVec3::new(0.2, -0.4, 0.0),
            fall_distance: 2.5,
            on_ground: false,
            no_gravity: true,
            time: 101,
            drop_item: false,
            cancel_drop: true,
        };
        let draft = draft(&original, state).unwrap();
        assert!(draft.matches_source(&original).unwrap());
        assert_eq!(draft.document().value.get("unknown"), Some(&Tag::Long(42)));
        assert_eq!(
            draft.document().value.get("UUID"),
            original.value.get("UUID")
        );
        let parsed = read(&draft.document().value).unwrap().unwrap();
        assert_eq!(parsed.position, state.position);
        assert_eq!(parsed.velocity, state.velocity);
        assert_eq!(parsed.fall_distance, state.fall_distance);
        assert_eq!(parsed.time, 101);
        assert!(!parsed.drop_item && parsed.cancel_drop && parsed.no_gravity);
        let mut changed = original.clone();
        let Tag::Compound(fields) = &mut changed.value else {
            unreachable!()
        };
        fields.push(("other".into(), Tag::Int(1)));
        assert!(!draft.matches_source(&changed).unwrap());
    }

    #[test]
    fn draft_rejects_duplicate_fields_and_clamped_motion() {
        let mut original = NamedTag {
            name: "".into(),
            value: base(),
        };
        let state = FallingSaveState {
            position: DVec3::ZERO,
            velocity: DVec3::ZERO,
            fall_distance: 0.0,
            on_ground: false,
            no_gravity: false,
            time: 0,
            drop_item: true,
            cancel_drop: false,
        };
        let Tag::Compound(fields) = &mut original.value else {
            unreachable!()
        };
        fields.push(("Time".into(), Tag::Int(1)));
        fields.push(("Time".into(), Tag::Int(2)));
        assert!(draft(&original, state).is_err());
        let mut fast = state;
        fast.velocity.x = 11.0;
        assert!(
            draft(
                &NamedTag {
                    name: "".into(),
                    value: base()
                },
                fast
            )
            .is_err()
        );
        let mut duplicate = NamedTag {
            name: "".into(),
            value: base(),
        };
        let Tag::Compound(fields) = &mut duplicate.value else {
            unreachable!()
        };
        fields.push(("fall_distance".into(), Tag::Double(1.0)));
        fields.push(("fall_distance".into(), Tag::Double(2.0)));
        assert!(draft(&duplicate, state).is_err());
        let mut invalid = state;
        invalid.fall_distance = f64::NAN;
        assert!(
            draft(
                &NamedTag {
                    name: "".into(),
                    value: base()
                },
                invalid
            )
            .is_err()
        );
    }

    #[test]
    fn entity_chunk_draft_replaces_only_matching_falling_entity() {
        let mut entity = base();
        let Tag::Compound(fields) = &mut entity else {
            unreachable!()
        };
        fields.push(("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])));
        fields.push(("unknown_entity_field".into(), Tag::Int(42)));
        let animal = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("UUID".into(), Tag::IntArray(vec![5, 6, 7, 8])),
        ]);
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![0, 0])),
                ("unknown_chunk_field".into(), Tag::Long(7)),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![entity.clone(), animal.clone()],
                    },
                ),
            ]),
        };
        let state = FallingSaveState {
            position: DVec3::new(4.5, 4.0, 5.5),
            velocity: DVec3::ZERO,
            fall_distance: 0.0,
            on_ground: false,
            no_gravity: false,
            time: 7,
            drop_item: true,
            cancel_drop: false,
        };
        let saved = draft(
            &NamedTag {
                name: "".into(),
                value: entity.clone(),
            },
            state,
        )
        .unwrap();
        let result = draft_entity_chunk(
            &original,
            native_world::loaded::ChunkPos { x: 0, z: 0 },
            &[saved],
        )
        .unwrap();
        assert!(result.matches_source(&original).unwrap());
        assert_eq!(
            result.document().value.get("unknown_chunk_field"),
            Some(&Tag::Long(7))
        );
        let Some(Tag::List { values, .. }) = result.document().value.get("Entities") else {
            panic!("missing entities")
        };
        assert_eq!(values[1], animal);
        assert_eq!(values[0].get("unknown_entity_field"), Some(&Tag::Int(42)));
        assert_eq!(read(&values[0]).unwrap().unwrap().position, state.position);
        assert_eq!(
            read(&entity).unwrap().unwrap().position,
            DVec3::new(1.0, 2.0, 3.0)
        );
        let replacement = draft(
            &NamedTag {
                name: "".into(),
                value: entity,
            },
            state,
        )
        .unwrap();
        let mut changed = original.clone();
        let Tag::Compound(fields) = &mut changed.value else {
            unreachable!()
        };
        let (_, Tag::List { values, .. }) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("Entities"))
            .unwrap()
        else {
            unreachable!()
        };
        let Tag::Compound(fields) = &mut values[0] else {
            unreachable!()
        };
        fields.push(("changed".into(), Tag::Int(1)));
        assert!(!result.matches_source(&changed).unwrap());
        assert!(
            draft_entity_chunk(
                &changed,
                native_world::loaded::ChunkPos { x: 0, z: 0 },
                &[replacement]
            )
            .is_err()
        );
    }

    #[test]
    fn entity_chunk_draft_rejects_cross_chunk_moves_and_duplicate_uuids() {
        let mut entity = base();
        let Tag::Compound(fields) = &mut entity else {
            unreachable!()
        };
        fields.push(("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])));
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![0, 0])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![entity.clone(), entity.clone()],
                    },
                ),
            ]),
        };
        let state = FallingSaveState {
            position: DVec3::new(16.0, 4.0, 5.0),
            velocity: DVec3::ZERO,
            fall_distance: 0.0,
            on_ground: false,
            no_gravity: false,
            time: 0,
            drop_item: true,
            cancel_drop: false,
        };
        let replacement = draft(
            &NamedTag {
                name: "".into(),
                value: entity.clone(),
            },
            state,
        )
        .unwrap();
        assert!(
            draft_entity_chunk(
                &original,
                native_world::loaded::ChunkPos { x: 0, z: 0 },
                &[replacement]
            )
            .is_err()
        );
        let replacement = draft(
            &NamedTag {
                name: "".into(),
                value: entity,
            },
            FallingSaveState {
                position: DVec3::new(1.0, 2.0, 3.0),
                ..state
            },
        )
        .unwrap();
        assert!(
            draft_entity_chunk(
                &original,
                native_world::loaded::ChunkPos { x: 0, z: 0 },
                &[replacement]
            )
            .is_err()
        );
    }

    #[test]
    fn multi_chunk_draft_moves_entities_atomically_and_preserves_unknown_data() {
        use native_world::loaded::ChunkPos;
        let left = ChunkPos { x: -1, z: 0 };
        let right = ChunkPos { x: 0, z: 0 };
        let entity = |uuid: [i32; 4], x: f64| {
            let mut value = base();
            let Tag::Compound(fields) = &mut value else {
                unreachable!()
            };
            fields.push(("UUID".into(), Tag::IntArray(uuid.to_vec())));
            fields.push(("unknown".into(), Tag::Long(42)));
            let (_, pos) = fields
                .iter_mut()
                .find(|(key, _)| key.matches("Pos"))
                .unwrap();
            *pos = Tag::List {
                element_type: TagType::Double,
                values: vec![Tag::Double(x), Tag::Double(2.0), Tag::Double(3.0)],
            };
            value
        };
        let a = entity([1, 2, 3, 4], -0.1);
        let b = entity([5, 6, 7, 8], 0.0);
        let pig = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("UUID".into(), Tag::IntArray(vec![9, 10, 11, 12])),
        ]);
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![13, 14, 15, 16])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![Tag::Double(-0.1), Tag::Double(2.0), Tag::Double(3.0)],
                },
            ),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(2)),
                ]),
            ),
            ("unknown_item".into(), Tag::Int(42)),
        ]);
        let chunk = |pos: ChunkPos, entities: Vec<Tag>| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![pos.x, pos.z])),
                ("unknown_chunk".into(), Tag::Long(17)),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: entities,
                    },
                ),
            ]),
        };
        let first = chunk(left, vec![a.clone(), pig.clone(), item.clone()]);
        let second = chunk(right, vec![b.clone()]);
        let state = |x| FallingSaveState {
            position: DVec3::new(x, 2.0, 3.0),
            velocity: DVec3::ZERO,
            fall_distance: 0.0,
            on_ground: false,
            no_gravity: false,
            time: 1,
            drop_item: true,
            cancel_drop: false,
        };
        let moved_a = draft(
            &NamedTag {
                name: "".into(),
                value: a.clone(),
            },
            state(0.0),
        )
        .unwrap();
        let moved_b = draft(
            &NamedTag {
                name: "".into(),
                value: b.clone(),
            },
            state(-0.1),
        )
        .unwrap();
        let saved = saved_item::read(&item).unwrap().unwrap();
        let moved_item = saved_item::draft(
            &NamedTag {
                name: "".into(),
                value: item,
            },
            saved_item::ItemSaveState {
                position: DVec3::new(0.0, 2.0, 3.0),
                velocity: saved.velocity,
                on_ground: saved.on_ground,
                no_gravity: saved.no_gravity,
                age: saved.age,
                pickup_delay: saved.pickup_delay,
                health: saved.health,
                fire_ticks: saved.fire_ticks,
                invulnerable: saved.invulnerable,
                invulnerable_time: saved.invulnerable_time,
                stack: &saved.stack,
            },
        )
        .unwrap();
        let originals = BTreeMap::from([(left, &first), (right, &second)]);
        let drafts = draft_entity_chunks_with_moves(
            &originals,
            &[
                EntityReplacement::Falling(&moved_a),
                EntityReplacement::Falling(&moved_b),
                EntityReplacement::Item(&moved_item),
            ],
        )
        .unwrap();
        assert!(drafts[&left].matches_source(&first).unwrap());
        assert!(drafts[&right].matches_source(&second).unwrap());
        assert_eq!(
            drafts[&left].document().value.get("unknown_chunk"),
            Some(&Tag::Long(17))
        );
        let Some(Tag::List {
            values: left_values,
            ..
        }) = drafts[&left].document().value.get("Entities")
        else {
            panic!()
        };
        let Some(Tag::List {
            values: right_values,
            ..
        }) = drafts[&right].document().value.get("Entities")
        else {
            panic!()
        };
        assert_eq!(left_values[0], pig);
        assert_eq!(
            left_values[1].get("UUID"),
            Some(&Tag::IntArray(vec![5, 6, 7, 8]))
        );
        assert_eq!(
            right_values[0].get("UUID"),
            Some(&Tag::IntArray(vec![1, 2, 3, 4]))
        );
        assert_eq!(right_values[0].get("unknown"), Some(&Tag::Long(42)));
        assert_eq!(
            right_values[1].get("UUID"),
            Some(&Tag::IntArray(vec![13, 14, 15, 16]))
        );
        assert_eq!(right_values[1].get("unknown_item"), Some(&Tag::Int(42)));
        assert!(
            draft_entity_chunks_with_moves(
                &BTreeMap::from([(left, &first)]),
                &[EntityReplacement::Falling(&moved_a)]
            )
            .is_err()
        );
        let duplicate = chunk(right, vec![a.clone()]);
        assert!(
            draft_entity_chunks_with_moves(
                &BTreeMap::from([(left, &first), (right, &duplicate)]),
                &[]
            )
            .is_err()
        );
        let mut changed = a;
        let Tag::Compound(fields) = &mut changed else {
            unreachable!()
        };
        fields.push(("changed".into(), Tag::Int(1)));
        let changed_chunk = chunk(left, vec![changed]);
        assert!(
            draft_entity_chunks_with_moves(
                &BTreeMap::from([(left, &changed_chunk), (right, &second)]),
                &[EntityReplacement::Falling(&moved_a)]
            )
            .is_err()
        );
    }
}
