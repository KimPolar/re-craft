import java.nio.file.*;
import java.util.*;
import it.unimi.dsi.fastutil.objects.Object2ObjectOpenHashMap;
import net.minecraft.core.BlockPos;

public class GeneratedEntityOrderOracle {
    public static void main(String[] args) throws Exception {
        var live = new Object2ObjectOpenHashMap<BlockPos, Boolean>();
        for (var line : Files.readAllLines(Path.of(args[0]))) {
            var parts = line.split(" ");
            var pos = new BlockPos(Integer.parseInt(parts[1]), Integer.parseInt(parts[2]), Integer.parseInt(parts[3]));
            if (parts[0].equals("I")) live.put(pos, true); else live.remove(pos);
            var pending = new HashSet<BlockPos>(Collections.emptySet());
            pending.addAll(live.keySet());
            var values = new ArrayList<String>();
            for (var p : pending) values.add(p.getX()+","+p.getY()+","+p.getZ());
            System.out.println(String.join(";", values));
        }
    }
}
