import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;

public class BlockModelOffsetOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var positions = new BlockPos[] {
            new BlockPos(0, 0, 0), new BlockPos(1, 64, 0), new BlockPos(-1, -64, 2),
            new BlockPos(492, 100, 377), new BlockPos(-30000000, -16, 30000000),
            new BlockPos(Integer.MIN_VALUE, 0, Integer.MAX_VALUE)
        };
        var records = new JsonArray();
        for (var block : BuiltInRegistries.BLOCK) {
            for (var pos : positions) {
                var offset = block.defaultBlockState().getOffset(pos);
                for (var state : block.getStateDefinition().getPossibleStates()) {
                    if (!state.getOffset(pos).equals(offset))
                        throw new AssertionError("state-dependent offset: " + state + " " + pos);
                }
                var record = new JsonObject();
                record.addProperty("id", BuiltInRegistries.BLOCK.getKey(block).toString());
                var position = new JsonArray();
                position.add(pos.getX()); position.add(pos.getY()); position.add(pos.getZ());
                record.add("position", position);
                var translation = new JsonArray();
                translation.add(offset.x); translation.add(offset.y); translation.add(offset.z);
                record.add("offset", translation);
                records.add(record);
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(records));
    }
}
