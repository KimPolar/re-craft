//! Opt-in differential test. Minecraft libraries remain in the user's installation.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_persistence::{
    anvil,
    nbt::{self, JavaString, NamedTag, Tag},
    palette,
    world::{
        ChunkPos, DimensionId, PlayerRespawnConfig, RespawnData, SavedDimensionType, WorldReader,
        read_player_respawn,
    },
};
use native_session::{
    GameSession, PlayerPose, PlayerState,
    loaded::{BuildBounds, LoadedSession},
};

const ORACLE: &str = r#"
import java.io.*;
import java.util.Base64;
import java.util.Arrays;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Direction;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.nbt.*;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.*;
import net.minecraft.util.SimpleBitStorage;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.ticks.SavedTick;
import net.minecraft.world.ticks.TickPriority;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.gamerules.GameRuleMap;
import net.minecraft.world.level.LevelSettings;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.storage.LevelData;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.WorldDimensions;
import net.minecraft.world.level.levelgen.WorldGenSettings;
import net.minecraft.world.level.levelgen.WorldOptions;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.Registry;
import com.mojang.serialization.Lifecycle;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.Difficulty;
import net.minecraft.world.clock.ClockState;
import net.minecraft.world.clock.PackedClockStates;
import net.minecraft.world.clock.ServerClockManager;

