import java.lang.reflect.Field;
import java.io.PrintStream;
import java.util.List;
import java.util.Set;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.chunk.UpgradeData;

public class CapturedUpgradeDataOracle {
    private static Object read(UpgradeData data, String name) throws Exception {
        Field field = UpgradeData.class.getDeclaredField(name);
        field.setAccessible(true);
        return field.get(data);
    }

    private static CompoundTag tick(String id) {
        CompoundTag tick = new CompoundTag();
        tick.putString("i", id);
        tick.putInt("x", 8);
        tick.putInt("y", 8);
        tick.putInt("z", 8);
        tick.putInt("t", 2);
        tick.putInt("p", 0);
        return tick;
    }

    private static void report(PrintStream out, String name, CompoundTag tag) throws Exception {
        UpgradeData data = new UpgradeData(tag, LevelHeightAccessor.create(0, 32));
        boolean hasIndices = false;
        for (int[] section : (int[][]) read(data, "index")) {
            hasIndices |= section != null && section.length != 0;
        }
        boolean inert = !hasIndices && ((Set<?>) read(data, "sides")).isEmpty()
                && ((List<?>) read(data, "neighborBlockTicks")).isEmpty()
                && ((List<?>) read(data, "neighborFluidTicks")).isEmpty();
        out.println(name + " " + inert);
    }

    public static void main(String[] args) throws Exception {
        PrintStream out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        out.println("BEGIN_RESULT");
        report(out, "empty", new CompoundTag());
        CompoundTag zero = new CompoundTag();
        zero.putInt("Sides", 0);
        report(out, "zero", zero);
        CompoundTag emptyIndices = new CompoundTag();
        CompoundTag emptySection = new CompoundTag();
        emptySection.putIntArray("0", new int[0]);
        emptyIndices.put("Indices", emptySection);
        report(out, "empty_indices", emptyIndices);
        CompoundTag sides = new CompoundTag();
        sides.putInt("Sides", 1);
        report(out, "sides", sides);
        CompoundTag indices = new CompoundTag();
        CompoundTag section = new CompoundTag();
        section.putIntArray("0", new int[] {1});
        indices.put("Indices", section);
        report(out, "indices", indices);
        for (String kind : new String[] {"block", "fluid"}) {
            CompoundTag tag = new CompoundTag();
            ListTag ticks = new ListTag();
            ticks.add(tick(kind.equals("block") ? "minecraft:stone" : "minecraft:water"));
            tag.put("neighbor_" + kind + "_ticks", ticks);
            report(out, kind + "_ticks", tag);
        }
        out.println("END_RESULT");
    }
}
