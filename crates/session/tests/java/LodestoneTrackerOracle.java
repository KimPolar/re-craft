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
        result.putInt("recovery_max", new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.RECOVERY_COMPASS).getMaxStackSize());
        result.put("LastDeathLocation", GlobalPos.CODEC.encodeStart(NbtOps.INSTANCE, GlobalPos.of(custom, new BlockPos(-123, -64, 456))).getOrThrow());
        var deathPositions = new net.minecraft.nbt.ListTag();
        for (double[] point : new double[][] {{-0.01,-64.5,0.99},{16000000.75,71.1,-16000000.99},{-30000000.0,319.99,29999999.99},{0.0,0.0,0.0}}) {
            var position = BlockPos.containing(point[0],point[1],point[2]);
            deathPositions.add(new net.minecraft.nbt.IntArrayTag(new int[]{position.getX(),position.getY(),position.getZ()}));
        }
        result.put("death_positions",deathPositions);
        var variations=new CompoundTag();
        var expected=new CompoundTag();
        var floats=new net.minecraft.nbt.ListTag(); floats.add(net.minecraft.nbt.FloatTag.valueOf(1.9f)); floats.add(net.minecraft.nbt.FloatTag.valueOf(-2.9f)); floats.add(net.minecraft.nbt.FloatTag.valueOf(3.0f));
        var strings=new net.minecraft.nbt.ListTag(); strings.add(net.minecraft.nbt.StringTag.valueOf("bad")); strings.add(net.minecraft.nbt.StringTag.valueOf("bad")); strings.add(net.minecraft.nbt.StringTag.valueOf("bad"));
        net.minecraft.nbt.Tag[] positions={new net.minecraft.nbt.ByteArrayTag(new byte[]{1,-2,3}), new net.minecraft.nbt.LongArrayTag(new long[]{1,-2,3}), floats, strings};
        for(int i=0;i<positions.length;i++) {
            var target=new CompoundTag(); target.putString("dimension","minecraft:overworld"); target.put("pos",positions[i]);
            variations.put("v"+i,target); expected.putBoolean("v"+i,GlobalPos.CODEC.parse(NbtOps.INSTANCE,target).result().isPresent());
        }
        result.put("variations",variations); result.put("variation_results",expected);
        for (int i = 0; i < cases.length; i++) {
            var encoded = LodestoneTracker.CODEC.encodeStart(NbtOps.INSTANCE, cases[i]).getOrThrow();
            if (!LodestoneTracker.CODEC.parse(NbtOps.INSTANCE, encoded).getOrThrow().equals(cases[i]))
                throw new AssertionError("codec round trip " + i);
            result.put("case" + i, encoded);
        }
        NbtIo.write(result, Path.of(args[0]));
    }
}
