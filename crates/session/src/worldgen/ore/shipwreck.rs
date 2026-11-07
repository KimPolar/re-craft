//! Apply a prepared shipwreck to an unpublished decoration region.
use super::*;
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    shipwreck::{HeightWorld, MarkerWorld},
    structure_start::Heightmap,
    structure_transform::Rotation,
    tree_leaf_update::ShapeEdgeWorld,
};
use native_world::fluid::FluidState;

use super::super::{
    jigsaw::{JigsawPlacementWorld, StructurePlacementWorld},
    shipwreck::{PreparedStart, ShipwreckPlacementWorld},
};

struct ShipwreckRegion<'r, 'a, 'g> {
    template: super::ruined_portal::PortalTemplateRegion<'r, 'a, 'g>,
}

impl JigsawPlacementWorld for ShipwreckRegion<'_, '_, '_> {
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        JigsawPlacementWorld::block(&mut self.template, pos)
    }
    fn world_surface_height(&mut self, x: i32, z: i32) -> Result<i32> {
        self.template.inner.region.world_surface_height(x, z)
    }
    fn fluid_at(&mut self, pos: [i32; 3]) -> Result<Option<FluidState>> {
        self.template.fluid_at(pos)
    }
    fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>> {
        self.template.fluid_of_state(state)
    }
    fn place_template_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        self.template.place_template_block(pos, state, nbt, random)
    }
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
        self.template.is_liquid_container(state)
    }
    fn place_liquid(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        fluid: Option<FluidState>,
    ) -> Result<()> {
        self.template.place_liquid(pos, state, fluid)
    }
    fn mark_block_entity_changed(&mut self, pos: [i32; 3]) -> Result<()> {
        self.template.mark_block_entity_changed(pos)
    }
    fn place_template_entities(
        &mut self,
        _: &[Tag],
        _: [i32; 3],
        _: Rotation,
        _: BoundingBox,
    ) -> Result<()> {
        bail!("shipwreck template entities are not yet supported")
    }
}

impl HeightWorld for ShipwreckRegion<'_, '_, '_> {
    fn max_y(&self) -> i32 {
        self.template.inner.region.target.min_y + self.template.inner.region.target.height - 1
    }
    fn height(&mut self, map: Heightmap, x: i32, z: i32) -> Result<i32> {
        match map {
            Heightmap::WorldSurface => self.template.inner.region.world_surface_height(x, z),
            Heightmap::OceanFloor => {
                OreVeinWorld::ocean_floor_height(self.template.inner.region, x, z)
            }
        }
    }
}

impl MarkerWorld for ShipwreckRegion<'_, '_, '_> {
    fn is_randomizable_container(&mut self, pos: [i32; 3]) -> Result<bool> {
        let state = JigsawPlacementWorld::block(self, pos)?;
        if state.name == "minecraft:chest" {
            return self
                .template
                .inner
                .region
                .resolve_generated_chest_inner(pos);
        }
        ensure!(
            self.template.inner.registry.has_block_entity(&state.name) == Some(false),
            "unsupported shipwreck marker block entity"
        );
        Ok(false)
    }
    fn set_loot_table(&mut self, pos: [i32; 3], table: &str, seed: i64) -> Result<()> {
        let region = &mut self.template.inner.region;
        let chest = match region.monster_entities.get(&pos) {
            Some(Some(super::super::GeneratedMonsterRoomEntity::EmptyChest(chest))) => {
                chest.with_loot(table, seed)?
            }
            Some(Some(super::super::GeneratedMonsterRoomEntity::Chest(chest))) => {
                chest.with_loot_table(table, seed)?
            }
            _ => bail!("shipwreck chest entity is unavailable"),
        };
        Arc::make_mut(&mut region.monster_entities).insert(
            pos,
            Some(super::super::GeneratedMonsterRoomEntity::Chest(chest)),
        );
        Ok(())
    }
}

