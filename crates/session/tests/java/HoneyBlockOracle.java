import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Blocks;

public class HoneyBlockOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var state = Blocks.HONEY_BLOCK.defaultBlockState();
        var level = EmptyBlockGetter.INSTANCE;
        var box = state.getCollisionShape(level, BlockPos.ZERO).bounds();
        var outline = state.getShape(level, BlockPos.ZERO).bounds();
        var result = new JsonObject(); result.addProperty("hardness", state.getDestroySpeed(level, BlockPos.ZERO));
        var shape = new JsonArray(); for (double n : new double[]{box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ}) shape.add(n); result.add("collision", shape);
        var visual = new JsonArray(); for (double n : new double[]{outline.minX, outline.minY, outline.minZ, outline.maxX, outline.maxY, outline.maxZ}) visual.add(n); result.add("outline", visual);
        Files.writeString(Path.of(args[0]), new Gson().toJson(result));
    }
}
