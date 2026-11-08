import java.lang.reflect.Proxy;
import java.util.ArrayList;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.lighting.LightEngine;

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
    }
}
