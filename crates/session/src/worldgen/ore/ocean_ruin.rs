//! Access the current decoration region for ocean-ruin floor adjustment.
use super::*;
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
use native_resources::ocean_ruin::{DataMarkerKind, FloorWorld, MarkerWorld, handle_data_marker};
use native_resources::tree_leaf_update::ShapeEdgeWorld;
use native_resources::{
    drowned_spawn::{
        MainHandRoll, NautilusJockeyWorld, StructureDrownedWorld, initialize_structure_drowned,
    },
    worldgen_region_random::RegionRandom,
};

fn fluid_in_water_tag(state: &BlockState, water_fluids: &BTreeSet<String>) -> Result<bool> {
    let fluid =
        match native_world::fluid::FluidState::from_block(state).map_err(anyhow::Error::msg)? {
            None => "minecraft:empty",
            Some(value) => match (value.kind, value.source) {
                (native_world::fluid::FluidKind::Water, true) => "minecraft:water",
                (native_world::fluid::FluidKind::Water, false) => "minecraft:flowing_water",
                (native_world::fluid::FluidKind::Lava, true) => "minecraft:lava",
                (native_world::fluid::FluidKind::Lava, false) => "minecraft:flowing_lava",
            },
        };
    Ok(water_fluids.contains(fluid))
}

struct RuinFloorWorld<'r, 'a, 't> {
    region: &'r mut DecorationRegion<'a>,
    ice_tags: &'t OreBlockTags,
    water_fluids: &'t BTreeSet<String>,
}

