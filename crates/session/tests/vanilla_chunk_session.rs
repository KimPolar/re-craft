//! Exercise session block edits through Java's 26.3 palette codec using synthetic NBT.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use glam::{IVec3, Vec3};
use native_persistence::{
    nbt::{self, NamedTag, Tag, TagType},
    palette,
    world::{ChunkArea, ChunkPos, DimensionId, LoadedChunk},
};
use native_session::{
    GameMode, GameSession, PlayerCommand, PlayerPose, PlayerState,
    loaded::{BuildBounds, LoadedSession},
};
use serde_json::Value;

const ORACLE: &str = r#"
import java.nio.file.*;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.core.BlockPos;

public class SessionChunkOracle {
    static class FakeChunk extends ProtoChunk {
        PalettedContainer<BlockState> blocks;
        FakeChunk() { super(null, null, null, null, null); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 16; }
        @Override public int getHighestSectionPosition() { return 0; }
        @Override public BlockState getBlockState(BlockPos pos) {
            return pos.getY() < 0 || pos.getY() >= 16 ? Blocks.AIR.defaultBlockState()
                : blocks.get(pos.getX() & 15, pos.getY() & 15, pos.getZ() & 15);
        }
    }
    static void compare(FakeChunk chunk, String[] args, int offset) {
        var types = new Heightmap.Types[]{Heightmap.Types.MOTION_BLOCKING, Heightmap.Types.WORLD_SURFACE, Heightmap.Types.OCEAN_FLOOR};
        for (int i=0; i<types.length; i++) {
            int actual = chunk.getHeight(types[i], 8, 6);
            int expected = Integer.parseInt(args[offset+i]);
            if (actual != expected) throw new AssertionError(types[i] + " height: " + actual + " != " + expected);
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var codec = PalettedContainer.codecRW(BlockState.CODEC, Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState());
        var before = codec.parse(NbtOps.INSTANCE, NbtIo.read(Path.of(args[0]))).getOrThrow();
        var after = codec.parse(NbtOps.INSTANCE, NbtIo.read(Path.of(args[1]))).getOrThrow();
        for (int y=0; y<16; y++) for (int z=0; z<16; z++) for (int x=0; x<16; x++) {
            var expected = x == 8 && y == 2 && z == 6 ? Blocks.AIR.defaultBlockState() : before.get(x,y,z);
            if (!after.get(x,y,z).equals(expected))
                throw new AssertionError("mismatched block at " + x + "," + y + "," + z + ": " + BuiltInRegistries.BLOCK.getKey(after.get(x,y,z).getBlock()));
        }
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var chunk = (FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        chunk.blocks = before;
        var mapsField = ChunkAccess.class.getDeclaredField("heightmaps"); mapsField.setAccessible(true);
        mapsField.set(chunk, new HashMap<>());
        var predicateField = Heightmap.class.getDeclaredField("isOpaque"); predicateField.setAccessible(true);
        var types = EnumSet.of(Heightmap.Types.MOTION_BLOCKING, Heightmap.Types.WORLD_SURFACE, Heightmap.Types.OCEAN_FLOOR);
        for (var type : types) {
            Predicate<BlockState> predicate = type == Heightmap.Types.WORLD_SURFACE
                ? s -> !s.isAir() : s -> s.is(Blocks.STONE);
            predicateField.set(chunk.getOrCreateHeightmapUnprimed(type), predicate);
        }
        Heightmap.primeHeightmaps(chunk, types);
        compare(chunk, args, 2);
        before.set(8, 2, 6, Blocks.AIR.defaultBlockState());
        for (var type : types) chunk.getOrCreateHeightmapUnprimed(type).update(8, 2, 6, Blocks.AIR.defaultBlockState());
        compare(chunk, args, 5);
    }
}
"#;

fn game_and_java() -> (PathBuf, PathBuf) {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    (game, java)
}

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
        "Java chunk oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn chunk_source() -> NamedTag {
    let mut indices = vec![0; 4096];
    for z in 0..16 {
        for x in 0..16 {
            indices[x + z * 16] = 1;
        }
    }
    indices[8 + 6 * 16 + 2 * 256] = 1;
    let data = palette::encode_indices(2, &indices, 4).unwrap().unwrap();
    NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            ("xPos".into(), Tag::Int(0)),
            ("zPos".into(), Tag::Int(0)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            ("future_data".into(), Tag::String("preserved".into())),
            (
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(0)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![
                                (
                                    "palette".into(),
                                    Tag::List {
                                        element_type: TagType::String,
                                        values: vec![
                                            Tag::String("minecraft:air".into()),
                                            Tag::String("minecraft:stone".into()),
                                        ],
                                    },
                                ),
                                ("data".into(), Tag::LongArray(data)),
                            ]),
                        ),
                    ])],
                },
            ),
        ]),
    }
}

