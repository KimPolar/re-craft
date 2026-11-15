import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.ticks.ProtoChunkTicks;
import net.minecraft.world.ticks.ScheduledTick;

public class PortalChainOracle {
    public static void main(String[] args) {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var ticks = new ArrayList<String>();
        var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
            new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, values) -> switch (method.getName()) {
                case "scheduleTick" -> { ticks.add(BuiltInRegistries.FLUID.getKey((Fluid) values[1]) + ":" + values[2]); yield null; }
                case "getBlockState" -> Blocks.AIR.defaultBlockState();
                case "getFluidState" -> Blocks.AIR.defaultBlockState().getFluidState();
                default -> throw new UnsupportedOperationException(method.toString());
            });
        for (var axis : Direction.Axis.values()) for (boolean wet : new boolean[]{false, true}) {
            var state = Blocks.IRON_CHAIN.defaultBlockState().setValue(BlockStateProperties.AXIS, axis)
                .setValue(BlockStateProperties.WATERLOGGED, wet);
            var boxes = state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO).toAabbs();
            if (boxes.size() != 1) throw new AssertionError("unexpected collision shape " + state + " " + boxes);
            var box = boxes.get(0);
            System.out.printf("SHAPE %s %s %.4f %.4f %.4f %.4f %.4f %.4f%n", axis.getSerializedName(), wet,
                box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ);
            System.out.printf("LIGHT %s %s %d %d %s %s %s %s%n", axis.getSerializedName(), wet,
                state.getLightDampening(), state.getLightEmission(), state.useShapeForLightOcclusion(),
                LightEngine.hasDifferentLightProperties(state, Blocks.AIR.defaultBlockState()),
                LightEngine.hasDifferentLightProperties(state, Blocks.WATER.defaultBlockState()),
                LightEngine.hasDifferentLightProperties(state, Blocks.IRON_BARS.defaultBlockState()));
            for (var direction : Direction.values()) {
                ticks.clear();
                var result = state.updateShape(world, world, BlockPos.ZERO, direction, BlockPos.ZERO.relative(direction),
                    Blocks.AIR.defaultBlockState(), RandomSource.create(17));
                System.out.printf("UPDATE %s %s %s %s %s%n", axis.getSerializedName(), wet,
                    direction.getSerializedName(), result == state, String.join(",", ticks));
                if (direction.getAxis().isHorizontal()) {
                    for (var block : new net.minecraft.world.level.block.Block[]{Blocks.STONE_BRICK_WALL, Blocks.IRON_BARS}) {
                        var neighbor = block.defaultBlockState();
                        var updated = neighbor.updateShape(world, world, BlockPos.ZERO, direction,
                            BlockPos.ZERO.relative(direction), state, RandomSource.create(17));
                        if (updated != neighbor) throw new AssertionError("unexpected connection to chain: " + state + " " + direction + " " + updated);
                    }
                }
            }
        }
        for (var block : List.of(Blocks.SAND, Blocks.RED_SAND, Blocks.GRAVEL))
            for (var direction : Direction.values())
                for (var neighbor : List.of(Blocks.AIR, Blocks.STONE, Blocks.WATER, Blocks.OBSIDIAN)) {
                    var proto = new ProtoChunkTicks<Block>();
                    var delays = new ArrayList<Integer>();
                    var fallingWorld = (ServerLevelAccessor) Proxy.newProxyInstance(
                        ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class},
                        (proxy, method, values) -> switch (method.getName()) {
                            case "scheduleTick" -> {
                                var scheduled = (Block) values[1];
                                int delay = (int) values[2];
                                delays.add(delay);
                                proto.schedule(new ScheduledTick<>(scheduled, (BlockPos) values[0], delay, 0L));
                                yield null;
                            }
                            default -> throw new UnsupportedOperationException(method.toString());
                        });
                    var state = block.defaultBlockState();
                    var pos = new BlockPos(3, 7, 3);
                    var random = RandomSource.create(17);
                    for (int i = 0; i < 2; i++) {
                        var result = state.updateShape(fallingWorld, fallingWorld, pos, direction,
                            pos.relative(direction), neighbor.defaultBlockState(), random);
                        if (result != state) throw new AssertionError("falling block state changed");
                    }
                    if (random.nextInt() != RandomSource.create(17).nextInt())
                        throw new AssertionError("falling edge consumed random values");
                    var saved = proto.pack(0);
                    if (!delays.equals(List.of(2, 2)) || saved.size() != 1)
                        throw new AssertionError("unexpected falling tick requests " + delays + " " + saved);
                    var tick = saved.get(0);
                    if (tick.type() != block || !tick.pos().equals(pos))
                        throw new AssertionError("unexpected falling tick identity " + tick);
                    System.out.printf("FALLING %s %s %s %d %d %d%n",
                        BuiltInRegistries.BLOCK.getKey(block), direction.getSerializedName(),
                        BuiltInRegistries.BLOCK.getKey(neighbor), delays.get(0), tick.delay(), tick.priority().getValue());
                }
    }
}