impl DecorationRegion<'_> {
    /// Run a complete ocean-ruin template piece against this unpublished
    /// region. The spawn callback must create the exact structure drowned;
    /// cloning it must isolate all callback state. The caller must stage the
    /// piece, RNG and callback again until the region itself is committed.
    #[allow(clippy::too_many_arguments)]
    pub fn place_ocean_ruin_piece<F>(
        &mut self,
        piece: &mut super::super::ocean_ruin::PreparedPiece,
        ice_tags: &OreBlockTags,
        water_fluids: &BTreeSet<String>,
        bubble_fluids: &BTreeSet<String>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        temperature: native_resources::ocean_ruin::Temperature,
        sea_level: i32,
        clip: BoundingBox,
        random: &mut DecorationRandom,
        spawn: &mut F,
    ) -> Result<bool>
    where
        F: Clone + FnMut([i32; 3], &World) -> Result<Option<super::super::GeneratedEntityRecord>>,
    {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next_spawn = spawn.clone();
        let world_seed = self.target.seed;
        let result = piece.post_process(
            &mut RuinPieceRegion {
                marker: RuinMarkerRegion {
                    region: self,
                    registry,
                    water_fluids,
                    sea_level,
                    spawn: &mut next_spawn,
                },
                ice_tags,
                bubble_fluids,
            },
            registry,
            world_seed,
            temperature,
            clip,
            random,
        );
        match result {
            Ok(value) => {
                *spawn = next_spawn;
                Ok(value)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    /// Apply one processed archaeology replacement and materialize its block
    /// entity in Java's template-placement order. The caller must still run
    /// the template fluid and shape passes and the later marker pass.
    pub fn place_ocean_ruin_archaeology_block(
        &mut self,
        position: [i32; 3],
        replacement: &super::super::ocean_ruin::ArchaeologyReplacement,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<()> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        self.place_ocean_ruin_archaeology_inner(position, replacement, registry)?;
        self.failed = false;
        Ok(())
    }

    fn place_ocean_ruin_archaeology_inner(
        &mut self,
        position: [i32; 3],
        replacement: &super::super::ocean_ruin::ArchaeologyReplacement,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<()> {
        ensure!(
            self.writable.contains(&chunk(position)) && !self.is_outside(position[1]),
            "archaeology block outside writable region"
        );
        let brushable = super::super::GeneratedBrushable::from_replacement(position, replacement)?;
        ensure!(
            registry.is_canonical_state(&brushable.state.name, &brushable.state.properties)
                && registry.has_block_entity(&brushable.state.name) == Some(true),
            "invalid archaeology block registry state"
        );
        let old = self.read(position)?;
        ensure!(
            registry.is_canonical_state(&old.name, &old.properties)
                && registry.has_block_entity(&old.name) == Some(false)
                && !self.monster_entities.contains_key(&position)
                && !self.bee_nests.contains_key(&position),
            "unsupported archaeology replacement source"
        );
        // StructureTemplate writes a barrier first whenever processed NBT is
        // present, then writes the final state and resolves its block entity.
        self.set_proto_block(
            position,
            &BlockState {
                name: "minecraft:barrier".into(),
                properties: BTreeMap::new(),
            },
        )?;
        self.set_proto_block(position, &brushable.state)?;
        Arc::make_mut(&mut self.monster_entities).insert(
            position,
            Some(super::super::GeneratedMonsterRoomEntity::Brushable(
                brushable,
            )),
        );
        Arc::make_mut(self.entity_order.entry(chunk(position)).or_default()).insert(position);
        Ok(())
    }

    /// Adjust a piece against this unpublished region using active data-pack
    /// ice and water tags. A failed read does not alter the piece origin.
    pub fn align_ocean_ruin(
        &mut self,
        piece: &mut super::super::ocean_ruin::PreparedPiece,
        ice_tags: &OreBlockTags,
        water_fluids: &BTreeSet<String>,
    ) -> Result<()> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut world = RuinFloorWorld {
            region: self,
            ice_tags,
            water_fluids,
        };
        piece.align_to_floor(&mut world)
    }

    /// Run one clipped marker on an unpublished region. The spawn callback
    /// must create, initialize and serialize a persistent STRUCTURE drowned
    /// against the current difficulty, or return None for a null entity.
    /// Cloning the callback must isolate its state. Callback state and the
    /// decoration random are committed only on success.
    pub fn handle_ocean_ruin_marker<F>(
        &mut self,
        marker: &super::super::ocean_ruin::RuinDataMarker,
        large: bool,
        sea_level: i32,
        water_fluids: &BTreeSet<String>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        random: &mut DecorationRandom,
        spawn: &mut F,
    ) -> Result<()>
    where
        F: Clone + FnMut([i32; 3], &World) -> Result<Option<super::super::GeneratedEntityRecord>>,
    {
        let mut next_spawn = spawn.clone();
        self.handle_ocean_ruin_marker_inner(
            marker,
            large,
            sea_level,
            water_fluids,
            registry,
            random,
            &mut next_spawn,
        )?;
        *spawn = next_spawn;
        Ok(())
    }

    /// Initialize and save a structure drowned using separate region and
    /// entity random sources. The constructor returns None when Java's entity
    /// factory returns null. Mounted drowned fail closed until their vehicle
    /// and passenger serialization is supported.
    /// All callback state must be cloned independently and staged again by
    /// the caller until the enclosing decoration region is committed.
    pub fn handle_ocean_ruin_marker_with_initialized_drowned<C, W>(
        &mut self,
        marker: &super::super::ocean_ruin::RuinDataMarker,
        large: bool,
        sea_level: i32,
        water_fluids: &BTreeSet<String>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        decoration_random: &mut DecorationRandom,
        region_random: &mut RegionRandom,
        construct: &mut C,
        context: &mut W,
    ) -> Result<()>
    where
        C: Clone + FnMut() -> Result<Option<super::super::FreshCreatureRandom>>,
        W: super::super::ocean_ruin::DrownedMarkerContext,
    {
        let mut next_random = region_random.clone();
        let mut next_construct = construct.clone();
        let mut next_context = context.clone();
        let mut spawn = |position, world: &World| {
            let Some(mut creature) = next_construct()? else {
                return Ok(None);
            };
            ensure!(
                creature.construction.chicken_egg_time.is_none(),
                "invalid drowned construction"
            );
            let special = next_context.special_multiplier(position, world)?;
            let halloween = next_context.halloween()?;
            let mut adapter = DrownedMarkerAdapter {
                position,
                world,
                context: &mut next_context,
                enchanted_hand: None,
            };
            let initialized = initialize_structure_drowned(
                &mut adapter,
                &mut next_random,
                &mut creature.random,
                special,
                halloween,
            )?;
            let main_hand = match (initialized.main_hand, adapter.enchanted_hand) {
                (MainHandRoll::Unchanged, None) => None,
                (MainHandRoll::Trident, None) => Some(default_hand("minecraft:trident")),
                (MainHandRoll::FishingRod, None) => Some(default_hand("minecraft:fishing_rod")),
                (_, Some(tag)) => Some(tag),
            };
            let tag = super::super::draft_fresh_structure_drowned(
                creature.construction.uuid,
                position,
                &initialized,
                main_hand,
            )?;
            Ok(Some(super::super::GeneratedEntityRecord::Saved(tag)))
        };
        self.handle_ocean_ruin_marker_inner(
            marker,
            large,
            sea_level,
            water_fluids,
            registry,
            decoration_random,
            &mut spawn,
        )?;
        *region_random = next_random;
        *construct = next_construct;
        *context = next_context;
        Ok(())
    }

    fn handle_ocean_ruin_marker_inner<F>(
        &mut self,
        marker: &super::super::ocean_ruin::RuinDataMarker,
        large: bool,
        sea_level: i32,
        water_fluids: &BTreeSet<String>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        random: &mut DecorationRandom,
        spawn: &mut F,
    ) -> Result<()>
    where
        F: FnMut([i32; 3], &World) -> Result<Option<super::super::GeneratedEntityRecord>>,
    {
        ensure!(!self.failed, "generation attempt previously failed");
        let kind = if marker.metadata.matches("chest") {
            DataMarkerKind::Chest
        } else if marker.metadata.matches("drowned") {
            DataMarkerKind::Drowned
        } else {
            DataMarkerKind::Other
        };
        let mut next_random = random.clone();
        self.failed = true;
        handle_data_marker(
            &mut RuinMarkerRegion {
                region: self,
                registry,
                water_fluids,
                sea_level,
                spawn,
            },
            &mut next_random,
            kind,
            marker.position,
            large,
        )?;
        *random = next_random;
        self.failed = false;
        Ok(())
    }
}

fn default_hand(id: &str) -> Tag {
    Tag::Compound(vec![
        ("id".into(), Tag::String(id.into())),
        ("count".into(), Tag::Int(1)),
    ])
}

struct DrownedMarkerAdapter<'a, W> {
    position: [i32; 3],
    world: &'a World,
    context: &'a mut W,
    enchanted_hand: Option<Tag>,
}

impl<W: super::super::ocean_ruin::DrownedMarkerContext> NautilusJockeyWorld
    for DrownedMarkerAdapter<'_, W>
{
    fn more_frequent_drowned_biome(&mut self) -> Result<bool> {
        self.context
            .more_frequent_drowned_biome(self.position, self.world)
    }
    fn spawn_zombie_nautilus(
        &mut self,
        random: &mut RegionRandom,
        persistent: bool,
    ) -> Result<bool> {
        self.context
            .spawn_zombie_nautilus(self.position, self.world, random, persistent)
    }
}
impl<W: super::super::ocean_ruin::DrownedMarkerContext> StructureDrownedWorld
    for DrownedMarkerAdapter<'_, W>
{
    fn try_existing_chicken(&mut self) -> Result<bool> {
        self.context.try_existing_chicken(self.position, self.world)
    }
    fn spawn_chicken_jockey(&mut self, random: &mut RegionRandom) -> Result<bool> {
        self.context
            .spawn_chicken_jockey(self.position, self.world, random)
    }
    fn enchant_weapon(
        &mut self,
        random: &mut RegionRandom,
        weapon: MainHandRoll,
        special: f32,
    ) -> Result<()> {
        self.enchanted_hand = Some(self.context.enchant_weapon(
            self.position,
            self.world,
            random,
            weapon,
            special,
        )?);
        Ok(())
    }
}

struct RuinMarkerRegion<'r, 'a, 't, F> {
    region: &'r mut DecorationRegion<'a>,
    registry: &'t native_resources::block_registry::GeneratedBlockRegistry,
    water_fluids: &'t BTreeSet<String>,
    sea_level: i32,
    spawn: &'r mut F,
}

impl<F> RuinMarkerRegion<'_, '_, '_, F> {
    fn replace_non_entity(&mut self, position: [i32; 3], state: &BlockState) -> Result<()> {
        ensure!(
            self.region.writable.contains(&chunk(position)) && !self.region.is_outside(position[1]),
            "ruin marker outside writable region"
        );
        let old = self.region.read(position)?;
        ensure!(
            self.registry.has_block_entity(&old.name) == Some(false),
            "unsupported ruin marker replacement source: {}",
            old.name
        );
        ensure!(
            !self.region.monster_entities.contains_key(&position),
            "unexpected block entity at ruin marker"
        );
        self.region.set_proto_block(position, state)
    }
}

