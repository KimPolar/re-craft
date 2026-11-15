import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Holder;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SpeleothemBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SpeleothemThickness;

public class SulfurSpikeRandomOracle {
    static class World extends ServerLevel {
        BlockState above;
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public BlockState getBlockState(BlockPos pos) { return above; }
    }

    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", java.util.Collection.class);
        bind.setAccessible(true);
        for (boolean tagged : new boolean[] { false, true }) {
            bind.invoke(Blocks.SULFUR_SPIKE.builtInRegistryHolder(),
                    tagged ? List.of(BlockTags.SPELEOTHEMS) : List.of());
            for (var direction : new Direction[] { Direction.UP, Direction.DOWN }) {
                for (var thickness : SpeleothemThickness.values()) {
                    for (boolean wet : new boolean[] { false, true }) {
                        var state = Blocks.SULFUR_SPIKE.defaultBlockState()
                                .setValue(SpeleothemBlock.TIP_DIRECTION, direction)
                                .setValue(SpeleothemBlock.THICKNESS, thickness)
                                .setValue(SpeleothemBlock.WATERLOGGED, wet);
                        for (int support = 0; support < 4; support++) {
                            // Growing branches are intentionally left unsupported by Rust.
                            if (support == 3 && tagged && direction == Direction.DOWN) continue;
                            var world = (World) unsafe.allocateInstance(World.class);
                            world.above = switch (support) {
                                case 0 -> Blocks.AIR.defaultBlockState();
                                case 1 -> Blocks.STONE.defaultBlockState();
                                case 2 -> state;
                                default -> Blocks.SULFUR.defaultBlockState();
                            };
                            for (long index = -100; index <= 100; index++) {
                                long seed = index * 1000003L;
                                var random = RandomSource.create(seed);
                                state.randomTick(world, BlockPos.ZERO, random);
                                output.println("TICK\t" + seed + "\t" + direction.getName()
                                        + "\t" + thickness.getSerializedName() + "\t" + wet
                                        + "\t" + tagged + "\t" + support + "\t" + random.nextInt());
                            }
                        }
                    }
                }
            }
        }
    }
}
