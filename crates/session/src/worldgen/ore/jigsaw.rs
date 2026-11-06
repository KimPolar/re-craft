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

struct JigsawRegion<'r, 'a, 'g> {
    region: &'r mut DecorationRegion<'a>,
    registry: &'g GeneratedBlockRegistry,
}

fn jigsaw_static_full(state: &BlockState) -> Result<bool> {
    if state.name == "minecraft:chest" {
        monster_block_flags(state)?;
        return Ok(false);
    }
    if matches!(
        state.name.as_str(),
        "minecraft:cobbled_deepslate_slab"
            | "minecraft:polished_deepslate_slab"
            | "minecraft:deepslate_brick_slab"
            | "minecraft:deepslate_tile_slab"
    ) {
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
                            || key.matches("LootTableSeed"))
                            && keys.insert(key.0.clone())
                    })
                    && matches!(nbt.get("id"), Some(Tag::String(id)) if id.matches("minecraft:chest"))
                    && matches!(nbt.get("LootTableSeed"), None | Some(Tag::Long(_))),
                "jigsaw block entity placement is not yet supported"
            );
            let Some(Tag::String(table)) = nbt.get("LootTable") else {
                bail!("jigsaw chest loot table is missing")
            };
            let table = String::from_utf16(&table.0).context("invalid chest loot table")?;
            ResourceId::parse(&table)?;
            Some(table)
        } else {
            None
        };
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties)
                && self.registry.has_block_entity(&state.name) == Some(chest_table.is_some())
                && jigsaw_static_full(state).is_ok(),
            "unsupported jigsaw block effects"
        );
        let old = self.region.read(pos)?;
        ensure!(
            self.registry.is_canonical_state(&old.name, &old.properties)
                && self.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some() || jigsaw_static_full(&old).is_ok())
                && !self.region.monster_entities.contains_key(&pos)
                && !self.region.bee_nests.contains_key(&pos),
            "unsupported jigsaw replacement source"
        );
        if let Some(table) = chest_table {
            self.region.set_proto_block(
                pos,
                &BlockState {
                    name: "minecraft:barrier".into(),
                    properties: BTreeMap::new(),
                },
            )?;
            self.region.set_proto_block(pos, state)?;
            let chest = super::super::GeneratedEmptyChest {
                position: pos,
                state: state.clone(),
            }
            .with_loot(&table, random.next_long())?;
            Arc::make_mut(&mut self.region.monster_entities).insert(
                pos,
                Some(super::super::GeneratedMonsterRoomEntity::Chest(chest)),
            );
            Arc::make_mut(self.region.entity_order.entry(chunk(pos)).or_default()).insert(pos);
        } else {
            self.region.set_proto_block(pos, state)?;
        }
        Ok(true)
    }
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name.starts_with("minecraft:")
            && (state.name.ends_with("_stairs")
                || state.name.ends_with("_slab")
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
            if self.region.writable.contains(&chunk(pos)) && !self.region.is_outside(pos[1]) {
                let chest = if state.name == "minecraft:chest" {
                    let Some(Some(super::super::GeneratedMonsterRoomEntity::Chest(chest))) =
                        Arc::make_mut(&mut self.region.monster_entities).remove(&pos)
                    else {
                        bail!("jigsaw chest entity is missing")
                    };
                    Some(chest)
                } else {
                    None
                };
                self.region.set_proto_block(pos, &wet)?;
                if let Some(mut chest) = chest {
                    chest.update_state(wet)?;
                    Arc::make_mut(&mut self.region.monster_entities).insert(
                        pos,
                        Some(super::super::GeneratedMonsterRoomEntity::Chest(chest)),
                    );
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
                Some(Some(super::super::GeneratedMonsterRoomEntity::Chest(_)))
            ),
            "jigsaw block entity marking is not yet supported"
        );
        Ok(())
    }
    fn place_template_entities(
        &mut self,
        _: &[Tag],
        _: [i32; 3],
        _: native_resources::structure_transform::Rotation,
        _: BoundingBox,
    ) -> Result<()> {
        bail!("jigsaw template entity placement is not yet supported")
    }
}

impl DecorationRegion<'_> {
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
                        element_type: TagType::End,
                        values: vec![],
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
            let properties = BTreeMap::from([
                ("type".into(), (*kind).to_owned()),
                ("waterlogged".into(), (*wet).to_owned()),
            ]);
            let state = BlockState {
                name: name.clone(),
                properties: properties.clone(),
            };
            assert!(registry.is_canonical_state(&name, &properties));
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
        assert_eq!(slabs_checked, 288);

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
