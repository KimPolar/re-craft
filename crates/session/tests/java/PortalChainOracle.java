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
import net.minecraft.world.level.block.RedStoneOreBlock;
import net.minecraft.world.level.block.WallBlock;
import net.minecraft.world.level.block.IronBarsBlock;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.ticks.ProtoChunkTicks;
import net.minecraft.world.ticks.ScheduledTick;
import net.minecraft.resources.Identifier;

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
        var ores = List.of("coal_ore", "deepslate_coal_ore", "iron_ore", "deepslate_iron_ore",
            "copper_ore", "deepslate_copper_ore", "gold_ore", "deepslate_gold_ore",
            "emerald_ore", "deepslate_emerald_ore", "lapis_ore", "deepslate_lapis_ore",
            "diamond_ore", "deepslate_diamond_ore", "nether_gold_ore", "nether_quartz_ore",
            "redstone_ore", "deepslate_redstone_ore", "raw_iron_block", "raw_copper_block");
        for (var name : ores) {
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.withDefaultNamespace(name));
            for (int lit = 0; lit < (block instanceof RedStoneOreBlock ? 2 : 1); lit++) {
                var state = block.defaultBlockState();
                if (block instanceof RedStoneOreBlock) state = state.setValue(RedStoneOreBlock.LIT, lit != 0);
                if (!state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)
                        || !state.getFluidState().isEmpty() || state.hasBlockEntity()
                        || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                        || state.getPostProcessPos(EmptyBlockGetter.INSTANCE, BlockPos.ZERO) != null)
                    throw new AssertionError("unexpected ore properties " + state);
                for (var direction : Direction.values()) for (var neighbor : List.of(Blocks.AIR, Blocks.WATER, Blocks.MAGMA_BLOCK)) {
                    if (!state.isFaceSturdy(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, direction))
                        throw new AssertionError("ore face is not sturdy " + state + " " + direction);
                    ticks.clear();
                    var random = RandomSource.create(17);
                    if (state.updateShape(world, world, BlockPos.ZERO, direction, BlockPos.ZERO.relative(direction),
                            neighbor.defaultBlockState(), random) != state || !ticks.isEmpty())
                        throw new AssertionError("unexpected ore edge update " + state);
                    var wall = Blocks.STONE_BRICK_WALL.defaultBlockState();
                    var bars = Blocks.IRON_BARS.defaultBlockState();
                    String side = "-", up = "-", pane = "-";
                    if (direction.getAxis().isHorizontal()) {
                        var wallResult = wall.updateShape(world, world, BlockPos.ZERO, direction, BlockPos.ZERO.relative(direction), state, random);
                        var barsResult = bars.updateShape(world, world, BlockPos.ZERO, direction, BlockPos.ZERO.relative(direction), state, random);
                        side = wallResult.getValue(WallBlock.PROPERTY_BY_DIRECTION.get(direction)).getSerializedName();
                        up = wallResult.getValue(WallBlock.UP).toString();
                        pane = barsResult.getValue(IronBarsBlock.PROPERTY_BY_DIRECTION.get(direction)).toString();
                    }
                    if (random.nextInt() != RandomSource.create(17).nextInt())
                        throw new AssertionError("ore edge consumed random values");
                    System.out.printf("ORE %s %s %s %s %s %s %s%n", BuiltInRegistries.BLOCK.getKey(block),
                        block instanceof RedStoneOreBlock ? Boolean.toString(lit != 0) : "-",
                        direction.getSerializedName(), BuiltInRegistries.BLOCK.getKey(neighbor), side, up, pane);
                }
            }
        }
    }
}
