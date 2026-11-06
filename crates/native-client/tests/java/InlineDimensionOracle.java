import java.nio.file.Path;
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
    }
}