public class NbtOracle {
    static String sampleString() { return new String(new char[] { 'A', 0, (char)0xD800, (char)0xD83D, (char)0xDE00 }); }
    static long[] randomSamples(boolean xoroshiro) {
        long[] seeds = { 0, -1, Long.MIN_VALUE, Long.MAX_VALUE, 123456789012345L };
        int[] bounds = { 1, 2, 3, 7, 16, 17, 1073741825, Integer.MAX_VALUE };
        long[] result = new long[seeds.length * 64 * 6];
        int at = 0;
        for (long seed : seeds) {
            RandomSource random = xoroshiro ? new XoroshiroRandomSource(seed) : new LegacyRandomSource(seed);
            for (int i = 0; i < 64; i++) {
                result[at++] = random.nextInt();
                result[at++] = random.nextInt(bounds[i % bounds.length]);
                result[at++] = random.nextLong();
                result[at++] = Float.floatToRawIntBits(random.nextFloat());
                result[at++] = Double.doubleToRawLongBits(random.nextDouble());
                result[at++] = random.nextBoolean() ? 1 : 0;
            }
        }
        return result;
    }
    public static void main(String[] args) throws Exception {
        if (args[0].equals("write")) {
            CompoundTag root = new CompoundTag();
            root.putInt("number", -42);
            root.putString("text", sampleString());
            root.putByteArray("bytes", new byte[] {0, -128, -1});
            root.putLongArray("longs", new long[] {Long.MIN_VALUE, 0, Long.MAX_VALUE});
            CompoundTag child = new CompoundTag();
            child.putDouble("value", -42.5);
            root.put("nested", child);
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            NbtIo.write(root, (DataOutput) new DataOutputStream(bytes));
            System.out.println(Base64.getEncoder().encodeToString(bytes.toByteArray()));
        } else if (args[0].equals("random")) {
            CompoundTag root = new CompoundTag();
            root.putLongArray("legacy", randomSamples(false));
            root.putLongArray("xoroshiro", randomSamples(true));
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            NbtIo.write(root, (DataOutput) new DataOutputStream(bytes));
            System.out.println(Base64.getEncoder().encodeToString(bytes.toByteArray()));
        } else if (args[0].equals("world")) {
            PrintStream output = System.out;
            SharedConstants.tryDetectVersion();
            Bootstrap.bootStrap();
            String[] blockNames = {"stone", "oak_log", "white_wool", "orange_wool", "magenta_wool",
                "light_blue_wool", "yellow_wool", "lime_wool", "pink_wool", "gray_wool",
                "light_gray_wool", "cyan_wool", "purple_wool", "blue_wool", "brown_wool", "green_wool", "red_wool"};
            BlockState[] blockValues = new BlockState[blockNames.length];
            for (int j = 0; j < blockNames.length; j++) blockValues[j] = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + blockNames[j])).defaultBlockState();
            blockValues[1] = blockValues[1].setValue(BlockStateProperties.AXIS, Direction.Axis.X);
            var codec = PalettedContainer.codecRW(BlockState.CODEC, Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState());
            Path world = Path.of(args[1]);
            Path regions = world.resolve("dimensions/minecraft/overworld/region");
            Files.createDirectories(regions);
            CompoundTag level = new CompoundTag();
            CompoundTag data = new CompoundTag();
            data.putInt("DataVersion", 5023);
            data.putIntArray("singleplayer_uuid", new int[] {0x00112233, 0x44556677, 0x8899aabb, 0xccddeeff});
            data.put("spawn", LevelData.RespawnData.CODEC.encodeStart(NbtOps.INSTANCE,
                LevelData.RespawnData.of(Level.OVERWORLD, new BlockPos(-123, 70, 456), 45.5f, -12.25f)).getOrThrow());
            data.put("difficulty_settings", LevelSettings.DifficultySettings.CODEC.encodeStart(NbtOps.INSTANCE,
                new LevelSettings.DifficultySettings(Difficulty.HARD, true, true)).getOrThrow());
            level.put("Data", data);
            NbtIo.writeCompressed(level, world.resolve("level.dat"));
            var lookup = VanillaRegistries.createWorldLookup();
            var dimensions = new java.util.HashMap<>(WorldPresets.createNormalWorldDimensions(lookup).dimensions());
            var overworldStem = dimensions.get(LevelStem.OVERWORLD);
            var netherStem = dimensions.get(LevelStem.NETHER);
            dimensions.put(LevelStem.OVERWORLD, new LevelStem(netherStem.type(), overworldStem.generator()));
            var custom = ResourceKey.create(Registries.LEVEL_STEM, Identifier.parse("fixture:custom"));
            var endStem = dimensions.get(LevelStem.END);
            dimensions.put(custom, new LevelStem(endStem.type(),endStem.generator()));
            var generation = new WorldGenSettings(new WorldOptions(12345L,true,false),new WorldDimensions(dimensions));
            var packDimensions = new MappedRegistry<LevelStem>(Registries.LEVEL_STEM,Lifecycle.stable());
            Registry.register(packDimensions,LevelStem.OVERWORLD,overworldStem);
            var baked = generation.dimensions().bake(packDimensions.freeze()).dimensions();
            if (baked.getValue(LevelStem.OVERWORLD).type() != overworldStem.type())
                throw new AssertionError("active dimension registry did not override saved world stem");
            var generationFile = new CompoundTag();
            generationFile.putInt("DataVersion",5023);
            generationFile.put("data", WorldGenSettings.CODEC.encodeStart(lookup.createSerializationContext(NbtOps.INSTANCE),generation).getOrThrow());
            Path sharedData = world.resolve("data/minecraft"); Files.createDirectories(sharedData);
            NbtIo.writeCompressed(generationFile,sharedData.resolve("world_gen_settings.dat"));
            var clockLookup = lookup.lookupOrThrow(Registries.WORLD_CLOCK);
            var overworldClock = clockLookup.getOrThrow(ResourceKey.create(Registries.WORLD_CLOCK, Identifier.parse("minecraft:overworld")));
            var endClock = clockLookup.getOrThrow(ResourceKey.create(Registries.WORLD_CLOCK, Identifier.parse("minecraft:the_end")));
            var packedClocks = new PackedClockStates(java.util.Map.of(
                overworldClock, new ClockState(12345L, 0.25f, 0.3f, false),
                endClock, new ClockState(-100L, 0.75f, 2.5f, true)));
            var clockFile = new CompoundTag(); clockFile.putInt("DataVersion", 5023);
            clockFile.put("data", PackedClockStates.CODEC.encodeStart(lookup.createSerializationContext(NbtOps.INSTANCE), packedClocks).getOrThrow());
            NbtIo.writeCompressed(clockFile,sharedData.resolve("world_clocks.dat"));
            var clockSamples = new CompoundTag(); var clockLists = new ListTag();
            for (var initial : new ClockState[] {
                new ClockState(12345L,0.25f,0.3f,false), new ClockState(-100L,0.75f,2.5f,true),
                new ClockState(Long.MAX_VALUE,0.75f,2.5f,false), new ClockState(-4L,-0.75f,0.125f,false)
            }) {
                var clock = new ServerClockManager.ServerClockInstance(); clock.loadFrom(initial);
                var rows = new ListTag();
                for (int step = 0; step < 80; step++) {
                    clock.tick(); var row = new CompoundTag(); row.putLong("ticks", clock.totalTicks());
                    row.putFloat("partial",clock.partialTick()); rows.add(row);
                }
                var item = new CompoundTag(); item.put("rows",rows); clockLists.add(item);
            }
            clockSamples.put("samples",clockLists); NbtIo.writeCompressed(clockSamples,world.resolve("clock_samples.dat"));
            var border = new WorldBorder(new WorldBorder.Settings(123.5, -456.25, 0.4, 3.5, 7, 120, 64.0, 40, 100.0));
            border.applyInitialSettings(1000);
            CompoundTag borderFile = new CompoundTag();
            borderFile.putInt("DataVersion", 5023);
            borderFile.put("data", WorldBorder.CODEC.encodeStart(NbtOps.INSTANCE, border).getOrThrow());
            Path borderDir = DimensionType.getStorageFolder(Level.OVERWORLD, world).resolve("data/minecraft");
            Files.createDirectories(borderDir);
            NbtIo.writeCompressed(borderFile, borderDir.resolve("world_border.dat"));
            GameRuleMap rules=GameRuleMap.of();
            rules.set(GameRules.ADVANCE_TIME,false);
            rules.set(GameRules.WATER_SOURCE_CONVERSION,false);
            rules.set(GameRules.LAVA_SOURCE_CONVERSION,true);
            rules.set(GameRules.BLOCK_DROPS,false);
            rules.set(GameRules.ENTITY_DROPS,false);
            rules.set(GameRules.NATURAL_HEALTH_REGENERATION,false);
            rules.set(GameRules.FALL_DAMAGE,false);
            rules.set(GameRules.KEEP_INVENTORY,true);
            rules.set(GameRules.IMMEDIATE_RESPAWN,true);
            rules.set(GameRules.SHOW_DEATH_MESSAGES,false);
            rules.set(GameRules.RESPAWN_RADIUS,7);
            CompoundTag rulesFile=new CompoundTag();
            rulesFile.putInt("DataVersion",5023);
            rulesFile.put("data",GameRuleMap.CODEC.encodeStart(NbtOps.INSTANCE,rules).getOrThrow());
            Path rulesDir=world.resolve("data/minecraft"); Files.createDirectories(rulesDir);
            NbtIo.writeCompressed(rulesFile,rulesDir.resolve("game_rules.dat"));
            Path players = world.resolve("players/data");
            Files.createDirectories(players);
            CompoundTag player = new CompoundTag();
            player.putInt("DataVersion", 5023);
            player.putString("Dimension", "minecraft:overworld");
            player.putString("fixture_only", "preserved");
            player.put("respawn", ServerPlayer.RespawnConfig.CODEC.encodeStart(NbtOps.INSTANCE,
                new ServerPlayer.RespawnConfig(LevelData.RespawnData.of(Level.NETHER,
                    new BlockPos(56, -12, -987), -90.0f, 22.5f), true)).getOrThrow());
            NbtIo.writeCompressed(player, players.resolve("00112233-4455-6677-8899-aabbccddeeff.dat"));
            ListTag numericPosition = new ListTag();
            numericPosition.add(DoubleTag.valueOf(-1.75)); numericPosition.add(DoubleTag.valueOf(64)); numericPosition.add(DoubleTag.valueOf(2.9));
            ListTag invalidPosition = new ListTag();
            invalidPosition.add(StringTag.valueOf("x")); invalidPosition.add(StringTag.valueOf("y")); invalidPosition.add(StringTag.valueOf("z"));
            Tag[] positionCases = {
                new IntArrayTag(new int[] {-1, 64, 2}),
                new ByteArrayTag(new byte[] {-1, 64, 2}),
                new LongArrayTag(new long[] {4294967297L, -64, 2}),
                numericPosition,
                invalidPosition,
                new IntArrayTag(new int[] {1, 2}),
            };
            Path spawnCases = world.resolve("spawn-cases"); Files.createDirectories(spawnCases);
            for (int i = 0; i < positionCases.length; ++i) {
                CompoundTag sample = (CompoundTag) player.get("respawn").copy();
                sample.put("pos", positionCases[i]);
                var parsed = ServerPlayer.RespawnConfig.CODEC.parse(NbtOps.INSTANCE, sample).result();
                CompoundTag fixture = new CompoundTag(); fixture.put("respawn", sample);
                if (parsed.isPresent()) fixture.put("expected", ServerPlayer.RespawnConfig.CODEC.encodeStart(NbtOps.INSTANCE, parsed.get()).getOrThrow());
                NbtIo.writeCompressed(fixture, spawnCases.resolve(i + ".dat"));
            }
            RegionStorageInfo info = new RegionStorageInfo("parity", null, "region");
            int[] xs = { -33, -1, 0, 32 };
            RegionFileVersion[] versions = { RegionFileVersion.VERSION_GZIP, RegionFileVersion.VERSION_DEFLATE, RegionFileVersion.VERSION_NONE, RegionFileVersion.VERSION_LZ4 };
            Path entityRegions = DimensionType.getStorageFolder(Level.OVERWORLD, world).resolve("entities");
            Files.createDirectories(entityRegions);
            RegionStorageInfo entityInfo = new RegionStorageInfo("parity", null, "entities");
            for (int i = 0; i < xs.length; i++) {
                int x = xs[i];
                Path path = regions.resolve("r." + Math.floorDiv(x, 32) + ".-1.mca");
                try (RegionFile region = new RegionFile(info, path, regions, versions[i], false)) {
                    try (DataOutputStream out = region.getChunkDataOutputStream(new ChunkPos(x, -1))) {
                        CompoundTag chunk = new CompoundTag();
                        chunk.putInt("DataVersion", 5023);
                        chunk.putInt("xPos", x);
                        chunk.putInt("zPos", -1);
                        chunk.putString("Status", "minecraft:full");
                        CompoundTag section = new CompoundTag();
                        section.putByte("Y", (byte)-4);
                        var container = new PalettedContainer<>(Blocks.STONE.defaultBlockState(), Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
                        for (int j = 0; j < 4096; j++) container.set(j % 16, j / 256, (j / 16) % 16, blockValues[(j * 7) % blockValues.length]);
                        CompoundTag blockStates = (CompoundTag)codec.encodeStart(NbtOps.INSTANCE, container).getOrThrow();
                        section.put("block_states", blockStates);
                        CompoundTag biomes = new CompoundTag();
                        ListTag biomePalette = new ListTag();
                        for (int j = 0; j < 3; j++) biomePalette.add(StringTag.valueOf("fixture:biome_" + j));
                        SimpleBitStorage biomeData = new SimpleBitStorage(2, 64);
                        for (int j = 0; j < 64; j++) biomeData.set(j, j % 3);
                        biomes.put("palette", biomePalette);
                        biomes.putLongArray("data", biomeData.getRaw());
                        section.put("biomes", biomes);
                        ListTag sections = new ListTag();
                        sections.add(section);
                        CompoundTag lightOnly = new CompoundTag();
                        lightOnly.putByte("Y", (byte)-5);
                        byte[] sky = new byte[2048];
                        Arrays.fill(sky, (byte)0xff);
                        lightOnly.putByteArray("SkyLight", sky);
                        sections.add(lightOnly);
                        chunk.put("sections", sections);
                        var tickCodec = SavedTick.codec(BuiltInRegistries.FLUID.byNameCodec());
                        ListTag fluidTicks = new ListTag();
                        fluidTicks.add(tickCodec.encodeStart(NbtOps.INSTANCE, new SavedTick<>(Fluids.WATER, new BlockPos(x * 16 + 1, -63, -15), 5, TickPriority.HIGH)).getOrThrow());
                        fluidTicks.add(tickCodec.encodeStart(NbtOps.INSTANCE, new SavedTick<>(Fluids.FLOWING_LAVA, new BlockPos(x * 16 + 2, -62, -14), -2, TickPriority.LOW)).getOrThrow());
                        if (x == -1) {
                            CompoundTag mixedFluid = new CompoundTag(); mixedFluid.putString("i", "minecraft:water");
                            mixedFluid.putShort("x", (short)-10); mixedFluid.putByte("y", (byte)-58); mixedFluid.putLong("z", -10L);
                            mixedFluid.putDouble("t", 4.75); mixedFluid.putByte("p", (byte)-2);
                            var parsed = tickCodec.parse(NbtOps.INSTANCE, mixedFluid).getOrThrow();
                            if (!parsed.pos().equals(new BlockPos(-10,-58,-10)) || parsed.delay()!=4 || parsed.priority()!=TickPriority.VERY_HIGH) throw new AssertionError("numeric fluid tick decoding");
                            fluidTicks.add(mixedFluid);
                        }
                        chunk.put("fluid_ticks", fluidTicks);
                        var blockTickCodec = SavedTick.codec(BuiltInRegistries.BLOCK.byNameCodec());
                        ListTag blockTicks = new ListTag();
                        blockTicks.add(blockTickCodec.encodeStart(NbtOps.INSTANCE, new SavedTick<>(Blocks.SAND, new BlockPos(x * 16 + 3, -61, -13), 2, TickPriority.HIGH)).getOrThrow());
                        blockTicks.add(blockTickCodec.encodeStart(NbtOps.INSTANCE, new SavedTick<>(Blocks.GRAVEL, new BlockPos(x * 16 + 4, -60, -12), -4, TickPriority.LOW)).getOrThrow());
                        if (x == -1) {
                            CompoundTag mixedTick = new CompoundTag(); mixedTick.putString("i", "minecraft:sand");
                            mixedTick.putShort("x", (short)-11); mixedTick.putByte("y", (byte)-59); mixedTick.putLong("z", -11L);
                            mixedTick.putDouble("t", 3.75); mixedTick.putByte("p", (byte)2);
                            var parsed = blockTickCodec.parse(NbtOps.INSTANCE, mixedTick).getOrThrow();
                            if (!parsed.pos().equals(new BlockPos(-11,-59,-11)) || parsed.delay()!=3 || parsed.priority()!=TickPriority.VERY_LOW) throw new AssertionError("numeric tick decoding");
                            blockTicks.add(mixedTick);
                        }
                        chunk.put("block_ticks", blockTicks);
                        chunk.putString("fixture_unknown_tag", "preserved");
                        NbtIo.write(chunk, (DataOutput) out);
                    }
                }
                Path entityPath = entityRegions.resolve("r." + Math.floorDiv(x, 32) + ".-1.mca");
                try (RegionFile region = new RegionFile(entityInfo, entityPath, entityRegions, versions[i], false)) {
                    try (DataOutputStream out = region.getChunkDataOutputStream(new ChunkPos(x, -1))) {
                        CompoundTag entityChunk = new CompoundTag();
                        entityChunk.putInt("DataVersion", 5023);
                        entityChunk.put("Position", ChunkPos.CODEC.encodeStart(NbtOps.INSTANCE, new ChunkPos(x, -1)).getOrThrow());
                        ListTag entities = new ListTag();
                        CompoundTag pig = new CompoundTag(); pig.putString("id", "minecraft:pig");
                        pig.putString("fixture_unknown", "preserved"); entities.add(pig);
                        entityChunk.put("Entities", entities);
                        entityChunk.putString("fixture_extra", "preserved");
                        NbtIo.write(entityChunk, (DataOutput) out);
                    }
                }
            }
            output.println("OK");
        } else {
            byte[] bytes = Base64.getDecoder().decode(args[1]);
            CompoundTag root = NbtIo.read((DataInput) new DataInputStream(new ByteArrayInputStream(bytes)));
            if (root.getIntOr("number", 0) != -42 || !root.getStringOr("text", "").equals(sampleString())
                || !Arrays.equals(root.getByteArray("bytes").orElseThrow(), new byte[] {0, -128, -1})
                || !Arrays.equals(root.getLongArray("longs").orElseThrow(), new long[] {Long.MIN_VALUE, 0, Long.MAX_VALUE})
                || root.getCompoundOrEmpty("nested").getDoubleOr("value", 0.0) != -42.5) {
                throw new AssertionError("Rust NBT did not preserve the expected values");
            }
            System.out.println("OK");
        }
    }
}
"#;

