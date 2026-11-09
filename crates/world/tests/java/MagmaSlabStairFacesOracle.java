import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Block;

public class MagmaSlabStairFacesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var results = new JsonArray();
        for (var id : BuiltInRegistries.BLOCK.keySet()) {
            var name = id.toString();
            if (!name.startsWith("minecraft:") || !(name.endsWith("_slab") || name.endsWith("_stairs"))) continue;
            for (var state : BuiltInRegistries.BLOCK.getValue(id).getStateDefinition().getPossibleStates()) {
                var entry = new JsonObject();
                entry.addProperty("name", name);
                var properties = new JsonObject();
                state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                entry.add("properties", properties);
                var faces = new JsonArray();
                for (var direction : Direction.values()) {
                    faces.add(Block.isShapeFullBlock(state.getFaceOcclusionShape(direction)));
                }
                entry.add("faces", faces);
                results.add(entry);
            }
        }
        Files.writeString(Path.of(args[0]), results.toString());
    }
}
