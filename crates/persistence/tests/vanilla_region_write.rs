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
    edit::{BlockChange, BlockEntityChange, draft_chunk_blocks},
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
        } else if (args[0].equals("seedBlockEntity")) {
            net.minecraft.server.Bootstrap.bootStrap();
            var lookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
            var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
            for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
            Files.createDirectories(dir); var level=new CompoundTag(); var data=new CompoundTag(); data.putInt("DataVersion",5023); level.put("Data",data); NbtIo.writeCompressed(level,world.resolve("level.dat"));
            var chunk=chunk(0); var section=chunk.getListOrEmpty("sections").getCompoundOrEmpty(0); var states=section.getCompoundOrEmpty("block_states");
            states.getListOrEmpty("palette").add(StringTag.valueOf("minecraft:chest")); long[] words=new long[256]; words[0]=1; states.putLongArray("data",words);
            var chest=new net.minecraft.world.level.block.entity.ChestBlockEntity(new net.minecraft.core.BlockPos(0,0,0),net.minecraft.world.level.block.Blocks.CHEST.defaultBlockState());
            var state=chest.getBlockState();
            if(state.getValue(net.minecraft.world.level.block.ChestBlock.FACING)!=net.minecraft.core.Direction.NORTH || state.getValue(net.minecraft.world.level.block.ChestBlock.TYPE)!=net.minecraft.world.level.block.state.properties.ChestType.SINGLE || state.getValue(net.minecraft.world.level.block.ChestBlock.WATERLOGGED)) throw new AssertionError("unexpected default chest state");
            chest.setItem(0,new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.DIAMOND,3));
            var blockEntity=chest.saveWithFullMetadata(registries); blockEntity.putLong("future_data",123); var list=new ListTag(); list.add(blockEntity); chunk.put("block_entities",list);
            try(var region=open(dir); var out=region.getChunkDataOutputStream(new ChunkPos(0,0))) { NbtIo.write(chunk,(DataOutput)out); }
            System.out.println("OK");
        } else if (args[0].equals("verifyChestDraft")) {
            net.minecraft.server.Bootstrap.bootStrap();
            var lookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup(); var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
            for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
            try(var input=new DataInputStream(Files.newInputStream(world.resolve("chest-draft.nbt")))) {
                var tag=NbtIo.read((DataInput)input); var block=net.minecraft.world.level.block.entity.BlockEntity.loadStatic(new net.minecraft.core.BlockPos(0,0,0),net.minecraft.world.level.block.Blocks.CHEST.defaultBlockState(),tag,registries);
                if(!(block instanceof net.minecraft.world.level.block.entity.ChestBlockEntity chest) || !chest.getItem(0).isEmpty() || !chest.getItem(26).is(net.minecraft.world.item.Items.DIAMOND) || chest.getItem(26).getCount()!=3 || tag.getLongOr("future_data",0)!=123) throw new AssertionError("draft chest contents");
            }
            System.out.println("OK");
        } else if (args[0].equals("verifyBlockEntity")) {
            net.minecraft.server.Bootstrap.bootStrap();
            var lookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup(); var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
            for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
            try(var region=open(dir); var in=region.getChunkDataInputStream(new ChunkPos(0,0))) {
                var chunk=NbtIo.read((DataInput)in); var list=chunk.getListOrEmpty("block_entities"); if(list.size()!=1) throw new AssertionError("block entity count");
                var tag=list.getCompoundOrEmpty(0); var block=net.minecraft.world.level.block.entity.BlockEntity.loadStatic(new net.minecraft.core.BlockPos(0,0,0),net.minecraft.world.level.block.Blocks.CHEST.defaultBlockState(),tag,registries);
                if(!(block instanceof net.minecraft.world.level.block.entity.ChestBlockEntity chest) || !chest.getItem(0).is(net.minecraft.world.item.Items.DIAMOND) || chest.getItem(0).getCount()!=3 || tag.getLongOr("future_data",0)!=123) throw new AssertionError("chest contents");
            }
            System.out.println("OK");
        } else if (args[0].equals("seedItemMerge")) {
            Files.createDirectories(dir);
            Path entities=world.resolve("dimensions/minecraft/overworld/entities"); Files.createDirectories(entities);
            var level=new CompoundTag(); var data=new CompoundTag(); data.putInt("DataVersion",5023); level.put("Data",data); NbtIo.writeCompressed(level,world.resolve("level.dat"));
            var list=new ListTag();
            for (int i=0;i<2;i++) {
                var item=new CompoundTag(); item.putString("id","minecraft:item"); item.putIntArray("UUID", i==0 ? new int[]{1,2,3,4} : new int[]{5,6,7,8}); item.putLong("future",i+10);
                var pos=new ListTag(); pos.add(DoubleTag.valueOf(1.5)); pos.add(DoubleTag.valueOf(2)); pos.add(DoubleTag.valueOf(3)); item.put("Pos",pos);
                var stack=new CompoundTag(); stack.putString("id","minecraft:stone"); stack.putInt("count",i+1); item.put("Item",stack); list.add(item);
            }
            var chunk=new CompoundTag(); chunk.putInt("DataVersion",5023); chunk.putIntArray("Position",new int[]{0,0}); chunk.putLong("future_chunk",17); chunk.put("Entities",list);
            try (var region=open(entities); var out=region.getChunkDataOutputStream(new ChunkPos(0,0))) { NbtIo.write(chunk,(DataOutput)out); }
            System.out.println("OK");
        } else if (args[0].equals("verifyItemMerge")) {
            Path entities=world.resolve("dimensions/minecraft/overworld/entities");
            try (var region=open(entities); var in=region.getChunkDataInputStream(new ChunkPos(0,0))) {
                var chunk=NbtIo.read((DataInput)in); var list=chunk.getListOrEmpty("Entities"); var item=list.getCompoundOrEmpty(0);
                if (chunk.getLongOr("future_chunk",0)!=17 || list.size()!=1 || !Arrays.equals(item.getIntArray("UUID").orElseThrow(),new int[]{5,6,7,8}) || item.getLongOr("future",0)!=11 || item.getCompoundOrEmpty("Item").getIntOr("count",0)!=3) throw new AssertionError("item merge save");
            }
            System.out.println("OK");
        } else if (args[0].equals("seedItemRemoval")) {
            Files.createDirectories(dir);
            Path entities=world.resolve("dimensions/minecraft/overworld/entities"); Files.createDirectories(entities);
            var level=new CompoundTag(); var data=new CompoundTag(); data.putInt("DataVersion",5023); level.put("Data",data); NbtIo.writeCompressed(level,world.resolve("level.dat"));
            var item=new CompoundTag(); item.putString("id","minecraft:item"); item.putIntArray("UUID",new int[]{1,2,3,4});
            var pos=new ListTag(); pos.add(DoubleTag.valueOf(1.5)); pos.add(DoubleTag.valueOf(2)); pos.add(DoubleTag.valueOf(3)); item.put("Pos",pos);
            var stack=new CompoundTag(); stack.putString("id","minecraft:stone"); stack.putInt("count",3); item.put("Item",stack);
            var pig=new CompoundTag(); pig.putString("id","minecraft:pig"); pig.putIntArray("UUID",new int[]{5,6,7,8}); pig.putLong("future",42);
            var chunk=new CompoundTag(); chunk.putInt("DataVersion",5023); chunk.putIntArray("Position",new int[]{0,0}); chunk.putLong("future_chunk",17);
            var list=new ListTag(); list.add(item); list.add(pig); chunk.put("Entities",list);
            try (var region=open(entities); var out=region.getChunkDataOutputStream(new ChunkPos(0,0))) { NbtIo.write(chunk,(DataOutput)out); }
            System.out.println("OK");
        } else if (args[0].equals("verifyItemRemoval")) {
            Path entities=world.resolve("dimensions/minecraft/overworld/entities");
            try (var region=open(entities); var in=region.getChunkDataInputStream(new ChunkPos(0,0))) {
                var chunk=NbtIo.read((DataInput)in); var list=chunk.getListOrEmpty("Entities"); var pig=list.getCompoundOrEmpty(0);
                if (chunk.getLongOr("future_chunk",0)!=17 || list.size()!=1 || !pig.getStringOr("id","").equals("minecraft:pig") || pig.getLongOr("future",0)!=42) throw new AssertionError("item removal or retained entity");
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
        } else if (args[0].equals("verifyNewItem")) {
            Path entities = world.resolve("dimensions/minecraft/overworld/entities");
            try (var region = open(entities); var in = region.getChunkDataInputStream(new ChunkPos(0,0))) {
                var chunk=NbtIo.read((DataInput)in); var list=chunk.getListOrEmpty("Entities"); var item=list.getCompoundOrEmpty(0);
                if (list.size()!=1 || !item.getStringOr("id","").equals("minecraft:item") || !Arrays.equals(item.getIntArray("UUID").orElseThrow(), new int[]{1,2,3,4}) || item.getCompoundOrEmpty("Item").getIntOr("count",0)!=3 || item.getShortOr("PickupDelay",(short)0)!=10) throw new AssertionError("new item");
            }
            System.out.println("OK");
        } else if (args[0].equals("verifyCreatedEntityDirectory")) {
            Path entities = world.resolve("dimensions/minecraft/overworld/entities");
            if (!Files.isRegularFile(entities.resolve("r.0.0.mca"))) throw new AssertionError("missing created region");
            try (var region = open(entities); var in = region.getChunkDataInputStream(new ChunkPos(0,0))) {
                var chunk = NbtIo.read((DataInput)in);
                var list = chunk.getListOrEmpty("Entities");
                var entity = list.getCompoundOrEmpty(0);
                if (chunk.getLongOr("future_chunk",0)!=17 || list.size()!=1 || entity.getLongOr("future",0)!=42 || entity.getListOrEmpty("Pos").getDouble(0).orElseThrow()!=1.5) throw new AssertionError("created entity data");
            }
            System.out.println("OK");
        } else if (args[0].equals("verifyEntity") || args[0].equals("verifyEntityNewRegion")) {
            Path entities = world.resolve("dimensions/minecraft/overworld/entities");
            boolean newRegion=args[0].equals("verifyEntityNewRegion");
            try (var region = open(entities)) {
                try (var in = region.getChunkDataInputStream(new ChunkPos(0,0))) {
                    var source = NbtIo.read((DataInput)in);
                    if (source.getLongOr("future_chunk",0) != 17 || !source.getListOrEmpty("Entities").isEmpty()) throw new AssertionError("source chunk was not preserved");
                }
            }
            int chunkX=newRegion ? 32 : 1;
            if (!Files.isRegularFile(entities.resolve("r." + (newRegion ? 1 : 0) + ".0.mca"))) throw new AssertionError("missing region");
            try (var region = open(entities, newRegion ? 1 : 0, RegionFileVersion.VERSION_NONE)) {
                try (var in = region.getChunkDataInputStream(new ChunkPos(chunkX,0))) {
                    var destination = NbtIo.read((DataInput)in);
                    if (!Arrays.equals(destination.getIntArray("Position").orElseThrow(), new int[]{chunkX,0})) throw new AssertionError("position");
                    var list=destination.getListOrEmpty("Entities"); var entity=list.getCompoundOrEmpty(0);
                    if (list.size()!=1 || !entity.getStringOr("id", "").equals("minecraft:falling_block") || entity.getLongOr("future",0)!=42 || entity.getListOrEmpty("Pos").getDouble(0).orElseThrow()!=(newRegion ? 513.5 : 17.5)) throw new AssertionError("entity data");
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
fn block_entity_blocks_unsafe_replacement_and_survives_neighbor_edit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("java-block-entity-")
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
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
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
    invoke("seedBlockEntity");
    let path = world.join("dimensions/minecraft/overworld/region/r.0.0.mca");
    let original_bytes = std::fs::read(&path).unwrap();
    let original = RegionReader::open(&path)
        .unwrap()
        .read_chunk(0, 0)
        .unwrap()
        .unwrap();
    let mut reader = WorldReader::open(&world, DimensionId::parse("overworld").unwrap()).unwrap();
    let area = reader.read_area(ChunkPos { x: 0, z: 0 }, 0).unwrap();
    let defaults = std::collections::BTreeMap::from([(
        "minecraft:chest".into(),
        std::collections::BTreeMap::from([
            ("facing".into(), "north".into()),
            ("type".into(), "single".into()),
            ("waterlogged".into(), "false".into()),
        ]),
    )]);
    let runtime = area
        .materialize_with_defaults(&DimensionId::parse("overworld").unwrap(), &defaults)
        .unwrap();
    drop(reader);
    let player = native_session::PlayerState {
        position: glam::Vec3::new(0.5, 1.0, 0.5),
        yaw: 0.0,
        pitch: 0.0,
        vertical_velocity: 0.0,
        on_ground: true,
        pose: native_session::PlayerPose::Standing,
    };
    let session = native_session::loaded::LoadedSession::new(
        runtime,
        glam::IVec3::ZERO,
        native_session::loaded::BuildBounds {
            min_y: 0,
            max_y: 16,
        },
        player,
    )
    .unwrap();
    let chest = session.saved_chest(glam::IVec3::ZERO).unwrap().unwrap();
    assert_eq!(
        (
            chest.slots[0].as_ref().unwrap().id(),
            chest.slots[0].as_ref().unwrap().count()
        ),
        ("minecraft:diamond", 3)
    );
    assert_eq!(
        chest.source().get("future_data"),
        Some(&nbt::Tag::Long(123))
    );
    let mut slots = chest.slots.clone();
    slots[26] = slots[0].take();
    let entity = chest.draft_inventory(&slots).unwrap();
    let mut chunk_draft = draft_chunk_blocks(&original, ChunkPos { x: 0, z: 0 }, &[]).unwrap();
    chunk_draft
        .replace_block_entities(&[BlockEntityChange {
            expected: chest.source().clone(),
            replacement: entity,
        }])
        .unwrap();
    let Some(nbt::Tag::List { values, .. }) = chunk_draft.document().value.get("block_entities")
    else {
        panic!("missing block entities")
    };
    let draft = nbt::NamedTag {
        name: "".into(),
        value: values[0].clone(),
    };
    std::fs::write(world.join("chest-draft.nbt"), nbt::write(&draft).unwrap()).unwrap();
    invoke("verifyChestDraft");
    let mut staged_plan = WorldSavePlan::new();
    staged_plan
        .add_chunk(DimensionId::parse("overworld").unwrap(), chunk_draft, 1)
        .unwrap();
    let mut staged_writer = WorldWriteGuard::acquire(&world).unwrap();
    let error = staged_writer
        .write_plan(staged_plan, |_, _| Ok(()))
        .unwrap_err();
    assert!(format!("{error:#}").contains("block entity edits require a coordinated world save"));
    drop(staged_writer);
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
    let prepare = |x, expected, replacement| {
        let change = BlockChange {
            x,
            y: 0,
            z: 0,
            expected: state(expected),
            replacement: state(replacement),
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
        draft
    };
    let dim = DimensionId::parse("overworld").unwrap();
    let mut plan = WorldSavePlan::new();
    plan.add_chunk(
        dim.clone(),
        prepare(0, "minecraft:chest", "minecraft:air"),
        1,
    )
    .unwrap();
    let mut writer = WorldWriteGuard::acquire(&world).unwrap();
    let error = writer.write_plan(plan, |_, _| Ok(())).unwrap_err();
    assert!(format!("{error:#}").contains("uncoordinated block entity"));
    drop(writer);
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
    assert!(!world.join(".minecraft-native-pending").exists());
    let mut plan = WorldSavePlan::new();
    plan.add_chunk(dim, prepare(1, "minecraft:stone", "minecraft:dirt"), 2)
        .unwrap();
    WorldWriteGuard::acquire(&world)
        .unwrap()
        .write_plan(plan, |_, _| Ok(()))
        .unwrap();
    invoke("verifyBlockEntity");
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn entity_chunk_drafts_commit_and_load_in_java() {
    exercise_entity_chunk_drafts(false);
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn entity_chunk_drafts_create_region_readable_by_java() {
    exercise_entity_chunk_drafts(true);
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn created_entity_directory_and_region_are_readable_by_java() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("java-entity-dir-")
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
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
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
    let directory = world.join("dimensions/minecraft/overworld/entities");
    let region = directory.join("r.0.0.mca");
    let document = RegionReader::open(&region)
        .unwrap()
        .read_chunk(0, 0)
        .unwrap()
        .unwrap();
    std::fs::remove_file(&region).unwrap();
    std::fs::remove_dir(&directory).unwrap();
    let mut plan = WorldSavePlan::new();
    plan.add_entity_chunk(
        DimensionId::parse("overworld").unwrap(),
        ChunkPos { x: 0, z: 0 },
        None,
        &document,
        100,
        |_, _| Ok(()),
    )
    .unwrap();
    let mut guard = WorldWriteGuard::acquire(&world).unwrap();
    guard.write_plan(plan, |_, _| Ok(())).unwrap();
    drop(guard);
    assert!(
        invoke("verifyCreatedEntityDirectory")
            .lines()
            .any(|line| line == "OK")
    );
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn new_item_draft_is_readable_from_java_region() {
    use glam::DVec3;
    use native_persistence::nbt::Tag;
    use native_session::{inventory::ItemStack, saved_falling_block, saved_item};
    use native_world::loaded::ChunkPos as RuntimeChunkPos;
    use std::collections::BTreeMap;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("java-new-item-")
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
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
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
    let directory = world.join("dimensions/minecraft/overworld/entities");
    std::fs::remove_file(directory.join("r.0.0.mca")).unwrap();
    let stack = ItemStack::new("minecraft:stone".into(), 3, 64, Tag::Compound(vec![])).unwrap();
    let created = saved_item::draft_created(
        [1, 2, 3, 4],
        None,
        None,
        saved_item::CreatedItemBase {
            rotation: [0.0, 0.0],
            fall_distance: 0.0,
            air: 300,
            portal_cooldown: 0,
        },
        saved_item::ItemSaveState {
            position: DVec3::new(1.5, 2.0, 3.0),
            velocity: DVec3::ZERO,
            on_ground: false,
            no_gravity: false,
            age: 0,
            pickup_delay: 10,
            health: 5,
            fire_ticks: 0,
            invulnerable: false,
            invulnerable_time: 0,
            stack: &stack,
        },
    )
    .unwrap();
    let pos = RuntimeChunkPos { x: 0, z: 0 };
    let drafts = saved_falling_block::draft_entity_chunks_with_item_changes(
        &BTreeMap::from([(pos, None)]),
        &[],
        &[],
        &[created],
    )
    .unwrap();
    let mut plan = WorldSavePlan::new();
    drafts[&pos]
        .add_to_save_plan(
            &mut plan,
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 0, z: 0 },
            None,
            101,
        )
        .unwrap();
    let mut guard = WorldWriteGuard::acquire(&world).unwrap();
    guard.write_plan(plan, |_, _| Ok(())).unwrap();
    drop(guard);
    assert!(invoke("verifyNewItem").lines().any(|line| line == "OK"));
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn removed_saved_item_is_absent_when_java_reads_entity_region() {
    use native_persistence::nbt::{NamedTag, Tag};
    use native_session::{saved_falling_block, saved_item};
    use native_world::loaded::ChunkPos as RuntimeChunkPos;
    use std::collections::BTreeMap;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("java-item-removal-")
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
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
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
    assert!(invoke("seedItemRemoval").lines().any(|line| line == "OK"));
    let path = world.join("dimensions/minecraft/overworld/entities/r.0.0.mca");
    let original = RegionReader::open(&path)
        .unwrap()
        .read_chunk(0, 0)
        .unwrap()
        .unwrap();
    let Some(Tag::List { values, .. }) = original.value.get("Entities") else {
        panic!()
    };
    let entity = NamedTag {
        name: "".into(),
        value: values[0].clone(),
    };
    let saved = saved_item::read(&entity.value).unwrap().unwrap();
    let removal = saved_item::draft(
        &entity,
        saved_item::ItemSaveState {
            position: saved.position,
            velocity: saved.velocity,
            on_ground: saved.on_ground,
            no_gravity: saved.no_gravity,
            age: saved.age,
            pickup_delay: saved.pickup_delay,
            health: saved.health,
            fire_ticks: saved.fire_ticks,
            invulnerable: saved.invulnerable,
            invulnerable_time: saved.invulnerable_time,
            stack: &saved.stack,
        },
    )
    .unwrap();
    let pos = RuntimeChunkPos { x: 0, z: 0 };
    let drafts = saved_falling_block::draft_entity_chunks_with_removals(
        &BTreeMap::from([(pos, Some(&original))]),
        &[],
        &[removal],
    )
    .unwrap();
    let mut plan = WorldSavePlan::new();
    drafts[&pos]
        .add_to_save_plan(
            &mut plan,
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 0, z: 0 },
            Some(&original),
            101,
        )
        .unwrap();
    let mut guard = WorldWriteGuard::acquire(&world).unwrap();
    guard.write_plan(plan, |_, _| Ok(())).unwrap();
    drop(guard);
    assert!(invoke("verifyItemRemoval").lines().any(|line| line == "OK"));
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn merged_saved_items_round_trip_through_java_region() {
    use native_persistence::nbt::{NamedTag, Tag};
    use native_session::{
        inventory::ItemStack,
        saved_falling_block::{self, EntityReplacement},
        saved_item,
    };
    use native_world::loaded::ChunkPos as RuntimeChunkPos;
    use std::collections::BTreeMap;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&root).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("java-item-merge-")
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
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
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
    assert!(invoke("seedItemMerge").lines().any(|line| line == "OK"));
    let path = world.join("dimensions/minecraft/overworld/entities/r.0.0.mca");
    let original = RegionReader::open(&path)
        .unwrap()
        .read_chunk(0, 0)
        .unwrap()
        .unwrap();
    let Some(Tag::List { values, .. }) = original.value.get("Entities") else {
        panic!()
    };
    let draft = |index: usize, count: u32| {
        let entity = NamedTag {
            name: "".into(),
            value: values[index].clone(),
        };
        let saved = saved_item::read(&entity.value).unwrap().unwrap();
        let stack = ItemStack::new(
            saved.stack.id().into(),
            count,
            64,
            saved.stack.components().clone(),
        )
        .unwrap();
        saved_item::draft(
            &entity,
            saved_item::ItemSaveState {
                position: saved.position,
                velocity: saved.velocity,
                on_ground: saved.on_ground,
                no_gravity: saved.no_gravity,
                age: saved.age,
                pickup_delay: saved.pickup_delay,
                health: saved.health,
                fire_ticks: saved.fire_ticks,
                invulnerable: saved.invulnerable,
                invulnerable_time: saved.invulnerable_time,
                stack: &stack,
            },
        )
        .unwrap()
    };
    let removal = draft(0, 1);
    let survivor = draft(1, 3);
    let pos = RuntimeChunkPos { x: 0, z: 0 };
    let drafts = saved_falling_block::draft_entity_chunks_with_removals(
        &BTreeMap::from([(pos, Some(&original))]),
        &[EntityReplacement::Item(&survivor)],
        &[removal],
    )
    .unwrap();
    let mut plan = WorldSavePlan::new();
    drafts[&pos]
        .add_to_save_plan(
            &mut plan,
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 0, z: 0 },
            Some(&original),
            101,
        )
        .unwrap();
    let mut guard = WorldWriteGuard::acquire(&world).unwrap();
    guard.write_plan(plan, |_, _| Ok(())).unwrap();
    drop(guard);
    assert!(invoke("verifyItemMerge").lines().any(|line| line == "OK"));
}

fn exercise_entity_chunk_drafts(new_region: bool) {
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
            position: DVec3::new(if new_region { 513.5 } else { 17.5 }, 2.0, 3.0),
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
    let right = RuntimeChunkPos {
        x: if new_region { 32 } else { 1 },
        z: 0,
    };
    let empty = RuntimeChunkPos { x: 64, z: 0 };
    let drafts = saved_falling_block::draft_entity_chunks_with_optional_sources(
        &BTreeMap::from([(left, Some(&original)), (right, None), (empty, None)]),
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
        .add_to_save_plan(
            &mut plan,
            dimension,
            ChunkPos { x: right.x, z: 0 },
            None,
            101,
        )
        .unwrap();
    drafts[&empty]
        .add_to_save_plan(
            &mut plan,
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 64, z: 0 },
            None,
            102,
        )
        .unwrap();
    let mut guard = WorldWriteGuard::acquire(&world).unwrap();
    guard.write_plan(plan, |_, _| Ok(())).unwrap();
    drop(guard);
    assert!(
        !world
            .join("dimensions/minecraft/overworld/entities/r.2.0.mca")
            .exists()
    );
    assert!(
        invoke(if new_region {
            "verifyEntityNewRegion"
        } else {
            "verifyEntity"
        })
        .lines()
        .any(|line| line == "OK")
    );
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
