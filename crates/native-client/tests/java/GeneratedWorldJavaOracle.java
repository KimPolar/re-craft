import com.mojang.authlib.GameProfile;
import com.mojang.serialization.Codec;
import com.mojang.serialization.Dynamic;
import com.mojang.serialization.Lifecycle;
import java.io.DataInput;
import java.io.DataOutput;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.TreeSet;
import java.util.UUID;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.*;
import net.minecraft.world.level.levelgen.WorldGenSettings;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pieces.*;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalPiece;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.level.gamerules.GameRuleMap;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.storage.PrimaryLevelData;
import net.minecraft.world.entity.ai.village.poi.PoiSection;
import net.minecraft.world.phys.Vec3;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityProcessor;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.TagValueOutput;

/** Validate a disposable generated world and rewrite all listed chunks with Java. */
public class GeneratedWorldJavaOracle {
    private static RegistryAccess entityAccess;
    private static final class TestLevel extends ClientLevel {
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public int getNextEntityId() { return 1; }
    }
    private static final class TestPlayer extends Player {
        TestPlayer(Level level, UUID uuid) { super(level, new GameProfile(uuid, "Oracle")); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
    }
    private static final class TestEntityLevel extends ClientLevel {
        TestEntityLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public int getNextEntityId() { return 1; }
        @Override public RegistryAccess registryAccess() { return entityAccess; }
        @Override public net.minecraft.world.flag.FeatureFlagSet enabledFeatures() {
            return net.minecraft.world.flag.FeatureFlags.VANILLA_SET;
        }
    }

    private static TestEntityLevel entityLevel() throws Exception {
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        return (TestEntityLevel) ((sun.misc.Unsafe) field.get(null)).allocateInstance(TestEntityLevel.class);
    }

    private static <T> Registry<T> copyRegistry(HolderLookup.Provider lookup, ResourceKey<? extends Registry<T>> key) {
        var registry = new MappedRegistry<T>(key, Lifecycle.stable());
        lookup.lookupOrThrow(key).listElements().forEach(holder ->
                registry.register(holder.key(), holder.value(), RegistrationInfo.BUILT_IN));
        return registry.freeze();
    }

    /** Exercise Java's structure objects, not only the opaque chunk NBT. */
    private static final class StructureVerifier {
        private final StructurePieceSerializationContext context;
        private final RegistryAccess access;
        private final StructureTemplateManager manager;
        private final java.lang.reflect.Method unpackStarts, unpackReferences, pack;
        private final Set<String> starts = new HashSet<>();
        private final Set<String> references = new HashSet<>();
        private int portalStarts, portalReferences, mineshaftStarts, mineshaftReferences;

