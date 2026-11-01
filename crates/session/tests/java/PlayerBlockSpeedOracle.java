import com.google.gson.*;
import java.lang.reflect.Field;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;

public class PlayerBlockSpeedOracle {
    static class World extends ServerLevel {
        BlockState feet, below;
        World() { super(null,null,null,null,null,null,false,0L,java.util.List.of(),false); }
        @Override public BlockState getBlockState(BlockPos pos) {
            return pos.equals(BlockPos.ZERO) ? feet : below;
        }
    }
    static class TestPlayer extends Player {
        double efficiency;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean isFallFlying() { return false; }
        @Override public BlockPos blockPosition() { return BlockPos.ZERO; }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return BlockPos.ZERO.below(); }
        @Override public double getAttributeValue(Holder<Attribute> attribute) {
            if (attribute == Attributes.MOVEMENT_EFFICIENCY) return efficiency;
            throw new IllegalStateException("unexpected attribute " + attribute);
        }
        float speedFactor() { return getBlockSpeedFactor(); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        Field level=Entity.class.getDeclaredField("level"); level.setAccessible(true); level.set(player,world);
        Field abilities=Player.class.getDeclaredField("abilities"); abilities.setAccessible(true); abilities.set(player,new Abilities());
        var blocks=new java.util.LinkedHashMap<String,Block>();
        blocks.put("air",Blocks.AIR); blocks.put("stone",Blocks.STONE);
        blocks.put("honey_block",Blocks.HONEY_BLOCK); blocks.put("soul_sand",Blocks.SOUL_SAND);
        blocks.put("soul_soil",Blocks.SOUL_SOIL); blocks.put("water",Blocks.WATER);
        blocks.put("bubble_column",Blocks.BUBBLE_COLUMN); blocks.put("lava",Blocks.LAVA);
        var rows=new JsonArray();
        for(var feet:blocks.entrySet()) for(var below:blocks.entrySet())
            for(double efficiency:new double[]{0.0,0.2,0.5,1.0}) {
                world.feet=feet.getValue().defaultBlockState(); world.below=below.getValue().defaultBlockState();
                player.efficiency=efficiency;
                var row=new JsonObject(); row.addProperty("feet",feet.getKey()); row.addProperty("below",below.getKey());
                row.addProperty("efficiency",efficiency); row.addProperty("bits",Float.floatToIntBits(player.speedFactor()));
                if (feet.getKey().equals("soul_sand")) {
                    var state=feet.getValue().defaultBlockState();
                    var collision=state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO).bounds();
                    var outline=state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO).bounds();
                    var boxes=new JsonArray();
                    for(var box:new net.minecraft.world.phys.AABB[]{collision,outline}) {
                        var coords=new JsonArray();
                        for(double v:new double[]{box.minX,box.minY,box.minZ,box.maxX,box.maxY,box.maxZ}) coords.add(v);
                        boxes.add(coords);
                    }
                    row.add("boxes",boxes);
                    var sturdy=new JsonArray();
                    for(var direction:Direction.values()) sturdy.add(state.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,direction));
                    row.add("sturdy",sturdy);
                }
                rows.add(row);
            }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
