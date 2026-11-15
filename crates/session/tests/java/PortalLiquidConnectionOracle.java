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
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.material.*;
import net.minecraft.world.ticks.*;

/** Compare portal wall and pane updates against liquid neighbors. */
public class PortalLiquidConnectionOracle {
    static final BlockPos POSITION = new BlockPos(3, 7, 3);
    static final Direction[] SIDES = {Direction.NORTH, Direction.EAST, Direction.SOUTH, Direction.WEST};
    static void tags(int mode, Block liquid) {
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.WALLS, (mode & 1) != 0 ? List.of(liquid.builtInRegistryHolder()) : List.of(),
            BlockTags.WALL_POST_OVERRIDE, (mode & 2) != 0 ? List.of(liquid.builtInRegistryHolder()) : List.of()))).apply();
    }
    static void properties(JsonObject row, String prefix, BlockState state, boolean wall) {
        for (var direction : SIDES) {
            var name = direction.getSerializedName();
            var value = wall ? state.getValue(WallBlock.PROPERTY_BY_DIRECTION.get(direction)).getSerializedName()
                : state.getValue(IronBarsBlock.PROPERTY_BY_DIRECTION.get(direction)).toString();
            row.addProperty(prefix + name, value);
        }
        if (wall) row.addProperty(prefix + "up", state.getValue(WallBlock.UP).toString());
        row.addProperty(prefix + "waterlogged", state.getValue(BlockStateProperties.WATERLOGGED).toString());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var rows = new JsonArray();
        for (var liquid : List.of(Blocks.WATER, Blocks.LAVA)) for (int mode = 0; mode < 4; mode++) {
            tags(mode, liquid);
            for (var block : List.of(Blocks.STONE_BRICK_WALL, Blocks.MOSSY_STONE_BRICK_WALL, Blocks.IRON_BARS)) {
                boolean wall = block instanceof WallBlock;
                for (int mask : new int[]{0, 1, 3, 5, 7, 10, 14, 15}) for (int up = 0; up < (wall ? 2 : 1); up++)
                    for (int wet = 0; wet < 2; wet++) for (Direction direction : Direction.values())
                        for (int level : new int[]{0, 1, 8, 15})
                            for (var above : List.of(Blocks.AIR, Blocks.STONE, liquid)) {
                                var state = block.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED, wet != 0);
                                if (wall) state = state.setValue(WallBlock.UP, up != 0);
                                for (int i = 0; i < SIDES.length; i++) {
                                    boolean present = (mask & (1 << i)) != 0;
                                    if (wall) state = state.setValue(WallBlock.PROPERTY_BY_DIRECTION.get(SIDES[i]),
                                        present ? (((mask >> ((i + 2) % 4)) & 1) == 0 ? WallSide.LOW : WallSide.TALL) : WallSide.NONE);
                                    else state = state.setValue(IronBarsBlock.PROPERTY_BY_DIRECTION.get(SIDES[i]), present);
                                }
                                var neighbor = liquid.defaultBlockState().setValue(LiquidBlock.LEVEL, level);
                                var ticks = new ProtoChunkTicks<Fluid>();
                                var aboveState = above.defaultBlockState();
                                var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                                    new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, values) -> switch (method.getName()) {
                                        case "getBlockState" -> ((BlockPos) values[0]).equals(POSITION.above()) ? aboveState : Blocks.AIR.defaultBlockState();
                                        case "scheduleTick" -> {
                                            var fluid = (Fluid) values[1];
                                            ticks.schedule(new ScheduledTick<>(fluid, (BlockPos) values[0], (int) values[2], 0L));
                                            yield null;
                                        }
                                        default -> throw new UnsupportedOperationException(method.toString());
                                    });
                                var random = RandomSource.create(17);
                                var updated = state.updateShape(world, world, POSITION, direction,
                                    POSITION.relative(direction), neighbor, random);
                                if (random.nextInt() != RandomSource.create(17).nextInt())
                                    throw new AssertionError("wall or pane update consumed random values");
                                var row = new JsonObject();
                                row.addProperty("block", BuiltInRegistries.BLOCK.getKey(block).toString());
                                row.addProperty("liquid", BuiltInRegistries.BLOCK.getKey(liquid).toString());
                                row.addProperty("mode", mode); row.addProperty("direction", direction.getSerializedName());
                                row.addProperty("level", level); row.addProperty("above", BuiltInRegistries.BLOCK.getKey(above).toString());
                                properties(row, "before_", state, wall); properties(row, "after_", updated, wall);
                                var saved = ticks.pack(0);
                                row.addProperty("ticks", saved.size());
                                if (!saved.isEmpty()) {
                                    var tick = saved.get(0);
                                    row.addProperty("tickId", BuiltInRegistries.FLUID.getKey(tick.type()).toString());
                                    row.addProperty("delay", tick.delay()); row.addProperty("priority", tick.priority().getValue());
                                    if (!tick.pos().equals(POSITION)) throw new AssertionError("wrong tick position");
                                }
                                rows.add(row);
                            }
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
