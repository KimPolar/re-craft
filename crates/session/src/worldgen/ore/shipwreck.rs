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
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn installed_shipwreck_drafts_match_java() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let update_tags =
            super::super::ruined_portal::PortalUpdateTags::load(&mut resources, &ids)?;
        for id in &ids {
            for (tag, expected) in [
                ("minecraft:fences", id.ends_with("_fence")),
                (
                    "minecraft:wooden_fences",
                    id.ends_with("_fence") && id != "minecraft:nether_brick_fence",
                ),
                ("minecraft:shulker_boxes", id.ends_with("shulker_box")),
            ] {
                ensure!(
                    update_tags
                        .fence_tags
                        .contains(&ResourceId::parse(tag)?, id)?
                        == expected,
                    "Java oracle tag setup differs for {tag}: {id}"
                );
            }
        }
        let names = [
            "with_mast",
            "upsidedown_full",
            "upsidedown_fronthalf",
            "upsidedown_backhalf",
            "sideways_full",
            "sideways_fronthalf",
            "sideways_backhalf",
            "rightsideup_full",
            "rightsideup_fronthalf",
            "rightsideup_backhalf",
            "with_mast_degraded",
            "upsidedown_full_degraded",
            "upsidedown_fronthalf_degraded",
            "upsidedown_backhalf_degraded",
            "sideways_full_degraded",
            "sideways_fronthalf_degraded",
            "sideways_backhalf_degraded",
            "rightsideup_full_degraded",
            "rightsideup_fronthalf_degraded",
            "rightsideup_backhalf_degraded",
        ];
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
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
        let source = temp.path().join("ShipwreckDraftOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/ShipwreckDraftOracle.java"),
        )?;
        let compiled = std::process::Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(cp)
            .arg("ShipwreckDraftOracle")
            .args(names)
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout)?;
        let mut oracle = BTreeMap::<(String, usize), BTreeMap<[i32; 3], String>>::new();
        let mut current = None;
        for line in text.lines() {
            let row: Vec<_> = line.split('\t').collect();
            match row.as_slice() {
                ["C", name, rotation] => {
                    let key = ((*name).to_owned(), rotation.parse()?);
                    ensure!(
                        oracle.insert(key.clone(), BTreeMap::new()).is_none(),
                        "duplicate Java case"
                    );
                    current = Some(key);
                }
                ["B", x, y, z, state] => {
                    let case = current.as_ref().context("Java block before case")?;
                    let position = [x.parse()?, y.parse()?, z.parse()?];
                    ensure!(
                        oracle
                            .get_mut(case)
                            .unwrap()
                            .insert(position, (*state).to_owned())
                            .is_none(),
                        "duplicate Java block"
                    );
                }
                _ => {}
            }
        }
        ensure!(oracle.len() == names.len() * 4, "missing Java cases");
        for name in names {
            let id = ResourceId::parse(&format!("minecraft:shipwreck/{name}"))?;
            let template = Arc::new(
                StructureTemplate::load(&mut resources, &id)?.context("missing template")?,
            );
            for (rotation_index, rotation) in [
                Rotation::None,
                Rotation::Clockwise90,
                Rotation::Clockwise180,
                Rotation::Counterclockwise90,
            ]
            .into_iter()
            .enumerate()
            {
                let piece = Piece {
                    template: format!("minecraft:shipwreck/{name}"),
                    position: [0, 5, 0],
                    rotation,
                    beached: false,
                };
                let bounds = piece.bounds(template.size)?;
                let mut start = PreparedStart {
                    chunk: [0, 0],
                    generation_point: [8, 8, 8],
                    terrain_adjustment: TerrainAdjustment::None,
                    piece,
                    template: Arc::clone(&template),
                    bounds,
                    height_adjusted: true,
                };
                let (mut terrain, tags, _, _) = super::super::tests::fixture();
                let writable = terrain.snapshot().chunks.keys().copied().collect();
                let mut region = terrain.decoration_region(&tags, &motion, writable)?;
                let clip = BoundingBox::new([-16, 0, -16], [31, 15, 31]).unwrap();
                region
                    .place_shipwreck_start(
                        &mut start,
                        &registry,
                        &update_tags,
                        clip,
                        &mut DecorationRandom::new(42),
                    )
                    .with_context(|| format!("placing {name} {rotation:?}"))?;
                region.finish()?;
                let snapshot = terrain.snapshot();
                let mut placed = BTreeMap::new();
                for y in 0..16 {
                    for z in -16..32 {
                        for x in -16..32 {
                            let state = snapshot.block(x, y, z).context("fixture block missing")?;
                            let original = if y < 8 {
                                "minecraft:stone"
                            } else {
                                "minecraft:air"
                            };
                            if state.name != original {
                                let properties = state
                                    .properties
                                    .iter()
                                    .map(|(key, value)| format!("{key}={value}"))
                                    .collect::<Vec<_>>()
                                    .join(",");
                                placed.insert([x, y, z], format!("{}|{properties}", state.name));
                            }
                        }
                    }
                }
                ensure!(
                    !placed.is_empty(),
                    "template {name} {rotation:?} placed no blocks"
                );
                let expected = oracle
                    .remove(&(name.to_owned(), rotation_index))
                    .context("missing Java case")?;
                for pos in placed.keys().chain(expected.keys()) {
                    ensure!(
                        placed.get(pos) == expected.get(pos),
                        "{name} {rotation:?} at {pos:?}: Rust {:?}, Java {:?}",
                        placed.get(pos),
                        expected.get(pos)
                    );
                }
            }
        }
        Ok(())
    }

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
