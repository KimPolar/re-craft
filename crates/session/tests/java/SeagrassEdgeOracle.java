import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.material.*;
import net.minecraft.world.ticks.*;

/** Exercise real seagrass shape callbacks and prototype fluid tick storage. */
public class SeagrassEdgeOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        out.println("BEGIN_RESULT");
        var attrs = EnvironmentAttributeSystem.builder().build();
        var supports = new ArrayList<>(List.of(
            Blocks.STONE.defaultBlockState(), Blocks.MAGMA_BLOCK.defaultBlockState(),
            Blocks.AIR.defaultBlockState(), Blocks.WATER.defaultBlockState(),
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM),
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP),
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.DOUBLE),
            Blocks.STONE_BRICK_STAIRS.defaultBlockState().setValue(StairBlock.HALF, Half.TOP),
            Blocks.STONE_BRICK_STAIRS.defaultBlockState().setValue(StairBlock.HALF, Half.BOTTOM)));
        for (Block block : List.of(Blocks.SUSPICIOUS_SAND, Blocks.SUSPICIOUS_GRAVEL)) {
            var dusted = (IntegerProperty) block.getStateDefinition().getProperty("dusted");
            for (int value = 0; value < 4; value++) {
                var state = block.defaultBlockState().setValue(dusted, value);
                supports.add(state);
                for (Direction direction : Direction.values())
                    out.printf("S %s %d %s %s%n", BuiltInRegistries.BLOCK.getKey(block), value,
                        direction.getSerializedName(), state.isFaceSturdy(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, direction));
            }
        }
        supports.addAll(List.of(Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE,17),
            Blocks.KELP_PLANT.defaultBlockState(),Blocks.SEAGRASS.defaultBlockState(),
            Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF,DoubleBlockHalf.LOWER),
            Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF,DoubleBlockHalf.UPPER)));
        var neighbors = List.of(Blocks.AIR.defaultBlockState(), Blocks.STONE.defaultBlockState(),
            Blocks.WATER.defaultBlockState());
        var pos = new BlockPos(8, 8, 8);
        for (int mask = 0; mask < 4; mask++) {
            var tagged = new ArrayList<Holder<Block>>();
            if ((mask & 1) != 0) {
                tagged.add(Blocks.STONE.builtInRegistryHolder());
                tagged.add(Blocks.SUSPICIOUS_GRAVEL.builtInRegistryHolder());
            }
            if ((mask & 2) != 0) {
                tagged.add(Blocks.MAGMA_BLOCK.builtInRegistryHolder());
                tagged.add(Blocks.SUSPICIOUS_SAND.builtInRegistryHolder());
            }
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,
                Map.of(BlockTags.CANNOT_SUPPORT_SEAGRASS, tagged))).apply();
            for (int index = 0; index < supports.size(); index++) {
                var support = supports.get(index);
                for (int n = 0; n < neighbors.size(); n++) for (Direction direction : Direction.values()) {
                    var ticks = new ProtoChunkTicks<Fluid>();
                    var requests = new ArrayList<Integer>();
                    var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                        new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, values) -> switch (method.getName()) {
                            case "getBlockState" -> {
                                if (!pos.below().equals(values[0])) throw new AssertionError("unexpected block read " + values[0]);
                                yield support;
                            }
                            case "environmentAttributes" -> attrs;
                            case "scheduleTick" -> {
                                if (!(values[1] instanceof Fluid fluid) || fluid != Fluids.WATER || !pos.equals(values[0]))
                                    throw new AssertionError("unexpected tick");
                                int delay = (int) values[2];
                                requests.add(delay);
                                ticks.schedule(new ScheduledTick<>(fluid, pos, delay, 0L));
                                yield null;
                            }
                            default -> throw new UnsupportedOperationException(method.toString());
                        });
                    var state = Blocks.SEAGRASS.defaultBlockState();
                    var random = RandomSource.create(17);
                    BlockState updated = null;
                    for (int repeat = 0; repeat < 2; repeat++)
                        updated = state.updateShape(world, world, pos, direction, pos.relative(direction), neighbors.get(n), random);
                    if (random.nextInt() != RandomSource.create(17).nextInt())
                        throw new AssertionError("shape update consumed random values");
                    var saved = ticks.pack(0);
                    if (saved.size() > 1) throw new AssertionError("water ticks were not deduplicated");
                    var id = BuiltInRegistries.BLOCK.getKey(updated.getBlock());
                    out.printf("R %d%n", requests.isEmpty() ? -1 : requests.get(0));
                    out.printf("E %d %d %d %s %s %d %d %d %d%n", mask, index, n,
                        direction.getSerializedName(), id, requests.size(), saved.size(),
                        saved.isEmpty() ? -1 : saved.get(0).delay(),
                        saved.isEmpty() ? -1 : saved.get(0).priority().getValue());
                }
            }
        }
        out.println("END_RESULT");
    }
}
