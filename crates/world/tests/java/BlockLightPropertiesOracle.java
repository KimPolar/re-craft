import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.block.RedStoneOreBlock;
import net.minecraft.world.level.block.ChestBlock;
import net.minecraft.world.level.block.CaveVines;
import net.minecraft.world.level.block.GrowingPlantHeadBlock;
import net.minecraft.world.level.block.SegmentableBlock;
import net.minecraft.world.level.block.state.properties.ChestType;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.block.state.properties.DoubleBlockHalf;
import net.minecraft.world.level.block.state.properties.Tilt;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.core.Direction;

public class BlockLightPropertiesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var item : cases) {
            var entry = item.getAsJsonObject();
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(entry.get("name").getAsString()));
            var state = block.defaultBlockState();
            if (entry.has("axis")) state = state.setValue(BlockStateProperties.AXIS, Direction.Axis.byName(entry.get("axis").getAsString()));
            if (entry.has("lit")) state = state.setValue(RedStoneOreBlock.LIT, entry.get("lit").getAsBoolean());
            if (entry.has("berries")) state = state.setValue(CaveVines.BERRIES, entry.get("berries").getAsBoolean());
            if (entry.has("age")) state = state.setValue(GrowingPlantHeadBlock.AGE, entry.get("age").getAsInt());
            if (entry.has("facing")) state = state.setValue(BlockStateProperties.HORIZONTAL_FACING, Direction.byName(entry.get("facing").getAsString()));
            if (entry.has("waterlogged")) state = state.setValue(BlockStateProperties.WATERLOGGED, entry.get("waterlogged").getAsBoolean());
            if (entry.has("north")) state = state.setValue(BlockStateProperties.NORTH, entry.get("north").getAsBoolean());
            if (entry.has("east")) state = state.setValue(BlockStateProperties.EAST, entry.get("east").getAsBoolean());
            if (entry.has("south")) state = state.setValue(BlockStateProperties.SOUTH, entry.get("south").getAsBoolean());
            if (entry.has("west")) state = state.setValue(BlockStateProperties.WEST, entry.get("west").getAsBoolean());
            if (entry.has("segment_amount")) state = state.setValue(SegmentableBlock.AMOUNT, entry.get("segment_amount").getAsInt());
            if (entry.has("type")) state = state.setValue(ChestBlock.TYPE, ChestType.valueOf(entry.get("type").getAsString().toUpperCase(java.util.Locale.ROOT)));
            if (entry.has("half")) state = state.setValue(BlockStateProperties.DOUBLE_BLOCK_HALF, DoubleBlockHalf.valueOf(entry.get("half").getAsString().toUpperCase(java.util.Locale.ROOT)));
            if (entry.has("tilt")) state = state.setValue(BlockStateProperties.TILT, Tilt.valueOf(entry.get("tilt").getAsString().toUpperCase(java.util.Locale.ROOT)));
            var row = new JsonArray();
            row.add(state.getLightDampening());
            row.add(state.getLightEmission());
            row.add(state.useShapeForLightOcclusion());
            row.add(LightEngine.hasDifferentLightProperties(state, Blocks.STONE.defaultBlockState()));
            output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
