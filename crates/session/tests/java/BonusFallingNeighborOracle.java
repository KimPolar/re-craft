import java.lang.reflect.Proxy;
import java.util.ArrayList;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.ScheduledTickAccess;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;

public class BonusFallingNeighborOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var level = (LevelReader) Proxy.newProxyInstance(LevelReader.class.getClassLoader(),
            new Class[]{LevelReader.class}, (proxy, method, values) -> { throw new AssertionError(method.toString()); });
        out.println("BEGIN_RESULT");
        for (var block : new Block[]{Blocks.SAND, Blocks.RED_SAND, Blocks.GRAVEL})
            for (var direction : Direction.values())
                for (var replacement : new Block[]{Blocks.CHEST, Blocks.TORCH}) {
                    var events = new ArrayList<String>();
                    var ticks = (ScheduledTickAccess) Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(),
                        new Class[]{ScheduledTickAccess.class}, (proxy, method, values) -> {
                            if (method.getName().equals("scheduleTick")) {
                                if (!(values[1] instanceof Block falling)) throw new AssertionError("unexpected fluid tick");
                                events.add(BuiltInRegistries.BLOCK.getKey(falling) + ":" + values[2]);
                                return null;
                            }
                            throw new AssertionError(method.toString());
                        });
                    var state = block.defaultBlockState();
                    var pos = new BlockPos(8, 8, 8);
                    var random = RandomSource.create(42);
                    var next = state.updateShape(level, ticks, pos, direction, pos.relative(direction), replacement.defaultBlockState(), random);
                    out.printf("%s %s %s %s %s %d%n", BuiltInRegistries.BLOCK.getKey(block),
                        direction.getSerializedName(), BuiltInRegistries.BLOCK.getKey(replacement), state == next,
                        events.isEmpty() ? "none" : String.join(",", events), random.nextLong());
                }
        out.println("END_RESULT");
    }
}
