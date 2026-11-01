import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.blockpredicates.*;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.shapes.VoxelShape;

public class ReplaceDiskPredicateOracle {
    static final BlockPos CENTER = new BlockPos(2, 10, 3);
    static class World extends ServerLevel {
        BlockState center, above; boolean clear; JsonArray calls;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos pos) {
            JsonArray call = new JsonArray(); call.add("block"); call.add(pos.getX()); call.add(pos.getY()); call.add(pos.getZ()); calls.add(call);
            if (pos.equals(CENTER)) return center;
            if (pos.equals(CENTER.above())) return above;
            return Blocks.STONE.defaultBlockState();
        }
        @Override public boolean isUnobstructed(Entity except, VoxelShape shape) {
            var box = shape.bounds();
            JsonArray call = new JsonArray(); call.add("clear"); call.add((int)box.minX); call.add((int)box.minY); call.add((int)box.minZ); calls.add(call);
            return clear;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var world = (World)unsafe.allocateInstance(World.class);
        var water = Blocks.WATER.defaultBlockState();
        var flowing = water.setValue(LiquidBlock.LEVEL, 1);
        var slab = Blocks.OAK_SLAB.defaultBlockState();
        var states = new BlockState[]{water, flowing, Blocks.LAVA.defaultBlockState(), Blocks.STONE.defaultBlockState(), Blocks.AIR.defaultBlockState(), slab.setValue(SlabBlock.WATERLOGGED, true), slab};
        var above = new BlockState[]{Blocks.AIR.defaultBlockState(), Blocks.CAVE_AIR.defaultBlockState(), Blocks.STONE.defaultBlockState()};
        var air = BlockPredicate.matchesBlocks(new Vec3i(0,1,0), List.of(Blocks.AIR, Blocks.CAVE_AIR, Blocks.VOID_AIR));
        var block = BlockPredicate.matchesBlocks(Blocks.WATER);
        var fluid = BlockPredicate.matchesFluids(Fluids.WATER);
        var clear = new UnobstructedPredicate(new Vec3i(7,8,9));
        var predicates = new BlockPredicate[]{BlockPredicate.allOf(air, block, fluid, clear),
            BlockPredicate.anyOf(BlockPredicate.not(block), BlockPredicate.matchesFluids(new Vec3i(0,1,0), List.of(Fluids.WATER))),
            clear, BlockPredicate.allOf(List.of()), BlockPredicate.anyOf(List.of()), fluid};
        var rows = new JsonArray();
        for (int type=0; type<predicates.length; type++) for (int state=0; state<states.length; state++)
        for (int airState=0; airState<above.length; airState++) for (boolean unobstructed : new boolean[]{false,true}) {
            world.center=states[state]; world.above=above[airState]; world.clear=unobstructed; world.calls=new JsonArray();
            boolean result=predicates[type].test(world,CENTER);
            var row=new JsonObject(); row.addProperty("type",type); row.addProperty("state",state); row.addProperty("above",airState);
            row.addProperty("clear",unobstructed); row.addProperty("result",result); row.add("calls",world.calls); rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