impl ShapeEdgeWorld for ShipwreckRegion<'_, '_, '_> {
    type State = BlockState;
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        JigsawPlacementWorld::block(self, pos)
    }
    fn update_shape(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        direction: [i32; 3],
        neighbor_pos: [i32; 3],
        neighbor: &BlockState,
    ) -> Result<BlockState> {
        if super::jigsaw::jigsaw_wood(state) {
            return Ok(state.clone());
        }
        if let Some(waterlogged) = native_world::collision::fence_waterlogged(state) {
            ensure!(
                self.template
                    .inner
                    .registry
                    .is_canonical_state(&neighbor.name, &neighbor.properties),
                "invalid shipwreck fence neighbor"
            );
            if waterlogged {
                SpringWorld::schedule_spring_tick(
                    self.template.inner.region,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            let side = match direction {
                [-1, 0, 0] => Some("west"),
                [1, 0, 0] => Some("east"),
                [0, 0, -1] => Some("north"),
                [0, 0, 1] => Some("south"),
                [0, -1, 0] | [0, 1, 0] => None,
                _ => bail!("invalid shipwreck fence direction"),
            };
            let mut updated = state.clone();
            if let Some(side) = side {
                let connected = super::super::postprocessing_world::fence_connection(
                    &self.template.wall_tags.fence_tags,
                    state,
                    neighbor,
                    direction,
                )?;
                updated
                    .properties
                    .insert(side.into(), connected.to_string());
            }
            return Ok(updated);
        }
        if native_world::collision::door_open(state).is_some() {
            ensure!(
                self.template
                    .inner
                    .registry
                    .is_canonical_state(&neighbor.name, &neighbor.properties),
                "unsupported shipwreck door neighbor"
            );
            let below_sturdy = if state.properties["half"] == "lower" && direction == [0, -1, 0] {
                let below =
                    JigsawPlacementWorld::block(self, [pos[0], pos[1].wrapping_sub(1), pos[2]])?;
                ensure!(
                    self.template
                        .inner
                        .registry
                        .is_canonical_state(&below.name, &below.properties),
                    "unsupported shipwreck door support"
                );
                Some(
                    native_world::collision::button_support_face(&below, glam::IVec3::Y)
                        .context("unsupported shipwreck door support shape")?,
                )
            } else {
                None
            };
            return native_world::door::update_shape(
                state,
                glam::IVec3::from_array(direction),
                neighbor,
                below_sturdy,
            )
            .context("unsupported shipwreck door update");
        }
        if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            return ShapeEdgeWorld::update_shape(
                self.template.inner.region,
                pos,
                state,
                direction,
                neighbor_pos,
                neighbor,
            );
        }
        ShapeEdgeWorld::update_shape(
            &mut self.template,
            pos,
            state,
            direction,
            neighbor_pos,
            neighbor,
        )
    }
    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        let old = JigsawPlacementWorld::block(self, pos)?;
        if super::jigsaw::jigsaw_wood(state)
            || native_world::collision::door_open(state).is_some()
            || native_world::collision::fence_waterlogged(state).is_some()
            || (is_air(state) && native_world::collision::door_open(&old).is_some())
        {
            ensure!(matches!(flags, 2 | 18), "unsupported shipwreck shape flags");
            self.template.inner.place_template_block(
                pos,
                state,
                None,
                &mut DecorationRandom::new(0),
            )?;
            return Ok(());
        }
        ShapeEdgeWorld::set_block(&mut self.template, pos, state, flags)
    }
}

