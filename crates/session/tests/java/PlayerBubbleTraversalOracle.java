import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

public class PlayerBubbleTraversalOracle {
    static final BlockPos POS=new BlockPos(8,10,8);
    static class World extends ClientLevel {
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public BlockState getBlockState(BlockPos pos) { return pos.equals(POS)?Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(net.minecraft.world.level.block.BubbleColumnBlock.DRAG_DOWN,false):Blocks.AIR.defaultBlockState(); }
        @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    }
    static class TestPlayer extends PlayerBubblePhysicsOracle.TestPlayer {
        TestPlayer() { super(); }
        @Override public boolean isAlive() { return true; }
        @Override protected AABB makeBoundingBox(Vec3 pos) { double half=(double)0.3f,height=(double)1.8f; return new AABB(pos.x-half,pos.y,pos.z-half,pos.x+half,pos.y+height,pos.z+half); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.abilities=new Abilities();
        var level=Entity.class.getDeclaredField("level"); unsafe.putObject(player,unsafe.objectFieldOffset(level),world);
        var visited=Entity.class.getDeclaredField("visitedBlocks"); unsafe.putObject(player,unsafe.objectFieldOffset(visited),new LongOpenHashSet());
        var movement=Class.forName("net.minecraft.world.entity.Entity$Movement").getDeclaredConstructor(Vec3.class,Vec3.class,Vec3.class); movement.setAccessible(true);
        var method=Entity.class.getDeclaredMethod("checkInsideBlocks",List.class,InsideBlockEffectApplier.StepBasedCollector.class); method.setAccessible(true);
        var rows=new JsonArray();
        for(double[] value:new double[][] {
            {8.1,10.0,8.5,0,0,0}, {8.1,10.0,8.5,-0.7,0,0}, {7.4,10.0,8.5,0.7,0,0},
            {6.5,10.0,8.5,4,0,0}, {4,10.0,8.5,2,0,0}, {8.5,8.0,8.5,0,3,0},
            {8.5,11.9,8.5,0,-0.5,0}, {8.5,10.0,9.4,0,0,-0.6},
            {7.0,10.0,7.0,2,0,2}, {7.0,10.0,9.5,2,0,-2},
            {8.1,10.0,8.1,-0.7,0,-0.7}, {7.2,10.0,7.2,1,0,1},
            {8.1,10.0,8.1,-0.8,0,-0.8}, {6.5,10.0,6.5,4,0,4},
            {8.1,10.0,8.5,-0.99998999,0,0}, {8.1,10.0,8.5,-0.99999001,0,0}
        }) {
            var from=new Vec3(value[0],value[1],value[2]); var to=from.add(value[3],value[4],value[5]); player.velocity=Vec3.ZERO;
            method.invoke(player,List.of(movement.newInstance(from,to,to.subtract(from))),new InsideBlockEffectApplier.StepBasedCollector());
            var input=new JsonArray(); for(double x:value) input.add(x); var row=new JsonObject(); row.add("input",input); row.addProperty("active",player.velocity.y!=0); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
