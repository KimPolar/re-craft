//! Access the current decoration region for ocean-ruin floor adjustment.
use super::*;
use native_resources::ocean_ruin::{DataMarkerKind, FloorWorld, MarkerWorld, handle_data_marker};

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
        ensure!(!self.failed, "generation attempt previously failed");
        let kind = if marker.metadata.matches("chest") {
            DataMarkerKind::Chest
        } else if marker.metadata.matches("drowned") {
            DataMarkerKind::Drowned
        } else {
            DataMarkerKind::Other
        };
        let mut next_random = random.clone();
        let mut next_spawn = spawn.clone();
        self.failed = true;
        handle_data_marker(
            &mut RuinMarkerRegion {
                region: self,
                registry,
                water_fluids,
                sea_level,
                spawn: &mut next_spawn,
            },
            &mut next_random,
            kind,
            marker.position,
            large,
        )?;
        *random = next_random;
        *spawn = next_spawn;
        self.failed = false;
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

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::beardifier::BoundingBox;
    use native_persistence::{
        nbt::{NamedTag, Tag},
        structure_template::StructureTemplate,
    };
    use native_resources::{
        ocean_ruin::{Piece, Rotation},
        packs::ResourceProvider,
    };

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
}
