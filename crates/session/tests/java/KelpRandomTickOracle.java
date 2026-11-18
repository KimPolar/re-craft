import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.GrowingPlantHeadBlock;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.block.state.BlockState;

public class KelpRandomTickOracle {
    private static final BlockPos POSITION = BlockPos.ZERO;

    private static final class World extends ServerLevel {
        BlockState above;
        RandomSource random;
        int reads;
        int writtenAge;
        int levelRandomReads;

        World() {
            super(null, null, null, null, null, null, false, 0, List.of(), false);
        }

        @Override
        public BlockState getBlockState(BlockPos position) {
            if (!position.equals(POSITION.above())) {
                throw new AssertionError("unexpected kelp read: " + position);
            }
            reads++;
            return above;
        }

        @Override
        public RandomSource getRandom() {
            levelRandomReads++;
            return random;
        }

        @Override
        public boolean setBlock(BlockPos position, BlockState state, int flags, int depth) {
            if (!position.equals(POSITION.above()) || !state.is(Blocks.KELP) || flags != 3) {
                throw new AssertionError("unexpected kelp placement");
            }
            writtenAge = state.getValue(GrowingPlantHeadBlock.AGE);
            return true;
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        int[] levels = {0, 1, 7, 8, 15};
        int[] ages = {0, 7, 24, 25};
        for (int age : ages) {
            var head = Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE, age);
            for (int kind = 0; kind < 8; kind++) {
                world.above = kind < levels.length
                    ? Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, levels[kind])
                    : kind == 5 ? Blocks.AIR.defaultBlockState()
                    : kind == 6 ? Blocks.STONE.defaultBlockState()
                    : Blocks.KELP_PLANT.defaultBlockState();
                for (long sample = 0; sample < 512; sample++) {
                    long seed = sample * 1_000_003L - 17L;
                    world.random = RandomSource.create(seed);
                    world.reads = 0;
                    world.writtenAge = -1;
                    world.levelRandomReads = 0;
                    head.randomTick(world, POSITION, world.random);
                    int next = world.random.nextInt();
                    System.out.printf("K %d %d %d %d %d %d %d%n", seed, age, kind,
                        world.reads, world.writtenAge, world.levelRandomReads, next);
                }
            }
        }
    }
}