impl StructurePlacementWorld for ShipwreckRegion<'_, '_, '_> {
    fn place_structure_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        if super::jigsaw::jigsaw_wood(state)
            || native_world::collision::door_open(state).is_some()
            || native_world::collision::fence_waterlogged(state).is_some()
        {
            ensure!(nbt.is_none(), "wood block entity is unsupported");
            return self
                .template
                .inner
                .place_template_block(pos, state, None, random);
        }
        self.template.place_structure_block(pos, state, nbt, random)
    }
    fn update_from_neighbor_shapes(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
    ) -> Result<BlockState> {
        super::super::postprocessing::update_from_neighbor_shapes(self, pos, state)
    }
    fn update_neighbors_at(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        self.template.update_neighbors_at(pos, state)
    }
}
impl ShipwreckPlacementWorld for ShipwreckRegion<'_, '_, '_> {}

impl DecorationRegion<'_> {
    /// Place one start in the caller's unpublished region. A failed attempt
    /// poisons the region and cannot be published by `finish`.
    pub(super) fn place_shipwreck_start(
        &mut self,
        start: &mut PreparedStart,
        registry: &GeneratedBlockRegistry,
        update_tags: &super::ruined_portal::PortalUpdateTags,
        clip: BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        let mut world = ShipwreckRegion {
            template: super::ruined_portal::PortalTemplateRegion {
                inner: super::jigsaw::JigsawRegion {
                    region: self,
                    registry,
                },
                wall_tags: update_tags,
                temporary_jigsaws: BTreeSet::new(),
            },
        };
        let result = start.post_process(
            &mut world,
            registry,
            clip,
            super::jigsaw::jigsaw_static_full,
            random,
        )?;
        self.failed = false;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::beardifier::TerrainAdjustment;
    use native_persistence::{
        nbt::NamedTag,
        structure_template::{StructureTemplate, TemplateBlock},
    };
    use native_resources::{packs::ResourceStack, shipwreck::Piece, structure_transform::Rotation};

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn places_shipwreck_blocks_in_generation_draft() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let update_tags =
            super::super::ruined_portal::PortalUpdateTags::load(&mut resources, &ids)?;
        let palette = |name: &str| Tag::Compound(vec![("id".into(), Tag::String(name.into()))]);
        let chest = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:chest".into())),
            (
                "Items".into(),
                Tag::List {
                    element_type: native_persistence::nbt::TagType::End,
                    values: vec![],
                },
            ),
        ]);
        let marker = Tag::Compound(vec![
            ("mode".into(), Tag::String("DATA".into())),
            ("metadata".into(), Tag::String("map_chest".into())),
        ]);
        let template = Arc::new(StructureTemplate {
            document: NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![]),
            },
            data_version: Some(5023),
            size: [6, 2, 1],
            palettes: vec![vec![
                palette("minecraft:oak_planks"),
                palette("minecraft:chest"),
                palette("minecraft:dark_oak_trapdoor"),
                palette("minecraft:structure_block"),
                palette("minecraft:oak_door"),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:oak_door".into())),
                    (
                        "properties".into(),
                        Tag::Compound(vec![("half".into(), Tag::String("upper".into()))]),
                    ),
                ]),
                palette("minecraft:oak_fence"),
            ]],
            blocks: vec![
                TemplateBlock {
                    position: [0, 0, 0],
                    state: 0,
                    nbt: None,
                },
                TemplateBlock {
                    position: [1, 0, 0],
                    state: 1,
                    nbt: Some(chest),
                },
                TemplateBlock {
                    position: [2, 0, 0],
                    state: 2,
                    nbt: None,
                },
                TemplateBlock {
                    position: [1, 1, 0],
                    state: 3,
                    nbt: Some(marker),
                },
                TemplateBlock {
                    position: [3, 0, 0],
                    state: 4,
                    nbt: None,
                },
                TemplateBlock {
                    position: [3, 1, 0],
                    state: 5,
                    nbt: None,
                },
                TemplateBlock {
                    position: [4, 0, 0],
                    state: 0,
                    nbt: None,
                },
                TemplateBlock {
                    position: [5, 0, 0],
                    state: 6,
                    nbt: None,
                },
            ],
            entities: vec![],
        });
        let piece = Piece {
            template: "minecraft:shipwreck/with_mast".into(),
            position: [4, 8, 4],
            rotation: Rotation::None,
            beached: false,
        };
        let bounds = piece.bounds(template.size)?;
        let mut start = PreparedStart {
            chunk: [0, 0],
            generation_point: [8, 8, 8],
            terrain_adjustment: TerrainAdjustment::None,
            piece,
            template,
            bounds,
            height_adjusted: true,
        };
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        expected.next_long();
        let loot_seed = expected.next_long();
        assert!(region.place_shipwreck_start(
            &mut start,
            &registry,
            &update_tags,
            clip,
            &mut random
        )?);
        assert_eq!(random.next_long(), expected.next_long());
        region.finish()?;
        let snapshot = terrain.snapshot();
        assert_eq!(
            snapshot.block(4, 8, 4).unwrap().name,
            "minecraft:oak_planks"
        );
        assert_eq!(snapshot.block(5, 8, 4).unwrap().name, "minecraft:chest");
        assert_eq!(
            snapshot.block(6, 8, 4).unwrap().name,
            "minecraft:dark_oak_trapdoor"
        );
        assert_eq!(snapshot.block(7, 8, 4).unwrap().properties["half"], "lower");
        assert_eq!(snapshot.block(7, 9, 4).unwrap().properties["half"], "upper");
        let fence = snapshot.block(9, 8, 4).unwrap();
        assert_eq!(fence.name, "minecraft:oak_fence");
        assert_eq!(fence.properties["east"], "false");
        assert_eq!(fence.properties["west"], "true");
        let Tag::List { values, .. } =
            terrain.generated_block_entity_data(ChunkPos { x: 0, z: 0 }, &registry)?
        else {
            bail!("missing entities")
        };
        assert_eq!(values.len(), 1);
        assert!(
            matches!(values[0].get("LootTable"),Some(Tag::String(id)) if id.matches("minecraft:chests/shipwreck_map"))
        );
        assert_eq!(values[0].get("LootTableSeed"), Some(&Tag::Long(loot_seed)));

        let (mut wet_terrain, wet_tags, _, _) = super::super::tests::fixture();
        let writable = wet_terrain.snapshot().chunks.keys().copied().collect();
        let mut region = wet_terrain.decoration_region(&wet_tags, &motion, writable)?;
        let (name, properties) = registry.resolve(&serde_json::json!("minecraft:oak_fence"))?;
        let mut wet = BlockState { name, properties };
        wet.properties.insert("waterlogged".into(), "true".into());
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        };
        let mut world = ShipwreckRegion {
            template: super::super::ruined_portal::PortalTemplateRegion {
                inner: super::super::jigsaw::JigsawRegion {
                    region: &mut region,
                    registry: &registry,
                },
                wall_tags: &update_tags,
                temporary_jigsaws: BTreeSet::new(),
            },
        };
        let updated = ShapeEdgeWorld::update_shape(
            &mut world,
            [8, 8, 4],
            &wet,
            [1, 0, 0],
            [9, 8, 4],
            &stone,
        )?;
        assert_eq!(updated.properties["east"], "true");
        assert_eq!(updated.properties["waterlogged"], "true");
        region.finish()?;
        let wet_snapshot = wet_terrain.snapshot();
        let ticks = wet_snapshot
            .saved_fluid_ticks
            .get(&ChunkPos { x: 0, z: 0 })
            .context("waterlogged fence tick missing")?;
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].position, [8, 8, 4]);
        assert_eq!(ticks[0].fluid, "minecraft:water");

        let template = Arc::make_mut(&mut start.template);
        template.palettes[0].push(palette("minecraft:lever"));
        template.blocks[0].state = 7;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        let original = start.piece.clone();
        assert!(
            region
                .place_shipwreck_start(&mut start, &registry, &update_tags, clip, &mut random)
                .is_err()
        );
        assert_eq!(start.piece, original);
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }
}
