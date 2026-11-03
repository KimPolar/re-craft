import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.JsonArray;
import com.google.gson.JsonParser;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import net.minecraft.core.BlockPos;

public class PendingLightChecksOracle {
    public static void main(String[] args) throws Exception {
        var batches = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var pending = new LongOpenHashSet(512, 0.5f);
        var results = new JsonArray();
        for (var batch : batches) {
            for (var entry : batch.getAsJsonArray()) {
                var p = entry.getAsJsonArray();
                pending.add(BlockPos.asLong(p.get(0).getAsInt(), p.get(1).getAsInt(), p.get(2).getAsInt()));
            }
            var result = new JsonArray();
            var iterator = pending.iterator();
            while (iterator.hasNext()) {
                var p = BlockPos.of(iterator.nextLong());
                var point = new JsonArray();
                point.add(p.getX());
                point.add(p.getY());
                point.add(p.getZ());
                result.add(point);
            }
            results.add(result);
            pending.clear();
            pending.trim(512);
        }
        Files.writeString(Path.of(args[1]), results.toString());
    }
}