fn minecraft_dir() -> PathBuf {
    std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("APPDATA").expect("APPDATA or MINECRAFT_DIR is required"),
            )
            .join(".minecraft")
        })
}

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to a Java 25 JDK"))
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn classpath(game: &Path, extra: Option<&Path>) -> OsString {
    let version_dir = game.join("versions/26.3");
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(version_dir.join("26.3.json")).expect("Minecraft 26.3 metadata not found"),
    )
    .unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version_dir.join("26.3.jar")];
    if let Some(extra) = extra {
        paths.push(extra.to_path_buf());
    }
    for library in manifest["libraries"]
        .as_array()
        .expect("invalid libraries metadata")
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).expect("cannot construct Java classpath")
}

fn run_checked(command: &mut Command) -> String {
    let output = command.output().expect("could not run Java 25");
    assert!(
        output.status.success(),
        "Java oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn java_poi_sections_are_decoded_from_dimension_region() {
    let game = minecraft_dir();
    let java = java_bin();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("PoiOracle.java");
    std::fs::write(&source, r#"
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.world.entity.ai.village.poi.*;
public class PoiOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var home = BuiltInRegistries.POINT_OF_INTEREST_TYPE.getOrThrow(PoiTypes.HOME);
    var ops = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY).createSerializationContext(NbtOps.INSTANCE);
    if (args.length > 1) {
      var root = NbtIo.read(Path.of(args[1]));
      var sections = (CompoundTag)root.get("Sections");
      var section = PoiSection.Packed.CODEC.parse(ops, sections.get("-1")).getOrThrow();
      if (!section.isValid() || section.records().size() != 1) throw new AssertionError("wrong section");
      var record = section.records().getFirst();
      if (!record.pos().equals(new BlockPos(-2,-2,1)) || !record.poiType().getRegisteredName().equals("minecraft:meeting") || record.freeTickets() != 32)
        throw new AssertionError("wrong drafted POI " + record);
      return;
    }
    var first = new PoiSection.Packed(true, List.of(new PoiRecord.Packed(new BlockPos(-1,-1,0), home, 1)));
    var second = new PoiSection.Packed(false, List.of(new PoiRecord.Packed(new BlockPos(-16,3,15), home, 0)));
    var sections = new CompoundTag();
    sections.put("-1", PoiSection.Packed.CODEC.encodeStart(ops, first).getOrThrow());
    sections.put("0", PoiSection.Packed.CODEC.encodeStart(ops, second).getOrThrow());
    var root = new CompoundTag(); root.putInt("DataVersion", 5023); root.put("Sections", sections);
    NbtIo.write(root, Path.of(args[0], "poi.nbt"));
    var invalid = new PoiSection.Packed(false, List.of(
      new PoiRecord.Packed(new BlockPos(-1,-1,0), home, 0),
      new PoiRecord.Packed(new BlockPos(-2,-2,1), home, 1)));
    var refreshSections = new CompoundTag();
    refreshSections.put("-1", PoiSection.Packed.CODEC.encodeStart(ops, invalid).getOrThrow());
    var refreshRoot = new CompoundTag(); refreshRoot.putInt("DataVersion", 5023); refreshRoot.put("Sections", refreshSections);
    NbtIo.write(refreshRoot, Path.of(args[0], "refresh-source.nbt"));
    var meeting = BuiltInRegistries.POINT_OF_INTEREST_TYPE.getOrThrow(PoiTypes.MEETING);
    var runtime = invalid.unpack(() -> {});
    runtime.refresh(add -> { add.accept(new BlockPos(-1,-1,0), meeting); add.accept(new BlockPos(-3,-3,2), meeting); });
    refreshSections.put("-1", PoiSection.Packed.CODEC.encodeStart(ops, runtime.pack()).getOrThrow());
    NbtIo.write(refreshRoot, Path.of(args[0], "refresh-expected.nbt"));
    var occupied = new PoiSection.Packed(true, List.of(new PoiRecord.Packed(new BlockPos(-1,-1,0), home, 0)));
    var orderedSections = new CompoundTag(); orderedSections.put("-1", PoiSection.Packed.CODEC.encodeStart(ops, occupied).getOrThrow());
    var orderedRoot = new CompoundTag(); orderedRoot.putInt("DataVersion", 5023); orderedRoot.put("Sections", orderedSections);
    NbtIo.write(orderedRoot, Path.of(args[0], "ordered-source.nbt"));
    var ordered = occupied.unpack(() -> {}); ordered.remove(new BlockPos(-1,-1,0)); ordered.add(new BlockPos(-1,-1,0), home);
    orderedSections.put("-1", PoiSection.Packed.CODEC.encodeStart(ops, ordered.pack()).getOrThrow());
    NbtIo.write(orderedRoot, Path.of(args[0], "ordered-expected.nbt"));
    var data = new CompoundTag(); data.putInt("DataVersion", 5023);
    var level = new CompoundTag(); level.put("Data", data);
    NbtIo.writeCompressed(level, Path.of(args[0], "level.dat"));
  }
}
"#).unwrap();
    let cp = classpath(&game, Some(temp.path()));
    run_checked(
        Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source),
    );
    run_checked(
        Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PoiOracle")
            .arg(temp.path()),
    );
    let bytes = std::fs::read(temp.path().join("poi.nbt")).unwrap();
    let mut region = vec![0u8; 3 * 4096];
    assert!(bytes.len() + 5 <= 4096);
    region[31 * 4..32 * 4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
    region[8192..8196].copy_from_slice(&((bytes.len() + 1) as u32).to_be_bytes());
    region[8196] = 3;
    region[8197..8197 + bytes.len()].copy_from_slice(&bytes);
    let poi = temp.path().join("dimensions/minecraft/overworld/poi");
    std::fs::create_dir_all(&poi).unwrap();
    std::fs::write(poi.join("r.-1.0.mca"), region).unwrap();
    let mut reader =
        WorldReader::open(temp.path(), DimensionId::parse("overworld").unwrap()).unwrap();
    let chunk = reader.read_poi_chunk(-1, 0).unwrap().unwrap();
    assert_eq!(*chunk.document, nbt::read(&bytes).unwrap());
    for (y, valid, position, free) in [(-1, true, [-1, -1, 0], 1), (0, false, [-16, 3, 15], 0)] {
        let section = &chunk.sections[&y];
        assert_eq!(section.valid, valid);
        assert_eq!(section.records.len(), 1);
        assert_eq!(section.records[0].position, position);
        assert_eq!(section.records[0].type_id, "minecraft:home".into());
        assert_eq!(section.records[0].free_tickets, free);
    }
    use native_persistence::poi::{PoiBlockChange, draft_block_changes};
    use native_world::loaded::BlockState;
    let state = |name: &str, properties: &[(&str, &str)]| BlockState {
        name: format!("minecraft:{name}"),
        properties: properties
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
    };
    let draft = draft_block_changes(
        &chunk,
        &[
            PoiBlockChange {
                position: [-1, -1, 0],
                expected: state("red_bed", &[("part", "head")]),
                replacement: state("air", &[]),
            },
            PoiBlockChange {
                position: [-2, -2, 1],
                expected: state("air", &[]),
                replacement: state("bell", &[]),
            },
        ],
        |_| Ok(()),
    )
    .unwrap();
    let edited = temp.path().join("edited-poi.nbt");
    std::fs::write(&edited, nbt::write(draft.document()).unwrap()).unwrap();
    run_checked(
        Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PoiOracle")
            .arg(temp.path())
            .arg(&edited),
    );
    use native_persistence::{
        poi::refresh_section,
        world::{ChunkPos, SavedPoiChunk},
    };
    use native_world::loaded::{self, BLOCK_COUNT};
    use std::{collections::BTreeMap, sync::Arc};
    let source = SavedPoiChunk {
        position: ChunkPos { x: -1, z: 0 },
        document: Arc::new(
            nbt::read(&std::fs::read(temp.path().join("refresh-source.nbt")).unwrap()).unwrap(),
        ),
        sections: BTreeMap::new(),
    };
    let mut world = loaded::World::new("minecraft:overworld");
    let air = world.intern_state(state("air", &[]));
    let bell = world.intern_state(state("bell", &[]));
    let mut blocks = Box::new([air; BLOCK_COUNT]);
    blocks[15 + 15 * 256] = bell;
    blocks[13 + 2 * 16 + 13 * 256] = bell;
    let position = loaded::ChunkPos { x: -1, z: 0 };
    world.chunks.insert(
        position,
        Arc::new(loaded::Chunk {
            position,
            status: Some("minecraft:full".into()),
            block_light_correct: false,
            sections: BTreeMap::from([(
                -1,
                Arc::new(loaded::Section {
                    blocks: Some(blocks),
                    biomes: None,
                    block_light: None,
                    sky_light: None,
                }),
            )]),
        }),
    );
    let actual = refresh_section(
        Some(&source),
        source.position,
        -1,
        &world,
        |_| Ok(()),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        actual.discarded_positions,
        [[-2, -2, 1]].into_iter().collect()
    );
    let expected =
        nbt::read(&std::fs::read(temp.path().join("refresh-expected.nbt")).unwrap()).unwrap();
    let records = |doc: &NamedTag| -> BTreeMap<Vec<i32>, (String, i32)> {
        let section = doc.value.get("Sections").unwrap().get("-1").unwrap();
        assert_eq!(section.get("Valid"), Some(&Tag::Byte(1)));
        let Some(Tag::List { values, .. }) = section.get("Records") else {
            panic!()
        };
        values
            .iter()
            .map(|record| {
                let Some(Tag::IntArray(pos)) = record.get("pos") else {
                    panic!()
                };
                let Some(Tag::String(kind)) = record.get("type") else {
                    panic!()
                };
                (
                    pos.clone(),
                    (
                        kind.to_string_lossy(),
                        record
                            .get("free_tickets")
                            .and_then(Tag::as_java_int)
                            .unwrap_or(0),
                    ),
                )
            })
            .collect()
    };
    assert_eq!(
        records(actual.document.as_ref().unwrap()),
        records(&expected)
    );
    let ordered_source = SavedPoiChunk {
        position: source.position,
        document: Arc::new(
            nbt::read(&std::fs::read(temp.path().join("ordered-source.nbt")).unwrap()).unwrap(),
        ),
        sections: BTreeMap::new(),
    };
    let bed = state("red_bed", &[("part", "head")]);
    let air = state("air", &[]);
    let ordered = native_persistence::poi::draft_ordered_block_changes(
        &ordered_source,
        &[
            PoiBlockChange {
                position: [-1, -1, 0],
                expected: bed.clone(),
                replacement: air.clone(),
            },
            PoiBlockChange {
                position: [-1, -1, 0],
                expected: air,
                replacement: bed,
            },
        ],
        |_| Ok(()),
    )
    .unwrap();
    let expected =
        nbt::read(&std::fs::read(temp.path().join("ordered-expected.nbt")).unwrap()).unwrap();
    assert_eq!(records(ordered.document()), records(&expected));
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn allow_commands_matches_java_level_settings() {
    let game = minecraft_dir();
    let java = java_bin();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("AllowCommandsOracle.java");
    std::fs::write(&source, r#"
import java.nio.file.*;
import com.google.gson.*;
import com.mojang.serialization.Dynamic;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.nbt.*;
import net.minecraft.world.level.LevelSettings;
import net.minecraft.world.level.WorldDataConfiguration;
public class AllowCommandsOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Path root = Path.of(args[0]); JsonArray result = new JsonArray();
    Tag[] values = {null, ByteTag.valueOf((byte)0), ByteTag.valueOf((byte)1), ShortTag.valueOf((short)-1), IntTag.valueOf(256), LongTag.valueOf(256), FloatTag.valueOf(0.25f), DoubleTag.valueOf(-0.25), StringTag.valueOf("true")};
    int i = 0;
    for (int mode : new int[]{0,1,2,3,999}) for (Tag value : values) {
      CompoundTag data = new CompoundTag(); data.putInt("DataVersion", 5023); data.putInt("GameType", mode);
      if (value != null) data.put("allowCommands", value);
      result.add(LevelSettings.parse(new Dynamic<>(NbtOps.INSTANCE, data), WorldDataConfiguration.DEFAULT).allowCommands());
      CompoundTag document = new CompoundTag(); document.put("Data", data);
      Path dir = root.resolve("world-" + i++); Files.createDirectories(dir); NbtIo.writeCompressed(document, dir.resolve("level.dat"));
    }
    Files.writeString(root.resolve("result.json"), new Gson().toJson(result));
  }
}
"#).unwrap();
    let cp = classpath(&game, Some(temp.path()));
    run_checked(
        Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source),
    );
    run_checked(
        Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("AllowCommandsOracle")
            .arg(temp.path()),
    );
    let expected: Vec<bool> =
        serde_json::from_slice(&std::fs::read(temp.path().join("result.json")).unwrap()).unwrap();
    for (index, expected) in expected.into_iter().enumerate() {
        let reader = WorldReader::open(
            &temp.path().join(format!("world-{index}")),
            DimensionId::parse("overworld").unwrap(),
        )
        .unwrap();
        assert_eq!(reader.allow_commands().unwrap(), expected, "case {index}");
    }
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn java_and_rust_read_each_others_nbt() {
    let game = minecraft_dir();
    let java = java_bin();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("NbtOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    let compile_cp = classpath(&game, None);
    run_checked(
        Command::new(java.join("javac.exe"))
            .current_dir(temp.path())
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(compile_cp)
            .arg(&source),
    );
    let runtime_cp = classpath(&game, Some(temp.path()));
    let java_output = run_checked(
        Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&runtime_cp)
            .arg("NbtOracle")
            .arg("write"),
    );
    let root = nbt::read(&STANDARD.decode(java_output).unwrap()).unwrap();
    assert_eq!(root.value.get("number"), Some(&Tag::Int(-42)));
    let text = JavaString(vec![0x41, 0, 0xd800, 0xd83d, 0xde00]);
    assert_eq!(root.value.get("text"), Some(&Tag::String(text.clone())));
    assert_eq!(
        root.value.get("bytes"),
        Some(&Tag::ByteArray(vec![0, 128, 255]))
    );
    assert_eq!(
        root.value.get("longs"),
        Some(&Tag::LongArray(vec![i64::MIN, 0, i64::MAX]))
    );
    assert_eq!(
        root.value.get("nested").and_then(|v| v.get("value")),
        Some(&Tag::Double(-42.5))
    );

    let rust_root = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("number".into(), Tag::Int(-42)),
            ("text".into(), Tag::String(text)),
            ("bytes".into(), Tag::ByteArray(vec![0, 128, 255])),
            ("longs".into(), Tag::LongArray(vec![i64::MIN, 0, i64::MAX])),
            (
                "nested".into(),
                Tag::Compound(vec![("value".into(), Tag::Double(-42.5))]),
            ),
        ]),
    };
    let rust_bytes = STANDARD.encode(nbt::write(&rust_root).unwrap());
    let checked = run_checked(
        Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&runtime_cp)
            .arg("NbtOracle")
            .arg("check")
            .arg(rust_bytes),
    );
    assert_eq!(checked, "OK");
    let random_data = run_checked(
        Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&runtime_cp)
            .arg("NbtOracle")
            .arg("random"),
    );
    let random_root = nbt::read(&STANDARD.decode(random_data).unwrap()).unwrap();
    let mut legacy_values = Vec::new();
    let mut xor_values = Vec::new();
    let bounds = [1, 2, 3, 7, 16, 17, 1_073_741_825, i32::MAX];
    for seed in [0, -1, i64::MIN, i64::MAX, 123_456_789_012_345] {
        let mut legacy = LegacyRandom::new(seed);
        let mut xor = XoroshiroRandom::new(seed);
        for i in 0..64 {
            let bound = bounds[i % bounds.len()];
            legacy_values.extend([
                i64::from(legacy.next_int()),
                i64::from(legacy.next_int_bound(bound).unwrap()),
                legacy.next_long(),
                i64::from(legacy.next_float().to_bits() as i32),
                legacy.next_double().to_bits() as i64,
                i64::from(legacy.next_bool()),
            ]);
            xor_values.extend([
                i64::from(xor.next_int()),
                i64::from(xor.next_int_bound(bound).unwrap()),
                xor.next_long(),
                i64::from(xor.next_float().to_bits() as i32),
                xor.next_double().to_bits() as i64,
                i64::from(xor.next_bool()),
            ]);
        }
    }
    assert_eq!(
        random_root.value.get("legacy"),
        Some(&Tag::LongArray(legacy_values))
    );
    assert_eq!(
        random_root.value.get("xoroshiro"),
        Some(&Tag::LongArray(xor_values))
    );
    let world = temp.path().join("world");
    let made = run_checked(
        Command::new(java.join("java.exe"))
            .current_dir(temp.path())
            .arg("-classpath")
            .arg(&runtime_cp)
            .arg("NbtOracle")
            .arg("world")
            .arg(&world),
    );
    assert_eq!(made.lines().last(), Some("OK"));
    assert_eq!(anvil::read_level(&world).unwrap().data_version, 5023);
    for x in [-33, -1, 0, 32] {
        let path = anvil::overworld_region_path(&world, x, -1);
        let mut region = anvil::RegionReader::open(&path).unwrap();
        let chunk = region.read_chunk(x, -1).unwrap().unwrap();
        assert_eq!(chunk.value.get("xPos"), Some(&Tag::Int(x)));
        assert_eq!(chunk.value.get("zPos"), Some(&Tag::Int(-1)));
        assert_eq!(
            chunk.value.get("Status"),
            Some(&Tag::String("minecraft:full".into()))
        );
        let sections = palette::decode_block_sections(&chunk.value).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].y, -4);
        let names = [
            "minecraft:stone",
            "minecraft:oak_log",
            "minecraft:white_wool",
            "minecraft:orange_wool",
            "minecraft:magenta_wool",
            "minecraft:light_blue_wool",
            "minecraft:yellow_wool",
            "minecraft:lime_wool",
            "minecraft:pink_wool",
            "minecraft:gray_wool",
            "minecraft:light_gray_wool",
            "minecraft:cyan_wool",
            "minecraft:purple_wool",
            "minecraft:blue_wool",
            "minecraft:brown_wool",
            "minecraft:green_wool",
            "minecraft:red_wool",
        ];
        for index in 0..4096 {
            let x = index % 16;
            let z = (index / 16) % 16;
            let y = index / 256;
            let expected = names[(index * 7) % names.len()];
            let state = sections[0].block(x, y, z).unwrap();
            assert!(state.name.matches(expected));
            if expected == "minecraft:oak_log" {
                assert_eq!(state.properties, vec![("axis".into(), "x".into())]);
            }
        }
        let biomes = sections[0].biomes.as_ref().unwrap();
        for index in 0..64 {
            let x = index % 4;
            let z = (index / 4) % 4;
            let y = index / 16;
            assert!(
                biomes
                    .biome(x, y, z)
                    .unwrap()
                    .matches(&format!("fixture:biome_{}", index % 3))
            );
        }
    }
    let mut reader =
        WorldReader::open(&world, DimensionId::parse("minecraft:overworld").unwrap()).unwrap();
    use native_persistence::world::SavedClock;
    let clocks = reader.world_clocks().unwrap();
    let initial = [
        SavedClock {
            total_ticks: 12345,
            partial_tick: 0.25,
            rate: 0.3,
            paused: false,
        },
        SavedClock {
            total_ticks: -100,
            partial_tick: 0.75,
            rate: 2.5,
            paused: true,
        },
        SavedClock {
            total_ticks: i64::MAX,
            partial_tick: 0.75,
            rate: 2.5,
            paused: false,
        },
        SavedClock {
            total_ticks: -4,
            partial_tick: -0.75,
            rate: 0.125,
            paused: false,
        },
    ];
    assert_eq!(
        clocks.get(&DimensionId::parse("minecraft:overworld").unwrap()),
        Some(&initial[0])
    );
    assert_eq!(
        clocks.get(&DimensionId::parse("minecraft:the_end").unwrap()),
        Some(&initial[1])
    );
    let clock_samples =
        nbt::read_gzip(std::fs::File::open(world.join("clock_samples.dat")).unwrap()).unwrap();
    let Some(Tag::List {
        values: samples, ..
    }) = clock_samples.value.get("samples")
    else {
        panic!("missing clock samples")
    };
    assert_eq!(samples.len(), initial.len());
    for (mut clock, sample) in initial.into_iter().zip(samples) {
        let Some(Tag::List { values: rows, .. }) = sample.get("rows") else {
            panic!("missing clock rows")
        };
        for row in rows {
            clock.tick(true);
            assert_eq!(row.get("ticks"), Some(&Tag::Long(clock.total_ticks)));
            assert_eq!(row.get("partial"), Some(&Tag::Float(clock.partial_tick)));
        }
    }
    let dimensions = reader.saved_dimension_types().unwrap().unwrap();
    for (dimension, expected) in [
        ("minecraft:overworld", "minecraft:the_nether"),
        ("minecraft:the_nether", "minecraft:the_nether"),
        ("minecraft:the_end", "minecraft:the_end"),
        ("fixture:custom", "minecraft:the_end"),
    ] {
        assert_eq!(
            dimensions.get(&DimensionId::parse(dimension).unwrap()),
            Some(&SavedDimensionType::Named(
                DimensionId::parse(expected).unwrap()
            )),
        );
    }
    assert_eq!(
        reader.world_border().unwrap(),
        native_persistence::world::SavedWorldBorder {
            center_x: 123.5,
            center_z: -456.25,
            damage_per_block: 0.4,
            safe_zone: 3.5,
            warning_blocks: 7,
            warning_time: 120,
            size: 64.0,
            lerp_time: 40,
            lerp_target: 100.0,
        }
    );
    let nether =
        WorldReader::open(&world, DimensionId::parse("minecraft:the_nether").unwrap()).unwrap();
    assert_eq!(
        nether.world_border().unwrap(),
        native_persistence::world::SavedWorldBorder::default()
    );
    assert_eq!(
        reader.world_spawn().unwrap(),
        RespawnData {
            dimension: DimensionId::parse("minecraft:overworld").unwrap(),
            position: [-123, 70, 456],
            yaw: 45.5,
            pitch: -12.25,
        }
    );
    let rules = reader.fluid_game_rules().unwrap();
    assert!(!rules.advance_time);
    assert!(!rules.water_source_conversion);
    assert!(rules.lava_source_conversion);
    assert!(!rules.block_drops);
    assert!(!rules.entity_drops);
    assert!(!rules.natural_health_regeneration);
    assert!(!rules.fall_damage);
    assert!(rules.keep_inventory);
    assert!(rules.immediate_respawn);
    assert!(!rules.show_death_messages);
    assert_eq!(rules.respawn_radius, 7);
    for x in [-33, -1, 0, 32] {
        let entities = reader.read_entity_chunk(x, -1).unwrap().unwrap();
        assert_eq!(entities.position, ChunkPos { x, z: -1 });
        assert_eq!(entities.entities().len(), 1);
        assert_eq!(
            entities.entities()[0].get("id"),
            Some(&Tag::String("minecraft:pig".into()))
        );
        assert_eq!(
            entities.entities()[0].get("fixture_unknown"),
            Some(&Tag::String("preserved".into()))
        );
        assert_eq!(
            entities.document.value.get("fixture_extra"),
            Some(&Tag::String("preserved".into()))
        );
    }
    assert!(reader.read_entity_chunk(1, -1).unwrap().is_none());
    assert_eq!(
        reader.difficulty_settings().unwrap(),
        native_persistence::world::DifficultySettings {
            difficulty: native_persistence::world::Difficulty::Hard,
            hardcore: true,
            locked: true,
        }
    );
    assert_eq!(
        reader.singleplayer_uuid().unwrap().as_deref(),
        Some("00112233-4455-6677-8899-aabbccddeeff")
    );
    let player = reader.read_player().unwrap().unwrap();
    assert_eq!(
        read_player_respawn(&player).unwrap(),
        Some(PlayerRespawnConfig {
            data: RespawnData {
                dimension: DimensionId::parse("minecraft:the_nether").unwrap(),
                position: [56, -12, -987],
                yaw: -90.0,
                pitch: 22.5,
            },
            forced: true,
        })
    );
    assert_eq!(
        player.value.get("fixture_only"),
        Some(&Tag::String("preserved".into()))
    );
    for i in 0..6 {
        let case = nbt::read_gzip(
            std::fs::File::open(world.join(format!("spawn-cases/{i}.dat"))).unwrap(),
        )
        .unwrap();
        let actual = read_player_respawn(&case);
        let expected = case.value.get("expected").map(|tag| {
            read_player_respawn(&NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![("respawn".into(), tag.clone())]),
            })
            .unwrap()
            .unwrap()
        });
        match expected {
            Some(expected) => assert_eq!(actual.unwrap(), Some(expected), "spawn case {i}"),
            None => assert!(actual.is_err(), "spawn case {i}"),
        }
    }
    let area = reader.read_area(ChunkPos { x: 0, z: -1 }, 1).unwrap();
    assert_eq!(area.chunks.len(), 2);
    assert_eq!(area.missing.len(), 7);
    let chunk = area.chunks.get(&ChunkPos { x: -1, z: -1 }).unwrap();
    assert!(chunk.is_full());
    assert_eq!(chunk.sections.len(), 2);
    assert_eq!(chunk.light(-16, -80, -16, true), Some(15));
    assert_eq!(
        chunk.document.value.get("fixture_unknown_tag"),
        Some(&Tag::String("preserved".into()))
    );
    let runtime = area.materialize(reader.dimension()).unwrap();
    let ticks = &runtime.saved_fluid_ticks[&native_world::loaded::ChunkPos { x: -1, z: -1 }];
    assert_eq!(ticks.len(), 3);
    let block_ticks = &runtime.saved_block_ticks[&native_world::loaded::ChunkPos { x: -1, z: -1 }];
    assert_eq!(block_ticks.len(), 3);
    assert_eq!(
        (
            &block_ticks[0].block,
            block_ticks[0].position,
            block_ticks[0].delay,
            block_ticks[0].priority
        ),
        (&"minecraft:sand".to_string(), [-13, -61, -13], 2, -1)
    );
    assert_eq!(
        (
            &block_ticks[1].block,
            block_ticks[1].position,
            block_ticks[1].delay,
            block_ticks[1].priority
        ),
        (&"minecraft:gravel".to_string(), [-12, -60, -12], -4, 1)
    );
    assert_eq!(
        (
            &block_ticks[2].block,
            block_ticks[2].position,
            block_ticks[2].delay,
            block_ticks[2].priority
        ),
        (&"minecraft:sand".to_string(), [-11, -59, -11], 3, 2)
    );
    assert_eq!(
        (
            ticks[0].fluid.as_str(),
            ticks[0].position,
            ticks[0].delay,
            ticks[0].priority
        ),
        ("minecraft:water", [-15, -63, -15], 5, -1)
    );
    assert_eq!(
        (
            ticks[1].fluid.as_str(),
            ticks[1].position,
            ticks[1].delay,
            ticks[1].priority
        ),
        ("minecraft:flowing_lava", [-14, -62, -14], -2, 1)
    );
    assert_eq!(
        (
            ticks[2].fluid.as_str(),
            ticks[2].position,
            ticks[2].delay,
            ticks[2].priority
        ),
        ("minecraft:water", [-10, -58, -10], 4, -2)
    );
    assert_eq!(runtime.state_count(), 17);
    assert_eq!(runtime.biome_count(), 3);
    assert_eq!(
        runtime.block(-16, -64, -16).unwrap().name,
        "minecraft:stone"
    );
    assert_eq!(runtime.biome(-16, -64, -16), Some("fixture:biome_0"));
    assert_eq!(runtime.light(-16, -80, -16, true), Some(15));
    assert!(runtime.block(-16, -80, -16).is_none());

    // Feed Java's saved tick through the actual runtime and confirm it changes
    // only the in-memory copy, after the encoded delay.
    let region_path = anvil::overworld_region_path(&world, -1, -1);
    let original_region = std::fs::read(&region_path).unwrap();
    let mut runtime = runtime;
    let water = runtime.intern_state(native_world::loaded::BlockState {
        name: "minecraft:water".into(),
        properties: [("level".into(), "0".into())].into_iter().collect(),
    });
    let air = runtime.intern_state(native_world::loaded::BlockState {
        name: "minecraft:air".into(),
        properties: Default::default(),
    });
    runtime.set_block_id(-15, -63, -15, water).unwrap();
    runtime.set_block_id(-14, -63, -15, air).unwrap();
    let player = PlayerState {
        position: glam::Vec3::new(8.5, -47.0, 8.5),
        yaw: 0.0,
        pitch: 0.0,
        vertical_velocity: 0.0,
        on_ground: false,
        pose: PlayerPose::Standing,
    };
    let mut session = LoadedSession::new(
        runtime,
        glam::IVec3::new(-16, 0, -16),
        BuildBounds {
            min_y: -64,
            max_y: 320,
        },
        player,
    )
    .unwrap();
    session.set_source_conversion(rules.water_source_conversion, rules.lava_source_conversion);
    for _ in 0..6 {
        session.update(std::time::Duration::from_millis(50));
    }
    let snapshot = session.snapshot();
    let changed = snapshot
        .world
        .loaded()
        .unwrap()
        .block(-14, -63, -15)
        .unwrap();
    assert_eq!(
        (changed.name.as_str(), changed.properties["level"].as_str()),
        ("minecraft:water", "1")
    );
    assert_eq!(std::fs::read(region_path).unwrap(), original_region);
}
