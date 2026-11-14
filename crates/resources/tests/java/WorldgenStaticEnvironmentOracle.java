import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.mojang.serialization.Lifecycle;
import java.lang.reflect.Proxy;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.RegistrationInfo;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.attribute.EnvironmentAttributeMap;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.attribute.EnvironmentAttributes;
import net.minecraft.world.clock.ClockManager;
import net.minecraft.world.clock.ClockState;
import net.minecraft.world.clock.ServerClockManager;
import net.minecraft.world.clock.WorldClock;
import net.minecraft.world.entity.MobCategory;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.MoonPhase;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.timeline.Timeline;

public class WorldgenStaticEnvironmentOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var biomeRegistry = new MappedRegistry<Biome>(Registries.BIOME, Lifecycle.stable());
        lookup.lookupOrThrow(Registries.BIOME).listElements().forEach(holder ->
            biomeRegistry.register(holder.key(), holder.value(), RegistrationInfo.BUILT_IN));
        biomeRegistry.freeze();
        var registryAccess = new RegistryAccess.ImmutableRegistryAccess(List.of(biomeRegistry));
        var plains = biomeRegistry.getOrThrow(ResourceKey.create(Registries.BIOME,
            Identifier.parse("minecraft:plains")));
        var biomeManager = new BiomeManager((x, y, z) -> plains, 0L);
        var original = lookup.lookupOrThrow(Registries.DIMENSION_TYPE).getOrThrow(
            ResourceKey.create(Registries.DIMENSION_TYPE, Identifier.parse("minecraft:overworld"))).value();
        var attributes = EnvironmentAttributeMap.builder()
            .set(EnvironmentAttributes.CREATURE_WORLD_GEN_SPAWN_PROBABILITY, 0.35f)
            .set(EnvironmentAttributes.MOON_PHASE, MoonPhase.NEW_MOON)
            .build();
        var timeline = Timeline.builder(Holder.direct(new WorldClock())).setPeriodTicks(20)
            .addTrack(EnvironmentAttributes.CREATURE_WORLD_GEN_SPAWN_PROBABILITY,
                track -> track.addKeyframe(0, 0.9f).addKeyframe(10, 0.7f))
            .addTrack(EnvironmentAttributes.MOON_PHASE,
                track -> track.addKeyframe(0, MoonPhase.FULL_MOON).addKeyframe(10, MoonPhase.THIRD_QUARTER))
            .build();
        var dimension = new DimensionType(original.hasFixedTime(), original.hasSkyLight(),
            original.hasCeiling(), original.hasEnderDragonFight(), original.coordinateScale(),
            original.minY(), original.height(), original.logicalHeight(), original.infiniburn(),
            original.ambientLight(), original.monsterSettings(), original.skybox(),
            original.cardinalLightType(), attributes, HolderSet.direct(Holder.direct(timeline)),
            original.defaultClock());
        var level = (LevelAccessor) Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),
            new Class<?>[] {LevelAccessor.class}, (proxy, method, arguments) -> switch (method.getName()) {
                case "registryAccess" -> registryAccess;
                case "getBiomeManager" -> biomeManager;
                case "dimensionType" -> dimension;
                default -> throw new UnsupportedOperationException(method.getName());
            });
        var clock = new ServerClockManager.ServerClockInstance();
        ClockManager manager = ignored -> clock;
        var worldgen = EnvironmentAttributeSystem.builder().addStaticLayers(level).build();
        var dynamic = EnvironmentAttributeSystem.builder().addStaticLayers(level)
            .addTimelineLayer(Holder.direct(timeline), manager).build();
        var position = new BlockPos(0, 319, 0);
        var output = new JsonArray();
        for (long tick : new long[] {0, 5, 10, 19, 20}) {
            clock.loadFrom(new ClockState(tick, 0, 1, false));
            worldgen.invalidateTickCache();
            dynamic.invalidateTickCache();
            var row = new JsonObject();
            row.addProperty("tick", tick);
            row.addProperty("probability", Float.floatToRawIntBits(worldgen.getValue(
                EnvironmentAttributes.CREATURE_WORLD_GEN_SPAWN_PROBABILITY, position)));
            row.addProperty("moon", worldgen.getValue(EnvironmentAttributes.MOON_PHASE, position).index());
            var choice = worldgen.getValue(EnvironmentAttributes.NATURAL_MOB_SPAWNS, position)
                .getMobsToSpawn(MobCategory.CREATURE).getRandom(new LegacyRandomSource(42));
            row.addProperty("entity", choice.map(entry -> BuiltInRegistries.ENTITY_TYPE
                .getKey(entry.type()).toString()).orElse("empty"));
            row.addProperty("dynamic_probability", Float.floatToRawIntBits(dynamic.getValue(
                EnvironmentAttributes.CREATURE_WORLD_GEN_SPAWN_PROBABILITY, position)));
            row.addProperty("dynamic_moon", dynamic.getValue(EnvironmentAttributes.MOON_PHASE, position).index());
            output.add(row);
        }
        Files.writeString(Path.of(args[0]), output.toString());
    }
}
