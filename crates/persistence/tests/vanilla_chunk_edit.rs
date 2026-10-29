//! Compare in-memory chunk palette drafts with the 26.3 PalettedContainer codec.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_persistence::{
    edit::{BlockChange, draft_chunk_blocks},
    nbt::{self, NamedTag, Tag, TagType},
    palette::{self, BlockState},
    world::ChunkPos,
};
use serde_json::Value;

const ORACLE: &str = r#"
import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.chunk.*;

public class ChunkEditOracle {
  public static void main(String[] args) throws Exception {
    PrintStream output = System.out;
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    var names = Files.readAllLines(Path.of(args[0]));
    var codec = PalettedContainer.codecRW(BlockState.CODEC, Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState());
    var sample = new PalettedContainer<>(Blocks.STONE.defaultBlockState(), Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
    sample.set(1, 0, 0, Blocks.OAK_LOG.defaultBlockState().setValue(BlockStateProperties.AXIS, Direction.Axis.X));
    sample.set(2, 0, 0, Blocks.GRASS_BLOCK.defaultBlockState());
    try (var out = new DataOutputStream(Files.newOutputStream(Path.of(args[0]).resolveSibling("java-sample.nbt")))) {
      NbtIo.write((CompoundTag)codec.encodeStart(NbtOps.INSTANCE, sample).getOrThrow(), (DataOutput)out);
    }
    for (int test = 1; test < args.length; test += 3) {
      int count = Integer.parseInt(args[test + 2]);
      CompoundTag storage;
      try (var in = new DataInputStream(Files.newInputStream(Path.of(args[test])))) { storage = NbtIo.read((DataInput)in); }
      var container = codec.parse(NbtOps.INSTANCE, storage).getOrThrow();
      for (int i = 0; i < 4096; i++) {
        int x = i % 16, z = (i / 16) % 16, y = i / 256;
        var actual = container.get(x, y, z);
        String name = BuiltInRegistries.BLOCK.getKey(actual.getBlock()).toString();
        String expected = i < count ? names.get(i) : "minecraft:stone";
        if (!name.equals(expected)) throw new AssertionError("block " + i + ": " + name + " != " + expected);
        if (expected.equals("minecraft:oak_log") && actual.getValue(BlockStateProperties.AXIS) != Direction.Axis.X) throw new AssertionError("log axis");
        if (expected.equals("minecraft:oak_stairs") && (actual.getValue(BlockStateProperties.HORIZONTAL_FACING) != Direction.EAST
            || actual.getValue(BlockStateProperties.HALF) != Half.TOP
            || actual.getValue(BlockStateProperties.STAIRS_SHAPE) != StairsShape.INNER_LEFT
            || actual.getValue(BlockStateProperties.WATERLOGGED))) throw new AssertionError("stairs properties");
      }
      var encoded = (CompoundTag)codec.encodeStart(NbtOps.INSTANCE, container).getOrThrow();
      try (var out = new DataOutputStream(Files.newOutputStream(Path.of(args[test + 1])))) { NbtIo.write(encoded, (DataOutput)out); }
      output.println("OK " + count);
    }
  }
}
"#;

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to Java 25 JDK"))
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "Java palette oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    }
}

fn fixture() -> NamedTag {
    NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            ("xPos".into(), Tag::Int(0)),
            ("zPos".into(), Tag::Int(0)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            (
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(0)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String("minecraft:stone".into())],
                                },
                            )]),
                        ),
                    ])],
                },
            ),
            ("unknown_chunk".into(), Tag::String("preserved".into())),
        ]),
    }
}

