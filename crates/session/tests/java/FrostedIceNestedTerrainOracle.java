import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.phys.Vec3;

public class FrostedIceNestedTerrainOracle {
    static final BlockPos CENTER=new BlockPos(8,10,8);
    static final Direction[] SIDES={Direction.WEST,Direction.EAST,Direction.NORTH,Direction.SOUTH};
    static class World extends ServerLevel {
        Map<BlockPos,BlockState> blocks; RandomSource random; JsonArray scheduled,fluids; Set<String> pending;
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
            String key="block:"+pos.toShortString()+":"+block; if(!pending.add(key)) return;
            var entry=new JsonArray(); entry.add(block==Blocks.FROSTED_ICE?"ice":"falling"); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); scheduled.add(entry);
        }
        @Override public void scheduleTick(BlockPos pos,net.minecraft.world.level.material.Fluid fluid,int delay) {
            String key="fluid:"+pos.toShortString()+":"+fluid; if(!pending.add(key)) return;
            var entry=new JsonArray(); entry.add(fluid==Fluids.WATER?"water":"flowing_water"); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); fluids.add(entry);
        }
        @Override public void gameEvent(Holder<GameEvent> event,Vec3 pos,GameEvent.Context context) { }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class); world.attributes=EnvironmentAttributeSystem.builder().build();
        var tick=FrostedIceBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class); tick.setAccessible(true);
        var rows=new JsonArray();
        for(int seed=0;seed<8;seed++) for(int filler=0;filler<4;filler++) for(int mask=1;mask<16;mask++) for(int outer=0;outer<2;outer++) for(boolean evaporates:new boolean[]{false,true}) {
            world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(seed); world.scheduled=new JsonArray(); world.fluids=new JsonArray(); world.pending=new HashSet<>(); world.updater=new CollectingNeighborUpdater(world,100000);
            world.attributes=EnvironmentAttributeSystem.builder().addConstantLayer(net.minecraft.world.attribute.EnvironmentAttributes.WATER_EVAPORATES, base -> evaporates).build();
            var fill=(filler<2?Blocks.WATER:filler==2?Blocks.SAND:Blocks.GRAVEL).defaultBlockState();
            if(filler==1) fill=fill.setValue(LiquidBlock.LEVEL,1);
            for(int dx=-2;dx<=2;dx++) for(int dy=-1;dy<=1;dy++) for(int dz=-2;dz<=2;dz++) if(Math.abs(dx)+Math.abs(dy)+Math.abs(dz)<=2 && (dx!=0||dy!=0||dz!=0)) world.blocks.put(CENTER.offset(dx,dy,dz),fill);
            var center=Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,3); world.blocks.put(CENTER,center);
            for(int i=0;i<4;i++) if((mask & (1<<i))!=0) {
                var pos=CENTER.relative(SIDES[i]); world.blocks.put(pos,Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,(seed+i)%4));
                if(outer!=0) world.blocks.put(pos.relative(SIDES[i]),Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,(seed+i+1)%4));
            }
            tick.invoke(Blocks.FROSTED_ICE,center,world,CENTER,world.random);
            var finalStates=new JsonArray();
            for(int i=-1;i<8;i++) {
                var pos=i<0?CENTER:CENTER.relative(SIDES[i%4],i<4?1:2); var state=world.getBlockState(pos);
                var entry=new JsonArray(); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
                entry.add(state.is(Blocks.FROSTED_ICE)?state.getValue(FrostedIceBlock.AGE):-1); finalStates.add(entry);
            }
            var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("filler",filler); row.addProperty("mask",mask); row.addProperty("outer",outer); row.addProperty("evaporates",evaporates);
            row.add("states",finalStates); row.add("scheduled",world.scheduled); row.add("fluids",world.fluids); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
