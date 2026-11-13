import java.lang.reflect.Proxy;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.ScheduledTickAccess;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.VineBlock;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.properties.SlabType;

public class TreeEdgeVineOracle {
    private static final Direction[] FACES = {Direction.UP, Direction.NORTH, Direction.EAST, Direction.SOUTH, Direction.WEST};
    private static net.minecraft.world.level.block.state.BlockState vine(int mask) {
        var state = Blocks.VINE.defaultBlockState();
        for (int i = 0; i < FACES.length; i++) state = state.setValue(VineBlock.getPropertyForFace(FACES[i]), (mask & (1 << i)) != 0);
        return state;
    }
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var origin = new BlockPos(8, 8, 8);
        var ticks = (ScheduledTickAccess) Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(), new Class[]{ScheduledTickAccess.class}, (p, m, a) -> { throw new AssertionError("unexpected tick: " + m); });
        for (int mask = 0; mask < 32; mask++) for (int support = 0; support < 16; support++) for (int above = 0; above < 6; above++) {
            final int sideMask = support, aboveKind = above;
            var level = (LevelReader) Proxy.newProxyInstance(LevelReader.class.getClassLoader(), new Class[]{LevelReader.class}, (p, m, a) -> {
                if (!m.getName().equals("getBlockState")) throw new AssertionError(m.toString());
                var pos = (BlockPos) a[0];
                if (pos.equals(origin.above())) return switch (aboveKind) {
                    case 0 -> Blocks.AIR.defaultBlockState();
                    case 1 -> Blocks.STONE.defaultBlockState();
                    case 2 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
                    case 3 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP);
                    case 4 -> vine(10);
                    case 5 -> vine(31);
                    default -> throw new AssertionError();
                };
                for (int i = 1; i < FACES.length; i++) if (pos.equals(origin.relative(FACES[i]))) return (sideMask & (1 << (i - 1))) != 0 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
                throw new AssertionError("unexpected lookup " + pos);
            });
            var state = vine(mask);
            for (var direction : Direction.values()) {
                var random = RandomSource.create(42);
                var result = state.updateShape(level, ticks, origin, direction, origin.relative(direction), Blocks.STONE.defaultBlockState(), random);
                if (random.nextLong() != RandomSource.create(42).nextLong()) throw new AssertionError("unexpected RNG use");
                int bits = -1;
                if (result.is(Blocks.VINE)) {
                    bits = 0;
                    for (int i = 0; i < FACES.length; i++) if (result.getValue(VineBlock.getPropertyForFace(FACES[i]))) bits |= 1 << i;
                } else if (!result.isAir()) throw new AssertionError("unexpected replacement: " + result);
                out.printf("V %d %d %d %s %d%n", mask, support, above, direction.getSerializedName(), bits);
            }
        }
    }
}