impl<F> MarkerWorld for RuinMarkerRegion<'_, '_, '_, F>
where
    F: FnMut([i32; 3], &World) -> Result<Option<super::super::GeneratedEntityRecord>>,
{
    fn fluid_in_water_tag(&mut self, position: [i32; 3]) -> Result<bool> {
        fluid_in_water_tag(&OreWorld::block(self.region, position)?, self.water_fluids)
    }
    fn set_default_chest(&mut self, position: [i32; 3], waterlogged: bool) -> Result<()> {
        let state = BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), waterlogged.to_string()),
            ]),
        };
        let old = OreWorld::block(self.region, position)?;
        if old.name == "minecraft:chest" {
            ensure!(
                old == state && self.region.monster_entities.contains_key(&position),
                "unsupported existing chest at ruin marker"
            );
            return Ok(());
        }
        self.replace_non_entity(position, &state)?;
        Arc::make_mut(&mut self.region.monster_entities).insert(
            position,
            Some(super::super::GeneratedMonsterRoomEntity::PendingChest { position, state }),
        );
        Arc::make_mut(
            self.region
                .pending_order
                .entry(chunk(position))
                .or_default(),
        )
        .insert(position);
        Ok(())
    }
    fn chest_entity_exists(&mut self, position: [i32; 3]) -> Result<bool> {
        self.region.resolve_generated_chest_inner(position)
    }
    fn set_chest_loot(&mut self, position: [i32; 3], table: &str, seed: i64) -> Result<()> {
        let chest = match self.region.monster_entities.get(&position) {
            Some(Some(super::super::GeneratedMonsterRoomEntity::EmptyChest(chest))) => {
                chest.with_loot(table, seed)?
            }
            Some(Some(super::super::GeneratedMonsterRoomEntity::Chest(chest))) => {
                chest.with_loot_table(table, seed)?
            }
            _ => bail!("ruin chest entity unavailable"),
        };
        Arc::make_mut(&mut self.region.monster_entities).insert(
            position,
            Some(super::super::GeneratedMonsterRoomEntity::Chest(chest)),
        );
        Ok(())
    }
    fn create_and_spawn_drowned(&mut self, position: [i32; 3]) -> Result<bool> {
        ensure!(
            self.region.writable.contains(&chunk(position)) && !self.region.is_outside(position[1]),
            "ruin drowned outside writable region"
        );
        let Some(record) = (self.spawn)(position, &self.region.world)? else {
            return Ok(false);
        };
        self.region.record_generated_entity(position, record)?;
        Ok(true)
    }
    fn sea_level(&mut self) -> Result<i32> {
        Ok(self.sea_level)
    }
    fn clear_drowned_marker(&mut self, position: [i32; 3], water: bool) -> Result<()> {
        let state = if water {
            BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([("level".into(), "0".into())]),
            }
        } else {
            BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            }
        };
        self.replace_non_entity(position, &state)
    }
}

impl FloorWorld for RuinFloorWorld<'_, '_, '_> {
    fn min_y(&self) -> i32 {
        self.region.target.min_y
    }

    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        OreVeinWorld::ocean_floor_height(self.region, x, z)
    }

    fn floor_scan_passable(&mut self, position: [i32; 3]) -> Result<bool> {
        let state = OreWorld::block(self.region, position)?;
        if is_air(&state) || fluid_in_water_tag(&state, self.water_fluids)? {
            return Ok(true);
        }
        static ICE_TAG: OnceLock<ResourceId> = OnceLock::new();
        self.ice_tags.contains(
            ICE_TAG.get_or_init(|| ResourceId::parse("minecraft:ice").unwrap()),
            &state.name,
        )
    }
}

struct RuinPieceRegion<'r, 'a, 't, F> {
    marker: RuinMarkerRegion<'r, 'a, 't, F>,
    ice_tags: &'t OreBlockTags,
    bubble_fluids: &'t BTreeSet<String>,
}

impl<F> FloorWorld for RuinPieceRegion<'_, '_, '_, F> {
    fn min_y(&self) -> i32 {
        self.marker.region.target.min_y
    }
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        OreVeinWorld::ocean_floor_height(self.marker.region, x, z)
    }
    fn floor_scan_passable(&mut self, position: [i32; 3]) -> Result<bool> {
        RuinFloorWorld {
            region: self.marker.region,
            ice_tags: self.ice_tags,
            water_fluids: self.marker.water_fluids,
        }
        .floor_scan_passable(position)
    }
}

impl<F> MarkerWorld for RuinPieceRegion<'_, '_, '_, F>
where
    F: FnMut([i32; 3], &World) -> Result<Option<super::super::GeneratedEntityRecord>>,
{
    fn fluid_in_water_tag(&mut self, position: [i32; 3]) -> Result<bool> {
        self.marker.fluid_in_water_tag(position)
    }
    fn set_default_chest(&mut self, position: [i32; 3], waterlogged: bool) -> Result<()> {
        self.marker.set_default_chest(position, waterlogged)
    }
    fn chest_entity_exists(&mut self, position: [i32; 3]) -> Result<bool> {
        self.marker.chest_entity_exists(position)
    }
    fn set_chest_loot(&mut self, position: [i32; 3], table: &str, seed: i64) -> Result<()> {
        self.marker.set_chest_loot(position, table, seed)
    }
    fn create_and_spawn_drowned(&mut self, position: [i32; 3]) -> Result<bool> {
        self.marker.create_and_spawn_drowned(position)
    }
    fn sea_level(&mut self) -> Result<i32> {
        self.marker.sea_level()
    }
    fn clear_drowned_marker(&mut self, position: [i32; 3], water: bool) -> Result<()> {
        self.marker.clear_drowned_marker(position, water)
    }
}

