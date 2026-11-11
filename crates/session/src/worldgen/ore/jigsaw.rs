//! Place supported jigsaw templates in an unpublished decoration region.
use super::*;
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
use native_resources::{block_registry::GeneratedBlockRegistry, structure_pool::LiquidSettings};
use native_world::fluid::{FluidKind, FluidState};

use super::super::jigsaw::{
    JigsawElement, JigsawPlacementWorld, PreparedJigsawPlacement, PreparedStart,
    PreparedTemplatePiece,
};

pub(super) struct JigsawRegion<'r, 'a, 'g> {
    pub(super) region: &'r mut DecorationRegion<'a>,
    pub(super) registry: &'g GeneratedBlockRegistry,
}

#[cfg(test)]
pub(super) fn place_block_for_light_test(
    region: &mut DecorationRegion<'_>,
    registry: &GeneratedBlockRegistry,
    pos: [i32; 3],
    state: &BlockState,
    waterlog: bool,
) -> Result<()> {
    let mut random = DecorationRandom::new(42);
    let mut draft = JigsawRegion { region, registry };
    let fluid = if waterlog { draft.fluid_at(pos)? } else { None };
    ensure!(draft.place_template_block(pos, state, None, &mut random)?);
    if waterlog {
        draft.place_liquid(pos, state, fluid)?;
    }
    Ok(())
}

pub(super) fn jigsaw_wood(state: &BlockState) -> bool {
    if native_core::wood::Planks::from_id(&state.name).is_some() {
        state.properties.is_empty()
    } else if native_core::wood::Wood::from_id(&state.name).is_some() {
        state.properties.len() == 1
            && matches!(
                state.properties.get("axis").map(String::as_str),
                Some("x" | "y" | "z")
            )
    } else {
        false
    }
}

pub(super) fn jigsaw_torch(state: &BlockState) -> bool {
    match state.name.as_str() {
        "minecraft:torch" | "minecraft:soul_torch" => state.properties.is_empty(),
        "minecraft:wall_torch" | "minecraft:soul_wall_torch" => {
            state.properties.len() == 1
                && matches!(
                    state.properties.get("facing").map(String::as_str),
                    Some("north" | "south" | "east" | "west")
                )
        }
        _ => false,
    }
}

pub(super) fn jigsaw_lantern(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:lantern" | "minecraft:soul_lantern"
    ) && native_world::collision::block_shape(state).is_some()
        && native_world::block_light::same_light_behavior(state, state).is_some()
}

pub(super) fn jigsaw_candle(state: &BlockState) -> bool {
    native_world::collision::candle_state(state).is_some()
}

pub(super) fn jigsaw_static_full(state: &BlockState) -> Result<bool> {
    if native_world::wall::Wall::from_state(state).is_some() {
        return Ok(false);
    }
    if jigsaw_wood(state) {
        return Ok(true);
    }
    if jigsaw_torch(state) || jigsaw_lantern(state) || jigsaw_candle(state) {
        return Ok(false);
    }
    if native_world::collision::full_glass(state) {
        return Ok(true);
    }
    if native_world::collision::full_terracotta(state) {
        return Ok(true);
    }
    if native_world::collision::pane_waterlogged(state).is_some() {
        return Ok(false);
    }
    if native_world::collision::fence_waterlogged(state).is_some() {
        return Ok(false);
    }
    if native_world::collision::fence_gate_open(state).is_some() {
        return Ok(false);
    }
    if native_world::collision::door_open(state).is_some() {
        return Ok(false);
    }
    if native_world::collision::trapdoor_state(state).is_some() {
        return Ok(false);
    }
    if native_world::collision::bed_leg_end(state).is_some() {
        return Ok(false);
    }
    if state.name == "minecraft:chest" {
        monster_block_flags(state)?;
        return Ok(false);
    }
    if state.name.starts_with("minecraft:")
        && state.name.ends_with("_slab")
        && native_world::collision::slab_stair_solid(state).is_some()
    {
        return match state.properties.get("type").map(String::as_str) {
            Some("double") => Ok(true),
            Some("top" | "bottom") => Ok(false),
            _ => bail!("invalid jigsaw slab type"),
        };
    }
    if matches!(
        state.name.as_str(),
        "minecraft:stone"
            | "minecraft:deepslate"
            | "minecraft:cobbled_deepslate"
            | "minecraft:polished_deepslate"
            | "minecraft:deepslate_bricks"
            | "minecraft:cracked_deepslate_bricks"
            | "minecraft:deepslate_tiles"
            | "minecraft:cracked_deepslate_tiles"
            | "minecraft:chiseled_deepslate"
            | "minecraft:reinforced_deepslate"
            | "minecraft:basalt"
            | "minecraft:polished_basalt"
            | "minecraft:smooth_basalt"
            | "minecraft:white_wool"
            | "minecraft:orange_wool"
            | "minecraft:magenta_wool"
            | "minecraft:light_blue_wool"
            | "minecraft:yellow_wool"
            | "minecraft:lime_wool"
            | "minecraft:pink_wool"
            | "minecraft:gray_wool"
            | "minecraft:light_gray_wool"
            | "minecraft:cyan_wool"
            | "minecraft:purple_wool"
            | "minecraft:blue_wool"
            | "minecraft:brown_wool"
            | "minecraft:green_wool"
            | "minecraft:red_wool"
            | "minecraft:black_wool"
    ) {
        return Ok(true);
    }
    super::super::ocean_ruin::ocean_template_static_full(state)
}

impl JigsawRegion<'_, '_, '_> {
    fn checked_fluid(&self, state: &BlockState) -> Result<Option<FluidState>> {
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties),
            "unverified jigsaw fluid state"
        );
        FluidState::from_block(state).map_err(anyhow::Error::msg)
    }
}

impl JigsawPlacementWorld for JigsawRegion<'_, '_, '_> {
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self.region, pos)
    }
    fn world_surface_height(&mut self, x: i32, z: i32) -> Result<i32> {
        self.region.world_surface_height(x, z)
    }
    fn fluid_at(&mut self, pos: [i32; 3]) -> Result<Option<FluidState>> {
        let state = self.block(pos)?;
        self.checked_fluid(&state)
    }
    fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>> {
        self.checked_fluid(state)
    }
    fn place_template_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        if !self.region.writable.contains(&chunk(pos)) {
            return Ok(false);
        }
        ensure!(
            self.region.world.chunks.contains_key(&chunk(pos)),
            "jigsaw block chunk missing"
        );
        if self.region.is_outside(pos[1]) {
            return Ok(true);
        }
        let chest_table = if let Some(nbt) = nbt {
            let Tag::Compound(fields) = nbt else {
                bail!("jigsaw block entity placement requires a compound")
            };
            let mut keys = BTreeSet::new();
            ensure!(
                state.name == "minecraft:chest"
                    && fields.iter().all(|(key, _)| {
                        (key.matches("id")
                            || key.matches("LootTable")
                            || key.matches("LootTableSeed")
                            || key.matches("Items"))
                            && keys.insert(key.0.clone())
                    })
                    && matches!(nbt.get("id"), Some(Tag::String(id)) if id.matches("minecraft:chest"))
                    && matches!(nbt.get("LootTableSeed"), None | Some(Tag::Long(_))),
                "jigsaw block entity placement is not yet supported"
            );
            let empty_items = match nbt.get("Items") {
                None => true,
                Some(Tag::List {
                    element_type,
                    values,
                }) => {
                    values.is_empty()
                        && matches!(
                            element_type,
                            native_persistence::nbt::TagType::End
                                | native_persistence::nbt::TagType::Compound
                        )
                }
                _ => false,
            };
            match nbt.get("LootTable") {
                Some(Tag::String(table)) => {
                    ensure!(
                        nbt.get("Items").is_none(),
                        "chest loot and inventory cannot both be preserved"
                    );
                    let table = String::from_utf16(&table.0).context("invalid chest loot table")?;
                    ResourceId::parse(&table)?;
                    Some(Some(table))
                }
                None => {
                    ensure!(empty_items, "nonempty chest inventory is not yet supported");
                    Some(None)
                }
                _ => bail!("invalid chest loot table"),
            }
        } else {
            None
        };
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties)
                && self.registry.has_block_entity(&state.name) == Some(chest_table.is_some())
                && jigsaw_static_full(state).is_ok(),
            "unsupported jigsaw block effects at {pos:?}: {state:?}"
        );
        let old = self.region.read(pos)?;
        ensure!(
            self.registry.is_canonical_state(&old.name, &old.properties)
                && self.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some() || jigsaw_static_full(&old).is_ok())
                && !self.region.monster_entities.contains_key(&pos)
                && !self.region.bee_nests.contains_key(&pos),
            "unsupported jigsaw replacement source at {pos:?}: {old:?}"
        );
        if let Some(table) = chest_table {
            self.region.set_proto_block(
                pos,
                &BlockState {
                    name: "minecraft:barrier".into(),
                    properties: BTreeMap::from([("waterlogged".into(), "false".into())]),
                },
            )?;
            self.region.set_proto_block(pos, state)?;
            let pending = self.region.pending_order.entry(chunk(pos)).or_default();
            Arc::make_mut(pending).insert(pos);
            Arc::make_mut(pending).remove(pos);
            let chest = super::super::GeneratedEmptyChest {
                position: pos,
                state: state.clone(),
            };
            let seed = random.next_long();
            let entity = match table {
                Some(table) => {
                    super::super::GeneratedMonsterRoomEntity::Chest(chest.with_loot(&table, seed)?)
                }
                None => super::super::GeneratedMonsterRoomEntity::EmptyChest(chest),
            };
            Arc::make_mut(&mut self.region.monster_entities).insert(pos, Some(entity));
            Arc::make_mut(self.region.entity_order.entry(chunk(pos)).or_default()).insert(pos);
        } else {
            self.region.jigsaw_block_write = jigsaw_wood(state)
                || native_world::collision::slab_stair_solid(state).is_some()
                || native_world::wall::Wall::from_state(state).is_some()
                || native_world::collision::fence_waterlogged(state).is_some()
                || native_world::collision::fence_gate_open(state).is_some()
                || native_world::collision::door_open(state).is_some()
                || native_world::collision::trapdoor_state(state).is_some()
                || native_world::collision::bed_leg_end(state).is_some()
                || native_world::collision::full_glass(state)
                || native_world::collision::full_terracotta(state)
                || native_world::collision::pane_waterlogged(state).is_some()
                || jigsaw_torch(state)
                || jigsaw_lantern(state)
                || jigsaw_candle(state);
            let result = self.region.set_proto_block(pos, state);
            self.region.jigsaw_block_write = false;
            result?;
        }
        Ok(true)
    }
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name.starts_with("minecraft:")
            && (state.name.ends_with("_stairs")
                || state.name.ends_with("_slab")
                || state.name.ends_with("_fence")
                || native_world::wall::Wall::from_state(state).is_some()
                || native_world::collision::pane_waterlogged(state).is_some()
                || native_world::collision::trapdoor_state(state).is_some()
                || jigsaw_lantern(state)
                || jigsaw_candle(state)
                || state.name == "minecraft:chest")
            && self
                .registry
                .is_canonical_state(&state.name, &state.properties))
    }
    fn place_liquid(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        fluid: Option<FluidState>,
    ) -> Result<()> {
        if state.properties.get("waterlogged").map(String::as_str) == Some("false")
            && !(state.name.ends_with("_slab")
                && state.properties.get("type").map(String::as_str) == Some("double"))
            && fluid.is_some_and(|fluid| fluid.kind == FluidKind::Water && fluid.source)
        {
            let mut wet = state.clone();
            wet.properties.insert("waterlogged".into(), "true".into());
            if jigsaw_candle(state) {
                // WorldGenRegion ignores the extinguish sound, particles and
                // game event; the block update still clears the lit flag.
                wet.properties.insert("lit".into(), "false".into());
            }
            if self.region.writable.contains(&chunk(pos)) && !self.region.is_outside(pos[1]) {
                let chest = if state.name == "minecraft:chest" {
                    let Some(Some(chest)) =
                        Arc::make_mut(&mut self.region.monster_entities).remove(&pos)
                    else {
                        bail!("jigsaw chest entity is missing")
                    };
                    Some(chest)
                } else {
                    None
                };
                self.region.jigsaw_block_write = native_world::collision::slab_stair_solid(state)
                    .is_some()
                    || native_world::wall::Wall::from_state(state).is_some()
                    || native_world::collision::fence_waterlogged(state).is_some()
                    || native_world::collision::trapdoor_state(state).is_some()
                    || native_world::collision::pane_waterlogged(state).is_some()
                    || jigsaw_lantern(state)
                    || jigsaw_candle(state);
                let result = self.region.set_proto_block(pos, &wet);
                self.region.jigsaw_block_write = false;
                result?;
                if let Some(mut chest) = chest {
                    match &mut chest {
                        super::super::GeneratedMonsterRoomEntity::Chest(chest) => {
                            chest.update_state(wet)?
                        }
                        super::super::GeneratedMonsterRoomEntity::EmptyChest(chest) => {
                            chest.state = wet
                        }
                        _ => bail!("unsupported waterlogged chest entity"),
                    }
                    Arc::make_mut(&mut self.region.monster_entities).insert(pos, Some(chest));
                }
            }
            SpringWorld::schedule_spring_tick(
                self.region,
                pos,
                &ResourceId::parse("minecraft:water")?,
            )?;
        }
        Ok(())
    }
    fn mark_block_entity_changed(&mut self, pos: [i32; 3]) -> Result<()> {
        ensure!(
            matches!(
                self.region.monster_entities.get(&pos),
                Some(Some(
                    super::super::GeneratedMonsterRoomEntity::Chest(_)
                        | super::super::GeneratedMonsterRoomEntity::EmptyChest(_)
                ))
            ),
            "jigsaw block entity marking is not yet supported"
        );
        Ok(())
    }
    fn place_template_entities(
        &mut self,
        entities: &[Tag],
        origin: [i32; 3],
        rotation: native_resources::structure_transform::Rotation,
        clip: BoundingBox,
    ) -> Result<()> {
        for entity in entities {
            if let Some((position, data)) = super::super::template_entities::draft_template_entity(
                entity,
                origin,
                rotation,
                [0; 3],
                clip,
                super::super::fresh_entity_uuid_from_entropy,
            )? {
                self.region.record_generated_entity(
                    position,
                    super::super::GeneratedEntityRecord::Saved(data),
                )?;
            }
        }
        Ok(())
    }
}

