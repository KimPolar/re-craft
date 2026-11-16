import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.BrushableBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;

/** Exercise freshly loaded brushable blocks on supported and free floors. */
public class BrushableTickOracle {
    static class Level extends ServerLevel {
        BlockEntity entity;
        BlockState floor = Blocks.STONE.defaultBlockState();
        BlockState replacement;
        FallingBlockEntity falling;
        int reads;
        int flags;
        Level() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getNextEntityId() { return 1; }
        @Override public BlockEntity getBlockEntity(BlockPos pos) { return entity; }
        @Override public BlockState getBlockState(BlockPos pos) { reads++; return floor; }
        @Override public int getMinY() { return 0; }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int recursionLeft) {
            this.replacement = state;
            this.flags = flags;
            return true;
        }
        @Override public boolean addFreshEntity(Entity entity) { falling = (FallingBlockEntity) entity; return true; }
        @Override public long getGameTime() { return 42; }
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var pos = new BlockPos(8, 8, 8);
        out.println("BEGIN_RESULT");
        for (var block : List.of(Blocks.SUSPICIOUS_SAND, Blocks.SUSPICIOUS_GRAVEL)) {
            for (int dusted = 0; dusted < 4; dusted++) {
                for (boolean hasEntity : new boolean[]{false, true}) {
                    var level = (Level) unsafe.allocateInstance(Level.class);
                    level.floor = Blocks.STONE.defaultBlockState();
                    var state = block.defaultBlockState().setValue(BlockStateProperties.DUSTED, dusted);
                    if (hasEntity) level.entity = new BrushableBlockEntity(pos, state);
                    var random = RandomSource.create(42);
                    state.tick(level, pos, random);
                    out.printf("%s %d %s %d %d%n", block == Blocks.SUSPICIOUS_SAND ? "sand" : "gravel", dusted, hasEntity, level.reads, random.nextLong());
                }
            }
        }
        out.println("END_RESULT");

        var cancel = FallingBlockEntity.class.getDeclaredField("cancelDrop");
        cancel.setAccessible(true);
        out.println("BEGIN_FALL_RESULT");
        for (var block : List.of(Blocks.SUSPICIOUS_SAND, Blocks.SUSPICIOUS_GRAVEL)) {
            for (int dusted = 0; dusted < 4; dusted++) {
                for (boolean hasEntity : new boolean[]{false, true}) {
                    var level = (Level) unsafe.allocateInstance(Level.class);
                    level.floor = Blocks.AIR.defaultBlockState();
                    var state = block.defaultBlockState().setValue(BlockStateProperties.DUSTED, dusted);
                    if (hasEntity) level.entity = new BrushableBlockEntity(pos, state);
                    var random = RandomSource.create(42);
                    state.tick(level, pos, random);
                    var falling = level.falling;
                    out.printf("%s %d %s %s %d %s %s %.1f %.1f %.1f %s %d%n",
                        block == Blocks.SUSPICIOUS_SAND ? "sand" : "gravel", dusted, hasEntity,
                        level.replacement == Blocks.AIR.defaultBlockState(), level.flags,
                        falling.getBlockState() == state, cancel.getBoolean(falling),
                        falling.getX(), falling.getY(), falling.getZ(), falling.dropItem, random.nextLong());
                }
            }
        }
        out.println("END_FALL_RESULT");
    }
}