impl<F> ShapeEdgeWorld for RuinPieceRegion<'_, '_, '_, F> {
    type State = BlockState;
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self.marker.region, pos)
    }
    fn update_shape(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        direction: [i32; 3],
        neighbor_pos: [i32; 3],
        neighbor: &BlockState,
    ) -> Result<BlockState> {
        use native_world::{
            fluid::{FluidKind, FluidState},
            stairs::{Neighbor, Stair},
        };
        let region = &mut *self.marker.region;
        if state.name.starts_with("minecraft:") && state.name.ends_with("_stairs") {
            ensure!(
                self.marker
                    .registry
                    .is_canonical_state(&state.name, &state.properties),
                "unsupported ruin stair state"
            );
            if state.properties["waterlogged"] == "true" {
                SpringWorld::schedule_spring_tick(
                    region,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            if direction[1] != 0 {
                return Ok(state.clone());
            }
            let stair = Stair::from_state(state).context("invalid ruin stair")?;
            let base = glam::IVec3::from_array(pos);
            let shape = native_world::stairs::shape(stair, |offset| {
                let Some(x) = base.x.checked_add(offset.x) else {
                    return Neighbor::Unknown;
                };
                let Some(z) = base.z.checked_add(offset.z) else {
                    return Neighbor::Unknown;
                };
                let Ok(other) = region.read([x, base.y, z]) else {
                    return Neighbor::Unknown;
                };
                if other.name.starts_with("minecraft:") && other.name.ends_with("_stairs") {
                    if !self
                        .marker
                        .registry
                        .is_canonical_state(&other.name, &other.properties)
                    {
                        return Neighbor::Unknown;
                    }
                    Stair::from_state(&other).map_or(Neighbor::Unknown, Neighbor::Stair)
                } else {
                    Neighbor::Other
                }
            })
            .context("ruin stair needs unavailable neighbors")?;
            let mut result = state.clone();
            result.properties.insert("shape".into(), shape.into());
            return Ok(result);
        }
        if matches!(
            state.name.as_str(),
            "minecraft:suspicious_sand" | "minecraft:suspicious_gravel"
        ) {
            ensure!(
                self.marker
                    .registry
                    .is_canonical_state(&state.name, &state.properties),
                "invalid brushable shape state"
            );
            SimpleBlockWorld::schedule_simple_tick(region, pos)?;
            return Ok(state.clone());
        }
        if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            ensure!(
                self.marker
                    .registry
                    .is_canonical_state(&state.name, &state.properties),
                "invalid ruin fluid state"
            );
            let fluid = FluidState::from_block(state)
                .map_err(anyhow::Error::msg)?
                .context("missing fluid")?;
            let other = FluidState::from_block(neighbor).map_err(anyhow::Error::msg)?;
            let id = match (fluid.kind, fluid.source) {
                (FluidKind::Water, true) => "minecraft:water",
                (FluidKind::Water, false) => "minecraft:flowing_water",
                (FluidKind::Lava, true) => "minecraft:lava",
                (FluidKind::Lava, false) => "minecraft:flowing_lava",
            };
            if fluid.source || other.is_some_and(|value| value.source) {
                SpringWorld::schedule_spring_tick(region, pos, &ResourceId::parse(id)?)?;
            }
            if direction == [0, -1, 0]
                && fluid.source
                && fluid.amount == 8
                && self.bubble_fluids.contains(id)
            {
                let down = ResourceId::parse("minecraft:enables_bubble_column_drag_down")?;
                let up = ResourceId::parse("minecraft:enables_bubble_column_push_up")?;
                if region.tags.contains(&down, &neighbor.name)?
                    || region.tags.contains(&up, &neighbor.name)?
                {
                    SimpleBlockWorld::schedule_simple_tick(region, pos)?;
                }
            }
            return Ok(state.clone());
        }
        if matches!(
            state.name.as_str(),
            "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
        ) && state.properties.is_empty()
        {
            SimpleBlockWorld::schedule_simple_tick(region, pos)?;
            return Ok(state.clone());
        }
        if self
            .marker
            .registry
            .is_canonical_state(&state.name, &state.properties)
            && super::super::ocean_ruin::ocean_template_static_full(state).ok() == Some(true)
        {
            return Ok(state.clone());
        }
        ShapeEdgeWorld::update_shape(region, pos, state, direction, neighbor_pos, neighbor)
    }
    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        ensure!(
            matches!(flags, 2 | 18),
            "unsupported ruin shape write flags"
        );
        if !self.marker.region.writable.contains(&chunk(pos))
            || self.marker.region.is_outside(pos[1])
        {
            return Ok(());
        }
        ensure!(
            self.marker
                .registry
                .is_canonical_state(&state.name, &state.properties),
            "unsupported ruin shape write state"
        );
        self.marker.region.set_proto_block(pos, state)
    }
}

