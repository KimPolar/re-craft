import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.material.*;

public class BubbleWaterFluidOracle {
    static final BlockPos CENTER=new BlockPos(8,10,8);
    static final Direction[] SIDES={Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
    static class World extends ServerLevel {
        Map<BlockPos,BlockState> blocks; JsonArray changes; GameRules rules;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos,Blocks.STONE.defaultBlockState()); }
        @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        @Override public GameRules getGameRules() { return rules; }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 32; }
        @Override public boolean setBlockAndUpdate(BlockPos pos,BlockState state) {
            var old=getBlockState(pos); if(old==state) return false;
            blocks.put(pos.immutable(),state);
            var entry=new JsonArray(); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ());
            entry.add(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
            entry.add(state.is(Blocks.WATER)?state.getValue(LiquidBlock.LEVEL):-1); changes.add(entry);
            return true;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder(),Blocks.LAVA.builtInRegistryHolder())))).apply();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,Map.of(FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder())))).apply();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class); world.rules=new GameRules(FeatureFlags.DEFAULT_FLAGS);
        var rows=new JsonArray();
        for(int below=0;below<4;below++) for(int mask=0;mask<16;mask++) for(int variant=0;variant<4;variant++) for(boolean holes:new boolean[]{false,true}) for(boolean drag:new boolean[]{false,true}) {
            world.blocks=new HashMap<>(); world.changes=new JsonArray();
            var bubble=Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN,drag);
            world.blocks.put(CENTER,bubble);
            var down=switch(below) { case 0->Blocks.AIR.defaultBlockState(); case 1->Blocks.STONE.defaultBlockState(); case 2->Blocks.WATER.defaultBlockState(); default->bubble; };
            world.blocks.put(CENTER.below(),down);
            for(int i=0;i<4;i++) if((mask & (1<<i))!=0) {
                var pos=CENTER.relative(SIDES[i]);
                var state=switch(variant) { case 0->Blocks.AIR.defaultBlockState(); case 1->Blocks.WATER.defaultBlockState(); case 2->bubble; default->Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1); };
                world.blocks.put(pos,state);
                if(holes) world.blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
            }
            Fluids.WATER.tick(world,CENTER,bubble,bubble.getFluidState());
            var row=new JsonObject(); row.addProperty("below",below); row.addProperty("mask",mask); row.addProperty("variant",variant); row.addProperty("holes",holes); row.addProperty("drag",drag); row.add("changes",world.changes); row.addProperty("bubbleCollisionEmpty",bubble.getCollisionShape(world,CENTER).isEmpty()); row.addProperty("rootSource",bubble.getFluidState().isSource()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
