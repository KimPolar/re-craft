import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.redstone.CollectingNeighborUpdater;

public class BubbleWaterTickSessionOracle {
    static final BlockPos CENTER=new BlockPos(8,10,8);
    static final Direction[] SIDES={Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
    static class World extends IceBubbleTagOracle.World {
        GameRules rules; JsonArray changes;
        World() { super(); }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos,Blocks.STONE.defaultBlockState()); }
        @Override public GameRules getGameRules() { return rules; }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 32; }
        @Override public boolean setBlockAndUpdate(BlockPos pos,BlockState state) { return setBlock(pos,state,3,512); }
        @Override public boolean setBlock(BlockPos pos,BlockState state,int flags,int depth) {
            var old=getBlockState(pos);
            if(old==state) return false;
            var entry=new JsonArray(); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()); entry.add(state.is(Blocks.WATER)?state.getValue(LiquidBlock.LEVEL):-1); changes.add(entry);
            return super.setBlock(pos,state,flags,depth);
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,Map.of(
            FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.BUBBLE_COLUMN_CAN_OCCUPY,List.of(Fluids.WATER.builtInRegistryHolder())))).apply();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class); world.attributes=EnvironmentAttributeSystem.builder().build(); world.rules=new GameRules(FeatureFlags.DEFAULT_FLAGS);
        var rows=new JsonArray();
        for(boolean tagged:new boolean[]{false,true}) for(boolean hole:new boolean[]{false,true}) for(int mask=0;mask<16;mask++) {
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(
                BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder(),Blocks.LAVA.builtInRegistryHolder()),
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,tagged?List.of(Blocks.STONE.builtInRegistryHolder()):List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,List.of()))).apply();
            world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(1); world.bubble=false; world.updater=new CollectingNeighborUpdater(world,100000); world.blockTicks=new JsonArray(); world.fluidTicks=new JsonArray(); world.pending=new HashSet<>(); world.changes=new JsonArray();
            var bubble=Blocks.BUBBLE_COLUMN.defaultBlockState(); world.blocks.put(CENTER,bubble);
            if(hole) world.blocks.put(CENTER.below(),Blocks.AIR.defaultBlockState());
            for(int i=0;i<4;i++) if((mask & (1<<i))!=0) world.blocks.put(CENTER.relative(SIDES[i]),Blocks.AIR.defaultBlockState());
            Fluids.WATER.tick(world,CENTER,bubble,bubble.getFluidState());
            var row=new JsonObject(); row.addProperty("tagged",tagged); row.addProperty("hole",hole); row.addProperty("mask",mask);
            row.add("changes",world.changes); row.add("blockTicks",world.blockTicks); row.add("fluidTicks",world.fluidTicks); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
