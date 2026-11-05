import java.nio.file.*;
import java.util.*;
import it.unimi.dsi.fastutil.objects.Object2ObjectOpenHashMap;
import net.minecraft.core.BlockPos;

public class GeneratedEntityOrderOracle {
    public static void main(String[] args) throws Exception {
        var live = new Object2ObjectOpenHashMap<BlockPos, Boolean>();
        var pending = new HashMap<BlockPos, Boolean>();
        for (var line : Files.readAllLines(Path.of(args[0]))) {
            var parts = line.split(" ");
            var pos = new BlockPos(Integer.parseInt(parts[1]), Integer.parseInt(parts[2]), Integer.parseInt(parts[3]));
            switch (parts[0]) {
                case "I" -> live.put(pos, true);
                case "R" -> live.remove(pos);
                case "P" -> { if (!live.containsKey(pos)) pending.put(pos, true); }
                case "C" -> { pending.remove(pos); live.put(pos, true); }
                case "X" -> pending.remove(pos);
                default -> throw new IllegalArgumentException(parts[0]);
            }
            var ordered = new HashSet<BlockPos>(pending.keySet());
            ordered.addAll(live.keySet());
            var values = new ArrayList<String>();
            for (var p : ordered) values.add(p.getX()+","+p.getY()+","+p.getZ());
            System.out.println(String.join(";", values));
        }
    }
}