        StructureVerifier(HolderLookup.Provider lookup) throws Exception {
            var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
            field.setAccessible(true);
            manager = (StructureTemplateManager) ((sun.misc.Unsafe) field.get(null))
                    .allocateInstance(StructureTemplateManager.class);
            var templates = new HashMap<Identifier, Optional<StructureTemplate>>();
            for (int i = 1; i <= 13; i++) {
                var name = i <= 10 ? "portal_" + i : "giant_portal_" + (i - 10);
                var id = Identifier.withDefaultNamespace("ruined_portal/" + name);
                var template = new StructureTemplate();
                try (var stream = GeneratedWorldJavaOracle.class.getClassLoader().getResourceAsStream(
                        "data/minecraft/structure/ruined_portal/" + name + ".nbt")) {
                    if (stream == null) throw new AssertionError("portal template missing: " + id);
                    template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap()));
                }
                templates.put(id, Optional.of(template));
            }
            var repository = StructureTemplateManager.class.getDeclaredField("structureRepository");
            repository.setAccessible(true);
            repository.set(manager, templates);
            var blocks = new MappedRegistry<Block>(Registries.BLOCK, Lifecycle.stable());
            blocks.bindTags(Map.of(BlockTags.FEATURES_CANNOT_REPLACE, List.of()));
            blocks.freeze();
            access = new RegistryAccess.ImmutableRegistryAccess(List.of(blocks, copyRegistry(lookup, Registries.STRUCTURE)));
            context = new StructurePieceSerializationContext(null, access, manager);
            unpackStarts = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart",
                    StructurePieceSerializationContext.class, CompoundTag.class, long.class);
            unpackReferences = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences",
                    RegistryAccess.class, ChunkPos.class, CompoundTag.class);
            pack = SerializableChunkData.class.getDeclaredMethod("packStructureData",
                    StructurePieceSerializationContext.class, ChunkPos.class, Map.class, Map.class);
            unpackStarts.setAccessible(true);
            unpackReferences.setAccessible(true);
            pack.setAccessible(true);
        }

        private static boolean isPortal(String key) {
            return key.equals("minecraft:ruined_portal") || key.startsWith("minecraft:ruined_portal_");
        }

        private static boolean isMineshaft(String key) {
            return key.equals("minecraft:mineshaft") || key.equals("minecraft:mineshaft_mesa");
        }

        private static boolean isSupported(String key) {
            return isPortal(key) || isMineshaft(key);
        }

        void verify(CompoundTag structures, ChunkPos pos, long seed, boolean collect) throws Exception {
            var originalStarts = structures.getCompoundOrEmpty("starts");
            var originalRefs = structures.getCompoundOrEmpty("References");
            var checkedStarts = new CompoundTag();
            var checkedRefs = new CompoundTag();
            for (var key : originalStarts.keySet()) if (isSupported(key)) checkedStarts.put(key, originalStarts.get(key));
            for (var key : originalRefs.keySet()) if (isSupported(key)) checkedRefs.put(key, originalRefs.get(key));
            var checked = new CompoundTag();
            checked.put("starts", checkedStarts);
            checked.put("References", checkedRefs);
            var decodedStarts = (Map<?, ?>) unpackStarts.invoke(null, context, checked, seed);
            var decodedRefs = (Map<?, ?>) unpackReferences.invoke(null, access, pos, checked);
            if (decodedStarts.size() != checkedStarts.size() || decodedRefs.size() != checkedRefs.size())
                throw new AssertionError("structure start or reference was dropped in chunk " + pos);
            for (var value : decodedStarts.values()) {
                var start = (StructureStart) value;
                if (start.isValid() && (!start.getChunkPos().equals(pos) || start.getPieces().isEmpty()))
                    throw new AssertionError("invalid structure start or pieces in chunk " + pos);
            }
            var repacked = (CompoundTag) pack.invoke(null, context, pos, decodedStarts, decodedRefs);
            if (!checked.equals(repacked))
                throw new AssertionError("structure changed on Java load/save in chunk " + pos + ": "
                        + checked + " != " + repacked);
            if (!collect) return;
            for (var key : checkedStarts.keySet()) {
                var start = checkedStarts.getCompoundOrEmpty(key);
                if (!start.getStringOr("id", "").equals("INVALID")) {
                    starts.add(key + " " + pos.pack());
                    if (isPortal(key)) portalStarts++;
                    else mineshaftStarts++;
                }
            }
            for (var key : checkedRefs.keySet()) {
                for (var source : checkedRefs.getLongArray(key).orElseThrow()) {
                    references.add(key + " " + source);
                    if (isPortal(key)) portalReferences++;
                    else mineshaftReferences++;
                }
            }
        }

        void finish() {
            for (var reference : references) if (!starts.contains(reference))
                throw new AssertionError("structure reference has no saved valid start: " + reference);
        }
    }

    private static void verifyPortalOracle() throws Exception {
        var lookup = VanillaRegistries.createWorldLookup();
        var verifier = new StructureVerifier(lookup);
        var id = Identifier.withDefaultNamespace("ruined_portal/portal_1");
        var structure = lookup.lookupOrThrow(Registries.STRUCTURE)
                .getOrThrow(ResourceKey.create(Registries.STRUCTURE,
                        Identifier.withDefaultNamespace("ruined_portal"))).value();
        var pos = new ChunkPos(-3, 3);
        var piece = new RuinedPortalPiece(verifier.access, verifier.manager, new BlockPos(-48, 90, 48),
                RuinedPortalPiece.VerticalPlacement.values()[0],
                new RuinedPortalPiece.Properties(false, 0.2f, false, false, false, false),
                id, verifier.manager.get(id).orElseThrow(), Rotation.NONE, Mirror.NONE, new BlockPos(3, 0, 2));
        var start = new StructureStart(structure, pos, 0, new PiecesContainer(List.of(piece)));
        var starts = new CompoundTag();
        starts.put("minecraft:ruined_portal", start.createTag(verifier.context, pos));
        var references = new CompoundTag();
        references.putLongArray("minecraft:ruined_portal", new long[]{pos.pack()});
        var structures = new CompoundTag();
        structures.put("starts", starts);
        structures.put("References", references);
        verifier.verify(structures, pos, 42L, true);
        verifier.finish();
        if (verifier.portalStarts != 1 || verifier.portalReferences != 1)
            throw new AssertionError("portal oracle did not exercise its fixture");

        var corrupted = structures.copy();
        corrupted.getCompoundOrEmpty("starts").getCompoundOrEmpty("minecraft:ruined_portal")
                .put("Children", new ListTag());
        boolean rejected = false;
        try {
            verifier.verify(corrupted, pos, 42L, false);
        } catch (AssertionError | Exception error) {
            rejected = true;
        }
        if (!rejected) throw new AssertionError("portal oracle accepted an empty start");

        var dangling = new StructureVerifier(lookup);
        var danglingStructures = new CompoundTag();
        danglingStructures.put("starts", new CompoundTag());
        danglingStructures.put("References", references.copy());
        dangling.verify(danglingStructures, new ChunkPos(-2, 3), 42L, true);
        rejected = false;
        try {
            dangling.finish();
        } catch (AssertionError error) {
            rejected = true;
        }
        if (!rejected) throw new AssertionError("portal oracle accepted a reference without its start");
    }

    private static void verifyPlayerRoundTrip(CompoundTag saved) throws Exception {
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var level = (TestLevel) ((sun.misc.Unsafe) field.get(null)).allocateInstance(TestLevel.class);
        var registry = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        var player = new TestPlayer(level, UUIDUtil.uuidFromIntArray(saved.getIntArray("UUID").orElseThrow()));
        player.load(TagValueInput.create(ProblemReporter.DISCARDING, registry, saved));
        var output = TagValueOutput.createWithContext(ProblemReporter.DISCARDING, registry);
        player.saveWithoutId(output);
        var restored = output.buildResult();
        for (var key : new String[]{"UUID", "Pos", "Motion", "Rotation", "fall_distance", "OnGround", "Air",
                "Health", "AbsorptionAmount", "foodLevel", "foodTickTimer", "foodSaturationLevel", "foodExhaustionLevel",
                "XpP", "XpLevel", "XpTotal", "Score"}) {
            if (saved.get(key) == null || !Objects.equals(saved.get(key), restored.get(key)))
                throw new AssertionError("generated player field changed on Java load/save: " + key + ": " + saved.get(key) + " != " + restored.get(key));
        }
        if (saved.getIntOr("XpSeed", 0) != 0 && !Objects.equals(saved.get("XpSeed"), restored.get("XpSeed")))
            throw new AssertionError("generated player enchantment seed changed");
    }
    private static boolean sameLight(DataLayer a, DataLayer b) {
        return a == null ? b == null : b != null && Arrays.equals(a.getData(), b.getData());
    }

    private static HolderLookup.Provider entityRegistry() {
        var registry = VanillaRegistries.createWorldLookup();
        entityAccess = new RegistryAccess.ImmutableRegistryAccess(java.util.List.of(
                copyRegistry(registry, Registries.PIG_VARIANT),
                copyRegistry(registry, Registries.COW_VARIANT),
                copyRegistry(registry, Registries.CHICKEN_VARIANT),
                copyRegistry(registry, Registries.PIG_SOUND_VARIANT),
                copyRegistry(registry, Registries.COW_SOUND_VARIANT),
                copyRegistry(registry, Registries.CHICKEN_SOUND_VARIANT),
                copyRegistry(registry, Registries.ZOMBIE_NAUTILUS_VARIANT)));
        return registry;
    }

    private static void verifyEntityRoundTrip(CompoundTag entity, HolderLookup.Provider registry,
            TestEntityLevel level, String location) {
        var id = Identifier.parse(entity.getStringOr("id", ""));
        if (!BuiltInRegistries.ENTITY_TYPE.containsKey(id) || entity.read("Pos", Vec3.CODEC).isEmpty())
            throw new AssertionError("unreadable entity " + id + " in " + location);
        var problems = new ProblemReporter.Collector();
        var loaded = EntityType.loadEntityRecursive(TagValueInput.create(problems, registry, entity),
                level, EntitySpawnReason.LOAD, EntityProcessor.NOP);
        if (loaded == null) throw new AssertionError("cannot load entity " + id + " in " + location);
        var output = TagValueOutput.createWithContext(problems, registry);
        if (!loaded.save(output) || !problems.isEmpty())
            throw new AssertionError("cannot save entity " + id + " in " + location + ": " + problems.getReport());
        var rewritten = output.buildResult();
        if (!entity.equals(rewritten))
            throw new AssertionError("entity " + id + " changed on Java load/save in " + location + ": " + entity + " != " + rewritten);
    }

    private static int verifyProtoEntities(Path input) throws Exception {
        var registry = entityRegistry();
        var level = entityLevel();
        var document = NbtIo.read(input);
        var entities = document.getListOrEmpty("entities");
        if (entities.isEmpty()) throw new AssertionError("spawned entities missing");
        for (var item : entities) {
            if (!(item instanceof CompoundTag entity)) throw new AssertionError("invalid proto entity");
            verifyEntityRoundTrip(entity, registry, level, input.toString());
        }
        return entities.size();
    }

    private static int verifySidecars(Path world, String kind, Path list) throws Exception {
        var positions = Files.readAllLines(list);
        var directory = world.resolve("dimensions/minecraft/overworld/" + kind);
        var poiOps = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY)
                .createSerializationContext(NbtOps.INSTANCE);
        int entityCount = 0;
        var entityRegistry = entityRegistry();
        var entityLevel = kind.equals("entities") ? entityLevel() : null;
        for (var position : positions) {
            var coordinates = position.split(" ");
            if (coordinates.length != 2) throw new AssertionError("invalid " + kind + " position " + position);
            var pos = new ChunkPos(Integer.parseInt(coordinates[0]), Integer.parseInt(coordinates[1]));
            var file = directory.resolve("r." + Math.floorDiv(pos.x(), 32) + "." + Math.floorDiv(pos.z(), 32) + ".mca");
            try (var region = new RegionFile(new RegionStorageInfo("verification", null, kind), file, directory, RegionFileVersion.VERSION_DEFLATE, false)) {
                CompoundTag original;
                try (var input = region.getChunkDataInputStream(pos)) {
                    if (input == null) throw new AssertionError(kind + " chunk missing: " + pos);
                    original = NbtIo.read((DataInput) input);
                }
                if (original.getIntOr("DataVersion", 0) != 5023) throw new AssertionError(kind + " version: " + pos);
                if (kind.equals("entities")) {
                    if (!original.read("Position", ChunkPos.CODEC).orElseThrow().equals(pos))
                        throw new AssertionError("entity position differs: " + pos);
                    for (var item : original.getListOrEmpty("Entities")) {
                        if (!(item instanceof CompoundTag entity)) throw new AssertionError("invalid entity: " + pos);
                        verifyEntityRoundTrip(entity, entityRegistry, entityLevel, pos.toString());
                        entityCount++;
                    }
                } else {
                    var sections = original.getCompoundOrEmpty("Sections");
                    for (var key : sections.keySet()) {
                        var before = sections.get(key);
                        var packed = PoiSection.Packed.CODEC.parse(poiOps, before).getOrThrow();
                        var encoded = PoiSection.Packed.CODEC.encodeStart(poiOps, packed).getOrThrow();
                        if (!Objects.equals(before, encoded)) throw new AssertionError("POI section changed: " + pos + " " + key);
                    }
                }
                try (var output = region.getChunkDataOutputStream(pos)) { NbtIo.write(original, (DataOutput) output); }
            }
        }
        return entityCount;
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        if (args[0].equals("--proto-entities")) {
            out.println("PROTO_ENTITIES_OK " + verifyProtoEntities(Path.of(args[1])));
            return;
        }
        if (args[0].equals("--player")) {
            verifyPlayerRoundTrip(NbtIo.read(Path.of(args[1])));
            out.println("PLAYER_OK");
            return;
        }
        if (args[0].equals("--portal-self-test")) {
            verifyPortalOracle();
            out.println("PORTAL_ORACLE_OK");
            return;
        }
        var world = Path.of(args[0]);
        long seed = Long.parseLong(args[1]);
        var worldSpawn = new BlockPos(Integer.parseInt(args[2]), Integer.parseInt(args[3]), Integer.parseInt(args[4]));
        var playerSpawn = new BlockPos(Integer.parseInt(args[5]), Integer.parseInt(args[6]), Integer.parseInt(args[7]));
        var lookup = VanillaRegistries.createWorldLookup();
        var ops = lookup.createSerializationContext(NbtOps.INSTANCE);

        var level = NbtIo.readCompressed(world.resolve("level.dat"), NbtAccounter.unlimitedHeap()).getCompoundOrEmpty("Data");
        var config = WorldDataConfiguration.CODEC.parse(NbtOps.INSTANCE, level).getOrThrow();
        var settings = LevelSettings.parse(new Dynamic<>(NbtOps.INSTANCE, level), config);
        var loaded = PrimaryLevelData.parse(new Dynamic<>(NbtOps.INSTANCE, level), settings,
            PrimaryLevelData.SpecialWorldProperty.NONE, Lifecycle.stable());
        if (loaded.getGameType() != GameType.SURVIVAL || loaded.getGameTime() != 0 ||
            !loaded.getRespawnData().dimension().equals(Level.OVERWORLD) || !loaded.getRespawnData().pos().equals(worldSpawn))
            throw new AssertionError("level settings or world spawn differ");
        var generationFile = NbtIo.readCompressed(world.resolve("data/minecraft/world_gen_settings.dat"), NbtAccounter.unlimitedHeap());
        var generation = WorldGenSettings.CODEC.parse(ops, generationFile.get("data")).getOrThrow();
        if (generation.options().seed() != seed || !generation.options().generateStructures())
            throw new AssertionError("world generation settings differ");
        var encoded = WorldGenSettings.CODEC.encodeStart(ops, generation).getOrThrow();
        var decodedAgain = WorldGenSettings.CODEC.parse(ops, encoded).getOrThrow();
        if (!WorldGenSettings.CODEC.encodeStart(ops, decodedAgain).getOrThrow().equals(encoded))
            throw new AssertionError("world generation settings did not survive Java codec");

        var rulesPath = world.resolve("data/minecraft/game_rules.dat");
        var rulesFile = NbtIo.readCompressed(rulesPath, NbtAccounter.unlimitedHeap());
        var originalRules = rulesFile.get("data");
        var rules = GameRuleMap.CODEC.parse(NbtOps.INSTANCE, originalRules).getOrThrow();
        int expectedRandomTickSpeed = args.length > 11 ? Integer.parseInt(args[11]) : 3;
        if (rules.size() != 16 || !rules.get(GameRules.SPAWN_MOBS) ||
            !rules.get(GameRules.SPECTATORS_GENERATE_CHUNKS) ||
            !rules.get(GameRules.ADVANCE_TIME) || !rules.get(GameRules.ADVANCE_WEATHER) ||
            !rules.get(GameRules.WATER_SOURCE_CONVERSION) || rules.get(GameRules.LAVA_SOURCE_CONVERSION) ||
            !rules.get(GameRules.BLOCK_DROPS) || !rules.get(GameRules.ENTITY_DROPS) ||
            !rules.get(GameRules.NATURAL_HEALTH_REGENERATION) || !rules.get(GameRules.FALL_DAMAGE) ||
            rules.get(GameRules.KEEP_INVENTORY) || rules.get(GameRules.IMMEDIATE_RESPAWN) ||
            !rules.get(GameRules.SHOW_DEATH_MESSAGES) || rules.get(GameRules.RANDOM_TICK_SPEED) != expectedRandomTickSpeed ||
            rules.get(GameRules.FIRE_SPREAD_RADIUS_AROUND_PLAYER) != 128 ||
            rules.get(GameRules.RESPAWN_RADIUS) != 10)
            throw new AssertionError("world game rules differ: " + rules);
        var rewrittenRules = GameRuleMap.CODEC.encodeStart(NbtOps.INSTANCE, rules).getOrThrow();
        if (!Objects.equals(originalRules, rewrittenRules)) throw new AssertionError("world game rules changed");
        rulesFile.put("data", rewrittenRules);
        NbtIo.writeCompressed(rulesFile, rulesPath);

        Path playerPath;
        try (var files = Files.list(world.resolve("players/data"))) {
            var players = files.filter(path -> path.getFileName().toString().endsWith(".dat")).toList();
            if (players.size() != 1) throw new AssertionError("expected one player");
            playerPath = players.get(0);
        }
        var player = NbtIo.readCompressed(playerPath, NbtAccounter.unlimitedHeap());
        if (!Arrays.equals(player.getIntArray("UUID").orElseThrow(), level.getIntArray("singleplayer_uuid").orElseThrow()))
            throw new AssertionError("player identity differs");
        var pos = player.getListOrEmpty("Pos");
        if (pos.size() != 3 || pos.getDoubleOr(0, Double.NaN) != playerSpawn.getX() + 0.5 ||
            pos.getDoubleOr(1, Double.NaN) != playerSpawn.getY() || pos.getDoubleOr(2, Double.NaN) != playerSpawn.getZ() + 0.5)
            throw new AssertionError("player spawn differs: " + pos);
        if (!player.getStringOr("Dimension", "").equals("minecraft:overworld")) throw new AssertionError("player dimension differs");
        verifyPlayerRoundTrip(player);

        var biomes = lookup.lookupOrThrow(Registries.BIOME);
        Holder<Biome> defaultBiome = biomes.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse("minecraft:plains")));
        var ids = new IdMapper<Holder<Biome>>();
        biomes.listElements().forEach(ids::add);
        var blockStrategy = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        var biomeStrategy = Strategy.createForBiomes(ids);
        Codec<Holder<Biome>> biomeCodec = Codec.STRING.xmap(
            name -> biomes.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse(name))),
            biome -> biome.unwrapKey().orElseThrow().identifier().toString());
        var factory = new PalettedContainerFactory(blockStrategy, Blocks.AIR.defaultBlockState(),
            PalettedContainer.codecRW(BlockState.CODEC, blockStrategy, Blocks.AIR.defaultBlockState()),
            biomeStrategy, defaultBiome, PalettedContainer.codecRO(biomeCodec, biomeStrategy, defaultBiome));
        var directory = world.resolve("dimensions/minecraft/overworld/region");
        var positions = Files.readAllLines(Path.of(args[8]));
        if (positions.isEmpty()) throw new AssertionError("no saved chunk positions");
        var structures = new StructureVerifier(lookup);
        var structureTypes = new TreeSet<String>();
        for (var position : positions) {
        var coordinates = position.split(" ");
        if (coordinates.length != 2) throw new AssertionError("invalid chunk position " + position);
        var chunkPos = new ChunkPos(Integer.parseInt(coordinates[0]), Integer.parseInt(coordinates[1]));
        var file = directory.resolve("r." + Math.floorDiv(chunkPos.x(), 32) + "." + Math.floorDiv(chunkPos.z(), 32) + ".mca");
        try (var region = new RegionFile(new RegionStorageInfo("verification", null, "region"), file, directory, RegionFileVersion.VERSION_DEFLATE, false)) {
            CompoundTag original;
            try (var input = region.getChunkDataInputStream(chunkPos)) {
                if (input == null) throw new AssertionError("saved chunk missing: " + chunkPos);
                original = NbtIo.read((DataInput) input);
            }
            var first = SerializableChunkData.parse(LevelHeightAccessor.create(-64, 384), factory, original);
            if (first == null) throw new AssertionError("Java could not parse chunk: " + chunkPos);
            structureTypes.addAll(original.getCompoundOrEmpty("structures").getCompoundOrEmpty("starts").keySet());
            structures.verify(original.getCompoundOrEmpty("structures"), chunkPos, seed, true);
            var rewritten = first.write();
            var second = SerializableChunkData.parse(LevelHeightAccessor.create(-64, 384), factory, rewritten);
            if (second == null || first.sectionData().size() != second.sectionData().size()) throw new AssertionError("section count differs");
            structures.verify(rewritten.getCompoundOrEmpty("structures"), chunkPos, seed, false);
            for (var before : first.sectionData()) {
                var after = second.sectionData().stream().filter(section -> section.y() == before.y()).findFirst().orElseThrow();
                if ((before.chunkSection() == null) != (after.chunkSection() == null)) throw new AssertionError("section changed");
                if (before.chunkSection() != null) {
                    for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
                        if (before.chunkSection().getBlockState(x,y,z) != after.chunkSection().getBlockState(x,y,z)) throw new AssertionError("block changed");
                    for (int y = 0; y < 4; y++) for (int z = 0; z < 4; z++) for (int x = 0; x < 4; x++)
                        if (!before.chunkSection().getNoiseBiome(x,y,z).equals(after.chunkSection().getNoiseBiome(x,y,z))) throw new AssertionError("biome changed");
                }
                if (!sameLight(before.blockLight(), after.blockLight()) || !sameLight(before.skyLight(), after.skyLight())) throw new AssertionError("light changed");
            }
            if (!Objects.equals(first.structureData(), second.structureData()) || !Objects.equals(first.blockEntities(), second.blockEntities()))
                throw new AssertionError("decoded chunk metadata changed");
            for (var key : new String[]{"structures", "block_entities", "block_ticks", "fluid_ticks", "Heightmaps", "PostProcessing"})
                if (!Objects.equals(original.get(key), rewritten.get(key))) throw new AssertionError("chunk " + chunkPos + " field changed: " + key);
            try (var output = region.getChunkDataOutputStream(chunkPos)) { NbtIo.write(rewritten, (DataOutput) output); }
        }
        }
        structures.finish();
        int entityCount = verifySidecars(world, "entities", Path.of(args[9]));
        verifySidecars(world, "poi", Path.of(args[10]));
        out.println("STRUCTURE_TYPES " + String.join(",", structureTypes));
        out.println("STRUCTURES " + structures.portalStarts + " " + structures.portalReferences + " "
                + structures.mineshaftStarts + " " + structures.mineshaftReferences);
        out.println("ENTITIES " + entityCount);
        out.println("OK " + positions.size() + " " + Files.readAllLines(Path.of(args[9])).size() + " " + Files.readAllLines(Path.of(args[10])).size());
    }
}
