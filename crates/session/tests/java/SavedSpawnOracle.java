import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.attribute.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BedPart;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.storage.LevelData;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.*;

/** Calls the real private saved-spawn selection using a synthetic server level. */
public class SavedSpawnOracle {
    static class TestLevel extends ServerLevel {
        Map<BlockPos, BlockState> blocks; WorldBorder border;
        EnvironmentAttributeSystem attributes; boolean dark;
        TestLevel() { super(null,null,null,null,null,null,false,0,null,false); }
        @Override public BlockState getBlockState(BlockPos pos) {
            return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState());
        }
        @Override public WorldBorder getWorldBorder() { return border; }
        @Override public EnvironmentAttributeSystem environmentAttributes() { return attributes; }
        @Override public boolean isDarkOutside() { return dark; }
        @Override public boolean setBlockAndUpdate(BlockPos pos, BlockState state) {
            blocks.put(pos, state); return true;
        }
        @Override public Iterable<VoxelShape> getBlockCollisions(Entity entity, AABB bounds) {
            List<VoxelShape> result = new ArrayList<>(); VoxelShape query = Shapes.create(bounds);
            for (var entry : blocks.entrySet()) {
                var pos = entry.getKey(); var state = entry.getValue();
                var shape = state.getCollisionShape(this,pos);
                if (shape.isEmpty()) continue;
                shape = shape.move(pos.getX(),pos.getY(),pos.getZ());
                if (state.isCollisionShapeFullBlock(this,pos) ? shape.bounds().intersects(bounds)
                    : Shapes.joinIsNotEmpty(shape,query,BooleanOp.AND)) result.add(shape);
            }
            return result;
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)f.get(null);
        TestLevel level = (TestLevel)unsafe.allocateInstance(TestLevel.class);
        level.border = new WorldBorder();
        Method find = ServerPlayer.class.getDeclaredMethod("findRespawnAndUseSpawnBlock",
            ServerLevel.class, ServerPlayer.RespawnConfig.class, boolean.class);
        find.setAccessible(true);
        BlockPos pos = new BlockPos(8,64,8);
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var input : cases) {
            var c = input.getAsJsonObject(); level.blocks = new HashMap<>();
            for (int x=4;x<=12;x++) for (int z=4;z<=12;z++) {
                level.blocks.put(new BlockPos(x,63,z),Blocks.STONE.defaultBlockState());
                if (c.get("blocked").getAsBoolean()) for (int y=64;y<=67;y++)
                    level.blocks.put(new BlockPos(x,y,z),Blocks.STONE.defaultBlockState());
            }
            Direction facing = Direction.valueOf(c.get("facing").getAsString());
            String kind = c.get("kind").getAsString();
            int charge = c.get("charge").getAsInt();
            BlockState start = switch (kind) {
                case "bed", "straw" -> BuiltInRegistries.BLOCK.getValue(Identifier.parse(kind.equals("bed") ? "minecraft:red_bed" : "minecraft:straw_bed")).defaultBlockState().setValue(AbstractBedBlock.FACING,facing).setValue(AbstractBedBlock.PART,BedPart.HEAD);
                case "anchor" -> Blocks.RESPAWN_ANCHOR.defaultBlockState().setValue(RespawnAnchorBlock.CHARGE,charge);
                case "stone" -> Blocks.STONE.defaultBlockState();
                case "water" -> Blocks.WATER.defaultBlockState();
                default -> Blocks.AIR.defaultBlockState();
            };
            level.blocks.put(pos,start);
            if (kind.equals("bed") || kind.equals("straw")) level.blocks.put(pos.relative(facing.getOpposite()),
                start.setValue(AbstractBedBlock.PART,BedPart.FOOT));
            if (c.get("above_stone").getAsBoolean()) level.blocks.put(pos.above(),Blocks.STONE.defaultBlockState());
            var bedRule = new BedRule(BedRule.Rule.NEVER, BedRule.Rule.valueOf(c.get("bed_rule").getAsString()),false,false,Optional.empty());
            var map = EnvironmentAttributeMap.builder().set(EnvironmentAttributes.BED_RULE,bedRule)
                .set(EnvironmentAttributes.STRAW_BED_RULE,bedRule)
                .set(EnvironmentAttributes.RESPAWN_ANCHOR_WORKS,c.get("anchor_works").getAsBoolean()).build();
            level.attributes = EnvironmentAttributeSystem.builder().addConstantLayer(map).build();
            level.dark = c.get("dark").getAsBoolean();
            var data = LevelData.RespawnData.of(Level.OVERWORLD,pos,c.get("yaw").getAsFloat(),c.get("pitch").getAsFloat());
            var config = new ServerPlayer.RespawnConfig(data,c.get("forced").getAsBoolean());
            var found = (Optional<?>)find.invoke(null,level,config,c.get("consume").getAsBoolean());
            var row = new JsonObject();
            if (found.isEmpty()) row.add("position",JsonNull.INSTANCE);
            else {
                Object value = found.get();
                Method position=value.getClass().getDeclaredMethod("position"); position.setAccessible(true);
                Method yaw=value.getClass().getDeclaredMethod("yaw"); yaw.setAccessible(true);
                Method pitch=value.getClass().getDeclaredMethod("pitch"); pitch.setAccessible(true);
                Vec3 p=(Vec3)position.invoke(value); var xyz=new JsonArray(); xyz.add(p.x); xyz.add(p.y); xyz.add(p.z);
                row.add("position",xyz); row.addProperty("yaw",(Float)yaw.invoke(value)); row.addProperty("pitch",(Float)pitch.invoke(value));
            }
            if (kind.equals("anchor")) row.addProperty("charge",level.getBlockState(pos).getValue(RespawnAnchorBlock.CHARGE));
            output.add(row);
        }
        Files.writeString(Path.of(args[1]),new GsonBuilder().serializeNulls().create().toJson(output));
    }
}
