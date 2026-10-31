import java.nio.file.Path;
import java.util.Optional;
import net.minecraft.core.BlockPos;
import net.minecraft.core.GlobalPos;
import net.minecraft.core.registries.Registries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.item.component.LodestoneTracker;
import net.minecraft.world.level.Level;

public class LodestoneTrackerOracle {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        var registries = net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
        for (var pending : net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var custom = ResourceKey.<Level>create(Registries.DIMENSION, Identifier.parse("example:deep/cave"));
        var cases = new LodestoneTracker[] {
            new LodestoneTracker(Optional.of(GlobalPos.of(Level.OVERWORLD, new BlockPos(-123, 64, 456))), true),
            new LodestoneTracker(Optional.of(GlobalPos.of(Level.NETHER, new BlockPos(0, -64, 0))), false),
            new LodestoneTracker(Optional.of(GlobalPos.of(custom, new BlockPos(30000000, 319, -30000000))), true),
            new LodestoneTracker(Optional.empty(), true),
            new LodestoneTracker(Optional.empty(), false)
        };
        var result = new CompoundTag();
        result.putInt("compass_max", new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.COMPASS).getMaxStackSize());
        for (int i = 0; i < cases.length; i++) {
            var encoded = LodestoneTracker.CODEC.encodeStart(NbtOps.INSTANCE, cases[i]).getOrThrow();
            if (!LodestoneTracker.CODEC.parse(NbtOps.INSTANCE, encoded).getOrThrow().equals(cases[i]))
                throw new AssertionError("codec round trip " + i);
            result.put("case" + i, encoded);
        }
        NbtIo.write(result, Path.of(args[0]));
    }
}
