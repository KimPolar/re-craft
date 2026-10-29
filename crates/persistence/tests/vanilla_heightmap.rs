//! Compare reconstructed heightmaps against Java 26.3's real Heightmap storage.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_persistence::{
    edit::{BlockChange, draft_chunk_blocks},
    heightmap::{BlockHeightFlags, HeightBounds, SpawnHeightmapKind, SpawnHeightmaps},
    nbt::{self, NamedTag, Tag, TagType},
    palette::{self, BlockState},
    world::ChunkPos,
};
use serde_json::Value;

const JAVA: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.nbt.*;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.Heightmap;

public class HeightmapOracle {
    static class FakeChunk extends ProtoChunk {
        int minimum, height;
        Map<Integer, PalettedContainer<BlockState>> blocks;
        FakeChunk() { super(null, null, null, null, null); }
        @Override public int getMinY() { return minimum; }
        @Override public int getHeight() { return height; }
        @Override public int getHighestSectionPosition() { return minimum + height - 16; }
        @Override public BlockState getBlockState(BlockPos pos) {
            if (pos.getY() < minimum || pos.getY() >= minimum + height) return Blocks.AIR.defaultBlockState();
            var section = blocks.get(Math.floorDiv(pos.getY(), 16));
            return section == null ? Blocks.AIR.defaultBlockState() : section.get(pos.getX() & 15, pos.getY() & 15, pos.getZ() & 15);
        }
    }
    static boolean solid(BlockState state) { return state.is(Blocks.STONE) || state.is(Blocks.OAK_LEAVES); }
    static Predicate<BlockState> predicate(Heightmap.Types type) {
        return switch (type) {
            case WORLD_SURFACE, WORLD_SURFACE_WG -> state -> !state.isAir();
            case OCEAN_FLOOR, OCEAN_FLOOR_WG -> HeightmapOracle::solid;
            case MOTION_BLOCKING -> state -> solid(state) || state.is(Blocks.WATER);
            case MOTION_BLOCKING_NO_LEAVES -> state -> state.is(Blocks.STONE) || state.is(Blocks.WATER);
        };
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var mapsField = ChunkAccess.class.getDeclaredField("heightmaps"); mapsField.setAccessible(true);
        var predicateField = Heightmap.class.getDeclaredField("isOpaque"); predicateField.setAccessible(true);
        var codec = PalettedContainer.codecRW(BlockState.CODEC, Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState());
        for (int i=0; i<args.length; i+=4) {
            var chunk = (FakeChunk)unsafe.allocateInstance(FakeChunk.class);
            chunk.minimum = Integer.parseInt(args[i+1]);
            chunk.height = Integer.parseInt(args[i+2]);
            chunk.blocks = new HashMap<>();
            mapsField.set(chunk, new HashMap<>());
            var tag = NbtIo.read(Path.of(args[i]));
            var list = (ListTag)tag.get("sections");
            for (var entry : list) {
                var section = (CompoundTag)entry;
                if (section.get("block_states") != null) {
                    var container = codec.parse(NbtOps.INSTANCE, section.get("block_states")).getOrThrow();
                    chunk.blocks.put((int)section.getByteOr("Y", (byte)0), container);
                }
            }
            var types = EnumSet.allOf(Heightmap.Types.class);
            for (var type : types) {
                var map = chunk.getOrCreateHeightmapUnprimed(type);
                predicateField.set(map, predicate(type));
            }
            Heightmap.primeHeightmaps(chunk, types);
            var output = new CompoundTag();
            for (var type : types) {
                output.putLongArray(type.getSerializationKey(), chunk.getOrCreateHeightmapUnprimed(type).getRawData().clone());
                var heights = new int[256];
                for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
                    heights[x + z * 16] = chunk.getHeight(type, x, z);
                output.putIntArray("HEIGHT_" + type.getSerializationKey(), heights);
            }
            var edits = tag.getList("NativeTestEdits").orElse(new ListTag());
            for (var entry : edits) {
                var edit = (CompoundTag)entry;
                int x = edit.getIntOr("x", 0), y = edit.getIntOr("y", 0), z = edit.getIntOr("z", 0);
                var state = BlockState.CODEC.parse(NbtOps.INSTANCE, edit.get("state")).getOrThrow();
                chunk.blocks.get(Math.floorDiv(y, 16)).set(x & 15, y & 15, z & 15, state);
                for (var type : types) chunk.getOrCreateHeightmapUnprimed(type).update(x, y, z, state);
            }
            for (var type : types) {
                var heights = new int[256];
                for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
                    heights[x + z * 16] = chunk.getHeight(type, x, z);
                output.putIntArray("UPDATED_" + type.getSerializationKey(), heights);
            }
            NbtIo.write(output, Path.of(args[i+3]));
        }
    }
}
"#;

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java heightmap oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

