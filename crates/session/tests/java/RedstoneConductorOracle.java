import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.EmptyBlockGetter;

public class RedstoneConductorOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var rows = new JsonArray();
        for (int i=1;i<args.length;i++) {
            var id=Identifier.parse(args[i]);
            var block=BuiltInRegistries.BLOCK.getValue(id);
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var row = new JsonObject();
                row.addProperty("id", id.toString());
                var properties = new JsonObject();
                state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                row.add("properties", properties);
                row.addProperty("conductor", state.isRedstoneConductor(EmptyBlockGetter.INSTANCE, BlockPos.ZERO));
                var weak = new JsonArray(); var direct = new JsonArray();
                for (var direction : Direction.values()) {
                    weak.add(state.getSignal(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, direction));
                    direct.add(state.getDirectSignal(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, direction));
                }
                row.add("weak", weak); row.add("direct", direct); rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