/// Reuse all block and fluid handling while providing the same region RNG to
/// every template mob in a mixed decoration pass.
struct JigsawRegionWithMobRandom<'r, 'a, 'g, 'm> {
    inner: JigsawRegion<'r, 'a, 'g>,
    random: &'m mut native_resources::worldgen_region_random::RegionRandom,
}

impl JigsawPlacementWorld for JigsawRegionWithMobRandom<'_, '_, '_, '_> {
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        self.inner.block(pos)
    }
    fn world_surface_height(&mut self, x: i32, z: i32) -> Result<i32> {
        self.inner.world_surface_height(x, z)
    }
    fn fluid_at(&mut self, pos: [i32; 3]) -> Result<Option<FluidState>> {
        self.inner.fluid_at(pos)
    }
    fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>> {
        self.inner.fluid_of_state(state)
    }
    fn place_template_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        self.inner.place_template_block(pos, state, nbt, random)
    }
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
        self.inner.is_liquid_container(state)
    }
    fn place_liquid(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        fluid: Option<FluidState>,
    ) -> Result<()> {
        self.inner.place_liquid(pos, state, fluid)
    }
    fn mark_block_entity_changed(&mut self, pos: [i32; 3]) -> Result<()> {
        self.inner.mark_block_entity_changed(pos)
    }
    fn place_template_entities(
        &mut self,
        entities: &[Tag],
        origin: [i32; 3],
        rotation: native_resources::structure_transform::Rotation,
        clip: BoundingBox,
    ) -> Result<()> {
        for entity in entities {
            let is_villager = matches!(entity.get("nbt").and_then(|data| data.get("id")), Some(Tag::String(id)) if id.matches("minecraft:villager"));
            let draft = if is_villager {
                super::super::generated_village_villager::draft_village_template_villager(
                    entity,
                    origin,
                    rotation,
                    [0; 3],
                    clip,
                    self.random,
                    super::super::fresh_entity_uuid_from_entropy,
                )?
            } else {
                super::super::template_entities::draft_template_entity(
                    entity,
                    origin,
                    rotation,
                    [0; 3],
                    clip,
                    super::super::fresh_entity_uuid_from_entropy,
                )?
            };
            if let Some((position, data)) = draft {
                self.inner.region.record_generated_entity(
                    position,
                    super::super::GeneratedEntityRecord::Saved(data),
                )?;
            }
        }
        Ok(())
    }
}

