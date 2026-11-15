import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.lang.reflect.Proxy;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.*;
import net.minecraft.world.ticks.*;

/** Exercise real LiquidBlock.updateShape and ProtoChunkTicks in 26.3. */
public class PortalLiquidEdgeOracle {
    static final BlockPos POSITION = new BlockPos(3, 7, 3);
    static void configure(int mask, int support) {
        var types = new Fluid[]{Fluids.EMPTY, Fluids.WATER, Fluids.FLOWING_WATER, Fluids.LAVA, Fluids.FLOWING_LAVA};
        var fluids = new ArrayList<Holder<Fluid>>();
        for (int i = 0; i < types.length; i++) if ((mask & (1 << i)) != 0)
            fluids.add(types[i].builtInRegistryHolder());
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,
            Map.of(FluidTags.BUBBLE_COLUMN_CAN_OCCUPY, fluids))).apply();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,
                List.of((support == 0 ? Blocks.MAGMA_BLOCK : Blocks.STONE).builtInRegistryHolder()),
            BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,
                List.of((support == 0 ? Blocks.SOUL_SAND : Blocks.MAGMA_BLOCK).builtInRegistryHolder())))).apply();
    }
    static void tick(JsonObject row, String prefix, List<? extends SavedTick<?>> ticks) {
        row.addProperty(prefix + "Count", ticks.size());
        if (!ticks.isEmpty()) {
            var tick = ticks.get(0);
            var id = tick.type() instanceof Fluid fluid ? BuiltInRegistries.FLUID.getKey(fluid)
                : BuiltInRegistries.BLOCK.getKey((Block) tick.type());
            row.addProperty(prefix + "Id", id.toString());
            row.addProperty(prefix + "Delay", tick.delay());
            row.addProperty(prefix + "Priority", tick.priority().getValue());
            if (!tick.pos().equals(POSITION)) throw new AssertionError("wrong tick position");
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var attrs = EnvironmentAttributeSystem.builder().build();
        var neighbors = List.of(Blocks.AIR.defaultBlockState(), Blocks.STONE.defaultBlockState(),
            Blocks.WATER.defaultBlockState(), Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1),
            Blocks.LAVA.defaultBlockState(), Blocks.MAGMA_BLOCK.defaultBlockState(), Blocks.SOUL_SAND.defaultBlockState());
        var rows = new JsonArray();
        for (int mask : new int[]{0, 2, 4, 8, 10, 16, 30}) for (int support = 0; support < 2; support++) {
            configure(mask, support);
            for (Block block : List.of(Blocks.WATER, Blocks.LAVA)) for (int level = 0; level < 16; level++)
                for (Direction direction : Direction.values()) for (BlockState neighbor : neighbors) {
                    var fluidTicks = new ProtoChunkTicks<Fluid>();
                    var blockTicks = new ProtoChunkTicks<Block>();
                    var requests = new ArrayList<String>();
                    var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                        new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, values) -> switch (method.getName()) {
                            case "environmentAttributes" -> attrs;
                            case "scheduleTick" -> {
                                var position = (BlockPos) values[0];
                                int delay = (int) values[2];
                                if (values[1] instanceof Fluid fluid) {
                                    requests.add("fluid:" + delay);
                                    fluidTicks.schedule(new ScheduledTick<>(fluid, position, delay, 0L));
                                } else {
                                    requests.add("block:" + delay);
                                    blockTicks.schedule(new ScheduledTick<>((Block) values[1], position, delay, 0L));
                                }
                                yield null;
                            }
                            default -> throw new UnsupportedOperationException(method.toString());
                        });
                    var state = block.defaultBlockState().setValue(LiquidBlock.LEVEL, level);
                    var random = RandomSource.create(17);
                    for (int repeat = 0; repeat < 2; repeat++) {
                        var updated = state.updateShape(world, world, POSITION, direction,
                            POSITION.relative(direction), neighbor, random);
                        if (updated != state) throw new AssertionError("liquid state changed");
                    }
                    if (random.nextInt() != RandomSource.create(17).nextInt())
                        throw new AssertionError("liquid edge consumed random values");
                    var row = new JsonObject();
                    row.addProperty("mask", mask); row.addProperty("support", support);
                    row.addProperty("block", BuiltInRegistries.BLOCK.getKey(block).toString());
                    row.addProperty("level", level); row.addProperty("direction", direction.getSerializedName());
                    row.addProperty("neighbor", BuiltInRegistries.BLOCK.getKey(neighbor.getBlock()).toString());
                    row.addProperty("neighborLevel", neighbor.hasProperty(LiquidBlock.LEVEL) ? neighbor.getValue(LiquidBlock.LEVEL) : -1);
                    var calls = new JsonArray(); for (var request : requests) calls.add(request); row.add("requests", calls);
                    tick(row, "fluid", fluidTicks.pack(0)); tick(row, "block", blockTicks.pack(0));
                    rows.add(row);
                }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
