import java.lang.reflect.Proxy;
import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.FenceGateBlock;

public class SoilGateOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var results = new JsonArray();
        for (var block : BuiltInRegistries.BLOCK) {
            if (!(block instanceof FenceGateBlock)) continue;
            for (var above : block.getStateDefinition().getPossibleStates()) {
                var world = (LevelReader) Proxy.newProxyInstance(
                    SoilGateOracle.class.getClassLoader(), new Class<?>[]{LevelReader.class},
                    (proxy, method, values) -> {
                        if (method.getName().equals("getBlockState")) return above;
                        throw new UnsupportedOperationException(method.toString());
                    });
                var row = new JsonObject();
                row.addProperty("id", BuiltInRegistries.BLOCK.getKey(block).toString());
                row.addProperty("solid", above.isSolid());
                row.addProperty("path_survives", Blocks.DIRT_PATH.defaultBlockState().canSurvive(world, BlockPos.ZERO));
                results.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), results.toString());
    }
}