impl DecorationRegion<'_> {
    /// Like `place_jigsaw_start`, with the independent WorldGenRegion RNG used
    /// by template mobs. Both RNGs are committed only after the start succeeds.
    pub fn place_jigsaw_start_with_region_random(
        &mut self,
        prepared: &PreparedJigsawPlacement,
        start: &PreparedStart,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
        random: &mut DecorationRandom,
        region_random: &mut native_resources::worldgen_region_random::RegionRandom,
    ) -> Result<usize> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut staged_region_random = region_random.clone();
        let result = prepared.place_with(start, clip, random, |piece, liquids, next| match piece {
            JigsawElement::Template(piece) => self.place_rigid_jigsaw_piece_with_region_random(
                piece,
                registry,
                clip,
                liquids,
                next,
                &mut staged_region_random,
            ),
            JigsawElement::Feature(feature, origin) => {
                let (min_y, height) = self
                    .jigsaw_feature_bounds
                    .context("jigsaw feature generator bounds are missing")?;
                self.place_nested(feature, next, origin, min_y, height)
            }
        });
        match result {
            Ok(placed) => {
                *region_random = staged_region_random;
                Ok(placed)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    /// Place every intersecting piece of a preflighted start as one staged
    /// operation. A failure prevents publication and preserves the RNG.
    pub fn place_jigsaw_start(
        &mut self,
        prepared: &PreparedJigsawPlacement,
        start: &PreparedStart,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<usize> {
        ensure!(!self.failed, "generation attempt previously failed");
        let result = prepared.place_with(start, clip, random, |piece, liquids, next| match piece {
            JigsawElement::Template(piece) => {
                self.place_rigid_jigsaw_piece(piece, registry, clip, liquids, next)
            }
            JigsawElement::Feature(feature, origin) => {
                let (min_y, height) = self
                    .jigsaw_feature_bounds
                    .context("jigsaw feature generator bounds are missing")?;
                self.place_nested(feature, next, origin, min_y, height)
            }
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// Place one supported piece in this unpublished region. On failure,
    /// `finish` refuses to publish any of the region's intermediate writes.
    pub fn place_rigid_jigsaw_piece(
        &mut self,
        piece: &PreparedTemplatePiece,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
        liquids: LiquidSettings,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        let result = piece.place(
            &mut JigsawRegion {
                region: self,
                registry,
            },
            registry,
            clip,
            liquids,
            jigsaw_static_full,
            random,
        )?;
        self.failed = false;
        Ok(result)
    }

    fn place_rigid_jigsaw_piece_with_region_random(
        &mut self,
        piece: &PreparedTemplatePiece,
        registry: &GeneratedBlockRegistry,
        clip: BoundingBox,
        liquids: LiquidSettings,
        random: &mut DecorationRandom,
        region_random: &mut native_resources::worldgen_region_random::RegionRandom,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        let result = piece.place(
            &mut JigsawRegionWithMobRandom {
                inner: JigsawRegion {
                    region: self,
                    registry,
                },
                random: region_random,
            },
            registry,
            clip,
            liquids,
            jigsaw_static_full,
            random,
        )?;
        self.failed = false;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use native_persistence::{
        nbt::{self, NamedTag, TagType},
        structure_processors::PreparedProcessors,
    };
    use native_resources::{
        jigsaw_placement::Piece,
        structure_pool::{HolderValue, PoolElement, Projection},
        structure_transform::Rotation,
    };
    use std::io::Write;

    struct TemplateData {
        bytes: Vec<u8>,
        reads: usize,
    }
    impl ResourceProvider for TemplateData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            self.reads += 1;
            Ok(Some(self.bytes.clone()))
        }
    }
    fn ints(values: &[i32]) -> Tag {
        Tag::List {
            element_type: TagType::Int,
            values: values.iter().copied().map(Tag::Int).collect(),
        }
    }
    fn template(with_unsupported_nbt: bool) -> Result<TemplateData> {
        template_with_entities(with_unsupported_nbt, Vec::new())
    }

    fn template_with_entities(
        with_unsupported_nbt: bool,
        entities: Vec<Tag>,
    ) -> Result<TemplateData> {
        let mut second = vec![
            ("pos".into(), ints(&[1, 0, 0])),
            ("state".into(), Tag::Int(0)),
        ];
        if with_unsupported_nbt {
            second.push(("nbt".into(), Tag::Compound(vec![])));
        }
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("size".into(), ints(&[2, 1, 1])),
                (
                    "palette".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![(
                            "id".into(),
                            Tag::String("minecraft:stone".into()),
                        )])],
                    },
                ),
                (
                    "blocks".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![
                            Tag::Compound(vec![
                                ("pos".into(), ints(&[0, 0, 0])),
                                ("state".into(), Tag::Int(0)),
                            ]),
                            Tag::Compound(second),
                        ],
                    },
                ),
                (
                    "entities".into(),
                    Tag::List {
                        element_type: if entities.is_empty() {
                            TagType::End
                        } else {
                            TagType::Compound
                        },
                        values: entities,
                    },
                ),
            ]),
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&document)?)?;
        Ok(TemplateData {
            bytes: encoder.finish()?,
            reads: 0,
        })
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn shipwreck_empty_chest_matches_java_and_waterlogs() -> Result<()> {
        use native_persistence::{
            jigsaw_template::resolve_template_state, nbt, structure_template::StructureTemplate,
        };
        use std::{path::PathBuf, process::Command};

        fn canonical(tag: Tag) -> Tag {
            match tag {
                Tag::Compound(fields) => {
                    let mut fields: Vec<_> = fields
                        .into_iter()
                        .map(|(key, value)| (key, canonical(value)))
                        .collect();
                    fields.sort_by(|a, b| a.0.0.cmp(&b.0.0));
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
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let blocks = resources.vanilla_block_ids()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        let template = StructureTemplate::load(
            &mut resources,
            &ResourceId::parse("minecraft:shipwreck/with_mast")?,
        )?
        .context("missing shipwreck template")?;
        let entry = template.blocks.iter().find(|block| matches!(block.nbt.as_ref().and_then(|nbt| nbt.get("id")), Some(Tag::String(id)) if id.matches("minecraft:chest"))).context("missing shipwreck chest")?;
        let state = resolve_template_state(&template.palettes[0][entry.state], &registry)?;
        let tag = entry.nbt.as_ref().unwrap();
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        let first_seed = expected.next_long();
        let pos = [1, 8, 1];
        {
            let mut draft = JigsawRegion {
                region: &mut region,
                registry: &registry,
            };
            assert!(draft.place_template_block(pos, &state, Some(tag), &mut random)?);
            draft.mark_block_entity_changed(pos)?;
            draft.place_liquid(
                pos,
                &state,
                Some(FluidState {
                    kind: FluidKind::Water,
                    source: true,
                    falling: false,
                    amount: 8,
                }),
            )?;
        }
        region.finish()?;
        let next = expected.next_long();
        assert_eq!(random.next_long(), next);
        assert_eq!(
            terrain
                .snapshot()
                .block(pos[0], pos[1], pos[2])
                .unwrap()
                .properties["waterlogged"],
            "true"
        );
        let Tag::List { values, .. } =
            terrain.generated_block_entity_data(ChunkPos { x: 0, z: 0 }, &registry)?
        else {
            bail!("missing entity list")
        };
        ensure!(values.len() == 1, "unexpected block entity count");

        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("JigsawEmptyChestOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/JigsawEmptyChestOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output_path = temp.path().join("chest.nbt");
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(cp)
            .arg("JigsawEmptyChestOracle")
            .arg(&output_path)
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8(output.stdout)?
                .lines()
                .any(|line| line == format!("E {first_seed} {next}"))
        );
        let java_tag = nbt::read(&std::fs::read(output_path)?)?.value;
        assert_eq!(canonical(values[0].clone()), canonical(java_tag));

        let mut invalid = tag.clone();
        if let Tag::Compound(fields) = &mut invalid {
            let (_, items) = fields
                .iter_mut()
                .find(|(key, _)| key.matches("Items"))
                .context("missing items")?;
            *items = Tag::List {
                element_type: native_persistence::nbt::TagType::Compound,
                values: vec![Tag::Compound(vec![])],
            };
        }
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        assert!(
            JigsawRegion {
                region: &mut region,
                registry: &registry
            }
            .place_template_block(pos, &state, Some(&invalid), &mut random)
            .is_err()
        );
        assert_eq!(random.next_long(), expected.next_long());
        drop(region);
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn template_loot_chest_retains_seed_state_and_rolls_back_unknown_data() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let blocks = resources.vanilla_block_ids()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        let state = BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        };
        let tag = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:chest".into())),
            (
                "LootTable".into(),
                Tag::String("minecraft:chests/village/village_armorer".into()),
            ),
            ("LootTableSeed".into(), Tag::Long(123)),
        ]);
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        let seed = expected.next_long();
        let pos = [1, 8, 1];
        {
            let mut draft = JigsawRegion {
                region: &mut region,
                registry: &registry,
            };
            assert!(draft.place_template_block(pos, &state, Some(&tag), &mut random)?);
            draft.mark_block_entity_changed(pos)?;
            draft.place_liquid(
                pos,
                &state,
                Some(FluidState {
                    kind: FluidKind::Water,
                    source: true,
                    falling: false,
                    amount: 8,
                }),
            )?;
        }
        region.finish()?;
        let next_random = expected.next_long();
        assert_eq!(random.next_long(), next_random);
        assert_eq!(
            terrain
                .snapshot()
                .block(pos[0], pos[1], pos[2])
                .unwrap()
                .properties["waterlogged"],
            "true"
        );
        let data = terrain.generated_block_entity_data(ChunkPos { x: 0, z: 0 }, &registry)?;
        let Tag::List { values, .. } = data else {
            bail!("missing entity list")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].get("LootTableSeed"), Some(&Tag::Long(seed)));
        assert!(
            matches!(values[0].get("LootTable"), Some(Tag::String(id)) if id.matches("minecraft:chests/village/village_armorer"))
        );

        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(std::path::PathBuf::from)
            .context("Java 25 home is missing")?
            .join("bin");
        let source = temp.path().join("JigsawChestOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/JigsawChestOracle.java"),
        )?;
        let compiled = std::process::Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("JigsawChestOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let expected_line =
            format!("CHEST\tminecraft:chests/village/village_armorer\t{seed}\t{next_random}");
        let output = String::from_utf8(output.stdout)?;
        assert!(output.lines().any(|line| line == expected_line));
        assert!(output.lines().any(|line| line == "PROTO_WATER_TICK\t0"));
        assert!(
            output
                .lines()
                .any(|line| line == "TRAPDOOR_WATER_UPDATE\t12")
        );
        let mut states_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("STATE\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                facing,
                chest_type,
                waterlogged,
                mirror,
                rotation,
                expected_facing,
                expected_type,
                expected_wet,
            ] = fields.as_slice()
            else {
                bail!("invalid Java chest state result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let properties = BTreeMap::from([
                ("facing".into(), (*facing).to_owned()),
                ("type".into(), (*chest_type).to_owned()),
                ("waterlogged".into(), (*waterlogged).to_owned()),
            ]);
            let actual = registry.transform_template_state(
                "minecraft:chest",
                &properties,
                mirror,
                rotation,
            )?;
            assert_eq!(actual["facing"], *expected_facing);
            assert_eq!(actual["type"], *expected_type);
            assert_eq!(actual["waterlogged"], *expected_wet);
            states_checked += 1;
        }
        assert_eq!(states_checked, 288);
        let mut lanterns_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("LANTERN\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                hanging,
                wet,
                mirror,
                rotation,
                expected_hanging,
                expected_wet,
                emission,
            ] = fields.as_slice()
            else {
                bail!("invalid Java lantern result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let id = format!("minecraft:{name}");
            let properties = BTreeMap::from([
                ("hanging".into(), (*hanging).into()),
                ("waterlogged".into(), (*wet).into()),
            ]);
            let state = BlockState {
                name: id.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&id, &properties));
            assert_eq!(registry.has_block_entity(&id), Some(false));
            assert!(jigsaw_lantern(&state));
            assert!(!jigsaw_static_full(&state)?);
            let transformed =
                registry.transform_template_state(&id, &properties, mirror, rotation)?;
            assert_eq!(transformed["hanging"], *expected_hanging);
            assert_eq!(transformed["waterlogged"], *expected_wet);
            assert_eq!(
                FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some_and(|fluid| fluid.source),
                *wet == "true"
            );
            assert_eq!(*emission, if *name == "lantern" { "15" } else { "10" });
            lanterns_checked += 1;
        }
        assert_eq!(lanterns_checked, 96);
        let mut candles_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("CANDLE\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                count,
                lit,
                wet,
                mirror,
                rotation,
                emission,
                dampening,
                x0,
                z0,
                x1,
                z1,
            ] = fields.as_slice()
            else {
                bail!("invalid Java candle result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let id = format!("minecraft:{name}");
            let properties = BTreeMap::from([
                ("candles".into(), (*count).into()),
                ("lit".into(), (*lit).into()),
                ("waterlogged".into(), (*wet).into()),
            ]);
            let state = BlockState {
                name: id.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&id, &properties), "{line}");
            assert_eq!(registry.has_block_entity(&id), Some(false));
            assert!(jigsaw_candle(&state));
            assert!(!jigsaw_static_full(&state)?);
            assert_eq!(
                registry.transform_template_state(&id, &properties, mirror, rotation)?,
                properties
            );
            let boxes =
                native_world::collision::block_shape(&state).context("candle shape missing")?;
            assert_eq!(boxes.len(), 1);
            let b = boxes[0];
            assert_eq!(
                [
                    b.min.x * 16.0,
                    b.min.y * 16.0,
                    b.min.z * 16.0,
                    b.max.x * 16.0,
                    b.max.y * 16.0,
                    b.max.z * 16.0
                ],
                [
                    x0.parse::<f32>()?,
                    0.0,
                    z0.parse::<f32>()?,
                    x1.parse::<f32>()?,
                    6.0,
                    z1.parse::<f32>()?
                ],
                "{line}"
            );
            assert_eq!(
                *emission,
                if *lit == "true" {
                    (count.parse::<u8>()? * 3).to_string()
                } else {
                    "0".to_string()
                }
            );
            assert_eq!(*dampening, if *wet == "true" { "1" } else { "0" });
            assert_eq!(
                FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some_and(|fluid| fluid.source),
                *wet == "true"
            );
            let air = BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            };
            assert_eq!(
                native_world::block_light::same_light_behavior(&air, &state),
                Some(*lit == "false" && *wet == "false")
            );
            candles_checked += 1;
        }
        assert_eq!(candles_checked, 3264);
        let mut liquids_checked = 0;
        for line in output
            .lines()
            .filter(|line| line.starts_with("CANDLE_LIQUID\t"))
        {
            let fields: Vec<_> = line.split('\t').collect();
            let [_, count, lit, wet, source, changed, result_lit, result_wet] = fields.as_slice()
            else {
                bail!("invalid Java candle liquid result")
            };
            let state = BlockState {
                name: "minecraft:candle".into(),
                properties: BTreeMap::from([
                    ("candles".into(), (*count).into()),
                    ("lit".into(), (*lit).into()),
                    ("waterlogged".into(), (*wet).into()),
                ]),
            };
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let pos = [1, 8, 1];
            let mut random = DecorationRandom::new(42);
            let mut expected_random = random.clone();
            {
                let mut draft = JigsawRegion {
                    region: &mut region,
                    registry: &registry,
                };
                assert!(draft.place_template_block(pos, &state, None, &mut random)?);
                assert!(draft.is_liquid_container(&state)?);
                let source = *source == "true";
                draft.place_liquid(
                    pos,
                    &state,
                    Some(FluidState {
                        kind: FluidKind::Water,
                        source,
                        falling: false,
                        amount: if source { 8 } else { 7 },
                    }),
                )?;
            }
            region.finish()?;
            assert_eq!(random.next_long(), expected_random.next_long());
            let world = terrain.snapshot();
            let placed = world
                .block(pos[0], pos[1], pos[2])
                .context("missing candle")?;
            assert_eq!(placed.properties["candles"], *count);
            assert_eq!(placed.properties["lit"], *result_lit, "{line}");
            assert_eq!(placed.properties["waterlogged"], *result_wet, "{line}");
            let ticks = world.saved_fluid_ticks.get(&ChunkPos { x: 0, z: 0 });
            if *changed == "true" {
                let ticks = ticks.context("missing candle water tick")?;
                assert_eq!(ticks.len(), 1);
                assert_eq!(ticks[0].position, pos);
                assert_eq!(ticks[0].fluid, "minecraft:water");
                assert_eq!(ticks[0].delay, 0);
            } else {
                assert!(ticks.is_none_or(|ticks| ticks.is_empty()));
            }
            liquids_checked += 1;
        }
        assert_eq!(liquids_checked, 32);
        let mut stones_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("STONE\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [_, name, full, block_entity, state_count] = fields.as_slice() else {
                bail!("invalid Java stone result")
            };
            let id = format!("minecraft:{name}");
            let properties = registry
                .default_properties(&id)
                .with_context(|| format!("missing stone state {id}"))?;
            let state = BlockState {
                name: id.clone(),
                properties,
            };
            assert_eq!(jigsaw_static_full(&state)?, *full == "true", "{id}");
            assert_eq!(
                registry.has_block_entity(&id),
                Some(*block_entity == "true"),
                "{id}"
            );
            assert_eq!(*full, "true", "{id}");
            assert_eq!(*block_entity, "false", "{id}");
            if name.ends_with("_wool") {
                assert_eq!(*state_count, "1", "{id}");
            }
            stones_checked += 1;
        }
        assert_eq!(stones_checked, 29);
        let mut wood_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("WOOD\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                axis,
                mirror,
                rotation,
                expected_axis,
                full,
                entity,
                poi,
                postprocess,
            ] = fields.as_slice()
            else {
                bail!("invalid Java wood result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let id = format!("minecraft:{name}");
            let properties = if *axis == "-" {
                BTreeMap::new()
            } else {
                BTreeMap::from([("axis".into(), (*axis).to_owned())])
            };
            let state = BlockState {
                name: id.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&id, &properties), "{state:?}");
            assert_eq!(*full, "true", "{state:?}");
            assert_eq!(*entity, "false", "{state:?}");
            assert_eq!(*poi, "false", "{state:?}");
            assert_eq!(*postprocess, "false", "{state:?}");
            assert_eq!(registry.has_block_entity(&id), Some(false), "{state:?}");
            assert!(jigsaw_static_full(&state)?);
            let transformed =
                registry.transform_template_state(&id, &properties, mirror, rotation)?;
            assert_eq!(
                transformed.get("axis").map(String::as_str).unwrap_or("-"),
                *expected_axis,
                "{state:?}"
            );
            wood_checked += 1;
        }
        assert_eq!(
            wood_checked,
            (native_core::wood::Wood::all().count() * 3 + native_core::wood::Planks::ALL.len())
                * 12
        );
        let mut axes_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("AXIS\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [_, axis, mirror, rotation, expected_axis] = fields.as_slice() else {
                bail!("invalid Java axis result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let properties = BTreeMap::from([("axis".into(), (*axis).to_owned())]);
            let actual = registry.transform_template_state(
                "minecraft:deepslate",
                &properties,
                mirror,
                rotation,
            )?;
            assert_eq!(actual["axis"], *expected_axis);
            axes_checked += 1;
        }
        assert_eq!(axes_checked, 36);
        let mut slabs_checked = 0;
        let mut slab_names = BTreeSet::new();
        for line in output.lines().filter(|line| line.starts_with("SLAB\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                kind,
                wet,
                mirror,
                rotation,
                full,
                source,
                expected_kind,
                expected_wet,
            ] = fields.as_slice()
            else {
                bail!("invalid Java slab result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let name = format!("minecraft:{name}");
            slab_names.insert(name.clone());
            let properties = BTreeMap::from([
                ("type".into(), (*kind).to_owned()),
                ("waterlogged".into(), (*wet).to_owned()),
            ]);
            let state = BlockState {
                name: name.clone(),
                properties: properties.clone(),
            };
            assert!(
                registry.is_canonical_state(&name, &properties),
                "{name}: {properties:?}"
            );
            assert_eq!(jigsaw_static_full(&state)?, *full == "true");
            assert_eq!(
                FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some_and(|fluid| fluid.source),
                *source == "true"
            );
            let actual = registry.transform_template_state(&name, &properties, mirror, rotation)?;
            assert_eq!(actual["type"], *expected_kind);
            assert_eq!(actual["waterlogged"], *expected_wet);
            slabs_checked += 1;
        }
        let expected_slabs: BTreeSet<_> = registry
            .ids()
            .filter(|name| name.starts_with("minecraft:") && name.ends_with("_slab"))
            .map(str::to_owned)
            .collect();
        assert_eq!(slab_names, expected_slabs);
        assert_eq!(slabs_checked, expected_slabs.len() * 72);
        let mut fence_names = BTreeSet::new();
        let mut fences_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("FENCE\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                before,
                wet,
                mirror,
                rotation,
                after,
                expected_wet,
                source,
            ] = fields.as_slice()
            else {
                bail!("invalid Java fence result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let mask: u8 = before.parse()?;
            let mut properties = BTreeMap::new();
            for (i, direction) in ["north", "east", "south", "west"].iter().enumerate() {
                properties.insert((*direction).into(), ((mask & (1 << i)) != 0).to_string());
            }
            properties.insert("waterlogged".into(), (*wet).into());
            let name = format!("minecraft:{name}");
            let state = BlockState {
                name: name.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&name, &properties), "{state:?}");
            assert_eq!(registry.has_block_entity(&name), Some(false), "{state:?}");
            assert!(!jigsaw_static_full(&state)?);
            assert_eq!(
                FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some_and(|fluid| fluid.source),
                *source == "true"
            );
            let transformed =
                registry.transform_template_state(&name, &properties, mirror, rotation)?;
            let mut actual = 0;
            for (i, direction) in ["north", "east", "south", "west"].iter().enumerate() {
                if transformed[*direction] == "true" {
                    actual |= 1 << i;
                }
            }
            assert_eq!(
                actual,
                after.parse::<u8>()?,
                "{state:?} {mirror:?} {rotation:?}"
            );
            assert_eq!(transformed["waterlogged"], *expected_wet);
            fence_names.insert(name);
            fences_checked += 1;
        }
        let expected_fences: BTreeSet<_> = registry
            .ids()
            .filter(|name| name.starts_with("minecraft:") && name.ends_with("_fence"))
            .map(str::to_owned)
            .collect();
        assert_eq!(fence_names, expected_fences);
        assert_eq!(fences_checked, expected_fences.len() * 32 * 12);
        let parse_shape = |raw: &str| -> Result<Vec<[f32; 6]>> {
            if raw == "-" {
                return Ok(Vec::new());
            }
            let values = raw
                .split(',')
                .map(str::parse::<f32>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let values: [f32; 6] = values
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid Java gate shape"))?;
            Ok(vec![values])
        };
        let native_shape = |boxes: Vec<native_world::collision::Aabb>| {
            boxes
                .into_iter()
                .map(|a| [a.min.x, a.min.y, a.min.z, a.max.x, a.max.y, a.max.z])
                .collect::<Vec<_>>()
        };
        let mut gates_checked = 0;
        let mut gate_names = BTreeSet::new();
        for line in output.lines().filter(|line| line.starts_with("GATE\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                facing,
                in_wall,
                open,
                powered,
                mirror,
                rotation,
                expected_facing,
                collision,
                outline,
            ] = fields.as_slice()
            else {
                bail!("invalid Java gate result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let name = format!("minecraft:{name}");
            let properties = BTreeMap::from([
                ("facing".into(), (*facing).into()),
                ("in_wall".into(), (*in_wall).into()),
                ("open".into(), (*open).into()),
                ("powered".into(), (*powered).into()),
            ]);
            let state = BlockState {
                name: name.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&name, &properties), "{state:?}");
            assert_eq!(registry.has_block_entity(&name), Some(false), "{state:?}");
            assert!(!jigsaw_static_full(&state)?);
            assert_eq!(
                native_shape(
                    native_world::collision::block_shape(&state)
                        .context("gate collision missing")?
                ),
                parse_shape(collision)?,
                "{state:?}"
            );
            assert_eq!(
                native_shape(
                    native_world::collision::block_outline(&state)
                        .context("gate outline missing")?
                ),
                parse_shape(outline)?,
                "{state:?}"
            );
            let transformed =
                registry.transform_template_state(&name, &properties, mirror, rotation)?;
            assert_eq!(transformed["facing"], *expected_facing, "{state:?}");
            for key in ["in_wall", "open", "powered"] {
                assert_eq!(transformed[key], properties[key]);
            }
            gate_names.insert(name);
            gates_checked += 1;
        }
        assert_eq!(gate_names.len(), native_core::wood::Planks::ALL.len());
        assert_eq!(gates_checked, gate_names.len() * 32 * 12);
        let mut door_names = BTreeSet::new();
        let mut doors_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("DOOR\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                facing,
                half,
                hinge,
                open,
                powered,
                mirror,
                rotation,
                expected_facing,
                expected_hinge,
                collision,
                outline,
            ] = fields.as_slice()
            else {
                bail!("invalid Java door result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let name = format!("minecraft:{name}");
            let properties = BTreeMap::from([
                ("facing".into(), (*facing).into()),
                ("half".into(), (*half).into()),
                ("hinge".into(), (*hinge).into()),
                ("open".into(), (*open).into()),
                ("powered".into(), (*powered).into()),
            ]);
            let state = BlockState {
                name: name.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&name, &properties), "{state:?}");
            assert_eq!(registry.has_block_entity(&name), Some(false), "{state:?}");
            assert!(!jigsaw_static_full(&state)?);
            assert_eq!(
                native_shape(
                    native_world::collision::block_shape(&state)
                        .context("door collision missing")?
                ),
                parse_shape(collision)?,
                "{state:?}"
            );
            assert_eq!(
                native_shape(
                    native_world::collision::block_outline(&state)
                        .context("door outline missing")?
                ),
                parse_shape(outline)?,
                "{state:?}"
            );
            let transformed =
                registry.transform_template_state(&name, &properties, mirror, rotation)?;
            assert_eq!(transformed["facing"], *expected_facing, "{state:?}");
            assert_eq!(transformed["hinge"], *expected_hinge, "{state:?}");
            for key in ["half", "open", "powered"] {
                assert_eq!(transformed[key], properties[key]);
            }
            door_names.insert(name);
            doors_checked += 1;
        }
        assert_eq!(door_names.len(), native_core::wood::Planks::ALL.len() + 9);
        assert_eq!(doors_checked, door_names.len() * 64 * 12);
        let mut torches_checked = 0;
        let mut torch_names = BTreeSet::new();
        for line in output.lines().filter(|line| line.starts_with("TORCH\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                facing,
                mirror,
                rotation,
                expected_facing,
                collision,
                outline,
            ] = fields.as_slice()
            else {
                bail!("invalid Java torch result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let name = format!("minecraft:{name}");
            let properties = if *facing == "-" {
                BTreeMap::new()
            } else {
                BTreeMap::from([("facing".into(), (*facing).into())])
            };
            let state = BlockState {
                name: name.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&name, &properties), "{state:?}");
            assert_eq!(registry.has_block_entity(&name), Some(false), "{state:?}");
            assert!(!jigsaw_static_full(&state)?);
            assert_eq!(
                native_shape(
                    native_world::collision::block_shape(&state)
                        .context("torch collision missing")?
                ),
                parse_shape(collision)?,
                "{state:?}"
            );
            assert_eq!(
                native_shape(
                    native_world::collision::block_outline(&state)
                        .context("torch outline missing")?
                ),
                parse_shape(outline)?,
                "{state:?}"
            );
            let transformed =
                registry.transform_template_state(&name, &properties, mirror, rotation)?;
            assert_eq!(
                transformed.get("facing").map(String::as_str).unwrap_or("-"),
                *expected_facing,
                "{state:?}"
            );
            torch_names.insert(name);
            torches_checked += 1;
        }
        assert_eq!(torch_names.len(), 4);
        assert_eq!(torches_checked, 10 * 12);
        let shape_mask = |boxes: Vec<native_world::collision::Aabb>| -> Vec<u16> {
            (0..16)
                .map(|z| {
                    (0..16).fold(0u16, |row, x| {
                        let px = (x as f32 + 0.5) / 16.0;
                        let pz = (z as f32 + 0.5) / 16.0;
                        if boxes.iter().any(|a| {
                            a.min.x < px
                                && a.max.x > px
                                && a.min.y < 0.5
                                && a.max.y > 0.5
                                && a.min.z < pz
                                && a.max.z > pz
                        }) {
                            row | (1 << x)
                        } else {
                            row
                        }
                    })
                })
                .collect()
        };
        let parse_mask = |raw: &str| -> Result<Vec<u16>> {
            Ok(raw
                .split(',')
                .map(str::parse::<u16>)
                .collect::<std::result::Result<Vec<_>, _>>()?)
        };
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        };
        let water = BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        };
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        };
        let mut terracotta_checked = 0;
        for line in output
            .lines()
            .filter(|line| line.starts_with("TERRACOTTA\t"))
        {
            let name = line.strip_prefix("TERRACOTTA\t").unwrap();
            let state = BlockState {
                name: format!("minecraft:{name}"),
                properties: BTreeMap::new(),
            };
            assert!(registry.is_canonical_state(&state.name, &state.properties));
            assert_eq!(registry.has_block_entity(&state.name), Some(false));
            assert!(jigsaw_static_full(&state)?);
            assert_eq!(
                native_world::block_light::same_light_behavior(&stone, &state),
                Some(true)
            );
            assert_eq!(
                native_world::block_light::same_light_behavior(&air, &state),
                Some(false)
            );
            terracotta_checked += 1;
        }
        assert_eq!(terracotta_checked, 17);
        let volume = |boxes: &[native_world::collision::Aabb]| -> Vec<bool> {
            (0..16)
                .flat_map(|y| {
                    (0..16).flat_map(move |z| {
                        (0..16).map(move |x| {
                            let point = glam::Vec3::new(
                                (x as f32 + 0.5) / 16.0,
                                (y as f32 + 0.5) / 16.0,
                                (z as f32 + 0.5) / 16.0,
                            );
                            boxes
                                .iter()
                                .any(|b| point.cmpgt(b.min).all() && point.cmplt(b.max).all())
                        })
                    })
                })
                .collect()
        };
        let parse_boxes = |raw: &str| -> Result<Vec<native_world::collision::Aabb>> {
            raw.split(';')
                .map(|entry| {
                    let nums = entry
                        .split(',')
                        .map(str::parse::<f32>)
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    ensure!(nums.len() == 6, "invalid Java bed box");
                    Ok(native_world::collision::Aabb {
                        min: glam::Vec3::new(nums[0], nums[1], nums[2]),
                        max: glam::Vec3::new(nums[3], nums[4], nums[5]),
                    })
                })
                .collect()
        };
        let mut beds_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("BED\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [_, name, facing, part, occupied, poi, collision, outline] = fields.as_slice()
            else {
                bail!("invalid Java bed result")
            };
            let state = BlockState {
                name: format!("minecraft:{name}"),
                properties: BTreeMap::from([
                    ("facing".into(), (*facing).into()),
                    ("part".into(), (*part).into()),
                    ("occupied".into(), (*occupied).into()),
                ]),
            };
            assert!(
                registry.is_canonical_state(&state.name, &state.properties),
                "{state:?}"
            );
            assert_eq!(
                registry.has_block_entity(&state.name),
                Some(false),
                "{state:?}"
            );
            assert_eq!(jigsaw_static_full(&state)?, false);
            assert_eq!(*poi == "true", *part == "head");
            assert_eq!(
                native_world::block_light::same_light_behavior(&air, &state),
                Some(true)
            );
            assert_eq!(
                native_world::block_light::same_light_behavior(&water, &state),
                Some(false)
            );
            assert_eq!(
                volume(
                    &native_world::collision::block_shape(&state)
                        .context("bed collision missing")?
                ),
                volume(&parse_boxes(collision)?),
                "{state:?}"
            );
            assert_eq!(
                volume(
                    &native_world::collision::block_outline(&state)
                        .context("bed outline missing")?
                ),
                volume(&parse_boxes(outline)?),
                "{state:?}"
            );
            beds_checked += 1;
        }
        assert_eq!(beds_checked, 16 * 16);
        let mut bed_transforms = 0;
        for line in output
            .lines()
            .filter(|line| line.starts_with("BED_TRANS\t"))
        {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                facing,
                part,
                occupied,
                mirror,
                rotation,
                next_facing,
                next_part,
                next_occupied,
            ] = fields.as_slice()
            else {
                bail!("invalid Java bed transform")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown rotation"),
            };
            let properties = BTreeMap::from([
                ("facing".into(), (*facing).into()),
                ("part".into(), (*part).into()),
                ("occupied".into(), (*occupied).into()),
            ]);
            let next = registry.transform_template_state(
                &format!("minecraft:{name}"),
                &properties,
                mirror,
                rotation,
            )?;
            assert_eq!(next["facing"], *next_facing);
            assert_eq!(next["part"], *next_part);
            assert_eq!(next["occupied"], *next_occupied);
            bed_transforms += 1;
        }
        assert_eq!(bed_transforms, 16 * 16 * 12);
        let mut glass_names = BTreeSet::new();
        let mut glass_checked = 0;
        for line in output.lines().filter(|line| line.starts_with("GLASS\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            let [
                _,
                name,
                pane,
                before,
                wet,
                mirror,
                rotation,
                after,
                collision,
                outline,
            ] = fields.as_slice()
            else {
                bail!("invalid Java glass result")
            };
            let mirror = match *mirror {
                "NONE" => native_resources::structure_transform::Mirror::None,
                "LEFT_RIGHT" => native_resources::structure_transform::Mirror::LeftRight,
                "FRONT_BACK" => native_resources::structure_transform::Mirror::FrontBack,
                _ => bail!("unknown Java mirror"),
            };
            let rotation = match *rotation {
                "NONE" => Rotation::None,
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Clockwise180,
                "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
                _ => bail!("unknown Java rotation"),
            };
            let is_pane = *pane == "true";
            let mask: u8 = before.parse()?;
            let mut properties = BTreeMap::new();
            if is_pane {
                for (i, direction) in ["north", "east", "south", "west"].iter().enumerate() {
                    properties.insert((*direction).into(), ((mask & (1 << i)) != 0).to_string());
                }
                properties.insert("waterlogged".into(), (*wet).into());
            }
            let name = format!("minecraft:{name}");
            let state = BlockState {
                name: name.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&name, &properties), "{state:?}");
            assert_eq!(registry.has_block_entity(&name), Some(false), "{state:?}");
            assert_eq!(jigsaw_static_full(&state)?, !is_pane);
            assert_eq!(
                native_world::block_light::same_light_behavior(&air, &state),
                Some(*wet != "true"),
                "{state:?}"
            );
            assert_eq!(
                native_world::block_light::same_light_behavior(&water, &state),
                Some(*wet == "true"),
                "{state:?}"
            );
            assert_eq!(
                shape_mask(
                    native_world::collision::block_shape(&state)
                        .context("glass collision missing")?
                ),
                parse_mask(collision)?,
                "{state:?}"
            );
            assert_eq!(
                shape_mask(
                    native_world::collision::block_outline(&state)
                        .context("glass outline missing")?
                ),
                parse_mask(outline)?,
                "{state:?}"
            );
            let transformed =
                registry.transform_template_state(&name, &properties, mirror, rotation)?;
            let mut actual = 0;
            if is_pane {
                for (i, direction) in ["north", "east", "south", "west"].iter().enumerate() {
                    if transformed[*direction] == "true" {
                        actual |= 1 << i;
                    }
                }
                assert_eq!(transformed["waterlogged"], *wet);
            }
            assert_eq!(actual, after.parse::<u8>()?, "{state:?}");
            glass_names.insert(name);
            glass_checked += 1;
        }
        assert_eq!(glass_names.len(), 43);
        assert_eq!(glass_checked, 849 * 12);

        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut invalid = tag;
        if let Tag::Compound(fields) = &mut invalid {
            fields.push(("CustomName".into(), Tag::String("unsupported".into())));
        }
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        let mut draft = JigsawRegion {
            region: &mut region,
            registry: &registry,
        };
        assert!(
            draft
                .place_template_block(pos, &state, Some(&invalid), &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), expected.next_long());
        drop(region);
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn vanilla_plains_house_wood_and_shapes_are_placed_from_template() -> Result<()> {
        use native_persistence::{
            jigsaw_template::resolve_template_state, structure_template::StructureTemplate,
        };
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let blocks = resources.vanilla_block_ids()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        let mut found = BTreeSet::new();
        for name in [
            "plains_small_house_1",
            "plains_small_house_2",
            "plains_small_house_3",
            "plains_animal_pen_1",
        ] {
            let id = ResourceId::parse(&format!("minecraft:village/plains/houses/{name}"))?;
            let template = StructureTemplate::load(&mut resources, &id)?
                .context("village template missing")?;
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut expected_beds = BTreeSet::new();
            for block in &template.blocks {
                let raw = &template.palettes[0][block.state];
                let Some(Tag::String(id)) = raw.get("id") else {
                    continue;
                };
                let id = String::from_utf16(&id.0)?;
                if native_core::wood::Wood::from_id(&id).is_none()
                    && native_core::wood::Planks::from_id(&id).is_none()
                    && !(id.starts_with("minecraft:")
                        && (id.ends_with("_slab")
                            || id.ends_with("_stairs")
                            || id.ends_with("_fence")
                            || id.ends_with("_fence_gate")
                            || id.ends_with("_door")
                            || id.ends_with("_bed")
                            || id.ends_with("_stained_glass")
                            || id.ends_with("_stained_glass_pane")
                            || matches!(
                                id.as_str(),
                                "minecraft:torch"
                                    | "minecraft:wall_torch"
                                    | "minecraft:soul_torch"
                                    | "minecraft:soul_wall_torch"
                                    | "minecraft:glass"
                                    | "minecraft:glass_pane"
                                    | "minecraft:iron_bars"
                            )))
                {
                    continue;
                }
                let state = resolve_template_state(raw, &registry)?;
                if block.position.iter().any(|value| !(0..16).contains(value)) {
                    continue;
                }
                if id.ends_with("_bed") && state.properties["part"] == "head" {
                    expected_beds.insert(block.position);
                }
                let mut random = DecorationRandom::new(42);
                assert!(
                    JigsawRegion {
                        region: &mut region,
                        registry: &registry
                    }
                    .place_template_block(
                        block.position,
                        &state,
                        None,
                        &mut random
                    )?
                );
                found.insert(id);
            }
            region.finish()?;
            let poi_pos = native_persistence::world::ChunkPos { x: 0, z: 0 };
            let snapshot = terrain.snapshot();
            let poi = native_persistence::poi::refresh_missing_chunk(
                poi_pos,
                &snapshot,
                0,
                16,
                |state| {
                    ensure!(
                        registry.is_canonical_state(&state.name, &state.properties),
                        "unknown POI state"
                    );
                    Ok(())
                },
            )?;
            if expected_beds.is_empty() {
                assert!(poi.is_none());
            } else {
                let poi = native_persistence::world::SavedPoiChunk::from_document(
                    poi.context("missing bed POI")?,
                    poi_pos,
                )?;
                let records = &poi.sections[&0].records;
                assert_eq!(records.len(), expected_beds.len());
                for record in records {
                    assert!(expected_beds.contains(&record.position));
                    assert!(record.type_id.matches("minecraft:home"));
                    assert_eq!(record.free_tickets, 1);
                }
            }
        }
        assert!(found.contains("minecraft:oak_log"), "{found:?}");
        assert!(found.contains("minecraft:oak_planks"), "{found:?}");
        assert!(found.contains("minecraft:oak_stairs"), "{found:?}");
        assert!(found.contains("minecraft:oak_fence"), "{found:?}");
        assert!(found.contains("minecraft:oak_fence_gate"), "{found:?}");
        assert!(found.contains("minecraft:oak_door"), "{found:?}");
        assert!(found.contains("minecraft:wall_torch"), "{found:?}");
        assert!(found.contains("minecraft:glass_pane"), "{found:?}");
        assert!(found.contains("minecraft:white_bed"), "{found:?}");
        assert!(found.contains("minecraft:yellow_bed"), "{found:?}");
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_trapdoor_waterlogging_preserves_state_and_schedules_water() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let (name, mut properties) =
            registry.resolve(&serde_json::json!("minecraft:dark_oak_trapdoor"))?;
        properties.insert("facing".into(), "east".into());
        properties.insert("open".into(), "true".into());
        properties.insert("half".into(), "top".into());
        let state = BlockState { name, properties };
        let pos = [1, 8, 1];
        let mut draft = JigsawRegion {
            region: &mut region,
            registry: &registry,
        };
        assert_eq!(jigsaw_static_full(&state)?, false);
        assert!(draft.place_template_block(pos, &state, None, &mut DecorationRandom::new(42))?);
        assert!(draft.is_liquid_container(&state)?);
        draft.place_liquid(
            pos,
            &state,
            Some(FluidState {
                kind: FluidKind::Water,
                source: true,
                falling: false,
                amount: 8,
            }),
        )?;
        region.finish()?;
        let snapshot = terrain.snapshot();
        let placed = snapshot
            .block(pos[0], pos[1], pos[2])
            .context("missing trapdoor")?;
        let mut expected = state.clone();
        expected
            .properties
            .insert("waterlogged".into(), "true".into());
        assert_eq!(placed, &expected);
        let ticks = snapshot
            .saved_fluid_ticks
            .get(&ChunkPos { x: 0, z: 0 })
            .context("missing fluid tick")?;
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].position, pos);
        assert_eq!(ticks[0].fluid, "minecraft:water");
        assert_eq!(ticks[0].delay, 0);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_lanterns_waterlog_only_from_source_water() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        for name in ["lantern", "soul_lantern"] {
            for hanging in [false, true] {
                for source in [false, true] {
                    let (mut terrain, tags, _, _) = super::super::tests::fixture();
                    let writable = terrain.snapshot().chunks.keys().copied().collect();
                    let mut region = terrain.decoration_region(&tags, &motion, writable)?;
                    let state = BlockState {
                        name: format!("minecraft:{name}"),
                        properties: BTreeMap::from([
                            ("hanging".into(), hanging.to_string()),
                            ("waterlogged".into(), "false".into()),
                        ]),
                    };
                    let pos = [1, 8, 1];
                    let mut random = DecorationRandom::new(42);
                    let mut expected_random = random.clone();
                    {
                        let mut draft = JigsawRegion {
                            region: &mut region,
                            registry: &registry,
                        };
                        assert!(draft.place_template_block(pos, &state, None, &mut random)?);
                        assert!(draft.is_liquid_container(&state)?);
                        draft.place_liquid(
                            pos,
                            &state,
                            Some(FluidState {
                                kind: FluidKind::Water,
                                source,
                                falling: false,
                                amount: if source { 8 } else { 7 },
                            }),
                        )?;
                    }
                    region.finish()?;
                    assert_eq!(random.next_long(), expected_random.next_long());
                    let snapshot = terrain.snapshot();
                    let placed = snapshot
                        .block(pos[0], pos[1], pos[2])
                        .context("missing lantern")?;
                    assert_eq!(placed.properties["hanging"], hanging.to_string());
                    assert_eq!(placed.properties["waterlogged"], source.to_string());
                    let ticks = snapshot.saved_fluid_ticks.get(&ChunkPos { x: 0, z: 0 });
                    if source {
                        let ticks = ticks.context("waterlogged lantern tick missing")?;
                        assert_eq!(ticks.len(), 1);
                        assert_eq!(ticks[0].position, pos);
                        assert_eq!(ticks[0].fluid, "minecraft:water");
                        assert_eq!(ticks[0].delay, 0);
                    } else {
                        assert!(ticks.is_none_or(|ticks| ticks.is_empty()));
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_slabs_accept_water_only_when_not_double() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        for kind in ["bottom", "top", "double"] {
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let state = BlockState {
                name: "minecraft:deepslate_tile_slab".into(),
                properties: BTreeMap::from([
                    ("type".into(), kind.into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            };
            let pos = [1, 8, 1];
            {
                let mut draft = JigsawRegion {
                    region: &mut region,
                    registry: &registry,
                };
                assert!(draft.place_template_block(
                    pos,
                    &state,
                    None,
                    &mut DecorationRandom::new(42)
                )?);
                assert!(draft.is_liquid_container(&state)?);
                draft.place_liquid(
                    pos,
                    &state,
                    Some(FluidState {
                        kind: FluidKind::Water,
                        source: true,
                        falling: false,
                        amount: 8,
                    }),
                )?;
            }
            region.finish()?;
            let world = terrain.snapshot();
            let placed = world.block(pos[0], pos[1], pos[2]).unwrap();
            assert_eq!(placed.properties["type"], kind);
            assert_eq!(
                placed.properties["waterlogged"],
                if kind == "double" { "false" } else { "true" }
            );
            let ticks = world.saved_fluid_ticks.get(&ChunkPos { x: 0, z: 0 });
            if kind == "double" {
                assert!(ticks.is_none_or(|ticks| ticks.is_empty()));
            } else {
                let ticks = ticks.context("waterlogged slab tick missing")?;
                assert_eq!(ticks.len(), 1);
                assert_eq!(ticks[0].position, pos);
                assert_eq!(ticks[0].fluid, "minecraft:water");
                assert_eq!(ticks[0].delay, 0);
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_fence_waterlogging_preserves_connections_and_schedules_water() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let state = BlockState {
            name: "minecraft:oak_fence".into(),
            properties: BTreeMap::from([
                ("north".into(), "true".into()),
                ("east".into(), "false".into()),
                ("south".into(), "false".into()),
                ("west".into(), "true".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        };
        let pos = [1, 8, 1];
        let mut draft = JigsawRegion {
            region: &mut region,
            registry: &registry,
        };
        assert!(draft.place_template_block(pos, &state, None, &mut DecorationRandom::new(42))?);
        assert!(draft.is_liquid_container(&state)?);
        draft.place_liquid(
            pos,
            &state,
            Some(FluidState {
                kind: FluidKind::Water,
                source: true,
                falling: false,
                amount: 8,
            }),
        )?;
        region.finish()?;
        let world = terrain.snapshot();
        let placed = world
            .block(pos[0], pos[1], pos[2])
            .context("fence missing")?;
        assert_eq!(placed.properties["north"], "true");
        assert_eq!(placed.properties["west"], "true");
        assert_eq!(placed.properties["waterlogged"], "true");
        let ticks = world
            .saved_fluid_ticks
            .get(&ChunkPos { x: 0, z: 0 })
            .context("water tick missing")?;
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].position, pos);
        assert_eq!(ticks[0].fluid, "minecraft:water");
        assert_eq!(ticks[0].delay, 0);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_panes_waterlog_and_preserve_proto_water_ticks() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        for name in [
            "minecraft:glass_pane",
            "minecraft:red_stained_glass_pane",
            "minecraft:exposed_copper_bars",
        ] {
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let state = BlockState {
                name: name.into(),
                properties: BTreeMap::from([
                    ("north".into(), "true".into()),
                    ("east".into(), "false".into()),
                    ("south".into(), "false".into()),
                    ("west".into(), "true".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            };
            let pos = [1, 8, 1];
            let mut draft = JigsawRegion {
                region: &mut region,
                registry: &registry,
            };
            assert!(draft.place_template_block(
                pos,
                &state,
                None,
                &mut DecorationRandom::new(42)
            )?);
            assert!(draft.is_liquid_container(&state)?);
            draft.place_liquid(
                pos,
                &state,
                Some(FluidState {
                    kind: FluidKind::Water,
                    source: true,
                    falling: false,
                    amount: 8,
                }),
            )?;
            region.finish()?;
            let world = terrain.snapshot();
            let placed = world
                .block(pos[0], pos[1], pos[2])
                .context("pane missing")?;
            assert_eq!(placed.properties["north"], "true");
            assert_eq!(placed.properties["west"], "true");
            assert_eq!(placed.properties["waterlogged"], "true");
            let ticks = world
                .saved_fluid_ticks
                .get(&ChunkPos { x: 0, z: 0 })
                .context("water tick missing")?;
            assert_eq!(ticks.len(), 1);
            assert_eq!(ticks[0].position, pos);
            assert_eq!(ticks[0].fluid, "minecraft:water");
            assert_eq!(ticks[0].delay, 0);
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn feature_pool_placement_and_random_match_java() -> Result<()> {
        use super::super::super::jigsaw::{PreparedJigsawPlacement, PreparedStart};
        use native_core::beardifier::TerrainAdjustment;
        use std::{path::PathBuf, process::Command};
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home is missing")?
            .join("bin");
        let source = temp.path().join("JigsawFeatureOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/JigsawFeatureOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("JigsawFeatureOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );

        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = data.vanilla_block_ids()?;
        let registry = data.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut data, &blocks)?;
        let origin = [1, 8, 1];
        let start = PreparedStart {
            chunk: [0, 0],
            generation_point: origin,
            terrain_adjustment: TerrainAdjustment::None,
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
            pieces: vec![Piece {
                element: PoolElement::Feature {
                    feature: HolderValue::Inline(
                        serde_json::json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:melon"}},"placement":[{"type":"minecraft:count","count":3},{"type":"minecraft:in_square"}]}),
                    ),
                    projection: Projection::Rigid,
                },
                origin,
                rotation: Rotation::Clockwise90,
                bounds: BoundingBox::new(origin, origin).map_err(anyhow::Error::msg)?,
                ground_level_delta: 0,
                junctions: vec![],
            }],
        };
        let prepared = PreparedJigsawPlacement::load(&start, &mut data, &blocks, &registry)?;
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let mut cases = Vec::<(i64, i64, usize, BTreeSet<[i32; 3]>)>::new();
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["CASE", seed, result, next, count] => {
                    assert_eq!(*result, "true");
                    cases.push((
                        seed.parse()?,
                        next.parse()?,
                        count.parse()?,
                        BTreeSet::new(),
                    ));
                }
                ["WRITE", x, y, z] => {
                    cases.last_mut().context("write before case")?.3.insert([
                        x.parse()?,
                        y.parse()?,
                        z.parse()?,
                    ]);
                }
                _ => {}
            }
        }
        assert_eq!(cases.len(), 4);
        for (seed, next, count, expected) in cases {
            assert_eq!(count, 3);
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            region.jigsaw_feature_bounds = Some((0, 16));
            let mut random = DecorationRandom::new(seed);
            assert_eq!(
                region.place_jigsaw_start(&prepared, &start, &registry, clip, &mut random)?,
                1
            );
            assert_eq!(random.next_long(), next);
            region.finish()?;
            let world = terrain.snapshot();
            let mut actual = BTreeSet::new();
            for z in 1..=16 {
                for x in 1..=16 {
                    if world.block(x, 8, z).context("missing block")?.name == "minecraft:melon" {
                        actual.insert([x, 8, z]);
                    }
                }
            }
            assert_eq!(actual, expected, "seed {seed}");
        }
        let mut failing = start.clone();
        failing.pieces[0].element = PoolElement::List {
            elements: vec![
                start.pieces[0].element.clone(),
                PoolElement::Feature {
                    feature: HolderValue::Inline(
                        serde_json::json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:stone"}},"placement":[]}),
                    ),
                    projection: Projection::Rigid,
                },
            ],
            projection: Projection::Rigid,
        };
        let prepared = PreparedJigsawPlacement::load(&failing, &mut data, &blocks, &registry)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        region.jigsaw_feature_bounds = Some((0, 16));
        let mut random = DecorationRandom::new(42);
        let mut original = random.clone();
        assert!(
            region
                .place_jigsaw_start(&prepared, &failing, &registry, clip, &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), original.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn vanilla_hay_pile_writes_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home missing")?
            .join("bin");
        let source = temp.path().join("BlockPileOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/BlockPileOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("BlockPileOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let mut cases = Vec::<(i64, i64, BTreeMap<[i32; 3], String>)>::new();
        let mut selected = false;
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["CASE", mode, seed, result, next, _] => {
                    selected = *mode == "0";
                    if selected {
                        assert_eq!(*result, "true");
                        cases.push((seed.parse()?, next.parse()?, BTreeMap::new()));
                    }
                }
                ["WRITE", x, y, z, axis, flags] if selected => {
                    assert_eq!(*flags, "260");
                    cases
                        .last_mut()
                        .context("write before case")?
                        .2
                        .insert([x.parse()?, y.parse()?, z.parse()?], (*axis).into());
                }
                _ => {}
            }
        }
        assert_eq!(cases.len(), 5);
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        let placed = PlacedDecorationFeature::load(
            &mut resources,
            &ResourceId::parse("minecraft:pile_hay")?,
            &mut |value| {
                let (name, properties) = registry.resolve(value)?;
                Ok(BlockState { name, properties })
            },
        )?;
        for (seed, next, expected) in cases {
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut random = DecorationRandom::new(seed);
            assert!(region.place_nested(&placed, &mut random, [8, 8, 8], 0, 16)?);
            assert_eq!(random.next_long(), next, "seed {seed}");
            region.finish()?;
            let world = terrain.snapshot();
            let mut actual = BTreeMap::new();
            for y in 8..=9 {
                for z in 5..=11 {
                    for x in 5..=11 {
                        let state = world.block(x, y, z).context("missing generated block")?;
                        if state.name == "minecraft:hay_block" {
                            actual.insert([x, y, z], state.properties["axis"].clone());
                        }
                    }
                }
            }
            assert_eq!(actual, expected, "seed {seed}");
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn vanilla_village_piles_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home missing")?
            .join("bin");
        let source = temp.path().join("VillagePilesOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/VillagePilesOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        let mut found_lantern = false;
        for kind in ["snow", "melon", "ice", "pumpkin"] {
            let output = Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&classpath)
                .arg("VillagePilesOracle")
                .arg(kind)
                .output()?;
            ensure!(
                output.status.success(),
                "Java oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut cases = Vec::<(i64, i64, BTreeMap<[i32; 3], BlockState>)>::new();
            for line in String::from_utf8(output.stdout)?.lines() {
                let fields: Vec<_> = line.split('\t').collect();
                match fields.as_slice() {
                    ["CASE", seed, result, next] => {
                        assert_eq!(*result, "true");
                        cases.push((seed.parse()?, next.parse()?, BTreeMap::new()));
                    }
                    ["WRITE", x, y, z, name, property, flags] => {
                        assert_eq!(*flags, "260");
                        let mut properties = BTreeMap::new();
                        if !property.is_empty() {
                            let (key, value) =
                                property.split_once('=').context("invalid property")?;
                            properties.insert(key.into(), value.into());
                        }
                        found_lantern |= *name == "minecraft:jack_o_lantern";
                        cases.last_mut().context("write before case")?.2.insert(
                            [x.parse()?, y.parse()?, z.parse()?],
                            BlockState {
                                name: (*name).into(),
                                properties,
                            },
                        );
                    }
                    _ => {}
                }
            }
            assert_eq!(cases.len(), 20);
            let placed = PlacedDecorationFeature::load(
                &mut resources,
                &ResourceId::parse(&format!("minecraft:pile_{kind}"))?,
                &mut |value| {
                    let (name, properties) = registry.resolve(value)?;
                    Ok(BlockState { name, properties })
                },
            )?;
            for (seed, next, expected) in cases {
                let (mut terrain, tags, _, _) = super::super::tests::fixture();
                let writable = terrain.snapshot().chunks.keys().copied().collect();
                let mut region = terrain.decoration_region(&tags, &motion, writable)?;
                let mut random = DecorationRandom::new(seed);
                assert!(region.place_nested(&placed, &mut random, [8, 8, 8], 0, 16)?);
                assert_eq!(random.next_long(), next, "{kind} seed {seed}");
                region.finish()?;
                let world = terrain.snapshot();
                let mut actual = BTreeMap::new();
                for y in 8..=9 {
                    for z in 5..=11 {
                        for x in 5..=11 {
                            let state = world.block(x, y, z).context("missing generated block")?;
                            if !is_air(state) {
                                actual.insert([x, y, z], state.clone());
                            }
                        }
                    }
                }
                assert_eq!(actual, expected, "{kind} seed {seed}");
            }
        }
        assert!(found_lantern, "oracle never generated a jack o'lantern");
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn rigid_piece_updates_draft_and_failed_piece_is_not_published() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = data.vanilla_block_ids()?;
        let registry = data.vanilla_generated_block_registry()?;
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let processors = super::super::super::jigsaw::PieceProcessors::Single(Arc::new(
            PreparedProcessors::load(&holder, &mut data, &blocks, &registry)?,
        ));
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let piece = Piece {
            element: PoolElement::Single {
                legacy: false,
                location: ResourceId::parse("example:test")?,
                processors: holder,
                projection: Projection::Rigid,
                override_liquid_settings: None,
            },
            origin: [1, 8, 1],
            rotation: Rotation::None,
            bounds: clip,
            ground_level_delta: 0,
            junctions: vec![],
        };
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let prepared = PreparedTemplatePiece::load(&piece, &processors, &mut template(false)?)?;
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        assert!(region.place_rigid_jigsaw_piece(
            &prepared,
            &registry,
            clip,
            LiquidSettings::IgnoreWaterlogging,
            &mut DecorationRandom::new(42)
        )?);
        region.finish()?;
        assert_eq!(
            terrain.snapshot().block(1, 8, 1).unwrap().name,
            "minecraft:stone"
        );
        assert_eq!(
            terrain.snapshot().block(2, 8, 1).unwrap().name,
            "minecraft:stone"
        );

        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let prepared = PreparedTemplatePiece::load(&piece, &processors, &mut template(true)?)?;
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut original = random.clone();
        assert!(
            region
                .place_rigid_jigsaw_piece(
                    &prepared,
                    &registry,
                    clip,
                    LiquidSettings::IgnoreWaterlogging,
                    &mut random
                )
                .is_err()
        );
        assert_eq!(random.next_long(), original.next_long());
        assert!(region.finish().is_err());
        assert_eq!(terrain.snapshot().block(1, 8, 1), before.block(1, 8, 1));
        assert_eq!(terrain.snapshot().block(2, 8, 1), before.block(2, 8, 1));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn installed_ancient_city_entrance_matches_java_chunk_placement() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let motion = HeightmapTags::load(&mut resources, &blocks)?;
        let holder = HolderValue::Reference(ResourceId::parse(
            "minecraft:ancient_city_generic_degradation",
        )?);
        let processors = super::super::super::jigsaw::PieceProcessors::Single(Arc::new(
            PreparedProcessors::load(&holder, &mut resources, &blocks, &registry)?,
        ));
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let mut piece = Piece {
            element: PoolElement::Single {
                legacy: false,
                location: ResourceId::parse(
                    "minecraft:ancient_city/city/entrance/entrance_connector",
                )?,
                processors: holder,
                projection: Projection::Rigid,
                override_liquid_settings: None,
            },
            origin: [0, 0, 0],
            rotation: Rotation::None,
            bounds: clip,
            ground_level_delta: 0,
            junctions: vec![],
        };
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let source = temp.path().join("AncientCityEntranceOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/AncientCityEntranceOracle.java"),
        )?;
        let compiled = std::process::Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut saw_candle = false;
        let mut saw_soul_lantern = false;
        for (origin, rotation, java_rotation) in [
            ([0, 0, 0], Rotation::None, "NONE"),
            ([0, 0, -16], Rotation::None, "NONE"),
            ([16, 0, 0], Rotation::Clockwise90, "CLOCKWISE_90"),
            ([16, 0, 16], Rotation::Clockwise180, "CLOCKWISE_180"),
            (
                [0, 0, 16],
                Rotation::Counterclockwise90,
                "COUNTERCLOCKWISE_90",
            ),
        ] {
            piece.origin = origin;
            piece.rotation = rotation;
            let prepared = PreparedTemplatePiece::load(&piece, &processors, &mut resources)?;
            let output = std::process::Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&classpath)
                .arg("AncientCityEntranceOracle")
                .arg(version.join("26.3.jar"))
                .arg(origin[0].to_string())
                .arg(origin[2].to_string())
                .arg(java_rotation)
                .output()?;
            ensure!(
                output.status.success(),
                "Java entrance oracle failed: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let mut expected = BTreeMap::new();
            let mut expected_random = None;
            for line in std::str::from_utf8(&output.stdout)?.lines() {
                let fields: Vec<_> = line.split('\t').collect();
                match fields.as_slice() {
                    ["BLOCK", x, y, z, id, properties] => {
                        let pos = [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?];
                        let state = BlockState {
                            name: (*id).into(),
                            properties: serde_json::from_str(properties)?,
                        };
                        ensure!(
                            expected.insert(pos, state).is_none(),
                            "duplicate Java block"
                        );
                    }
                    ["RANDOM", value] => {
                        ensure!(
                            expected_random.replace(value.parse::<i64>()?).is_none(),
                            "duplicate Java random"
                        );
                    }
                    _ => {}
                }
            }
            ensure!(expected.len() == 4096, "incomplete Java entrance output");
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut random = DecorationRandom::new(42);
            assert!(region.place_rigid_jigsaw_piece(
                &prepared,
                &registry,
                clip,
                LiquidSettings::IgnoreWaterlogging,
                &mut random
            )?);
            region.finish()?;
            let world = terrain.snapshot();
            for (pos, expected) in &expected {
                saw_candle |= expected.name == "minecraft:candle";
                saw_soul_lantern |= expected.name == "minecraft:soul_lantern";
                assert_eq!(
                    world.block(pos[0], pos[1], pos[2]),
                    Some(expected),
                    "entrance block at {pos:?}, origin {origin:?}, rotation {rotation:?}"
                );
            }
            assert_eq!(Some(random.next_long()), expected_random);
            assert!((0..16).any(|y| (0..16).any(|z| (0..16).any(|x| {
                world
                    .block(x, y, z)
                    .is_some_and(|state| state.name == "minecraft:deepslate_tiles")
            }))));
        }
        assert!(
            saw_candle && saw_soul_lantern,
            "expected entrance light sources were not exercised"
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_entities_outside_clip_do_not_block_placement() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let processors = super::super::super::jigsaw::PieceProcessors::Single(Arc::new(
            PreparedProcessors::load(&holder, &mut resources, &blocks, &registry)?,
        ));
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let piece = Piece {
            element: PoolElement::Single {
                legacy: false,
                location: ResourceId::parse("example:test")?,
                processors: holder,
                projection: Projection::Rigid,
                override_liquid_settings: None,
            },
            origin: [1, 8, 1],
            rotation: Rotation::None,
            bounds: clip,
            ground_level_delta: 0,
            junctions: vec![],
        };
        for (anchor, expected) in [
            (Some([15, 0, 0]), true),
            (Some([1, 0, 0]), false),
            (None, false),
        ] {
            let mut entity = vec![
                (
                    "pos".into(),
                    Tag::List {
                        element_type: TagType::Double,
                        values: vec![Tag::Double(1.5), Tag::Double(0.0), Tag::Double(0.5)],
                    },
                ),
                (
                    "nbt".into(),
                    Tag::Compound(vec![("id".into(), Tag::String("minecraft:pig".into()))]),
                ),
            ];
            if let Some(anchor) = anchor {
                entity.push(("blockPos".into(), ints(&anchor)));
            }
            let prepared = PreparedTemplatePiece::load(
                &piece,
                &processors,
                &mut template_with_entities(false, vec![Tag::Compound(entity)])?,
            )?;
            let (mut terrain, tags, motion, _) = super::super::tests::fixture();
            let before = terrain.snapshot();
            let writable = before.chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut random = DecorationRandom::new(42);
            let mut initial_random = random.clone();
            let result = region.place_rigid_jigsaw_piece(
                &prepared,
                &registry,
                clip,
                LiquidSettings::IgnoreWaterlogging,
                &mut random,
            );
            if expected {
                assert!(result?);
                region.finish()?;
                assert_eq!(
                    terrain.snapshot().block(1, 8, 1).unwrap().name,
                    "minecraft:stone"
                );
            } else {
                let error = result.err().context("entity was not rejected")?;
                if anchor.is_some() {
                    assert!(format!("{error:#}").contains("entity placement is not yet supported"));
                } else {
                    assert!(format!("{error:#}").contains("blockPos missing"));
                }
                assert!(region.finish().is_err());
                assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
            }
            assert_eq!(random.next_long(), initial_random.next_long());
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_records_vanilla_cushion_and_rolls_back_unsupported_entity() -> Result<()> {
        use native_persistence::structure_template::StructureTemplate;
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let source = StructureTemplate::load(
            &mut resources,
            &ResourceId::parse(
                "minecraft:abandoned_camp/camp/bamboo_jungle/campsite_bamboo_jungle_1",
            )?,
        )?
        .context("cushion template missing")?;
        let cushion = source
            .entities
            .first()
            .context("cushion entity missing")?
            .clone();
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let processors = super::super::super::jigsaw::PieceProcessors::Single(Arc::new(
            PreparedProcessors::load(&holder, &mut resources, &blocks, &registry)?,
        ));
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let piece = Piece {
            element: PoolElement::Single {
                legacy: false,
                location: ResourceId::parse("example:test")?,
                processors: holder,
                projection: Projection::Rigid,
                override_liquid_settings: None,
            },
            origin: [1, 8, 1],
            rotation: Rotation::None,
            bounds: clip,
            ground_level_delta: 0,
            junctions: vec![],
        };
        for reject in [false, true] {
            let mut entities = vec![cushion.clone()];
            if reject {
                entities.push(Tag::Compound(vec![
                    ("blockPos".into(), ints(&[1, 0, 0])),
                    (
                        "pos".into(),
                        Tag::List {
                            element_type: TagType::Double,
                            values: vec![Tag::Double(1.5), Tag::Double(0.0), Tag::Double(0.5)],
                        },
                    ),
                    (
                        "nbt".into(),
                        Tag::Compound(vec![("id".into(), Tag::String("minecraft:pig".into()))]),
                    ),
                ]));
            }
            let prepared = PreparedTemplatePiece::load(
                &piece,
                &processors,
                &mut template_with_entities(false, entities)?,
            )?;
            let (mut terrain, tags, motion, _) = super::super::tests::fixture();
            let before = terrain.snapshot();
            let writable = before.chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut random = DecorationRandom::new(42);
            let mut unchanged = random.clone();
            let result = region.place_rigid_jigsaw_piece(
                &prepared,
                &registry,
                clip,
                LiquidSettings::IgnoreWaterlogging,
                &mut random,
            );
            if reject {
                assert!(result.is_err());
                assert!(region.finish().is_err());
                assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
                assert!(
                    terrain
                        .recorded_generated_entities(ChunkPos { x: 0, z: 0 })?
                        .is_empty()
                );
            } else {
                assert!(result?);
                region.finish()?;
                let recorded = terrain.recorded_generated_entities(ChunkPos { x: 0, z: 0 })?;
                assert_eq!(recorded.len(), 1);
                assert_eq!(
                    recorded[0].get("id"),
                    Some(&Tag::String("minecraft:cushion".into()))
                );
                assert_eq!(
                    recorded[0].get("block_pos"),
                    Some(&Tag::IntArray(vec![5, 8, 7]))
                );
                assert_ne!(
                    recorded[0].get("UUID"),
                    cushion.get("nbt").and_then(|nbt| nbt.get("UUID"))
                );
            }
            assert_eq!(random.next_long(), unchanged.next_long());
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_records_vanilla_armor_stand_and_rolls_back_unsupported_data() -> Result<()> {
        use native_persistence::structure_template::StructureTemplate;
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let source = StructureTemplate::load(
            &mut resources,
            &ResourceId::parse("minecraft:village/taiga/houses/taiga_armorer_2")?,
        )?
        .context("armor stand template missing")?;
        let stand = source
            .entities
            .first()
            .context("armor stand missing")?
            .clone();
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let processors = super::super::super::jigsaw::PieceProcessors::Single(Arc::new(
            PreparedProcessors::load(&holder, &mut resources, &blocks, &registry)?,
        ));
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let piece = Piece {
            element: PoolElement::Single {
                legacy: false,
                location: ResourceId::parse("example:test")?,
                processors: holder,
                projection: Projection::Rigid,
                override_liquid_settings: None,
            },
            origin: [1, 8, 1],
            rotation: Rotation::None,
            bounds: clip,
            ground_level_delta: 0,
            junctions: vec![],
        };
        for reject in [false, true] {
            let mut entities = vec![stand.clone()];
            if reject {
                let mut unknown = stand.clone();
                let Tag::Compound(outer) = &mut unknown else {
                    unreachable!()
                };
                let (_, Tag::Compound(fields)) = outer
                    .iter_mut()
                    .find(|(name, _)| name.matches("nbt"))
                    .context("stand NBT missing")?
                else {
                    unreachable!()
                };
                fields.push(("unknown".into(), Tag::Int(1)));
                entities.push(unknown);
            }
            let prepared = PreparedTemplatePiece::load(
                &piece,
                &processors,
                &mut template_with_entities(false, entities)?,
            )?;
            let (mut terrain, tags, motion, _) = super::super::tests::fixture();
            let before = terrain.snapshot();
            let writable = before.chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut random = DecorationRandom::new(42);
            let mut unchanged = random.clone();
            let result = region.place_rigid_jigsaw_piece(
                &prepared,
                &registry,
                clip,
                LiquidSettings::IgnoreWaterlogging,
                &mut random,
            );
            if reject {
                assert!(result.is_err());
                assert!(region.finish().is_err());
                assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
                assert!(
                    terrain
                        .recorded_generated_entities(ChunkPos { x: 0, z: 0 })?
                        .is_empty()
                );
            } else {
                assert!(result?);
                region.finish()?;
                let recorded = terrain.recorded_generated_entities(ChunkPos { x: 0, z: 0 })?;
                assert_eq!(recorded.len(), 1);
                assert!(
                    matches!(recorded[0].get("id"), Some(Tag::String(id)) if id.matches("minecraft:armor_stand"))
                );
                assert_eq!(
                    recorded[0].get("equipment"),
                    stand.get("nbt").and_then(|nbt| nbt.get("equipment"))
                );
                assert_ne!(
                    recorded[0].get("UUID"),
                    stand.get("nbt").and_then(|nbt| nbt.get("UUID"))
                );
            }
            assert_eq!(random.next_long(), unchanged.next_long());
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn jigsaw_villagers_share_region_random_and_rollback_on_failure() -> Result<()> {
        use super::super::{
            MixedJigsawPlacementContext, MixedOceanRuinPlacement, MixedPortalPlacementContext,
        };
        use native_core::{beardifier::TerrainAdjustment, random::LegacyRandom};
        use native_persistence::structure_template::StructureTemplate;
        use native_resources::worldgen_region_random::{RegionRandom, finalize_mob_base};
        #[derive(Clone)]
        struct OtherStructure {
            random: RegionRandom,
        }
        impl MixedOceanRuinPlacement for OtherStructure {
            fn worldgen_region_random(&mut self) -> Option<&mut RegionRandom> {
                Some(&mut self.random)
            }
            fn place(
                &mut self,
                _: &mut DecorationRegion<'_>,
                _: &mut super::super::super::ocean_ruin::PreparedStart,
                _: BoundingBox,
                _: &mut DecorationRandom,
            ) -> Result<()> {
                anyhow::bail!("unexpected ocean ruin")
            }
        }
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let source = StructureTemplate::load(
            &mut resources,
            &ResourceId::parse("minecraft:village/plains/villagers/unemployed")?,
        )?
        .context("villager template missing")?;
        let villager = source.entities.first().context("villager missing")?.clone();
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let origin = [1, 8, 1];
        let start = PreparedStart {
            chunk: [0, 0],
            generation_point: origin,
            terrain_adjustment: TerrainAdjustment::None,
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
            pieces: vec![Piece {
                element: PoolElement::Single {
                    legacy: false,
                    location: ResourceId::parse("example:test")?,
                    processors: HolderValue::Inline(serde_json::json!({"processors":[]})),
                    projection: Projection::Rigid,
                    override_liquid_settings: None,
                },
                origin,
                rotation: Rotation::None,
                bounds: clip,
                ground_level_delta: 0,
                junctions: vec![],
            }],
        };
        for (reject, prior_draw) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut entities = vec![villager.clone(), villager.clone()];
            if reject {
                let mut unsupported = villager.clone();
                let Tag::Compound(outer) = &mut unsupported else {
                    unreachable!()
                };
                let (_, Tag::Compound(fields)) = outer
                    .iter_mut()
                    .find(|(name, _)| name.matches("nbt"))
                    .context("villager NBT missing")?
                else {
                    unreachable!()
                };
                fields.push(("unknown".into(), Tag::Int(1)));
                entities.push(unsupported);
            }
            let prepared = PreparedJigsawPlacement::load(
                &start,
                &mut template_with_entities(false, entities)?,
                &blocks,
                &registry,
            )?;
            let (mut terrain, tags, motion, _) = super::super::tests::fixture();
            let before = terrain.snapshot();
            let writable = before.chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut decoration_random = DecorationRandom::new(42);
            let mut original_decoration = decoration_random.clone();
            let mut region_random = RegionRandom::Legacy(LegacyRandom::new(38));
            if prior_draw {
                region_random.next_long();
            }
            let mut original_region = region_random.clone();
            let mut expected_random = region_random.clone();
            let expected: Vec<_> = (0..2)
                .map(|_| finalize_mob_base(&mut expected_random, false))
                .collect();
            let key = (ResourceId::parse("example:test")?, ChunkPos { x: 0, z: 0 });
            let mut mixed = MixedJigsawPlacementContext {
                other: MixedPortalPlacementContext {
                    other: OtherStructure {
                        random: region_random,
                    },
                    starts: Arc::new(BTreeMap::new()),
                },
                registry: &registry,
                starts: Arc::new(BTreeMap::from([(key.clone(), prepared)])),
                shipwreck_tags: None,
            };
            let mut staged_start = start.clone();
            let result = mixed.place_jigsaw(
                &mut region,
                &key,
                &mut staged_start,
                clip,
                &mut decoration_random,
            );
            if reject {
                assert!(result.is_err());
                assert!(region.finish().is_err());
                assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
                assert!(
                    terrain
                        .recorded_generated_entities(ChunkPos { x: 0, z: 0 })?
                        .is_empty()
                );
                assert_eq!(
                    mixed.other.other.random.next_long(),
                    original_region.next_long()
                );
            } else {
                result?;
                region.finish()?;
                let recorded = terrain.recorded_generated_entities(ChunkPos { x: 0, z: 0 })?;
                assert_eq!(recorded.len(), 2);
                for (entity, initialization) in recorded.iter().zip(expected) {
                    assert_eq!(
                        entity.get("LeftHanded"),
                        Some(&Tag::Byte(i8::from(initialization.left_handed)))
                    );
                    let Some(Tag::List {
                        values: attributes, ..
                    }) = entity.get("attributes")
                    else {
                        anyhow::bail!("attributes missing")
                    };
                    let Some(Tag::List {
                        values: modifiers, ..
                    }) = attributes[0].get("modifiers")
                    else {
                        anyhow::bail!("modifier missing")
                    };
                    assert_eq!(
                        modifiers[0].get("amount"),
                        initialization.follow_range_bonus.map(Tag::Double).as_ref()
                    );
                }
                assert_eq!(
                    mixed.other.other.random.next_long(),
                    expected_random.next_long()
                );
            }
            assert_eq!(
                decoration_random.next_long(),
                original_decoration.next_long()
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn vanilla_plains_small_houses_place_complete_legacy_templates() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let holder = HolderValue::Reference(ResourceId::parse("minecraft:mossify_10_percent")?);
        let processors = super::super::super::jigsaw::PieceProcessors::Single(Arc::new(
            PreparedProcessors::load(&holder, &mut resources, &blocks, &registry)?,
        ));
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(std::path::PathBuf::from)
            .context("Java 25 home is missing")?
            .join("bin");
        let source = temp.path().join("VillageHouseOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/VillageHouseOracle.java"),
        )?;
        let compiled = std::process::Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("VillageHouseOracle")
            .arg(version.join("26.3.jar"))
            .output()?;
        ensure!(
            output.status.success(),
            "Java house oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let output = String::from_utf8(output.stdout)?;
        let mut expected = BTreeMap::new();
        let mut expected_random = BTreeMap::new();
        for line in output.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["BLOCK", name, x, y, z, id, properties] => {
                    let pos = [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?];
                    let state = BlockState {
                        name: (*id).into(),
                        properties: serde_json::from_str(properties)?,
                    };
                    ensure!(
                        expected.insert(((*name).to_owned(), pos), state).is_none(),
                        "duplicate Java house block"
                    );
                }
                ["RANDOM", name, value] => {
                    ensure!(
                        expected_random
                            .insert((*name).to_owned(), value.parse::<i64>()?)
                            .is_none(),
                        "duplicate Java random"
                    );
                }
                _ => {}
            }
        }
        ensure!(
            expected.len() == 3 * 7 * 7 * 7 && expected_random.len() == 3,
            "incomplete Java house output"
        );
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        for name in [
            "plains_small_house_1",
            "plains_small_house_2",
            "plains_small_house_3",
        ] {
            let piece = Piece {
                element: PoolElement::Single {
                    legacy: true,
                    location: ResourceId::parse(&format!(
                        "minecraft:village/plains/houses/{name}"
                    ))?,
                    processors: holder.clone(),
                    projection: Projection::Rigid,
                    override_liquid_settings: None,
                },
                origin: [1, 8, 1],
                rotation: Rotation::None,
                bounds: clip,
                ground_level_delta: 0,
                junctions: vec![],
            };
            let prepared = PreparedTemplatePiece::load(&piece, &processors, &mut resources)?;
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let motion = HeightmapTags::load(&mut resources, &blocks)?;
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut random = DecorationRandom::new(42);
            assert!(
                region
                    .place_rigid_jigsaw_piece(
                        &prepared,
                        &registry,
                        clip,
                        LiquidSettings::IgnoreWaterlogging,
                        &mut random
                    )
                    .with_context(|| format!("could not place {name}"))?
            );
            region.finish()?;
            let world = terrain.snapshot();
            let mut parts = BTreeSet::new();
            for y in 8..15 {
                for z in 1..8 {
                    for x in 1..8 {
                        let state = world.block(x, y, z).context("house block missing")?;
                        assert_eq!(
                            state,
                            &expected[&(name.to_owned(), [x, y, z])],
                            "{name} at {x},{y},{z}"
                        );
                        if state.name.ends_with("_bed") {
                            parts.insert(state.properties["part"].clone());
                        }
                    }
                }
            }
            assert_eq!(
                parts,
                BTreeSet::from(["foot".to_owned(), "head".to_owned()]),
                "{name}"
            );
            assert_eq!(
                random.next_long(),
                expected_random[name],
                "random after {name}"
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn prepared_start_places_intersecting_pieces_and_rejects_stale_start() -> Result<()> {
        use super::super::super::jigsaw::{PreparedJigsawPlacement, PreparedStart};
        use native_core::beardifier::TerrainAdjustment;
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let single = PoolElement::Single {
            legacy: false,
            location: ResourceId::parse("example:test")?,
            processors: holder,
            projection: Projection::Rigid,
            override_liquid_settings: None,
        };
        let make_piece = |x, element| -> Result<Piece> {
            Ok(Piece {
                element,
                origin: [x, 8, 1],
                rotation: Rotation::None,
                bounds: BoundingBox::new([x, 8, 1], [x + 1, 8, 1]).map_err(anyhow::Error::msg)?,
                ground_level_delta: 0,
                junctions: vec![],
            })
        };
        let start = PreparedStart {
            chunk: [0, 0],
            generation_point: [8, 8, 8],
            terrain_adjustment: TerrainAdjustment::None,
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
            pieces: vec![
                make_piece(1, single.clone())?,
                make_piece(
                    4,
                    PoolElement::List {
                        elements: vec![single.clone(), PoolElement::Empty],
                        projection: Projection::Rigid,
                    },
                )?,
                make_piece(20, single)?,
            ],
        };
        let mut templates = template(false)?;
        let prepared = PreparedJigsawPlacement::load(&start, &mut templates, &blocks, &registry)?;
        assert_eq!(
            templates.reads, 1,
            "duplicate templates should be loaded once"
        );
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        assert_eq!(
            region.place_jigsaw_start(
                &prepared,
                &start,
                &registry,
                clip,
                &mut DecorationRandom::new(42)
            )?,
            2
        );
        region.finish()?;
        for x in [1, 2, 4, 5] {
            assert_eq!(
                terrain.snapshot().block(x, 8, 1).unwrap().name,
                "minecraft:stone"
            );
        }
        assert_eq!(
            terrain.snapshot().block(20, 8, 1).unwrap().name,
            "minecraft:air"
        );

        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut stale = start.clone();
        stale.pieces[1].origin[0] += 1;
        let mut random = DecorationRandom::new(42);
        let mut original = random.clone();
        assert!(
            region
                .place_jigsaw_start(&prepared, &stale, &registry, clip, &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), original.next_long());
        assert!(region.finish().is_err());
        assert_eq!(terrain.snapshot().block(1, 8, 1), before.block(1, 8, 1));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn mixed_scheduler_places_jigsaw_and_rejects_missing_context() -> Result<()> {
        use super::super::super::{
            jigsaw::PreparedStart,
            structure_batch::{PlannedMixedStructureDecoration, PlannedStructureStart},
        };
        use native_core::beardifier::TerrainAdjustment;
        use native_resources::structure_decoration::StructureDecorationEntry;
        #[derive(Clone)]
        struct NoOcean;
        impl MixedOceanRuinPlacement for NoOcean {
            fn place(
                &mut self,
                _: &mut DecorationRegion<'_>,
                _: &mut super::super::super::ocean_ruin::PreparedStart,
                _: BoundingBox,
                _: &mut DecorationRandom,
            ) -> Result<()> {
                bail!("unexpected ocean ruin")
            }
        }
        #[derive(Clone)]
        struct Corrupt;
        impl MixedOceanRuinPlacement for Corrupt {
            fn place(
                &mut self,
                _: &mut DecorationRegion<'_>,
                _: &mut super::super::super::ocean_ruin::PreparedStart,
                _: BoundingBox,
                _: &mut DecorationRandom,
            ) -> Result<()> {
                bail!("unexpected ocean ruin")
            }
            fn place_jigsaw(
                &mut self,
                _: &mut DecorationRegion<'_>,
                _: &super::super::super::structure_batch::TerrainStartKey,
                start: &mut PreparedStart,
                _: BoundingBox,
                _: &mut DecorationRandom,
            ) -> Result<()> {
                start.pieces[0].origin[0] += 1;
                Ok(())
            }
        }
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
        let bounds = BoundingBox::new([1, 8, 1], [2, 8, 1]).map_err(anyhow::Error::msg)?;
        let piece = Piece {
            element: PoolElement::Single {
                legacy: false,
                location: ResourceId::parse("example:test")?,
                processors: holder,
                projection: Projection::Rigid,
                override_liquid_settings: None,
            },
            origin: [1, 8, 1],
            rotation: Rotation::None,
            bounds,
            ground_level_delta: 0,
            junctions: vec![],
        };
        let start = PreparedStart {
            chunk: [0, 0],
            generation_point: [8, 8, 8],
            terrain_adjustment: TerrainAdjustment::None,
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
            pieces: vec![piece],
        };
        let entry = StructureDecorationEntry {
            id: ResourceId::parse("example:jigsaw")?,
            step: 0,
            index: 0,
        };
        let structures = [PlannedMixedStructureDecoration {
            entry: &entry,
            starts: vec![PlannedStructureStart::Jigsaw(&start)],
        }];
        let (mut data, schedule, biomes) = super::super::tests::scheduled_fixture(None);
        let plan = super::super::tests::fixture_plan(&schedule, &biomes);
        let prepared = PreparedDecoration::load(
            &mut data,
            &plan,
            &super::super::tests::batch_registry(),
            &mut super::super::tests::resolve_test,
        )?;
        let (mut failed, _, _, _) = super::super::tests::fixture();
        let before = failed.snapshot();
        assert!(
            prepared
                .execute_with_mixed_structure_plan(&mut failed, &structures, &mut NoOcean)
                .is_err()
        );
        assert!(Arc::ptr_eq(&before, &failed.snapshot()));
        let error = prepared
            .execute_with_mixed_structure_plan(&mut failed, &structures, &mut Corrupt)
            .err()
            .context("corrupted jigsaw accepted")?;
        assert!(format!("{error:#}").contains("jigsaw start changed"));
        assert!(Arc::ptr_eq(&before, &failed.snapshot()));
        let mut context = MixedJigsawPlacementContext::load(
            NoOcean,
            &structures,
            &mut template(false)?,
            &blocks,
            &registry,
        )?;
        let (mut terrain, _, _, _) = super::super::tests::fixture();
        let (_, updated) =
            prepared.execute_with_mixed_structure_plan(&mut terrain, &structures, &mut context)?;
        assert!(matches!(
            &updated[0][0],
            super::super::super::PreparedStructureStart::Jigsaw(_)
        ));
        assert_eq!(
            terrain.snapshot().block(1, 8, 1).unwrap().name,
            "minecraft:stone"
        );
        assert_eq!(
            terrain.snapshot().block(2, 8, 1).unwrap().name,
            "minecraft:stone"
        );
        Ok(())
    }
}
