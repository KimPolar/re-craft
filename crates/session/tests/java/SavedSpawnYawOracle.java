import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.Vec3;

/** Calls the exact 26.3 look-at calculation for saved spawn results. */
public class SavedSpawnYawOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Class<?> type = Class.forName("net.minecraft.server.level.ServerPlayer$RespawnPosAngle");
        Method of = type.getDeclaredMethod("of", Vec3.class, BlockPos.class, float.class);
        Method yaw = type.getDeclaredMethod("yaw"); of.setAccessible(true); yaw.setAccessible(true);
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (JsonElement element : cases) {
            JsonArray c = element.getAsJsonArray();
            BlockPos pos = new BlockPos(c.get(0).getAsInt(),c.get(1).getAsInt(),c.get(2).getAsInt());
            Vec3 stand = new Vec3(c.get(3).getAsDouble(),c.get(4).getAsDouble(),c.get(5).getAsDouble());
            Object result = of.invoke(null,stand,pos,0.0f);
            results.add((Float)yaw.invoke(result));
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(results));
    }
}
