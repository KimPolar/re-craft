import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.FlowerBlock;

public class FlowerOutlineOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var names = new String[] {"dandelion", "golden_dandelion", "torchflower", "poppy",
            "blue_orchid", "allium", "azure_bluet", "red_tulip", "orange_tulip", "white_tulip",
            "pink_tulip", "oxeye_daisy", "cornflower", "wither_rose", "lily_of_the_valley",
            "open_eyeblossom", "closed_eyeblossom"};
        var positions = new BlockPos[] {new BlockPos(0, 0, 0), new BlockPos(1, 64, 0),
            new BlockPos(-1, -64, 2), new BlockPos(492, 100, 377),
            new BlockPos(-30000000, -16, 30000000), new BlockPos(Integer.MIN_VALUE, 0, Integer.MAX_VALUE)};
        var records = new JsonArray();
        for (var name : names) {
            var id = Identifier.withDefaultNamespace(name);
            if (!BuiltInRegistries.BLOCK.containsKey(id)) throw new AssertionError(id);
            var block = BuiltInRegistries.BLOCK.getValue(id);
            if (!(block instanceof FlowerBlock)) throw new AssertionError("not a flower: " + id);
            for (var state : block.getStateDefinition().getPossibleStates()) {
                for (var pos : positions) {
                    var record = new JsonObject(); record.addProperty("id", id.toString());
                    var properties = new JsonObject();
                    state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                    record.add("properties", properties);
                    var position = new JsonArray(); position.add(pos.getX()); position.add(pos.getY()); position.add(pos.getZ());
                    record.add("position", position);
                    var boxes = new JsonArray();
                    for (var box : state.getShape(EmptyBlockGetter.INSTANCE, pos).toAabbs()) {
                        var values = new JsonArray();
                        for (double value : new double[] {box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ}) values.add(value);
                        boxes.add(values);
                    }
                    record.add("outline", boxes);
                    record.addProperty("emptyCollision", state.getCollisionShape(EmptyBlockGetter.INSTANCE, pos).isEmpty());
                    records.add(record);
                }
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(records));
    }
}
