import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.Level;

public class RandomBlockPositionOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field u = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); u.setAccessible(true);
        var level = (ServerLevel) ((sun.misc.Unsafe)u.get(null)).allocateInstance(ServerLevel.class);
        Field seed = Level.class.getDeclaredField("randValue"); seed.setAccessible(true);
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : input) {
            seed.setInt(level, entry.getAsInt());
            var row = new JsonArray();
            for (int i = 0; i < 64; i++) {
                var pos = level.getBlockRandomPos(0, 0, 0, 15);
                var xyz = new JsonArray(); xyz.add(pos.getX()); xyz.add(pos.getY()); xyz.add(pos.getZ()); row.add(xyz);
            }
            output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
