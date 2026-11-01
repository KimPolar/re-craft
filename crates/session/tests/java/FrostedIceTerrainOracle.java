import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.util.RandomSource;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.Vec3;

public class FrostedIceTerrainOracle {
    static final BlockPos CENTER=new BlockPos(8,2,8);
    static final Direction[] DIRECTIONS=Direction.values();
    static class World extends ServerLevel {
        Map<BlockPos,BlockState> blocks; RandomSource random; JsonArray scheduled, fluids; Set<String> pending;
        CollectingNeighborUpdater updater; EnvironmentAttributeSystem attributes;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public RandomSource getRandom() { return random; }
        @Override public int getBrightness(LightLayer layer,BlockPos pos) { return 15; }
        @Override public int getMaxLocalRawBrightness(BlockPos pos) { return 15; }
        @Override public ResourceKey<Level> dimension() { return Level.OVERWORLD; }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.DEFAULT_FLAGS; }
        @Override public EnvironmentAttributeSystem environmentAttributes() { return attributes; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState()); }
        @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        @Override public void neighborChanged(BlockPos pos,Block block,Orientation orientation) { updater.neighborChanged(pos,block,orientation); }
        @Override public void neighborShapeChanged(Direction dir,BlockPos target,BlockPos source,BlockState state,int flags,int depth) { updater.shapeUpdate(dir,state,target,source,flags,depth); }
        @Override public boolean setBlock(BlockPos pos,BlockState state,int flags,int depth) {
            var old=getBlockState(pos); if(old==state) return false; blocks.put(pos.immutable(),state);
            if((flags & 512)==0) state.onPlace(this,pos,old,false);
            if((flags & 1)!=0) updater.updateNeighborsAtExceptFromFacing(pos,old.getBlock(),null,null);
            if((flags & 16)==0 && depth>0) { int next=flags & -34; old.updateIndirectNeighbourShapes(this,pos,next,depth-1); state.updateNeighbourShapes(this,pos,next,depth-1); state.updateIndirectNeighbourShapes(this,pos,next,depth-1); }
            return true;
        }
        @Override public void scheduleTick(BlockPos pos,Block block,int delay) {
            String key=pos.toShortString()+":"+block;
            if(!pending.add(key)) return;
            var entry=new JsonArray(); entry.add(block==Blocks.FROSTED_ICE ? "ice" : "falling"); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); scheduled.add(entry);
        }
        @Override public void scheduleTick(BlockPos pos,net.minecraft.world.level.material.Fluid fluid,int delay) {
            String key="fluid:"+pos.toShortString()+":"+fluid; if(!pending.add(key)) return;
            var entry=new JsonArray(); entry.add(fluid==Fluids.WATER ? "water" : "flowing_water"); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); fluids.add(entry);
        }
        @Override public void gameEvent(Holder<GameEvent> event,Vec3 pos,GameEvent.Context context) { }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class); world.attributes=EnvironmentAttributeSystem.builder().build();
        var tick=FrostedIceBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class); tick.setAccessible(true);
        Block[] terrain={Blocks.SAND,Blocks.RED_SAND,Blocks.GRAVEL,Blocks.GLASS,Blocks.COBBLESTONE,Blocks.MOSSY_COBBLESTONE,
            Blocks.GRANITE,Blocks.DIORITE,Blocks.ANDESITE,Blocks.CALCITE,Blocks.TUFF,Blocks.SANDSTONE,Blocks.RED_SANDSTONE,Blocks.STONE_BRICKS,Blocks.CLAY,Blocks.WATER,Blocks.WATER}; var rows=new JsonArray();
        for(int seed=0;seed<4;seed++) for(int type=0;type<terrain.length;type++) for(int age:new int[]{0,1,2,3}) for(int mask:new int[]{1,2,4,8,16,32,63}) {
            world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(seed); world.scheduled=new JsonArray(); world.fluids=new JsonArray(); world.pending=new HashSet<>(); world.updater=new CollectingNeighborUpdater(world,10000);
            var center=Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,age); world.blocks.put(CENTER,center);
            for(int i=0;i<6;i++) {
                var state=((mask & (1<<i))!=0 ? terrain[type] : Blocks.STONE).defaultBlockState();
                if(type==16 && (mask & (1<<i))!=0) state=state.setValue(LiquidBlock.LEVEL,1);
                world.blocks.put(CENTER.relative(DIRECTIONS[i]),state);
            }
            tick.invoke(Blocks.FROSTED_ICE,center,world,CENTER,world.random);
            var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("type",type); row.addProperty("age",age); row.addProperty("mask",mask); row.add("scheduled",world.scheduled); row.add("fluids",world.fluids); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
