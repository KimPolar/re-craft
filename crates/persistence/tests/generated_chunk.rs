use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};

use native_core::postprocessing::PostProcessing;
use native_persistence::{
    generated_chunk::{SpawnChunkComponents, assemble_spawn_chunk},
    generated_section::encode_generated_sections,
    generated_ticks::{encode_block_ticks, encode_fluid_ticks},
    heightmap::{GeneratedHeightmaps, HeightBounds, encode_generated_heightmaps},
    nbt::{self, Tag, TagType},
    postprocessing,
};
use native_resources::structure_start::HorizontalDirection;
use native_session::worldgen::GeneratedLootChest;
use native_world::loaded::{
    BIOME_COUNT, BLOCK_COUNT, BlockState, Chunk, ChunkPos, LIGHT_BYTES, SavedBlockTick,
    SavedFluidTick, Section, World,
};

fn fixture(min_y: i32) -> SpawnChunkComponents {
    let position = ChunkPos { x: -1, z: 2 };
    let mut world = World::new("minecraft:overworld");
    let stone = world.intern_state(BlockState {
        name: "minecraft:stone".into(),
        properties: BTreeMap::new(),
    });
    let chest = GeneratedLootChest::new(
        [-15, min_y + 4, 34],
        HorizontalDirection::West,
        "minecraft:chests/simple_dungeon",
        -7,
    )
    .unwrap();
    let chest_state = world.intern_state(BlockState {
        name: "minecraft:chest".into(),
        properties: BTreeMap::from([
            ("facing".into(), "west".into()),
            ("type".into(), "single".into()),
            ("waterlogged".into(), "false".into()),
        ]),
    });
    let biome = world.intern_biome("minecraft:plains".into());
    let mut blocks = Box::new([stone; BLOCK_COUNT]);
    blocks[1 + 2 * 16 + 4 * 256] = chest_state;
    world.chunks.insert(
        position,
        Arc::new(Chunk {
            position,
            status: Some("minecraft:spawn".into()),
            block_light_correct: false,
            sections: BTreeMap::from([(
                i8::try_from(min_y / 16).unwrap(),
                Arc::new(Section {
                    blocks: Some(blocks),
                    biomes: Some(Box::new([biome; BIOME_COUNT])),
                    block_light: Some(Box::new([0x34; LIGHT_BYTES])),
                    sky_light: Some(Box::new([0x12; LIGHT_BYTES])),
                }),
            )]),
        }),
    );
    let sections =
        encode_generated_sections(&world, position, min_y, 16, true, |_| true, |_| true).unwrap();
    let heights = [min_y + 6; 256];
    let heightmaps = encode_generated_heightmaps(
        HeightBounds {
            min_y,
            max_y: min_y + 16,
        },
        &GeneratedHeightmaps {
            world_surface: Some(&heights),
            ocean_floor_wg: Some(&heights),
            ..Default::default()
        },
    )
    .unwrap();
    let block_ticks = encode_block_ticks(
        position,
        &[SavedBlockTick {
            block: "minecraft:stone".into(),
            position: [-15, min_y + 4, 34],
            delay: 0,
            priority: -1,
        }],
        |_| true,
    )
    .unwrap();
    let fluid_ticks = encode_fluid_ticks(
        position,
        &[SavedFluidTick {
            fluid: "minecraft:water".into(),
            position: [-14, min_y + 4, 34],
            delay: 0,
            priority: 1,
        }],
    )
    .unwrap();
    let mut marks = PostProcessing::new(min_y, 16).unwrap();
    marks.mark(-15, min_y + 4, 34);
    marks.mark(-15, min_y + 4, 34);
    let entity = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:pig".into())),
        (
            "Pos".into(),
            Tag::List {
                element_type: TagType::Double,
                values: vec![
                    Tag::Double(-14.5),
                    Tag::Double(f64::from(min_y + 4)),
                    Tag::Double(34.5),
                ],
            },
        ),
    ]);
    SpawnChunkComponents {
        position,
        min_y,
        height: 16,
        last_update: 42,
        sections,
        heightmaps,
        block_ticks,
        fluid_ticks,
        postprocessing: postprocessing::encode(&marks),
        block_entities: Tag::List {
            element_type: TagType::Compound,
            values: vec![chest.block_entity()],
        },
        entities: vec![entity],
    }
}

#[test]
fn assembles_spawn_chunk_and_rejects_incomplete_components() {
    let draft = assemble_spawn_chunk(fixture(0)).unwrap();
    assert_eq!(
        draft.value.get("Status"),
        Some(&Tag::String("minecraft:spawn".into()))
    );
    assert!(draft.value.get("isLightOn").is_none());
    assert_eq!(nbt::read(&nbt::write(&draft).unwrap()).unwrap(), draft);
    let mut bad = fixture(0);
    bad.sections = Tag::List {
        element_type: TagType::End,
        values: vec![],
    };
    assert!(assemble_spawn_chunk(bad).is_err());
    let mut bad = fixture(0);
    bad.heightmaps = Tag::Compound(vec![("UNKNOWN".into(), Tag::LongArray(vec![]))]);
    assert!(assemble_spawn_chunk(bad).is_err());
    let mut bad = fixture(0);
    let mut invalid = vec![0; 22];
    invalid[0] = 31;
    bad.heightmaps = Tag::Compound(vec![("WORLD_SURFACE".into(), Tag::LongArray(invalid))]);
    assert!(assemble_spawn_chunk(bad).is_err());
    let mut bad = fixture(0);
    bad.entities.push(Tag::Int(1));
    assert!(assemble_spawn_chunk(bad).is_err());
    let mut bad = fixture(0);
    bad.position = ChunkPos { x: 0, z: 0 };
    assert!(assemble_spawn_chunk(bad).is_err());
    let mut bad = fixture(0);
    bad.postprocessing = Tag::List {
        element_type: TagType::End,
        values: vec![],
    };
    assert!(assemble_spawn_chunk(bad).is_err());
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn assembled_spawn_chunk_roundtrips_java_serializable_chunk_data() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("GeneratedChunkOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedChunkOracle.java")).unwrap();
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
    for min_y in [-64, 0, 64] {
        let input = temp.path().join("chunk.nbt");
        std::fs::write(
            &input,
            nbt::write(&assemble_spawn_chunk(fixture(min_y)).unwrap()).unwrap(),
        )
        .unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GeneratedChunkOracle")
            .arg(input)
            .arg(min_y.to_string()));
    }
}