fn storage(document: &NamedTag) -> NamedTag {
    let Tag::List { values, .. } = document.value.get("sections").unwrap() else {
        unreachable!()
    };
    NamedTag {
        name: "".into(),
        value: values[0].get("block_states").unwrap().clone(),
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn edited_session_chunk_round_trips_through_vanilla_palette() {
    let (game, java) = game_and_java();
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_block_ids().unwrap();
    let tags = Arc::new(
        native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &registry).unwrap(),
    );
    let temp = tempfile::tempdir().unwrap();
    let source = chunk_source();
    let original_bytes = nbt::write(&source).unwrap();
    let position = ChunkPos { x: 0, z: 0 };
    let loaded = LoadedChunk {
        position,
        data_version: 5023,
        status: Some("minecraft:full".into()),
        document: Arc::new(source.clone()),
        sections: palette::decode_sections(&source.value).unwrap(),
    };
    let area = ChunkArea {
        center: position,
        radius: 0,
        chunks: BTreeMap::from([(position, Arc::new(loaded))]),
        missing: vec![],
    };
    let world = area
        .materialize(&DimensionId::parse("minecraft:overworld").unwrap())
        .unwrap();
    let player = PlayerState {
        position: Vec3::new(8.5, 1.0, 8.5),
        yaw: 0.0,
        pitch: 0.0,
        vertical_velocity: 0.0,
        on_ground: true,
        pose: PlayerPose::Standing,
    };
    let mut session = LoadedSession::new(
        world,
        IVec3::ZERO,
        BuildBounds {
            min_y: 0,
            max_y: 16,
        },
        player,
    )
    .unwrap();
    session.set_spawn_heightmap_tags(tags).unwrap();
    session
        .prepare_initial_spawn_heightmaps(&area.chunks[&position])
        .unwrap();
    let initial_heights = session.spawn_column_heights(8, 6).unwrap();
    session.set_game_mode(GameMode::Creative);
    session.submit_command(PlayerCommand::Break);
    session.update(Duration::from_millis(50));
    let edited_heights = session.spawn_column_heights(8, 6).unwrap();
    assert_eq!(
        session
            .snapshot()
            .world
            .loaded()
            .unwrap()
            .block(8, 2, 6)
            .unwrap()
            .name,
        "minecraft:air"
    );
    let draft = session
        .draft_changed_chunk(native_world::loaded::ChunkPos { x: 0, z: 0 }, &source)
        .unwrap();
    assert_eq!(draft.changed_blocks, 1);
    assert!(draft.matches_source(&source).unwrap());
    assert_eq!(
        draft.document().value.get("future_data"),
        source.value.get("future_data")
    );
    assert_eq!(nbt::write(&source).unwrap(), original_bytes);
    let before = temp.path().join("before.nbt");
    let after = temp.path().join("after.nbt");
    std::fs::write(&before, nbt::write(&storage(&source)).unwrap()).unwrap();
    std::fs::write(&after, nbt::write(&storage(draft.document())).unwrap()).unwrap();
    let java_source = temp.path().join("SessionChunkOracle.java");
    std::fs::write(&java_source, ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg(&java_source));
    run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("SessionChunkOracle")
        .arg(&before)
        .arg(&after)
        .args(
            [
                initial_heights.motion_blocking,
                initial_heights.world_surface,
                initial_heights.ocean_floor,
                edited_heights.motion_blocking,
                edited_heights.world_surface,
                edited_heights.ocean_floor,
            ]
            .map(|height| height.to_string()),
        ));
}