impl<F> super::super::ocean_ruin::OceanRuinPlacementWorld for RuinPieceRegion<'_, '_, '_, F>
where
    F: FnMut([i32; 3], &World) -> Result<Option<super::super::GeneratedEntityRecord>>,
{
    fn fluid_at(&mut self, position: [i32; 3]) -> Result<Option<native_world::fluid::FluidState>> {
        let state = OreWorld::block(self.marker.region, position)?;
        self.fluid_of_state(&state)
    }
    fn fluid_of_state(
        &mut self,
        state: &BlockState,
    ) -> Result<Option<native_world::fluid::FluidState>> {
        native_world::fluid::FluidState::from_block(state).map_err(anyhow::Error::msg)
    }
    fn place_template_block(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        _: &mut DecorationRandom,
    ) -> Result<bool> {
        let region = &mut self.marker.region;
        if !region.writable.contains(&chunk(position)) {
            return Ok(false);
        }
        ensure!(
            region.world.chunks.contains_key(&chunk(position)),
            "ruin block chunk missing"
        );
        if region.is_outside(position[1]) {
            return Ok(true);
        }
        if let Some(nbt) = nbt {
            let replacement = super::super::ocean_ruin::ArchaeologyReplacement {
                state: state.clone(),
                nbt: nbt.clone(),
            };
            region.place_ocean_ruin_archaeology_inner(
                position,
                &replacement,
                self.marker.registry,
            )?;
            return Ok(true);
        }
        ensure!(
            self.marker
                .registry
                .is_canonical_state(&state.name, &state.properties)
                && self.marker.registry.has_block_entity(&state.name) == Some(false)
                && super::super::ocean_ruin::ocean_template_static_full(state).is_ok(),
            "unsupported ruin template block"
        );
        let old = region.read(position)?;
        ensure!(
            self.marker
                .registry
                .is_canonical_state(&old.name, &old.properties)
                && self.marker.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some()
                    || super::super::ocean_ruin::ocean_template_static_full(&old).is_ok())
                && !region.monster_entities.contains_key(&position)
                && !region.bee_nests.contains_key(&position),
            "unsupported ruin template replacement source"
        );
        region.set_proto_block(position, state)?;
        Ok(true)
    }
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name.starts_with("minecraft:")
            && state.name.ends_with("_stairs")
            && self
                .marker
                .registry
                .is_canonical_state(&state.name, &state.properties))
    }
    fn place_liquid(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
        fluid: Option<native_world::fluid::FluidState>,
    ) -> Result<()> {
        if state.properties.get("waterlogged").map(String::as_str) == Some("false")
            && fluid.is_some_and(|value| {
                value.kind == native_world::fluid::FluidKind::Water && value.source
            })
        {
            let mut wet = state.clone();
            wet.properties.insert("waterlogged".into(), "true".into());
            if self.marker.region.writable.contains(&chunk(position))
                && !self.marker.region.is_outside(position[1])
            {
                self.marker.region.set_proto_block(position, &wet)?;
            }
            SpringWorld::schedule_spring_tick(
                self.marker.region,
                position,
                &ResourceId::parse("minecraft:water")?,
            )?;
        }
        Ok(())
    }
    fn update_neighbors_at(&mut self, _: [i32; 3], _: &BlockState) -> Result<()> {
        Ok(())
    }
    fn mark_block_entity_changed(&mut self, _: [i32; 3]) -> Result<()> {
        Ok(())
    }
    fn place_template_entities(
        &mut self,
        _: &[Tag],
        _: [i32; 3],
        _: native_resources::ocean_ruin::Rotation,
        _: BoundingBox,
    ) -> Result<()> {
        bail!("ocean ruin template entities are unsupported")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::beardifier::BoundingBox;
    use native_persistence::{
        nbt::{NamedTag, Tag},
        structure_template::StructureTemplate,
    };
    use native_resources::{
        ocean_ruin::{Piece, Rotation, Temperature},
        packs::ResourceProvider,
    };

    #[derive(Clone, Default, Debug, PartialEq)]
    struct DrownedContext {
        calls: usize,
        enchants: usize,
        invalid_difficulty: bool,
    }
    impl super::super::super::ocean_ruin::DrownedMarkerContext for DrownedContext {
        fn special_multiplier(&mut self, _: [i32; 3], _: &World) -> Result<f32> {
            self.calls += 1;
            Ok(if self.invalid_difficulty {
                f32::NAN
            } else {
                1.0
            })
        }
        fn halloween(&mut self) -> Result<bool> {
            Ok(false)
        }
        fn try_existing_chicken(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
            self.calls += 1;
            Ok(false)
        }
        fn spawn_chicken_jockey(
            &mut self,
            _: [i32; 3],
            _: &World,
            _: &mut RegionRandom,
        ) -> Result<bool> {
            self.calls += 1;
            Ok(false)
        }
        fn enchant_weapon(
            &mut self,
            _: [i32; 3],
            _: &World,
            random: &mut RegionRandom,
            weapon: MainHandRoll,
            _: f32,
        ) -> Result<Tag> {
            self.calls += 1;
            self.enchants += 1;
            let id = match weapon {
                MainHandRoll::Trident => "minecraft:trident",
                MainHandRoll::FishingRod => "minecraft:fishing_rod",
                MainHandRoll::Unchanged => bail!("unexpected empty weapon"),
            };
            let Tag::Compound(mut fields) = default_hand(id) else {
                unreachable!()
            };
            fields.push(("test_enchantment".into(), Tag::Long(random.next_long())));
            Ok(Tag::Compound(fields))
        }
        fn more_frequent_drowned_biome(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
            self.calls += 1;
            Ok(false)
        }
        fn spawn_zombie_nautilus(
            &mut self,
            _: [i32; 3],
            _: &World,
            _: &mut RegionRandom,
            persistent: bool,
        ) -> Result<bool> {
            ensure!(persistent, "structure nautilus must be persistent");
            self.calls += 1;
            Ok(false)
        }
    }

    struct IceTag(bool);
    impl ResourceProvider for IceTag {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            if path != "data/minecraft/tags/block/ice.json" {
                return Ok(None);
            }
            Ok(Some(if self.0 {
                br#"{"values":["minecraft:stone"]}"#.to_vec()
            } else {
                br#"{"values":[]}"#.to_vec()
            }))
        }
    }

    struct MotionTags;
    impl ResourceProvider for MotionTags {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                    Some(br#"{"values":["minecraft:stone","minecraft:chest"]}"#.to_vec())
                }
                _ => None,
            })
        }
    }

    #[test]
    fn region_uses_active_ice_tag_and_keeps_piece_on_failure() {
        let registry = BTreeSet::from(["minecraft:air".to_string(), "minecraft:stone".to_string()]);
        let id = ResourceId::parse("minecraft:ice").unwrap();
        let tags = |include| {
            OreBlockTags::load(
                &mut IceTag(include),
                &registry,
                &BTreeSet::from([id.clone()]),
            )
            .unwrap()
        };
        let empty_ice = tags(false);
        let custom_ice = tags(true);
        let missing = OreBlockTags::load(&mut IceTag(false), &registry, &BTreeSet::new()).unwrap();
        let (mut terrain, region_tags, motion, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain
            .decoration_region(&region_tags, &motion, writable)
            .unwrap();
        let mut piece = super::super::super::ocean_ruin::PreparedPiece {
            selection: Piece {
                template: "minecraft:test".into(),
                position: [8, 90, 8],
                rotation: Rotation::None,
                integrity: 1.0,
                large: false,
            },
            template: Arc::new(StructureTemplate {
                document: NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                },
                data_version: Some(5023),
                size: [1, 1, 1],
                palettes: vec![],
                blocks: vec![],
                entities: vec![],
            }),
            bounds: BoundingBox::new([8, 90, 8], [8, 90, 8]).unwrap(),
        };
        let water = BTreeSet::from([
            "minecraft:water".to_string(),
            "minecraft:flowing_water".to_string(),
        ]);
        assert!(
            region
                .align_ocean_ruin(&mut piece, &missing, &water)
                .is_err()
        );
        assert_eq!(piece.selection.position, [8, 90, 8]);
        region
            .align_ocean_ruin(&mut piece, &empty_ice, &water)
            .unwrap();
        assert_eq!(piece.selection.position, [8, 8, 8]);
        region
            .align_ocean_ruin(&mut piece, &custom_ice, &water)
            .unwrap();
        assert_eq!(piece.selection.position, [8, 2, 8]);
        assert_eq!(piece.bounds.min, [8, 2, 8]);
        assert_eq!(piece.bounds.max, [8, 2, 8]);
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn region_stages_waterlogged_ruin_chest_and_drowned_marker() -> Result<()> {
        use super::super::super::GeneratedEntityRecord;
        use super::super::super::ocean_ruin::RuinDataMarker;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let (mut terrain, region_tags, _, _) = super::super::tests::fixture();
        let block_ids = [
            "minecraft:stone",
            "minecraft:air",
            "minecraft:water",
            "minecraft:chest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let motion = HeightmapTags::load(&mut MotionTags, &block_ids)?;
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&region_tags, &motion, writable)?;
        let water = BTreeSet::from([
            "minecraft:water".to_owned(),
            "minecraft:flowing_water".to_owned(),
        ]);
        let chest_pos = [8, 8, 8];
        region.set_proto_block(
            chest_pos,
            &BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([("level".into(), "0".into())]),
            },
        )?;
        let mut random = DecorationRandom::new(31415);
        let mut expected = random.clone();
        let expected_seed = expected.next_long();
        let marker = RuinDataMarker {
            source_index: 0,
            position: chest_pos,
            metadata: "chest".into(),
        };
        let mut no_spawn =
            |_: [i32; 3], _: &World| -> Result<Option<GeneratedEntityRecord>> { Ok(None) };
        region.handle_ocean_ruin_marker(
            &marker,
            true,
            63,
            &water,
            &registry,
            &mut random,
            &mut no_spawn,
        )?;
        assert_eq!(random.next_long(), expected.next_long());
        let chest_state = OreWorld::block(&mut region, chest_pos)?;
        assert_eq!(chest_state.name, "minecraft:chest");
        assert_eq!(
            chest_state
                .properties
                .get("waterlogged")
                .map(String::as_str),
            Some("true")
        );
        let Some(Some(super::super::super::GeneratedMonsterRoomEntity::Chest(chest))) =
            region.monster_entities.get(&chest_pos)
        else {
            anyhow::bail!("missing ruin chest")
        };
        assert_eq!(chest.loot_seed, expected_seed);
        assert!(
            matches!(chest.block_entity().get("LootTable"), Some(Tag::String(id)) if id.matches("minecraft:chests/underwater_ruin_big"))
        );

        let drowned_pos = [9, 8, 8];
        let drowned = RuinDataMarker {
            source_index: 1,
            position: drowned_pos,
            metadata: "drowned".into(),
        };
        region.handle_ocean_ruin_marker(
            &drowned,
            false,
            63,
            &water,
            &registry,
            &mut random,
            &mut no_spawn,
        )?;
        assert_eq!(
            OreWorld::block(&mut region, drowned_pos)?.name,
            "minecraft:air"
        );
        let mut spawn = |_: [i32; 3], _: &World| -> Result<Option<GeneratedEntityRecord>> {
            Ok(Some(GeneratedEntityRecord::Saved(Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:drowned".into()),
            )]))))
        };
        region.handle_ocean_ruin_marker(
            &drowned,
            false,
            63,
            &water,
            &registry,
            &mut random,
            &mut spawn,
        )?;
        let state = OreWorld::block(&mut region, drowned_pos)?;
        assert_eq!(state.name, "minecraft:water");
        assert_eq!(state.properties.get("level").map(String::as_str), Some("0"));
        assert_eq!(
            region
                .generated_entities
                .get(&chunk(drowned_pos))
                .unwrap()
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn initialized_drowned_marker_composes_rng_equipment_and_rolls_back() -> Result<()> {
        use super::super::super::{
            FreshCreatureRandom,
            ocean_ruin::{DrownedMarkerContext, RuinDataMarker},
        };
        use native_core::random::LegacyRandom;

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let (mut terrain, region_tags, _, _) = super::super::tests::fixture();
        let ids = [
            "minecraft:stone",
            "minecraft:air",
            "minecraft:water",
            "minecraft:chest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let motion = HeightmapTags::load(&mut MotionTags, &ids)?;
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&region_tags, &motion, writable)?;
        let water = BTreeSet::from([
            "minecraft:water".to_owned(),
            "minecraft:flowing_water".to_owned(),
        ]);
        let position = [8, 8, 8];
        let marker = RuinDataMarker {
            source_index: 0,
            position,
            metadata: "drowned".into(),
        };
        let drowned = ResourceId::parse("minecraft:drowned")?;
        let mut decoration = DecorationRandom::new(8123);
        let mut expected_decoration = decoration.clone();
        let mut context = DrownedContext::default();
        for i in 0..512_i64 {
            let entity_seed = i * 98317 + 3329;
            let mut construct = || Ok(Some(FreshCreatureRandom::from_seed(&drowned, entity_seed)?));
            let mut random = RegionRandom::Legacy(LegacyRandom::new(i * 671231 - 12345));
            let mut expected_random = random.clone();
            let mut expected_context = context.clone();
            let mut creature = construct()?.unwrap();
            let special = expected_context.special_multiplier(position, &region.world)?;
            let halloween = expected_context.halloween()?;
            let mut adapter = DrownedMarkerAdapter {
                position,
                world: &region.world,
                context: &mut expected_context,
                enchanted_hand: None,
            };
            let initialized = initialize_structure_drowned(
                &mut adapter,
                &mut expected_random,
                &mut creature.random,
                special,
                halloween,
            )?;
            let hand = match (initialized.main_hand, adapter.enchanted_hand) {
                (MainHandRoll::Unchanged, None) => None,
                (MainHandRoll::Trident, None) => Some(default_hand("minecraft:trident")),
                (MainHandRoll::FishingRod, None) => Some(default_hand("minecraft:fishing_rod")),
                (_, Some(tag)) => Some(tag),
            };
            let expected = super::super::super::draft_fresh_structure_drowned(
                creature.construction.uuid,
                position,
                &initialized,
                hand,
            )?;
            region.handle_ocean_ruin_marker_with_initialized_drowned(
                &marker,
                false,
                63,
                &water,
                &registry,
                &mut decoration,
                &mut random,
                &mut construct,
                &mut context,
            )?;
            assert_eq!(context, expected_context);
            assert_eq!(random.next_long(), expected_random.next_long());
            let records = region.generated_entities.get(&chunk(position)).unwrap();
            assert_eq!(records.last(), Some(&expected));
            assert_eq!(records.len(), (i + 1) as usize);
        }
        assert!(
            context.enchants > 0,
            "sample must exercise enchantment provider"
        );
        assert_eq!(decoration.next_long(), expected_decoration.next_long());
        assert_eq!(
            OreWorld::block(&mut region, position)?.name,
            "minecraft:water"
        );

        let before_count = region
            .generated_entities
            .get(&chunk(position))
            .unwrap()
            .len();
        let mut null_factory = || -> Result<Option<FreshCreatureRandom>> { Ok(None) };
        let mut region_random = RegionRandom::Legacy(LegacyRandom::new(91));
        let mut untouched_random = region_random.clone();
        let before_context = context.clone();
        region.handle_ocean_ruin_marker_with_initialized_drowned(
            &marker,
            false,
            63,
            &water,
            &registry,
            &mut decoration,
            &mut region_random,
            &mut null_factory,
            &mut context,
        )?;
        assert_eq!(context, before_context);
        assert_eq!(region_random.next_long(), untouched_random.next_long());
        assert_eq!(
            region
                .generated_entities
                .get(&chunk(position))
                .unwrap()
                .len(),
            before_count
        );

        let mut context = DrownedContext {
            invalid_difficulty: true,
            ..Default::default()
        };
        let before_context = context.clone();
        let mut random = RegionRandom::Legacy(LegacyRandom::new(42));
        let mut before_random = random.clone();
        let mut before_decoration = decoration.clone();
        let mut construct = || Ok(Some(FreshCreatureRandom::from_seed(&drowned, 123)?));
        assert!(
            region
                .handle_ocean_ruin_marker_with_initialized_drowned(
                    &marker,
                    false,
                    63,
                    &water,
                    &registry,
                    &mut decoration,
                    &mut random,
                    &mut construct,
                    &mut context
                )
                .is_err()
        );
        assert_eq!(context, before_context);
        assert_eq!(random.next_long(), before_random.next_long());
        assert_eq!(decoration.next_long(), before_decoration.next_long());
        assert_eq!(
            region
                .generated_entities
                .get(&chunk(position))
                .unwrap()
                .len(),
            before_count
        );
        assert!(region.failed);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn archaeology_block_records_deferred_loot_and_rolls_back() -> Result<()> {
        use super::super::super::ocean_ruin::ArchaeologyReplacement;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let ids = [
            "minecraft:stone",
            "minecraft:air",
            "minecraft:chest",
            "minecraft:barrier",
            "minecraft:suspicious_sand",
            "minecraft:suspicious_gravel",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let motion = HeightmapTags::load(&mut MotionTags, &ids)?;
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let replacement = |name: &str, seed| ArchaeologyReplacement {
            state: BlockState {
                name: name.into(),
                properties: BTreeMap::from([("dusted".into(), "0".into())]),
            },
            nbt: Tag::Compound(vec![
                (
                    "LootTable".into(),
                    Tag::String("minecraft:archaeology/ocean_ruin_warm".into()),
                ),
                ("LootTableSeed".into(), Tag::Long(seed)),
            ]),
        };
        let sand = replacement("minecraft:suspicious_sand", 73);
        let gravel = replacement("minecraft:suspicious_gravel", 0);
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        region.place_ocean_ruin_archaeology_block([8, 8, 8], &sand, &registry)?;
        region.place_ocean_ruin_archaeology_block([9, 8, 8], &gravel, &registry)?;
        region.finish()?;
        let Tag::List { values, .. } =
            terrain.generated_block_entity_data(ChunkPos { x: 0, z: 0 }, &registry)?
        else {
            bail!("missing entities")
        };
        assert_eq!(values.len(), 2);
        assert!(values.iter().any(|tag| tag.get("x") == Some(&Tag::Int(8))
            && tag.get("LootTableSeed") == Some(&Tag::Long(73))));
        assert!(values.iter().any(|tag| tag.get("x") == Some(&Tag::Int(9)) && tag.get("LootTableSeed").is_none()));
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        assert!(
            region
                .place_ocean_ruin_archaeology_block(
                    [10, 8, 8],
                    &replacement("minecraft:stone", 0),
                    &registry
                )
                .is_err()
        );
        assert!(region.finish().is_err());
        assert_eq!(terrain.snapshot().block(10, 8, 8), before.block(10, 8, 8));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn region_places_ocean_ruin_piece_and_discards_failed_attempt() -> Result<()> {
        use super::super::super::{GeneratedEntityRecord, ocean_ruin::PreparedPiece};
        use native_persistence::structure_template::TemplateBlock;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let ice_id = ResourceId::parse("minecraft:ice")?;
        let ice = OreBlockTags::load(&mut IceTag(false), &ids, &BTreeSet::from([ice_id]))?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let mut palette: Vec<_> = [
            "stone_bricks",
            "stone_brick_stairs",
            "sand",
            "structure_block",
        ]
        .into_iter()
        .map(|id| {
            Tag::Compound(vec![(
                "id".into(),
                Tag::String(format!("minecraft:{id}").as_str().into()),
            )])
        })
        .collect();
        palette.push(Tag::Compound(vec![
            (
                "id".into(),
                Tag::String("minecraft:stone_brick_stairs".into()),
            ),
            (
                "properties".into(),
                Tag::Compound(vec![("facing".into(), Tag::String("east".into()))]),
            ),
        ]));
        let marker_nbt = Tag::Compound(vec![
            ("mode".into(), Tag::String("DATA".into())),
            ("metadata".into(), Tag::String("chest".into())),
        ]);
        let blocks = vec![
            TemplateBlock {
                position: [0, 0, 0],
                state: 0,
                nbt: None,
            },
            TemplateBlock {
                position: [1, 0, 0],
                state: 1,
                nbt: None,
            },
            TemplateBlock {
                position: [1, 0, 1],
                state: 4,
                nbt: None,
            },
            TemplateBlock {
                position: [2, 0, 0],
                state: 2,
                nbt: None,
            },
            TemplateBlock {
                position: [3, 0, 0],
                state: 3,
                nbt: Some(marker_nbt),
            },
        ];
        let piece = PreparedPiece {
            selection: Piece {
                template: "minecraft:test".into(),
                position: [1, 15, 1],
                rotation: Rotation::None,
                integrity: 1.0,
                large: false,
            },
            template: Arc::new(StructureTemplate {
                document: NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                },
                data_version: Some(5023),
                size: [4, 1, 2],
                palettes: vec![palette],
                blocks,
                entities: vec![],
            }),
            bounds: BoundingBox::new([1, 15, 1], [4, 15, 2]).unwrap(),
        };
        let water = BTreeSet::from(["minecraft:water".into(), "minecraft:flowing_water".into()]);
        let bubbles = BTreeSet::new();
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut piece = piece;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        let expected_loot_seed = expected.next_long();
        let mut no_spawn =
            |_: [i32; 3], _: &World| -> Result<Option<GeneratedEntityRecord>> { Ok(None) };
        assert!(region.place_ocean_ruin_piece(
            &mut piece,
            &ice,
            &water,
            &bubbles,
            &registry,
            Temperature::Warm,
            63,
            clip,
            &mut random,
            &mut no_spawn
        )?);
        assert_eq!(piece.selection.position, [1, 8, 1]);
        assert_eq!(random.next_long(), expected.next_long());
        assert_eq!(region.read([1, 8, 1])?.name, "minecraft:stone_bricks");
        assert_eq!(region.read([3, 8, 1])?.name, "minecraft:suspicious_sand");
        assert_eq!(region.read([4, 8, 1])?.name, "minecraft:chest");
        let Some(Some(super::super::super::GeneratedMonsterRoomEntity::Chest(chest))) =
            region.monster_entities.get(&[4, 8, 1])
        else {
            bail!("missing chest")
        };
        assert_eq!(chest.loot_seed, expected_loot_seed);
        assert!(
            region
                .world
                .saved_block_ticks
                .get(&ChunkPos { x: 0, z: 0 })
                .is_some_and(|ticks| ticks
                    .iter()
                    .any(|t| t.position == [3, 8, 1] && t.block == "minecraft:suspicious_sand"))
        );
        let actual = java_ruin_region_output(&game)?;
        let mut expected_lines = Vec::new();
        for p in [[1, 8, 1], [2, 8, 1], [2, 8, 2], [3, 8, 1]] {
            let state = region.read(p)?;
            let props = state
                .properties
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ");
            expected_lines.push(format!(
                "B\t{}\t{}\t{}\t{}\t{{{props}}}",
                p[0], p[1], p[2], state.name
            ));
        }
        if let Some(ticks) = region.world.saved_block_ticks.get(&ChunkPos { x: 0, z: 0 }) {
            for tick in ticks {
                expected_lines.push(format!(
                    "T\t{}\t{}\t{}\t{}",
                    tick.block, tick.position[0], tick.position[1], tick.position[2]
                ));
            }
        }
        if let Some(ticks) = region.world.saved_fluid_ticks.get(&ChunkPos { x: 0, z: 0 }) {
            for tick in ticks {
                expected_lines.push(format!(
                    "F\t{}\t{}\t{}\t{}",
                    tick.fluid, tick.position[0], tick.position[1], tick.position[2]
                ));
            }
        }
        assert_eq!(actual, expected_lines);
        region.finish()?;
        let Tag::List { values, .. } =
            terrain.generated_block_entity_data(ChunkPos { x: 0, z: 0 }, &registry)?
        else {
            bail!("missing block entities")
        };
        assert_eq!(values.len(), 2);
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let before_piece = piece.selection.clone();
        let mut before_random = random.clone();
        assert!(
            region
                .place_ocean_ruin_piece(
                    &mut piece,
                    &ice,
                    &water,
                    &bubbles,
                    &registry,
                    Temperature::Warm,
                    63,
                    clip,
                    &mut random,
                    &mut no_spawn
                )
                .is_err()
        );
        assert_eq!(piece.selection, before_piece);
        assert_eq!(random.next_long(), before_random.next_long());
        assert!(region.finish().is_err());
        assert_eq!(terrain.snapshot().block(3, 8, 1), before.block(3, 8, 1));

        let (mut wet_terrain, wet_tags, _, _) = super::super::tests::fixture();
        let world = Arc::make_mut(&mut wet_terrain.world);
        let water_id = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        for position in [[2, 8, 1], [2, 8, 2]] {
            world.set_block_id(position[0], position[1], position[2], water_id)?;
        }
        let writable = wet_terrain.snapshot().chunks.keys().copied().collect();
        let mut wet_region = wet_terrain.decoration_region(&wet_tags, &motion, writable)?;
        assert!(wet_region.place_ocean_ruin_piece(
            &mut piece,
            &ice,
            &water,
            &bubbles,
            &registry,
            Temperature::Warm,
            63,
            clip,
            &mut random,
            &mut no_spawn
        )?);
        for position in [[2, 8, 1], [2, 8, 2]] {
            assert_eq!(
                wet_region
                    .read(position)?
                    .properties
                    .get("waterlogged")
                    .map(String::as_str),
                Some("true")
            );
            assert!(
                wet_region
                    .world
                    .saved_fluid_ticks
                    .get(&ChunkPos { x: 0, z: 0 })
                    .is_some_and(|ticks| ticks
                        .iter()
                        .any(|tick| tick.position == position && tick.fluid == "minecraft:water"))
            );
        }
        wet_region.finish()?;
        Ok(())
    }

    fn java_ruin_region_output(game: &std::path::Path) -> Result<Vec<String>> {
        use std::{path::PathBuf, process::Command};
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("missing libraries")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let source = temp.path().join("OceanRuinRegionOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/OceanRuinRegionOracle.java"),
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
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(cp)
            .arg("OceanRuinRegionOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java placement failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?
            .lines()
            .filter(|line| {
                line.starts_with("B\t") || line.starts_with("T\t") || line.starts_with("F\t")
            })
            .map(str::to_owned)
            .collect())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn region_places_selected_vanilla_ruin_templates() -> Result<()> {
        use super::super::super::{GeneratedEntityRecord, ocean_ruin::prepare_pieces};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let registry = data.vanilla_generated_block_registry()?;
        let ids = data.vanilla_block_ids()?;
        let ice = OreBlockTags::load(
            &mut data,
            &ids,
            &BTreeSet::from([ResourceId::parse("minecraft:ice")?]),
        )?;
        let motion = HeightmapTags::load(&mut data, &ids)?;
        let water = native_resources::fluid_tags::load_water(&mut data)?;
        let bubbles = native_resources::fluid_tags::load_bubble_column_occupiable(&mut data)?;
        let clip = BoundingBox::new([-16, 0, -16], [31, 15, 31]).unwrap();
        let mut total = 0;
        for temperature in [Temperature::Warm, Temperature::Cold] {
            let definition = serde_json::json!({"type":"minecraft:ocean_ruin", "biome_temp": if temperature == Temperature::Warm {"warm"} else {"cold"}, "large_probability":0.0, "cluster_probability":0.0});
            for seed in 0..4 {
                let mut piece = prepare_pieces(&mut data, &definition, seed, [0, 0])?.remove(0);
                piece.selection.position = [4, 90, 4];
                let (mut terrain, tags, _, _) = super::super::tests::fixture();
                let writable = terrain.snapshot().chunks.keys().copied().collect();
                let mut region = terrain.decoration_region(&tags, &motion, writable)?;
                let mut random = DecorationRandom::new(seed);
                let mut no_spawn =
                    |_: [i32; 3], _: &World| -> Result<Option<GeneratedEntityRecord>> { Ok(None) };
                region.place_ocean_ruin_piece(
                    &mut piece,
                    &ice,
                    &water,
                    &bubbles,
                    &registry,
                    temperature,
                    63,
                    clip,
                    &mut random,
                    &mut no_spawn,
                )?;
                total += region
                    .world
                    .chunks
                    .values()
                    .filter(|chunk| {
                        !Arc::ptr_eq(
                            chunk,
                            region.target.world.chunks.get(&chunk.position).unwrap(),
                        )
                    })
                    .count();
                region.finish()?;
            }
        }
        assert!(total > 0, "expected vanilla template blocks to be placed");
        Ok(())
    }
}
