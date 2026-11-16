import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.SavedTick;
import net.minecraft.world.ticks.ScheduledTick;
import net.minecraft.world.ticks.TickPriority;

/** Saved entries wait for postprocessing while live schedules keep their clock. */
public class SavedPendingTickOracle {
    public static void main(String[] args) {
        var saved = new BlockPos(19, 2, 4);
        var fresh = new BlockPos(20, 2, 4);
        var blocks = new LevelChunkTicks<String>(List.of(
            new SavedTick<>("minecraft:sand", saved, 4, TickPriority.NORMAL),
            new SavedTick<>("minecraft:sand", saved, 6, TickPriority.NORMAL)));
        var fluids = new LevelChunkTicks<String>(List.of(
            new SavedTick<>("minecraft:water", saved, 4, TickPriority.NORMAL)));

        blocks.schedule(new ScheduledTick<>("minecraft:sand", saved, 54, TickPriority.NORMAL, 0));
        blocks.schedule(new ScheduledTick<>("minecraft:sand", fresh, 54, TickPriority.NORMAL, 1));
        fluids.schedule(new ScheduledTick<>("minecraft:water", saved, 55, TickPriority.NORMAL, 2));
        fluids.schedule(new ScheduledTick<>("minecraft:water", fresh, 55, TickPriority.NORMAL, 3));
        blocks.unpack(62);
        fluids.unpack(62);
        while (blocks.peek() != null) {
            var tick = blocks.poll();
            System.out.println("B " + (tick.triggerTick() - 42) + " " + (tick.pos().getX() & 15));
        }
        while (fluids.peek() != null) {
            var tick = fluids.poll();
            System.out.println("F " + (tick.triggerTick() - 42) + " " + (tick.pos().getX() & 15));
        }
    }
}
