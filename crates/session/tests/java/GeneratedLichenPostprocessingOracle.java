import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.material.*;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.ticks.*;

public class GeneratedLichenPostprocessingOracle {
    static final BlockPos POS = new BlockPos(8,8,8);
    static final Map<BlockPos,BlockState> blocks = new HashMap<>();
    static LevelTicks<Block> blockTicks; static LevelTicks<Fluid> fluidTicks;
    static RandomSource random; static int writes,lights; static long counter;
    static GameRules rules; static EnvironmentAttributeSystem attributes;
    static BlockState get(BlockPos p){return blocks.getOrDefault(p,Blocks.STONE.defaultBlockState());}
    static void set(Object obj,String name,Object value)throws Exception{
        for(var c=obj.getClass();c!=null;c=c.getSuperclass())try{var f=c.getDeclaredField(name);f.setAccessible(true);f.set(obj,value);return;}catch(NoSuchFieldException ignored){}
        throw new NoSuchFieldException(name);
    }
    static class FakeLevel extends ServerLevel {
        FakeLevel(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
        @Override public BlockState getBlockState(BlockPos p){return get(p);}
        @Override public FluidState getFluidState(BlockPos p){return get(p).getFluidState();}
        @Override public RandomSource getRandom(){return random;}
        @Override public GameRules getGameRules(){return rules;}
        @Override public FeatureFlagSet enabledFeatures(){return FeatureFlags.DEFAULT_FLAGS;}
        @Override public EnvironmentAttributeSystem environmentAttributes(){return attributes;}
        @Override public LevelTicks<Block> getBlockTicks(){return blockTicks;}
        @Override public LevelTicks<Fluid> getFluidTicks(){return fluidTicks;}
        @Override public long getGameTime(){return 100;}
        @Override public long nextSubTickCount(){return counter++;}
        @Override public boolean setBlock(BlockPos p,BlockState s,int flags,int depth){
            var old=get(p); if(old==s)return false; if(flags!=276)throw new AssertionError("unexpected flags "+flags);
            writes++; if(LightEngine.hasDifferentLightProperties(old,s))lights++; blocks.put(p.immutable(),s); return true;
        }
    }
    static class FakeChunk extends LevelChunk {
        FakeChunk(){super((Level)null,new ChunkPos(0,0));}
        @Override public ChunkPos getPos(){return new ChunkPos(0,0);}
        @Override public int getMinY(){return 0;}
        @Override public int getHeight(){return 16;}
        @Override public BlockState getBlockState(BlockPos p){return get(p);}
    }
    static class FakeUpgrade extends UpgradeData {
        FakeUpgrade(){super(new CompoundTag(),LevelHeightAccessor.create(0,16));}
        @Override public void upgrade(LevelChunk chunk){}
    }
    public static void main(String[] args)throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var level=(FakeLevel)unsafe.allocateInstance(FakeLevel.class); var chunk=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class)); rules=new GameRules(FeatureFlags.DEFAULT_FLAGS); attributes=EnvironmentAttributeSystem.builder().build();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,Map.of(
            FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.LAVA,List.of(Fluids.LAVA.builtInRegistryHolder(),Fluids.FLOWING_LAVA.builtInRegistryHolder())))).apply();
        out.println("BEGIN_RESULT");
        for(var mode:List.of("supported","lost_west","lost_one","empty","wet","all","lose_down","lose_up","lose_north","lose_south","lose_west","lose_east")) {
            blocks.clear(); var state=Blocks.GLOW_LICHEN.defaultBlockState();
            if(!mode.equals("empty")) state=state.setValue(MultifaceBlock.getFaceProperty(Direction.WEST),true);
            if(mode.equals("lost_one") || mode.equals("all")) state=state.setValue(MultifaceBlock.getFaceProperty(Direction.EAST),true);
            if(mode.equals("all") || mode.startsWith("lose_")) for(var dir:Direction.values()) state=state.setValue(MultifaceBlock.getFaceProperty(dir),true);
            if(mode.equals("wet")) state=state.setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED,true);
            blocks.put(POS,state);
            if(mode.startsWith("lost_"))blocks.put(POS.west(),Blocks.AIR.defaultBlockState());
            if(mode.startsWith("lose_"))blocks.put(POS.relative(Direction.byName(mode.substring(5))),Blocks.AIR.defaultBlockState());
            ShortList[] marks={new ShortArrayList()}; marks[0].add(ProtoChunk.packOffsetCoordinates(POS));
            set(chunk,"postProcessing",marks);set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
            blockTicks=new LevelTicks<>(p->true);fluidTicks=new LevelTicks<>(p->true);
            var bs=new LevelChunkTicks<Block>();var fs=new LevelChunkTicks<Fluid>();
            blockTicks.addContainer(new ChunkPos(0,0),bs);fluidTicks.addContainer(new ChunkPos(0,0),fs);
            random=RandomSource.create(42);counter=0;writes=0;lights=0;chunk.postProcessGeneration(level);
            state=get(POS); var saved=fs.pack(100); String mask="-";
            if(state.is(Blocks.GLOW_LICHEN)) { var b=new StringBuilder(); for(var dir:new Direction[]{Direction.DOWN,Direction.UP,Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST}) b.append(state.getValue(MultifaceBlock.getFaceProperty(dir))?'1':'0'); mask=b.toString(); }
            out.printf("L %s %s %s %d %d %d %d %d %d%n",mode,BuiltInRegistries.BLOCK.getKey(state.getBlock()),mask,
                bs.pack(100).size(),saved.size(),saved.isEmpty()?-1:saved.get(0).delay(),counter,writes,lights);
        }
        out.println("END_RESULT");
    }
}