const NAMES: [&str; 6] = [
    "minecraft:air",
    "minecraft:stone",
    "minecraft:oak_leaves",
    "minecraft:water",
    "minecraft:short_grass",
    "minecraft:cave_air",
];

fn block(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn classify(state: &BlockState) -> anyhow::Result<BlockHeightFlags> {
    let name = state.name.to_string_lossy();
    Ok(BlockHeightFlags {
        is_air: matches!(name.as_str(), "minecraft:air" | "minecraft:cave_air"),
        blocks_motion: matches!(name.as_str(), "minecraft:stone" | "minecraft:oak_leaves"),
        blocks_motion_no_leaves: name == "minecraft:stone",
        has_fluid: name == "minecraft:water",
    })
}

fn fixture(min_y: i32, max_y: i32, seed: u32) -> (NamedTag, Vec<BlockChange>) {
    let mut random = seed;
    let mut next = || {
        random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        random
    };
    let mut sections = Vec::new();
    let mut changes = Vec::new();
    let first = min_y / 16;
    let count = (max_y - min_y) / 16;
    let ys: std::collections::BTreeSet<_> = [
        first,
        first + count / 3,
        first + count / 2,
        first + count - 1,
    ]
    .into_iter()
    .collect();
    for y in ys {
        let mut indices = vec![0; 4096];
        for _ in 0..600 {
            let index = (next() as usize) % 4096;
            indices[index] = 1 + (next() % 5);
        }
        for _ in 0..15 {
            let index = (next() as usize) % 4096;
            let old = indices[index] as usize;
            let replacement = (old + 1 + (next() % 5) as usize) % NAMES.len();
            let x = (index % 16) as i32;
            let z = ((index / 16) % 16) as i32;
            let block_y = y * 16 + (index / 256) as i32;
            if !changes
                .iter()
                .any(|c: &BlockChange| c.x == x && c.y == block_y && c.z == z)
            {
                changes.push(BlockChange {
                    x,
                    y: block_y,
                    z,
                    expected: block(NAMES[old]),
                    replacement: block(NAMES[replacement]),
                });
            }
        }
        sections.push(Tag::Compound(vec![
            ("Y".into(), Tag::Byte(y as i8)),
            (
                "block_states".into(),
                Tag::Compound(vec![
                    (
                        "palette".into(),
                        Tag::List {
                            element_type: TagType::String,
                            values: NAMES
                                .iter()
                                .map(|name| Tag::String((*name).into()))
                                .collect(),
                        },
                    ),
                    (
                        "data".into(),
                        Tag::LongArray(
                            palette::encode_indices(NAMES.len(), &indices, 4)
                                .unwrap()
                                .unwrap(),
                        ),
                    ),
                ]),
            ),
        ]));
    }
    (
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
                        values: sections,
                    },
                ),
            ]),
        },
        changes,
    )
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn reconstructed_heightmaps_match_vanilla_priming_and_storage() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("HeightmapOracle.java");
    std::fs::write(&source, JAVA).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut args = Vec::<OsString>::new();
    let mut drafts = Vec::new();
    for (scenario, (min_y, max_y)) in [
        (-64, 320),
        (0, 256),
        (-128, 384),
        (0, 16),
        (-16, 32),
        (-2032, -1968),
    ]
    .into_iter()
    .enumerate()
    {
        let (original, changes) = fixture(min_y, max_y, 1234 + scenario as u32);
        for (version, edits) in [("original", &[][..]), ("edited", changes.as_slice())] {
            let mut draft = draft_chunk_blocks(&original, ChunkPos { x: 0, z: 0 }, edits).unwrap();
            draft
                .rebuild_heightmaps(HeightBounds { min_y, max_y }, classify)
                .unwrap();
            let chunk = temp.path().join(format!("{scenario}-{version}-chunk.nbt"));
            let output = temp
                .path()
                .join(format!("{scenario}-{version}-heights.nbt"));
            let oracle_edits = if version == "original" {
                changes.clone()
            } else {
                Vec::new()
            };
            let mut document = draft.document().clone();
            let Tag::Compound(root) = &mut document.value else {
                unreachable!()
            };
            root.push((
                "NativeTestEdits".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: oracle_edits
                        .iter()
                        .map(|edit| {
                            Tag::Compound(vec![
                                ("x".into(), Tag::Int(edit.x)),
                                ("y".into(), Tag::Int(edit.y)),
                                ("z".into(), Tag::Int(edit.z)),
                                ("state".into(), Tag::String(edit.replacement.name.clone())),
                            ])
                        })
                        .collect(),
                },
            ));
            std::fs::write(&chunk, nbt::write(&document).unwrap()).unwrap();
            args.extend([
                chunk.into_os_string(),
                min_y.to_string().into(),
                (max_y - min_y).to_string().into(),
                output.clone().into_os_string(),
            ]);
            drafts.push((draft, output, HeightBounds { min_y, max_y }, oracle_edits));
        }
    }
    run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("HeightmapOracle")
        .args(&args));
    for (index, (draft, path, bounds, edits)) in drafts.iter().enumerate() {
        let expected = nbt::read(&std::fs::read(path).unwrap()).unwrap();
        let actual = draft.document().value.get("Heightmaps").unwrap();
        for kind in [
            "WORLD_SURFACE",
            "OCEAN_FLOOR",
            "MOTION_BLOCKING",
            "MOTION_BLOCKING_NO_LEAVES",
        ] {
            assert_eq!(
                actual.get(kind),
                expected.value.get(kind),
                "scenario {index}, map {kind}"
            );
        }
        let mut read = SpawnHeightmaps::read(draft.document(), *bounds).unwrap();
        let blank = NamedTag {
            name: "".into(),
            value: Tag::Compound(Vec::new()),
        };
        let mut primed = SpawnHeightmaps::read(&blank, *bounds).unwrap();
        let sections = palette::decode_sections(&draft.document().value).unwrap();
        let air = block("minecraft:air");
        let source = |x: i32, y: i32, z: i32| {
            sections
                .iter()
                .find(|section| i32::from(section.y) == y.div_euclid(16))
                .and_then(|section| section.blocks.as_ref())
                .and_then(|blocks| blocks.block(x as usize, y.rem_euclid(16) as usize, z as usize))
                .unwrap_or(&air)
        };
        primed
            .prime_missing(|x, y, z| classify(source(x, y, z)))
            .unwrap();
        for (name, kind) in [
            ("MOTION_BLOCKING", SpawnHeightmapKind::MotionBlocking),
            ("WORLD_SURFACE", SpawnHeightmapKind::WorldSurface),
            ("OCEAN_FLOOR", SpawnHeightmapKind::OceanFloor),
        ] {
            let key = format!("HEIGHT_{name}");
            let Some(Tag::IntArray(expected)) = expected.value.get(&key) else {
                panic!("Java omitted {key}");
            };
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(
                        read.highest(kind, x, z),
                        Some(expected[(x + z * 16) as usize]),
                        "scenario {index}, map {name}, column {x}, {z}"
                    );
                    assert_eq!(
                        primed.highest(kind, x, z),
                        Some(expected[(x + z * 16) as usize]),
                        "primed scenario {index}, map {name}, column {x}, {z}"
                    );
                }
            }
        }
        let mut changed = std::collections::BTreeMap::new();
        for edit in edits {
            changed.insert((edit.x, edit.y, edit.z), edit.replacement.clone());
            read.update_after_change(
                edit.x,
                edit.y,
                edit.z,
                classify(&edit.replacement).unwrap(),
                |x, y, z| classify(changed.get(&(x, y, z)).unwrap_or_else(|| source(x, y, z))),
            )
            .unwrap();
        }
        for (name, kind) in [
            ("MOTION_BLOCKING", SpawnHeightmapKind::MotionBlocking),
            ("WORLD_SURFACE", SpawnHeightmapKind::WorldSurface),
            ("OCEAN_FLOOR", SpawnHeightmapKind::OceanFloor),
        ] {
            let key = format!("UPDATED_{name}");
            let Some(Tag::IntArray(expected)) = expected.value.get(&key) else {
                panic!("Java omitted {key}")
            };
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(
                        read.highest(kind, x, z),
                        Some(expected[(x + z * 16) as usize]),
                        "updated scenario {index}, map {name}, column {x}, {z}"
                    );
                }
            }
        }
    }
    println!(
        "Compared {} synthetic chunks and four heightmaps against Java",
        drafts.len()
    );
}
