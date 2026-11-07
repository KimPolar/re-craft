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

fn jigsaw_static_full(state: &BlockState) -> Result<bool> {
    if native_world::wall::Wall::from_state(state).is_some() {
        return Ok(false);
    }
    if jigsaw_wood(state) {
        return Ok(true);
    }
    if jigsaw_torch(state) {
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
            }
            .with_loot(&table, random.next_long())?;
            Arc::make_mut(&mut self.region.monster_entities).insert(
                pos,
                Some(super::super::GeneratedMonsterRoomEntity::Chest(chest)),
            );
            Arc::make_mut(self.region.entity_order.entry(chunk(pos)).or_default()).insert(pos);
        } else {
            self.region.jigsaw_block_write = jigsaw_wood(state)
                || native_world::collision::slab_stair_solid(state).is_some()
                || native_world::wall::Wall::from_state(state).is_some()
                || native_world::collision::fence_waterlogged(state).is_some()
                || native_world::collision::fence_gate_open(state).is_some()
                || native_world::collision::door_open(state).is_some()
                || native_world::collision::bed_leg_end(state).is_some()
                || native_world::collision::full_glass(state)
                || native_world::collision::full_terracotta(state)
                || native_world::collision::pane_waterlogged(state).is_some()
                || jigsaw_torch(state);
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
                self.region.jigsaw_block_write = native_world::collision::slab_stair_solid(state)
                    .is_some()
                    || native_world::wall::Wall::from_state(state).is_some()
                    || native_world::collision::fence_waterlogged(state).is_some()
                    || native_world::collision::pane_waterlogged(state).is_some();
                let result = self.region.set_proto_block(pos, &wet);
                self.region.jigsaw_block_write = false;
                result?;
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
        assert!(output.lines().any(|line| line == "PROTO_WATER_TICK\t0"));
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
