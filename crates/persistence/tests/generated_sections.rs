use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};

use native_persistence::{
    generated_section::encode_generated_sections,
    nbt::{self, NamedTag, Tag},
    palette::decode_sections,
};
use native_world::loaded::{
    BIOME_COUNT, BLOCK_COUNT, BlockState, Chunk, ChunkPos, LIGHT_BYTES, Section, World,
};

fn fixture() -> (World, ChunkPos) {
    let mut world = World::new("minecraft:overworld");
    let mut states: Vec<_> = [
        BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        },
        BlockState {
            name: "minecraft:oak_log".into(),
            properties: BTreeMap::from([("axis".into(), "x".into())]),
        },
        BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        },
        BlockState {
            name: "minecraft:oak_log".into(),
            properties: BTreeMap::from([("axis".into(), "z".into())]),
        },
    ]
    .map(|state| world.intern_state(state))
    .into();
    for name in [
        "dirt",
        "cobblestone",
        "granite",
        "diorite",
        "andesite",
        "gravel",
        "sand",
        "sandstone",
        "netherrack",
        "obsidian",
        "bedrock",
        "coal_ore",
        "iron_ore",
        "gold_ore",
        "diamond_ore",
    ] {
        states.push(world.intern_state(BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        }));
    }
    let biomes =
        ["minecraft:plains", "minecraft:desert"].map(|name| world.intern_biome(name.into()));
    let mut blocks = Box::new([states[0]; BLOCK_COUNT]);
    for (i, value) in blocks.iter_mut().enumerate() {
        *value = states[i % states.len()];
    }
    let mut biome_array = Box::new([biomes[0]; BIOME_COUNT]);
    for (i, value) in biome_array.iter_mut().enumerate() {
        *value = biomes[i % 2];
    }
    let pos = ChunkPos { x: -2, z: 3 };
    world.chunks.insert(
        pos,
        Arc::new(Chunk {
            position: pos,
            status: Some("minecraft:spawn".into()),
            block_light_correct: true,
            sections: BTreeMap::from([(
                -1,
                Arc::new(Section {
                    blocks: Some(blocks),
                    biomes: Some(biome_array),
                    block_light: Some(Box::new([0x34; LIGHT_BYTES])),
                    sky_light: Some(Box::new([0x12; LIGHT_BYTES])),
                }),
            )]),
        }),
    );
    (world, pos)
}

fn encode(world: &World, pos: ChunkPos) -> anyhow::Result<NamedTag> {
    let sections = encode_generated_sections(
        world,
        pos,
        -16,
        16,
        true,
        |_| true,
        |name| matches!(name, "minecraft:plains" | "minecraft:desert"),
    )?;
    Ok(NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![("sections".into(), sections)]),
    })
}

#[test]
fn sections_round_trip_and_reject_missing_or_unknown_data() {
    let (mut world, pos) = fixture();
    let document = encode(&world, pos).unwrap();
    let bytes = nbt::write(&document).unwrap();
    let decoded = decode_sections(&nbt::read(&bytes).unwrap().value).unwrap();
    assert_eq!(decoded.len(), 1);
    let section = &decoded[0];
    assert_eq!(section.y, -1);
    let blocks = section.blocks.as_ref().unwrap();
    let biomes = section.biomes.as_ref().unwrap();
    for i in 0..BLOCK_COUNT {
        let expected = world
            .state(world.chunks[&pos].sections[&-1].blocks.as_ref().unwrap()[i])
            .unwrap();
        let decoded = &blocks.states[blocks.indices[i] as usize];
        assert!(decoded.name.matches(&expected.name));
        assert_eq!(
            decoded
                .properties
                .iter()
                .map(|(k, v)| (k.to_string_lossy(), v.to_string_lossy()))
                .collect::<BTreeMap<_, _>>(),
            expected.properties
        );
    }
    for i in 0..BIOME_COUNT {
        assert!(
            biomes.names[biomes.indices[i] as usize].matches(if i % 2 == 0 {
                "minecraft:plains"
            } else {
                "minecraft:desert"
            })
        );
    }
    assert_eq!(section.block_light.as_ref().unwrap(), &[0x34; LIGHT_BYTES]);
    assert_eq!(section.sky_light.as_ref().unwrap(), &[0x12; LIGHT_BYTES]);
    let without_sky =
        encode_generated_sections(&world, pos, -16, 16, false, |_| true, |_| true).unwrap();
    let without_sky = Tag::Compound(vec![("sections".into(), without_sky)]);
    assert!(
        decode_sections(&without_sky).unwrap()[0]
            .sky_light
            .is_none()
    );
    assert!(encode_generated_sections(&world, pos, -16, 16, true, |_| false, |_| true).is_err());
    assert!(encode_generated_sections(&world, pos, -16, 16, true, |_| true, |_| false).is_err());
    assert!(encode_generated_sections(&world, pos, -16, 32, true, |_| true, |_| true).is_err());
    let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
    Arc::make_mut(chunk.sections.get_mut(&-1).unwrap()).biomes = None;
    assert!(encode(&world, pos).is_err());
}

#[test]
fn singleton_palettes_and_unknown_registry_ids_are_handled() {
    use native_world::loaded::{BiomeId, BlockStateId};
    let (mut world, pos) = fixture();
    let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
    let section = Arc::make_mut(chunk.sections.get_mut(&-1).unwrap());
    section.blocks.as_mut().unwrap().fill(BlockStateId(0));
    section.biomes.as_mut().unwrap().fill(BiomeId(0));
    let document = encode(&world, pos).unwrap();
    let Tag::List { values, .. } = document.value.get("sections").unwrap() else {
        panic!("missing sections")
    };
    assert!(values[0].get("block_states").unwrap().get("data").is_none());
    assert!(values[0].get("biomes").unwrap().get("data").is_none());
    let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
    Arc::make_mut(chunk.sections.get_mut(&-1).unwrap())
        .blocks
        .as_mut()
        .unwrap()[0] = BlockStateId(u32::MAX);
    assert!(encode(&world, pos).is_err());
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_section_palettes_decode_in_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("GeneratedSectionOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedSectionOracle.java")).unwrap();
    let run = |cmd: &mut Command| {
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let (world, pos) = fixture();
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_generated_block_registry().unwrap();
    let sections = encode_generated_sections(
        &world,
        pos,
        -16,
        16,
        true,
        |state| registry.is_canonical_state(&state.name, &state.properties),
        |name| matches!(name, "minecraft:plains" | "minecraft:desert"),
    )
    .unwrap();
    let root = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![("sections".into(), sections)]),
    };
    let input = temp.path().join("sections.nbt");
    std::fs::write(&input, nbt::write(&root).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("GeneratedSectionOracle")
        .arg(input));
}
