//! Opt-in comparison of region rewrites and world locks against Java 26.3.

#![cfg(windows)]

use std::{
    ffi::OsString,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use native_persistence::{
    anvil::RegionReader,
    edit::{BlockChange, draft_chunk_blocks},
    heightmap::{BlockHeightFlags, HeightBounds},
    nbt,
    palette::BlockState,
    save::{WorldSavePlan, WorldWriteGuard},
    world::{ChunkPos, DimensionId, WorldReader},
};
use serde_json::Value;

const ORACLE: &str = r#"
import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.*;
import net.minecraft.util.DirectoryLock;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.storage.*;

public class RegionWriteOracle {
    static RegionFile open(Path dir) throws Exception {
        return new RegionFile(new RegionStorageInfo("parity", null, "region"), dir.resolve("r.0.0.mca"), dir, RegionFileVersion.VERSION_NONE, false);
    }
    static RegionFile open(Path dir, int regionX, RegionFileVersion version) throws Exception {
        return new RegionFile(new RegionStorageInfo("parity", null, "region"), dir.resolve("r." + regionX + ".0.mca"), dir, version, false);
    }
    static CompoundTag chunk(int x) {
        var chunk = new CompoundTag();
        chunk.putInt("DataVersion", 5023);
        chunk.putInt("xPos", x);
        chunk.putInt("zPos", 0);
        chunk.putString("Status", "minecraft:full");
        var sections = new ListTag();
        if (x == 0) {
            var palette = new ListTag();
            palette.add(StringTag.valueOf("minecraft:stone"));
            var blockStates = new CompoundTag();
            blockStates.put("palette", palette);
            var section = new CompoundTag();
            section.putByte("Y", (byte)0);
            section.put("block_states", blockStates);
            sections.add(section);
        }
        chunk.put("sections", sections);
        var extra = new CompoundTag();
        extra.putString("marker", "unknown-" + x);
        extra.putLongArray("longs", new long[]{Long.MIN_VALUE, 42, Long.MAX_VALUE});
        chunk.put("fixture_unknown", extra);
        if (x == 2) {
            byte[] bytes = new byte[1_100_000];
            new Random(76543).nextBytes(bytes);
            chunk.putByteArray("oversized", bytes);
        }
        return chunk;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Path world = Path.of(args[1]);
        Path dir = world.resolve("dimensions/minecraft/overworld/region");
        if (args[0].equals("seed")) {
            Files.createDirectories(dir);
            var level = new CompoundTag();
            var data = new CompoundTag();
            data.putInt("DataVersion", 5023);
            level.put("Data", data);
            NbtIo.writeCompressed(level, world.resolve("level.dat"));
            try (var region = open(dir)) {
                for (int x = 0; x < 3; x++) {
                    try (var out = region.getChunkDataOutputStream(new ChunkPos(x, 0))) {
                        NbtIo.write(chunk(x), (DataOutput)out);
                    }
                }
            }
            int[] xs = {-32, 32, 64};
            RegionFileVersion[] versions = {RegionFileVersion.VERSION_GZIP, RegionFileVersion.VERSION_DEFLATE, RegionFileVersion.VERSION_LZ4};
            for (int i = 0; i < xs.length; i++) {
                try (var region = open(dir, Math.floorDiv(xs[i], 32), versions[i]);
                     var out = region.getChunkDataOutputStream(new ChunkPos(xs[i], 0))) {
                    NbtIo.write(chunk(xs[i]), (DataOutput)out);
                }
            }
            System.out.println("OK");
        } else if (args[0].equals("verify")) {
            try (var region = open(dir)) {
                for (int x = 0; x < 3; x++) {
                    try (var in = region.getChunkDataInputStream(new ChunkPos(x, 0))) {
                        if (in == null || !NbtIo.read((DataInput)in).equals(chunk(x))) throw new AssertionError("chunk mismatch " + x);
                    }
                }
            }
            for (int x : new int[]{-32, 32, 64}) {
                try (var region = open(dir, Math.floorDiv(x, 32), RegionFileVersion.VERSION_NONE);
                     var in = region.getChunkDataInputStream(new ChunkPos(x, 0))) {
                    if (in == null || !NbtIo.read((DataInput)in).equals(chunk(x))) throw new AssertionError("chunk mismatch " + x);
                }
            }
            System.out.println("OK");
        } else if (args[0].equals("verifyEdit")) {
            try (var region = open(dir);
                 var in = region.getChunkDataInputStream(new ChunkPos(0, 0))) {
                if (in == null) throw new AssertionError("edited chunk is missing");
                var saved = NbtIo.read((DataInput)in);
                var sections = saved.getListOrEmpty("sections");
                var blockStates = sections.getCompoundOrEmpty(0).getCompoundOrEmpty("block_states");
                var palette = blockStates.getListOrEmpty("palette");
                var words = blockStates.getLongArray("data").orElseThrow();
                int stateIndex = (int)(words[0] & 15L);
                var entry = palette.get(stateIndex);
                String id;
                if (entry instanceof StringTag text) {
                    id = text.value();
                } else {
                    var state = ((CompoundTag)entry).get("");
                    if (state == null) state = entry;
                    id = state instanceof StringTag text
                        ? text.value()
                        : ((CompoundTag)state).getString("id").orElseThrow();
                }
                if (!id.equals("minecraft:dirt")) throw new AssertionError("Java read " + id);
                if (palette.size() != 2 || words.length != 256) throw new AssertionError("invalid edited palette");
                if (!saved.getCompoundOrEmpty("fixture_unknown").getStringOr("marker", "").equals("unknown-0"))
                    throw new AssertionError("unknown chunk data was lost");
                var heights = saved.getCompoundOrEmpty("Heightmaps");
                var surface = heights.getLongArray("WORLD_SURFACE").orElseThrow();
                if (surface.length != 22 || (surface[0] & 31L) != 16L)
                    throw new AssertionError("Java could not read the rebuilt heightmap");
            }
            System.out.println("OK");
        } else if (args[0].equals("seedEntity")) {
            Files.createDirectories(dir);
            Path entities = world.resolve("dimensions/minecraft/overworld/entities");
            Files.createDirectories(entities);
            var level = new CompoundTag(); var data = new CompoundTag(); data.putInt("DataVersion", 5023); level.put("Data", data);
            NbtIo.writeCompressed(level, world.resolve("level.dat"));
            var entity = new CompoundTag(); entity.putString("id", "minecraft:falling_block"); entity.putIntArray("UUID", new int[]{1,2,3,4}); entity.putLong("future", 42);
            var pos = new ListTag(); pos.add(DoubleTag.valueOf(1.5)); pos.add(DoubleTag.valueOf(2)); pos.add(DoubleTag.valueOf(3)); entity.put("Pos", pos);
            var chunk = new CompoundTag(); chunk.putInt("DataVersion", 5023); chunk.putIntArray("Position", new int[]{0,0}); chunk.putLong("future_chunk", 17);
            var list = new ListTag(); list.add(entity); chunk.put("Entities", list);
            try (var region = open(entities); var out = region.getChunkDataOutputStream(new ChunkPos(0,0))) { NbtIo.write(chunk, (DataOutput)out); }
            System.out.println("OK");
        } else if (args[0].equals("verifyEntity")) {
            Path entities = world.resolve("dimensions/minecraft/overworld/entities");
            try (var region = open(entities)) {
                try (var in = region.getChunkDataInputStream(new ChunkPos(0,0))) {
                    var source = NbtIo.read((DataInput)in);
                    if (source.getLongOr("future_chunk",0) != 17 || !source.getListOrEmpty("Entities").isEmpty()) throw new AssertionError("source chunk was not preserved");
                }
                try (var in = region.getChunkDataInputStream(new ChunkPos(1,0))) {
                    var destination = NbtIo.read((DataInput)in);
                    if (!Arrays.equals(destination.getIntArray("Position").orElseThrow(), new int[]{1,0})) throw new AssertionError("position");
                    var list=destination.getListOrEmpty("Entities"); var entity=list.getCompoundOrEmpty(0);
                    if (list.size()!=1 || !entity.getStringOr("id", "").equals("minecraft:falling_block") || entity.getLongOr("future",0)!=42 || entity.getListOrEmpty("Pos").getDouble(0).orElseThrow()!=17.5) throw new AssertionError("entity data");
                }
            }
            System.out.println("OK");
        } else if (args[0].equals("locked")) {
            System.out.println(DirectoryLock.isLocked(world) ? "LOCKED" : "UNLOCKED");
        } else if (args[0].equals("tryCreate")) {
            try (var lock = DirectoryLock.create(world)) {
                System.out.println("ACQUIRED");
            } catch (Exception exception) {
                System.out.println("LOCKED");
            }
        } else if (args[0].equals("hold")) {
            try (var lock = DirectoryLock.create(world)) {
                System.out.println("LOCKED");
                System.out.flush();
                System.in.read();
            }
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
        "Java region oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn entity_chunk_drafts_commit_and_load_in_java() {
    use glam::DVec3;
    use native_persistence::nbt::{NamedTag, Tag};
    use native_session::saved_falling_block::{self, EntityReplacement, FallingSaveState};
    use native_world::loaded::ChunkPos as RuntimeChunkPos;
    use std::collections::BTreeMap;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("java-entity-")
        .tempdir_in(root)
        .unwrap();
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    let java_source = temp.path().join("RegionWriteOracle.java");
    std::fs::write(&java_source, ORACLE).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&java_source));
    let world = temp.path().join("world");
    std::fs::create_dir(&world).unwrap();
    let invoke = |action: &str| {
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("RegionWriteOracle")
            .arg(action)
            .arg(&world))
    };
    assert!(invoke("seedEntity").lines().any(|line| line == "OK"));
    let path = world.join("dimensions/minecraft/overworld/entities/r.0.0.mca");
    let original = RegionReader::open(&path)
        .unwrap()
        .read_chunk(0, 0)
        .unwrap()
        .unwrap();
    let Some(Tag::List { values, .. }) = original.value.get("Entities") else {
        panic!()
    };
    let falling = saved_falling_block::draft(
        &NamedTag {
            name: "".into(),
            value: values[0].clone(),
        },
        FallingSaveState {
            position: DVec3::new(17.5, 2.0, 3.0),
            velocity: DVec3::ZERO,
            on_ground: false,
            no_gravity: false,
            time: 1,
            drop_item: true,
            cancel_drop: false,
        },
    )
    .unwrap();
    let left = RuntimeChunkPos { x: 0, z: 0 };
    let right = RuntimeChunkPos { x: 1, z: 0 };
    let drafts = saved_falling_block::draft_entity_chunks_with_optional_sources(
        &BTreeMap::from([(left, Some(&original)), (right, None)]),
        &[EntityReplacement::Falling(&falling)],
    )
    .unwrap();
    let dimension = DimensionId::parse("overworld").unwrap();
    let mut plan = WorldSavePlan::new();
    assert!(
        drafts[&left]
            .add_to_save_plan(
                &mut plan,
                dimension.clone(),
                ChunkPos { x: 0, z: 0 },
                None,
                100
            )
            .is_err()
    );
    drafts[&left]
        .add_to_save_plan(
            &mut plan,
            dimension.clone(),
            ChunkPos { x: 0, z: 0 },
            Some(&original),
            100,
        )
        .unwrap();
    drafts[&right]
        .add_to_save_plan(&mut plan, dimension, ChunkPos { x: 1, z: 0 }, None, 101)
        .unwrap();
    let mut guard = WorldWriteGuard::acquire(&world).unwrap();
    guard.write_plan(plan, |_, _| Ok(())).unwrap();
    drop(guard);
    assert!(invoke("verifyEntity").lines().any(|line| line == "OK"));
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn lossless_regions_and_session_lock_match_java() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("java-region-")
        .tempdir_in(root)
        .unwrap();
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    let source = temp.path().join("RegionWriteOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let world = temp.path().join("world");
    std::fs::create_dir(&world).unwrap();
    assert!(
        run(Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&cp)
            .arg("RegionWriteOracle")
            .arg("seed")
            .arg(&world))
        .lines()
        .any(|line| line == "OK")
    );
    let region = world.join("dimensions/minecraft/overworld/region/r.0.0.mca");
    let external = region.parent().unwrap().join("c.2.0.mcc");
    assert!(external.is_file(), "Java did not create an external chunk");
    let region_before = std::fs::read(&region).unwrap();
    let external_before = std::fs::read(&external).unwrap();
    let mut original = RegionReader::open(&region).unwrap();
    let original_nbt = nbt::write(&original.read_chunk(0, 0).unwrap().unwrap()).unwrap();
    drop(original);

    let mut guard = WorldWriteGuard::acquire(&world).unwrap();
    assert!(
        run(Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&cp)
            .arg("RegionWriteOracle")
            .arg("locked")
            .arg(&world))
        .lines()
        .any(|line| line == "LOCKED")
    );
    let result = guard
        .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
        .unwrap();
    assert_eq!(
        std::fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
        region_before
    );
    assert_eq!(
        std::fs::read(result.backup_directory.join("c.2.0.mcc")).unwrap(),
        external_before
    );
    let mut region_backups = vec![("r.0.0.mca".to_owned(), result.backup_directory.clone())];
    let mut rewritten = RegionReader::open(&region).unwrap();
    assert_eq!(
        nbt::write(&rewritten.read_chunk(0, 0).unwrap().unwrap()).unwrap(),
        original_nbt
    );
    drop(rewritten);
    for x in [-32, 32, 64] {
        let path = world.join(format!(
            "dimensions/minecraft/overworld/region/r.{}.0.mca",
            x / 32
        ));
        let before = std::fs::read(&path).unwrap();
        let saved = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), x, 0)
            .unwrap();
        assert_eq!(
            std::fs::read(saved.backup_directory.join(path.file_name().unwrap())).unwrap(),
            before
        );
        region_backups.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            saved.backup_directory,
        ));
    }
    let backups_before = std::fs::read_dir(world.join(".minecraft-native-backups"))
        .unwrap()
        .count();
    let error = guard
        .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 2, 0)
        .unwrap_err();
    assert!(format!("{error:#}").contains("external storage"));
    assert_eq!(
        std::fs::read_dir(world.join(".minecraft-native-backups"))
            .unwrap()
            .count(),
        backups_before
    );
    assert_eq!(std::fs::read(&external).unwrap(), external_before);
    drop(guard);
    let reader =
        WorldReader::open_for_session(&world, DimensionId::parse("overworld").unwrap()).unwrap();
    let shared = reader.session_guard().unwrap();
    drop(reader);
    for mode in ["locked", "tryCreate"] {
        assert!(
            run(Command::new(java.join("java.exe"))
                .current_dir(temp.path())
                .arg("-classpath")
                .arg(&cp)
                .arg("RegionWriteOracle")
                .arg(mode)
                .arg(&world))
            .lines()
            .any(|line| line == "LOCKED"),
            "Java could open a world held by the session guard"
        );
    }
    drop(shared);
    assert!(
        run(Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&cp)
            .arg("RegionWriteOracle")
            .arg("tryCreate")
            .arg(&world))
        .lines()
        .any(|line| line == "ACQUIRED")
    );
    assert!(
        run(Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&cp)
            .arg("RegionWriteOracle")
            .arg("verify")
            .arg(&world))
        .lines()
        .any(|line| line == "OK")
    );
    let recovered = temp.path().join("recovered");
    let recovered_region = recovered.join("dimensions/minecraft/overworld/region");
    std::fs::create_dir_all(&recovered_region).unwrap();
    for (name, directory) in &region_backups {
        std::fs::copy(directory.join(name), recovered_region.join(name)).unwrap();
    }
    std::fs::copy(
        region_backups[0].1.join("c.2.0.mcc"),
        recovered_region.join("c.2.0.mcc"),
    )
    .unwrap();
    assert!(
        run(Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&cp)
            .arg("RegionWriteOracle")
            .arg("verify")
            .arg(&recovered))
        .lines()
        .any(|line| line == "OK")
    );

    let mut holder = Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("RegionWriteOracle")
        .arg("hold")
        .arg(&world)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(holder.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            output.read_line(&mut line).unwrap() != 0,
            "Java lock holder exited unexpectedly"
        );
        if line.trim() == "LOCKED" {
            break;
        }
    }
    assert!(WorldWriteGuard::acquire(&world).is_err());
    assert!(
        WorldReader::open_for_session(&world, DimensionId::parse("overworld").unwrap()).is_err()
    );
    holder.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(holder.wait().unwrap().success());
    assert!(WorldWriteGuard::acquire(&world).is_ok());
    assert!(
        WorldReader::open_for_session(&world, DimensionId::parse("overworld").unwrap()).is_ok()
    );
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn persisted_chunk_draft_is_readable_by_java_26_3() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("java-chunk-edit-")
        .tempdir_in(root)
        .unwrap();
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    let source = temp.path().join("RegionWriteOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));

    let world = temp.path().join("world");
    std::fs::create_dir(&world).unwrap();
    assert!(
        run(Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&cp)
            .arg("RegionWriteOracle")
            .arg("seed")
            .arg(&world))
        .lines()
        .any(|line| line == "OK")
    );

    let region_path = world.join("dimensions/minecraft/overworld/region/r.0.0.mca");
    let original = RegionReader::open(&region_path)
        .unwrap()
        .read_chunk(0, 0)
        .unwrap()
        .unwrap();
    let change = BlockChange {
        x: 0,
        y: 0,
        z: 0,
        expected: state("minecraft:stone"),
        replacement: state("minecraft:dirt"),
    };
    let mut draft = draft_chunk_blocks(&original, ChunkPos { x: 0, z: 0 }, &[change]).unwrap();
    draft
        .rebuild_heightmaps(
            HeightBounds {
                min_y: 0,
                max_y: 16,
            },
            |state| {
                let air = state.name.matches("minecraft:air");
                Ok(BlockHeightFlags {
                    is_air: air,
                    blocks_motion: !air,
                    blocks_motion_no_leaves: !air,
                    has_fluid: false,
                })
            },
        )
        .unwrap();

    let dimension = DimensionId::parse("overworld").unwrap();
    let mut plan = WorldSavePlan::new();
    plan.add_chunk(dimension.clone(), draft, 1_777_777).unwrap();
    let mut writer = WorldWriteGuard::acquire(&world).unwrap();
    writer
        .write_plan(plan, |planned_dimension, change| {
            anyhow::ensure!(planned_dimension == &dimension);
            anyhow::ensure!(
                change.expected == state("minecraft:stone")
                    && change.replacement == state("minecraft:dirt"),
                "unexpected block edit"
            );
            Ok(())
        })
        .unwrap();
    drop(writer);

    assert!(
        run(Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&cp)
            .arg("RegionWriteOracle")
            .arg("verifyEdit")
            .arg(&world))
        .lines()
        .any(|line| line == "OK")
    );
}