fn storage(chunk: &NamedTag) -> &Tag {
    let Tag::List { values, .. } = chunk.value.get("sections").unwrap() else {
        unreachable!()
    };
    values[0].get("block_states").unwrap()
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn drafted_palettes_round_trip_through_vanilla_container() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let temp = tempfile::tempdir().unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    let source = temp.path().join("ChunkEditOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let names = [
        "white_wool",
        "orange_wool",
        "magenta_wool",
        "light_blue_wool",
        "yellow_wool",
        "lime_wool",
        "pink_wool",
        "gray_wool",
        "light_gray_wool",
        "cyan_wool",
        "purple_wool",
        "blue_wool",
        "brown_wool",
        "green_wool",
        "red_wool",
        "black_wool",
        "white_concrete",
        "orange_concrete",
        "magenta_concrete",
        "light_blue_concrete",
        "yellow_concrete",
        "lime_concrete",
        "pink_concrete",
        "gray_concrete",
        "granite",
        "diorite",
        "andesite",
        "dirt",
        "cobblestone",
        "deepslate",
        "oak_log",
        "oak_stairs",
    ];
    let states: Vec<_> = names
        .iter()
        .map(|name| {
            let name = format!("minecraft:{name}");
            if name == "minecraft:oak_log" {
                state(&name, &[("axis", "x")])
            } else if name == "minecraft:oak_stairs" {
                state(
                    &name,
                    &[
                        ("facing", "east"),
                        ("half", "top"),
                        ("shape", "inner_left"),
                        ("waterlogged", "false"),
                    ],
                )
            } else {
                state(&name, &[])
            }
        })
        .collect();
    let expected_names = temp.path().join("names.txt");
    std::fs::write(
        &expected_names,
        states
            .iter()
            .map(|state| state.name.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let mut command = Command::new(java.join("java.exe"));
    command
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("ChunkEditOracle")
        .arg(&expected_names);
    for count in [0usize, 1, 16, 32] {
        let changes: Vec<_> = states
            .iter()
            .take(count)
            .enumerate()
            .map(|(i, replacement)| BlockChange {
                x: (i % 16) as i32,
                y: 0,
                z: (i / 16) as i32,
                expected: state("minecraft:stone", &[]),
                replacement: replacement.clone(),
            })
            .collect();
        let original = fixture();
        let draft = draft_chunk_blocks(&original, ChunkPos { x: 0, z: 0 }, &changes).unwrap();
        assert_eq!(draft.changed_blocks, count);
        assert_eq!(
            draft.document().value.get("unknown_chunk"),
            original.value.get("unknown_chunk")
        );
        let input = temp.path().join(format!("input-{count}.nbt"));
        let output = temp.path().join(format!("output-{count}.nbt"));
        std::fs::write(
            &input,
            nbt::write(&NamedTag {
                name: "".into(),
                value: storage(draft.document()).clone(),
            })
            .unwrap(),
        )
        .unwrap();
        command.arg(input).arg(output).arg(count.to_string());
    }
    let output = run(&mut command);
    let sample = nbt::read(&std::fs::read(temp.path().join("java-sample.nbt")).unwrap()).unwrap();
    let section = Tag::Compound(vec![
        ("Y".into(), Tag::Byte(0)),
        ("block_states".into(), sample.value),
    ]);
    let chunk = Tag::Compound(vec![(
        "sections".into(),
        Tag::List {
            element_type: TagType::Compound,
            values: vec![section],
        },
    )]);
    let decoded = palette::decode_sections(&chunk).unwrap();
    let blocks = decoded[0].blocks.as_ref().unwrap();
    assert_eq!(
        blocks.block(0, 0, 0).unwrap().name,
        state("minecraft:stone", &[]).name
    );
    assert_eq!(
        blocks.block(1, 0, 0).unwrap(),
        &state("minecraft:oak_log", &[("axis", "x")])
    );
    assert_eq!(
        blocks.block(2, 0, 0).unwrap().name,
        state("minecraft:grass_block", &[]).name
    );
    for count in [0usize, 1, 16, 32] {
        assert!(
            output.lines().any(|line| line == format!("OK {count}")),
            "{output}"
        );
        let encoded =
            nbt::read(&std::fs::read(temp.path().join(format!("output-{count}.nbt"))).unwrap())
                .unwrap();
        let section = Tag::Compound(vec![
            ("Y".into(), Tag::Byte(0)),
            ("block_states".into(), encoded.value),
        ]);
        let chunk = Tag::Compound(vec![(
            "sections".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![section],
            },
        )]);
        let decoded = palette::decode_sections(&chunk).unwrap();
        let blocks = decoded[0].blocks.as_ref().unwrap();
        for (i, expected) in states.iter().take(count).enumerate() {
            let actual = blocks.block(i % 16, 0, i / 16).unwrap();
            assert_eq!(actual.name, expected.name);
            let mut actual_properties = actual.properties.clone();
            let mut expected_properties = expected.properties.clone();
            actual_properties.sort_by(|a, b| a.0.0.cmp(&b.0.0));
            expected_properties.sort_by(|a, b| a.0.0.cmp(&b.0.0));
            assert_eq!(actual_properties, expected_properties);
        }
    }
}
