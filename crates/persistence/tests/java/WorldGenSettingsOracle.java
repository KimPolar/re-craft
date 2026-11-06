import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.RegistrationInfo;
import com.mojang.serialization.Lifecycle;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.WorldDimensions;
import net.minecraft.world.level.levelgen.WorldGenSettings;
import net.minecraft.world.level.levelgen.WorldOptions;

public class WorldGenSettingsOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var presets = lookup.lookupOrThrow(Registries.WORLD_PRESET);
        var ops = lookup.createSerializationContext(NbtOps.INSTANCE);
        var names = new String[] {"normal", "large_biomes", "amplified", "flat", "flat_all_dimensions", "single_biome_surface", "debug_all_block_states", "custom"};
        long[] seeds = {Long.MIN_VALUE, -42L, 0L, Long.MAX_VALUE};
        for (String name : names) {
            var key = ResourceKey.create(Registries.WORLD_PRESET, Identifier.parse("minecraft:" + (name.equals("custom") ? "normal" : name)));
            var dimensions = presets.getOrThrow(key).value().createWorldDimensions();
            if (name.equals("custom")) {
                var map = new HashMap<>(dimensions.dimensions());
                map.put(ResourceKey.create(Registries.LEVEL_STEM, Identifier.parse("example:sky")), map.get(LevelStem.END));
                dimensions = new WorldDimensions(map);
            }
            for (int seedIndex = 0; seedIndex < seeds.length; seedIndex++) {
                for (int flags = 0; flags < 4; flags++) {
                    var root = NbtIo.read(Path.of(args[0], name + "-" + seedIndex + "-" + flags + ".nbt"));
                    var actual = WorldGenSettings.CODEC.parse(ops, root.get("data")).getOrThrow();
                    var expected = new WorldGenSettings(new WorldOptions(seeds[seedIndex], (flags & 1) != 0, (flags & 2) != 0), dimensions);
                    var actualTag = WorldGenSettings.CODEC.encodeStart(ops, actual).getOrThrow();
                    var expectedTag = WorldGenSettings.CODEC.encodeStart(ops, expected).getOrThrow();
                    if (!actualTag.equals(expectedTag)) throw new AssertionError(name + " " + seedIndex + " " + flags + "\nactual=" + actualTag + "\nexpected=" + expectedTag);
                }
            }
        }
        var normal = presets.getOrThrow(ResourceKey.create(Registries.WORLD_PRESET, Identifier.parse("minecraft:normal"))).value().createWorldDimensions();
        var nether = normal.dimensions().get(LevelStem.NETHER);
        var end = normal.dimensions().get(LevelStem.END);
        var extraKey = ResourceKey.create(Registries.LEVEL_STEM, Identifier.parse("example:sky"));
        var active = new MappedRegistry<LevelStem>(Registries.LEVEL_STEM, Lifecycle.experimental());
        active.register(LevelStem.OVERWORLD, new LevelStem(nether.type(), nether.generator()), RegistrationInfo.BUILT_IN);
        active.register(extraKey, new LevelStem(end.type(), end.generator()), RegistrationInfo.BUILT_IN);
        var baked = normal.bake(active.freeze()).dimensions();
        var expected = new WorldGenSettings(new WorldOptions(-42L, false, true), new WorldDimensions(baked));
        var root = NbtIo.read(Path.of(args[0], "baked.nbt"));
        var actual = WorldGenSettings.CODEC.parse(ops, root.get("data")).getOrThrow();
        var actualTag = WorldGenSettings.CODEC.encodeStart(ops, actual).getOrThrow();
        var expectedTag = WorldGenSettings.CODEC.encodeStart(ops, expected).getOrThrow();
        if (!actualTag.equals(expectedTag)) throw new AssertionError("baked dimensions\nactual=" + actualTag + "\nexpected=" + expectedTag);
    }
}
