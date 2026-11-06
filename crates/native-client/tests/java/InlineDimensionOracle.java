import java.nio.file.Path;
import java.nio.file.Files;
import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.dimension.DimensionType;

public class InlineDimensionOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registry = lookup.lookupOrThrow(Registries.DIMENSION_TYPE);
        var ops = lookup.createSerializationContext(NbtOps.INSTANCE);
        for (String name : new String[] {"overworld", "the_nether", "the_end"}) {
            var key = ResourceKey.create(Registries.DIMENSION_TYPE, Identifier.parse("minecraft:" + name));
            var dimension = registry.getOrThrow(key).value();
            var encoded = DimensionType.DIRECT_CODEC.encodeStart(ops, dimension).getOrThrow();
            NbtIo.write((CompoundTag) encoded, Path.of(args[0], name + ".nbt"));
        }
        var overworld = registry.getOrThrow(ResourceKey.create(Registries.DIMENSION_TYPE,
            Identifier.parse("minecraft:overworld"))).value();
        var jsonOps = lookup.createSerializationContext(JsonOps.INSTANCE);
        var custom = DimensionType.DIRECT_CODEC.encodeStart(jsonOps, overworld).getOrThrow().getAsJsonObject();
        custom.add("attributes", JsonParser.parseString("""
            {"minecraft:visual/sky_light_color":"#abcdef",
             "minecraft:visual/fog_color":"#fedcba",
             "minecraft:visual/sunrise_sunset_color":"#80ff8040",
             "minecraft:gameplay/fast_lava":{"modifier":"xor","argument":true},
             "minecraft:gameplay/bed_rule":{"can_sleep":"never","can_set_spawn":"always","destroy_on_use":true}}
            """));
        var parsed = DimensionType.DIRECT_CODEC.parse(jsonOps, custom).getOrThrow();
        var encoded = DimensionType.DIRECT_CODEC.encodeStart(ops, parsed).getOrThrow();
        NbtIo.write((CompoundTag) encoded, Path.of(args[0], "custom.nbt"));
        Files.writeString(Path.of(args[0], "custom.json"), custom.toString());
    }
}
