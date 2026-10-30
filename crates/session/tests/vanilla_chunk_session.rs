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

#[test]
#[cfg(windows)]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn session_lighting_and_block_changes_survive_java_region_round_trip() {
    use native_persistence::{
        save::{WorldSavePlan, WorldWriteGuard},
        world::WorldReader,
    };
    use native_session::SessionWorld;
    let (game, java) = game_and_java();
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_block_ids().unwrap();
    let tags =
        native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &registry).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/session-light-region-test");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("world-")
        .tempdir_in(root)
        .unwrap();
    let java_source = temp.path().join("LightRegionOracle.java");
    std::fs::write(&java_source, r#"
import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.nbt.*;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.*;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
public class LightRegionOracle {
  static RegionFile open(Path dir) throws Exception {
    return new RegionFile(new RegionStorageInfo("light", null, "region"),dir.resolve("r.0.0.mca"),dir,RegionFileVersion.VERSION_NONE,false);
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var world=Path.of(args[1]); var dir=world.resolve("dimensions/minecraft/overworld/region");
    if(args[0].equals("seed")) {
      Files.createDirectories(dir);
      var level=new CompoundTag(); var data=new CompoundTag(); data.putInt("DataVersion",5023); level.put("Data",data); NbtIo.writeCompressed(level,world.resolve("level.dat"));
      var playerDir=world.resolve("players/data"); Files.createDirectories(playerDir);
      var player=new CompoundTag(); player.putInt("DataVersion",5023); player.putIntArray("UUID",new int[]{0,0,0,1}); player.putString("Dimension","minecraft:overworld"); player.putLong("future_player",83);
      NbtIo.writeCompressed(player,playerDir.resolve("00000000-0000-0000-0000-000000000001.dat"));
      var clockDir=world.resolve("data/minecraft"); Files.createDirectories(clockDir);
      var clock=new CompoundTag(); clock.putInt("DataVersion",5023); var clocks=new CompoundTag(); var overworld=new CompoundTag(); overworld.putLong("total_ticks",10); overworld.putInt("future_clock",17); clocks.put("minecraft:overworld",overworld); clock.put("data",clocks); NbtIo.writeCompressed(clock,clockDir.resolve("world_clocks.dat"));
      try(var region=open(dir)) { for(int x=0;x<3;x++) for(int z=0;z<3;z++) {
        var chunk=new CompoundTag(); chunk.putInt("DataVersion",5023); chunk.putInt("xPos",x); chunk.putInt("zPos",z); chunk.putString("Status","minecraft:full"); chunk.putBoolean("isLightOn",true); chunk.putLong("future_data",42+x*3+z);
        var palette=new ListTag(); var air=new CompoundTag(); air.putString("id","minecraft:air"); palette.add(air);
        var states=new CompoundTag();
        if(x==1 && z==1) { var ore=new CompoundTag(); ore.putString("id","minecraft:redstone_ore"); var properties=new CompoundTag(); properties.putString("lit","false"); ore.put("properties",properties); palette.add(ore); long[] words=new long[256]; int index=8+6*16+2*256; words[index/16] |= 1L << ((index%16)*4); states.putLongArray("data",words); }
        states.put("palette",palette); var section=new CompoundTag(); section.putByte("Y",(byte)0); section.put("block_states",states); section.putByteArray("BlockLight",new byte[2048]); section.putString("future_section","keep");
        var sections=new ListTag(); sections.add(section); chunk.put("sections",sections);
        try(var out=region.getChunkDataOutputStream(new ChunkPos(x,z))) { NbtIo.write(chunk,(DataOutput)out); }
      }}
    } else if(args[0].equals("alter")) {
      try(var region=open(dir)) { CompoundTag chunk;
        try(var in=region.getChunkDataInputStream(new ChunkPos(1,1))) { chunk=NbtIo.read((DataInput)in); }
        chunk.putLong("future_data",999);
        try(var out=region.getChunkDataOutputStream(new ChunkPos(1,1))) { NbtIo.write(chunk,(DataOutput)out); }
      }
    } else {
      byte[] expected=Files.readAllBytes(Path.of(args[2])); int at=0; var codec=PalettedContainer.codecRW(BlockState.CODEC,Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState()); var documents=new LinkedHashMap<ChunkPos,CompoundTag>();
      try(var region=open(dir)) { for(int x=0;x<3;x++) for(int z=0;z<3;z++) {
        try(var in=region.getChunkDataInputStream(new ChunkPos(x,z))) {
          var chunk=NbtIo.read((DataInput)in); documents.put(new ChunkPos(x,z),chunk); if(chunk.getLongOr("future_data",-1)!=42+x*3+z || !chunk.getBooleanOr("isLightOn",false)) throw new AssertionError("chunk metadata");
          var sections=chunk.getListOrEmpty("sections"); Map<Integer,CompoundTag> byY=new HashMap<>(); for(var tag:sections) { var section=(CompoundTag)tag; byY.put((int)section.getByteOr("Y",(byte)-99),section); }
          var primary=byY.get(0); if(primary==null || !primary.getStringOr("future_section","").equals("keep")) throw new AssertionError("section metadata");
          if(x==1 && z==1) { var blocks=codec.parse(NbtOps.INSTANCE,primary.get("block_states")).getOrThrow(); var ore=blocks.get(8,2,6); if(!ore.is(Blocks.REDSTONE_ORE) || !ore.getValue(net.minecraft.world.level.block.RedStoneOreBlock.LIT)) throw new AssertionError("ore state"); }
          for(int y=-1;y<=1;y++) { var section=byY.get(y); byte[] bytes=section==null ? new byte[2048] : section.getByteArray("BlockLight").orElse(new byte[2048]); var light=new DataLayer(bytes);
            for(int ly=0;ly<16;ly++) for(int lz=0;lz<16;lz++) for(int lx=0;lx<16;lx++) { if(light.get(lx,ly,lz)!=(expected[at++]&255)) throw new AssertionError("light mismatch chunk="+x+","+z+" section="+y+" local="+lx+","+ly+","+lz); }
          }
        }
      }
        for(var entry:documents.entrySet()) try(var out=region.getChunkDataOutputStream(entry.getKey())) { NbtIo.write(entry.getValue(),(DataOutput)out); }
      }
      if(at!=expected.length) throw new AssertionError("sample count");
      var player=NbtIo.readCompressed(world.resolve("players/data/00000000-0000-0000-0000-000000000001.dat"),NbtAccounter.unlimitedHeap());
      if(!Arrays.equals(player.getIntArray("UUID").orElseThrow(),new int[]{0,0,0,1}) || player.getLongOr("future_player",0)!=83 || player.getListOrEmpty("Pos").getDouble(0).orElseThrow()!=24.5) throw new AssertionError("player data");
      var clock=NbtIo.readCompressed(world.resolve("data/minecraft/world_clocks.dat"),NbtAccounter.unlimitedHeap()); var overworld=clock.getCompoundOrEmpty("data").getCompoundOrEmpty("minecraft:overworld");
      if(overworld.getLongOr("total_ticks",0)!=11 || overworld.getIntOr("future_clock",0)!=17) throw new AssertionError("clock data");
    }
  }
}
"#).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&java_source));
    let world_path = temp.path().join("world");
    std::fs::create_dir(&world_path).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("LightRegionOracle")
        .arg("seed")
        .arg(&world_path));
    let dimension = DimensionId::parse("minecraft:overworld").unwrap();
    let player_path = world_path.join("players/data/00000000-0000-0000-0000-000000000001.dat");
    let clock_path = world_path.join("data/minecraft/world_clocks.dat");
    let player_source = nbt::read_gzip(std::fs::File::open(&player_path).unwrap()).unwrap();
    let clock_source = nbt::read_gzip(std::fs::File::open(&clock_path).unwrap()).unwrap();
    let mut reader = WorldReader::open(&world_path, dimension.clone()).unwrap();
    let area = reader.read_area(ChunkPos { x: 1, z: 1 }, 1).unwrap();
    assert_eq!(area.chunks.len(), 9);
    let world = area.materialize(&dimension).unwrap();
    drop(reader);
    let player = PlayerState {
        position: Vec3::new(24.5, 1.0, 24.5),
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
    session
        .restore_saved_player_inventory(&player_source)
        .unwrap();
    session
        .restore_saved_world_clocks(
            &clock_source,
            BTreeMap::from([(
                dimension.clone(),
                native_persistence::world::SavedClock {
                    total_ticks: 10,
                    ..Default::default()
                },
            )]),
            true,
        )
        .unwrap();
    session.set_random_tick_speed(0);
    session.submit_command(PlayerCommand::Place);
    session.update(Duration::from_millis(50));
    let snapshot = session.snapshot();
    let SessionWorld::Loaded(loaded) = &snapshot.world else {
        panic!("loaded world expected")
    };
    assert_eq!(
        loaded
            .block(24, 2, 22)
            .unwrap()
            .properties
            .get("lit")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(loaded.light(24, 2, 23, false), Some(8));
    let mut samples = Vec::new();
    for x in 0..3 {
        for z in 0..3 {
            for sy in -1..=1 {
                for y in 0..16 {
                    for z0 in 0..16 {
                        for x0 in 0..16 {
                            samples.push(
                                loaded
                                    .light(x * 16 + x0, sy * 16 + y, z * 16 + z0, false)
                                    .unwrap_or(0),
                            );
                        }
                    }
                }
            }
        }
    }
    let sample_path = temp.path().join("light.bin");
    std::fs::write(&sample_path, &samples).unwrap();
    let dirty = session.dirty_chunk_positions();
    assert!(dirty.len() > 1, "light should cross chunk boundaries");
    let sources = dirty
        .iter()
        .map(|&position| {
            let source = &area.chunks[&ChunkPos {
                x: position.x,
                z: position.z,
            }]
                .document;
            (position, source.as_ref())
        })
        .collect::<BTreeMap<_, _>>();
    let mut incomplete = sources.clone();
    incomplete.pop_first();
    assert!(
        session
            .draft_player_and_dirty_chunks(&player_source, &incomplete, &tags, |_| Ok((
                true, false
            )))
            .is_err()
    );
    let make_plan = || {
        let mut plan = WorldSavePlan::new();
        let combined = session
            .draft_player_and_dirty_chunks(&player_source, &sources, &tags, |state| {
                Ok((state.name.matches("minecraft:air"), false))
            })
            .unwrap();
        assert_eq!(combined.chunks().len(), dirty.len());
        let (player, chunks) = combined.into_parts();
        for (_, draft) in chunks {
            plan.add_chunk(dimension.clone(), draft, 1_777_777).unwrap();
        }
        player
            .add_to_save_plan(&mut plan, [0, 0, 0, 1], &player_source)
            .unwrap();
        session
            .draft_world_clocks(&clock_source)
            .unwrap()
            .add_to_save_plan(&mut plan, &clock_source)
            .unwrap();
        plan
    };
    let stale_plan = make_plan();
    let region_path = world_path.join("dimensions/minecraft/overworld/region/r.0.0.mca");
    let original_region = std::fs::read(&region_path).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("LightRegionOracle")
        .arg("alter")
        .arg(&world_path));
    let changed_region = std::fs::read(&region_path).unwrap();
    let unchanged_player = std::fs::read(&player_path).unwrap();
    let unchanged_clock = std::fs::read(&clock_path).unwrap();
    let mut writer = WorldWriteGuard::acquire(&world_path).unwrap();
    assert!(writer.write_plan(stale_plan, |_, _| Ok(())).is_err());
    drop(writer);
    assert_eq!(std::fs::read(&region_path).unwrap(), changed_region);
    assert_eq!(std::fs::read(&player_path).unwrap(), unchanged_player);
    assert_eq!(std::fs::read(&clock_path).unwrap(), unchanged_clock);
    assert!(!world_path.join(".minecraft-native-pending").exists());
    std::fs::write(&region_path, &original_region).unwrap();
    let plan = make_plan();
    let mut writer = WorldWriteGuard::acquire(&world_path).unwrap();
    let result = writer
        .write_plan(plan, |_, change| {
            anyhow::ensure!(
                change.expected.name.matches("minecraft:redstone_ore")
                    && change.replacement.name.matches("minecraft:redstone_ore"),
                "unexpected block edit"
            );
            Ok(())
        })
        .unwrap();
    drop(writer);
    assert_eq!(
        std::fs::read(result.backup_directory.join("region-0/r.0.0.mca")).unwrap(),
        original_region
    );
    assert_eq!(
        std::fs::read(
            result
                .backup_directory
                .join("players/00000000-0000-0000-0000-000000000001.dat")
        )
        .unwrap(),
        unchanged_player
    );
    assert_eq!(
        std::fs::read(result.backup_directory.join("world-data/world_clocks.dat")).unwrap(),
        unchanged_clock
    );
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("LightRegionOracle")
        .arg("verify")
        .arg(&world_path)
        .arg(&sample_path));
    let mut reader = WorldReader::open(&world_path, dimension.clone()).unwrap();
    let reloaded = reader.read_area(ChunkPos { x: 1, z: 1 }, 1).unwrap();
    let reloaded_world = reloaded.materialize(&dimension).unwrap();
    assert_eq!(
        reloaded_world
            .block(24, 2, 22)
            .unwrap()
            .properties
            .get("lit")
            .map(String::as_str),
        Some("true")
    );
    let mut index = 0;
    for x in 0..3 {
        for z in 0..3 {
            for sy in -1..=1 {
                for y in 0..16 {
                    for z0 in 0..16 {
                        for x0 in 0..16 {
                            assert_eq!(
                                reloaded_world
                                    .light(x * 16 + x0, sy * 16 + y, z * 16 + z0, false)
                                    .unwrap_or(0),
                                samples[index]
                            );
                            index += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(index, samples.len());
    for (position, chunk) in reloaded.chunks {
        assert_eq!(
            chunk.document.value.get("future_data"),
            Some(&Tag::Long(
                42 + i64::from(position.x) * 3 + i64::from(position.z)
            ))
        );
    }
}
