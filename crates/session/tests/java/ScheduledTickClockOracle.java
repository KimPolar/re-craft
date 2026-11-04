import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Proxy;
import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.LevelTicks;
import net.minecraft.world.ticks.SavedTick;
import net.minecraft.world.ticks.TickPriority;

public class ScheduledTickClockOracle {
    public static void main(String[] args) {
        long[] time = {42};
        long[] subTick = {0};
        var level = (LevelAccessor) Proxy.newProxyInstance(
            LevelAccessor.class.getClassLoader(), new Class[]{LevelAccessor.class},
            (proxy, method, arguments) -> switch (method.getName()) {
                case "getGameTime" -> time[0];
                case "nextSubTickCount" -> subTick[0]++;
                default -> InvocationHandler.invokeDefault(proxy, method, arguments);
            });
        var a = new BlockPos(8, 2, 8);
        var c = new BlockPos(8, 3, 8);
        var water = new BlockPos(3, 2, 3);
        var blockContainer = new LevelChunkTicks<String>(List.of(
            new SavedTick<>("minecraft:sand", a, 1, TickPriority.NORMAL)));
        var fluidContainer = new LevelChunkTicks<String>(List.of(
            new SavedTick<>("minecraft:water", water, 1, TickPriority.NORMAL)));
        var blocks = new LevelTicks<String>(pos -> true);
        var fluids = new LevelTicks<String>(pos -> true);
        blocks.addContainer(new ChunkPos(0, 0), blockContainer);
        fluids.addContainer(new ChunkPos(0, 0), fluidContainer);
        blockContainer.unpack(time[0]);
        fluidContainer.unpack(time[0]);
        int[] blockCount = {0};
        for (int step = 0; step < 3; step++) {
            time[0]++;
            blocks.tick(time[0], 65536, (pos, kind) -> {
                blockCount[0]++;
                if (pos.equals(a)) blocks.schedule(level.createTick(c, "minecraft:sand", 2));
            });
            fluids.tick(time[0], 65536, (pos, kind) -> {});
            System.out.println(time[0] + " " + blockCount[0] + " " + fluidContainer.count() + " " + blockContainer.count());
        }
    }
}
