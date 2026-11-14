import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.ClipContext;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.HitResult;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.CollisionContext;

public class FallingWaterResetOracle {
    static class World implements BlockGetter {
        final Map<BlockPos, BlockState> blocks = new HashMap<>();
        public int getHeight() { return 384; }
        public int getMinY() { return -64; }
        public BlockEntity getBlockEntity(BlockPos pos) { return null; }
        public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
        public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    }

    static Vec3 vec(JsonArray value) {
        return new Vec3(value.get(0).getAsDouble(), value.get(1).getAsDouble(), value.get(2).getAsDouble());
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bind.setAccessible(true);
        var rows = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var value : rows) {
            var row = value.getAsJsonObject();
            bind.invoke(BuiltInRegistries.FLUID.wrapAsHolder(Fluids.WATER),
                row.get("source_tag").getAsBoolean() ? List.of(FluidTags.WATER) : List.of());
            bind.invoke(BuiltInRegistries.FLUID.wrapAsHolder(Fluids.FLOWING_WATER),
                row.get("flowing_tag").getAsBoolean() ? List.of(FluidTags.WATER) : List.of());
            bind.invoke(Blocks.STONE.builtInRegistryHolder(),
                row.get("stone_tag").getAsBoolean() ? List.of(BlockTags.FALL_DAMAGE_RESETTING) : List.of());
            var world = new World();
            var cell = row.getAsJsonArray("cell");
            var pos = new BlockPos(cell.get(0).getAsInt(), cell.get(1).getAsInt(), cell.get(2).getAsInt());
            int level = row.get("level").getAsInt();
            if (level >= 0) world.blocks.put(pos, Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, level));
            if (row.get("stacked").getAsBoolean()) world.blocks.put(pos.above(), Blocks.WATER.defaultBlockState());
            if (row.get("stone").getAsBoolean()) world.blocks.put(pos.below(), Blocks.STONE.defaultBlockState());
            var start = vec(row.getAsJsonArray("start"));
            var movement = vec(row.getAsJsonArray("movement"));
            double distance = row.get("distance").getAsDouble();
            boolean eligible = distance != 0.0 && movement.lengthSqr() >= 1.0;
            boolean reset = false;
            var result = new JsonObject();
            if (eligible) {
                var end = start.add(movement.normalize().scale(Math.min(movement.length(), 8.0)));
                var context = new ClipContext(start, end, ClipContext.Block.FALLDAMAGE_RESETTING,
                    ClipContext.Fluid.WATER, CollisionContext.empty());
                reset = world.clip(context).getType() != HitResult.Type.MISS;
                var endpoint = new JsonArray(); endpoint.add(end.x); endpoint.add(end.y); endpoint.add(end.z);
                result.add("end", endpoint);
            }
            result.addProperty("eligible", eligible);
            result.addProperty("reset", reset);
            output.add(result);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
