import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.FluidTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.material.Fluids;

/** Run Java 26.3's actual spreading-soil random tick on a controlled level. */
public class SpreadingSnowyOracle {
    static class World extends ServerLevel {
        Map<BlockPos, BlockState> blocks;
        JsonArray writes;
        int brightness;
        RandomSource callbackRandom;
        boolean consumeRandom;
        RegistryAccess access;
        World() { super(null,null,null,null,null,null,false,0,List.of(),false); }
        @Override public RegistryAccess registryAccess() { return access; }
        @Override public BlockState getBlockState(BlockPos p) {
            return blocks.getOrDefault(p, Blocks.AIR.defaultBlockState());
        }
        @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos p) {
            return getBlockState(p).getFluidState();
        }
        @Override public int getMaxLocalRawBrightness(BlockPos p) { return brightness; }
        @Override public boolean setBlockAndUpdate(BlockPos p, BlockState state) {
            p = p.immutable();
            blocks.put(p, state);
            var row = new JsonArray();
            row.add(p.getX()); row.add(p.getY()); row.add(p.getZ());
            row.add(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
            row.add(state.hasProperty(SnowyBlock.SNOWY) && state.getValue(SnowyBlock.SNOWY));
            writes.add(row);
            if (consumeRandom) callbackRandom.nextInt();
            return true;
        }
    }

    static BlockState cover(int index) {
        return switch (index) {
            case 0 -> Blocks.AIR.defaultBlockState();
            case 1 -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 1);
            case 2 -> Blocks.WATER.defaultBlockState();
            case 3 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
            case 4 -> Blocks.STONE.defaultBlockState();
            case 5 -> Blocks.GLASS.defaultBlockState();
            case 6 -> Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP);
            case 7 -> Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
            case 8 -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 8);
            case 9 -> Blocks.OAK_SLAB.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED, true);
            default -> throw new IllegalArgumentException();
        };
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bind.setAccessible(true);
        bind.invoke(Blocks.SNOW.builtInRegistryHolder(), List.of(BlockTags.SNOW));
        bind.invoke(Blocks.SNOW_BLOCK.builtInRegistryHolder(), List.of(BlockTags.SNOW));
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        world.access = new RegistryAccess.ImmutableRegistryAccess(List.of(BuiltInRegistries.BLOCK));
        var tick = SpreadingSnowyBlock.class.getDeclaredMethod("randomTick", BlockState.class,
                ServerLevel.class, BlockPos.class, RandomSource.class);
        tick.setAccessible(true);
        var rows = new JsonArray();
        var origin = new BlockPos(0, 8, 0);
        for (int tag = 0; tag < 3; tag++) {
            bind.invoke(Fluids.WATER.builtInRegistryHolder(), tag == 0 ? List.of(FluidTags.WATER) : List.of());
            bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(), tag == 0 ? List.of(FluidTags.WATER) : List.of());
            bind.invoke(Fluids.EMPTY.builtInRegistryHolder(), tag == 1 ? List.of(FluidTags.WATER) : List.of());
            for (int kind = 0; kind < 2; kind++) for (int cover = 0; cover < 10; cover++)
            for (int brightness : new int[]{0, 8, 9, 15}) for (int seed = 0; seed < 32; seed++)
            for (boolean snowy : new boolean[]{false, true}) for (boolean consume : new boolean[]{false, true}) {
                world.blocks = new HashMap<>(); world.writes = new JsonArray(); world.brightness = brightness;
                for (int x = -1; x <= 1; x++) for (int z = -1; z <= 1; z++) {
                    world.blocks.put(new BlockPos(x, 7, z), Blocks.DIRT.defaultBlockState());
                    if (x != 0 || z != 0) world.blocks.put(new BlockPos(x, 8, z), cover(Math.floorMod((x+1)*3 + z+1 + seed, 5)));
                }
                BlockState source = (kind == 0 ? Blocks.GRASS_BLOCK : Blocks.MYCELIUM).defaultBlockState().setValue(SnowyBlock.SNOWY, snowy);
                world.blocks.put(origin, source);
                world.blocks.put(origin.above(), cover(cover));
                RandomSource random = RandomSource.create(seed);
                world.callbackRandom = random; world.consumeRandom = consume;
                tick.invoke(source.getBlock(), source, world, origin, random);
                var row = new JsonObject();
                row.addProperty("tag", tag); row.addProperty("kind", kind); row.addProperty("cover", cover);
                row.addProperty("brightness", brightness); row.addProperty("seed", seed);
                row.addProperty("snowy", snowy); row.addProperty("consume", consume);
                row.add("writes", world.writes); row.addProperty("next", random.nextInt());
                rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
