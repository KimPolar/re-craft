import java.lang.reflect.Proxy;
import java.util.ArrayList;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeReader;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.ScheduledTickAccess;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.material.Fluid;

public class BonusWaterNeighborOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var environment = (EnvironmentAttributeReader) Proxy.newProxyInstance(
            EnvironmentAttributeReader.class.getClassLoader(), new Class[]{EnvironmentAttributeReader.class},
            (proxy, method, values) -> { if (method.getName().equals("getDimensionValue")) return false; throw new UnsupportedOperationException(method.toString()); });
        var level = (LevelReader) Proxy.newProxyInstance(LevelReader.class.getClassLoader(), new Class[]{LevelReader.class},
            (proxy, method, values) -> { if (method.getName().equals("environmentAttributes")) return environment; throw new UnsupportedOperationException(method.toString()); });
        out.println("BEGIN_RESULT");
        for (int depth = 0; depth < 16; depth++) for (var direction : Direction.values())
            for (var replacement : new net.minecraft.world.level.block.Block[]{Blocks.CHEST, Blocks.TORCH}) {
                var state = Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, depth);
                var events = new ArrayList<String>();
                var ticks = (ScheduledTickAccess) Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(), new Class[]{ScheduledTickAccess.class},
                    (proxy, method, values) -> {
                        if (method.getName().equals("scheduleTick")) {
                            if (!(values[1] instanceof Fluid fluid)) throw new AssertionError("unexpected block tick");
                            events.add(BuiltInRegistries.FLUID.getKey(fluid) + ":" + values[2]);
                            return null;
                        }
                        throw new UnsupportedOperationException(method.toString());
                    });
                var pos = new BlockPos(8, 8, 8);
                var random = RandomSource.create(42);
                var next = state.updateShape(level, ticks, pos, direction, pos.relative(direction), replacement.defaultBlockState(), random);
                out.printf("%d %s %s %s %s %d%n", depth, direction.getSerializedName(),
                    BuiltInRegistries.BLOCK.getKey(replacement), state == next,
                    events.isEmpty() ? "none" : String.join(",", events), random.nextLong());
            }
        out.println("END_RESULT");
    }
}
