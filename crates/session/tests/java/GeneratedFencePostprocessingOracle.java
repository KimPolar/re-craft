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

public class GeneratedFencePostprocessingOracle {
    static final BlockPos POS=new BlockPos(8,8,8);
    static final Map<BlockPos,BlockState> blocks=new HashMap<>();
    static LevelTicks<Block> blockTicks;static LevelTicks<Fluid> fluidTicks;
    static RandomSource random;static int writes,lights;static long counter;
    static GameRules rules;static EnvironmentAttributeSystem attributes;
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
            var old=get(p);if(old==s)return false;if(flags!=276)throw new AssertionError("unexpected flags "+flags);
            writes++;if(LightEngine.hasDifferentLightProperties(old,s))lights++;blocks.put(p.immutable(),s);return true;
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
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
        var level=(FakeLevel)unsafe.allocateInstance(FakeLevel.class);var chunk=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class));rules=new GameRules(FeatureFlags.DEFAULT_FLAGS);attributes=EnvironmentAttributeSystem.builder().build();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,Map.of(
            FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.LAVA,List.of(Fluids.LAVA.builtInRegistryHolder(),Fluids.FLOWING_LAVA.builtInRegistryHolder())))).apply();
        out.println("BEGIN_RESULT");
        for(var mode:List.of("mixed","same_fence","other_fence","tag_air","wet_enclosed","gate_north","gate_east")) {
            boolean airTagged=mode.equals("tag_air");
            var fences=new ArrayList<net.minecraft.core.Holder<Block>>(List.of(Blocks.OAK_FENCE.builtInRegistryHolder(),Blocks.NETHER_BRICK_FENCE.builtInRegistryHolder()));
            var wooden=new ArrayList<net.minecraft.core.Holder<Block>>(List.of(Blocks.OAK_FENCE.builtInRegistryHolder()));
            if(airTagged){fences.add(Blocks.AIR.builtInRegistryHolder());wooden.add(Blocks.AIR.builtInRegistryHolder());}
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(
                BlockTags.FENCES,fences,BlockTags.WOODEN_FENCES,wooden,BlockTags.SHULKER_BOXES,List.of()))).apply();
            blocks.clear();boolean wet=mode.equals("wet_enclosed");
            blocks.put(POS,Blocks.OAK_FENCE.defaultBlockState().setValue(FenceBlock.WATERLOGGED,wet));
            if(!wet){
                blocks.put(POS.west(),Blocks.AIR.defaultBlockState());blocks.put(POS.east(),Blocks.SPRUCE_PLANKS.defaultBlockState());
                blocks.put(POS.north(),Blocks.OAK_LOG.defaultBlockState());blocks.put(POS.south(),Blocks.AIR.defaultBlockState());
                if(mode.equals("same_fence"))blocks.put(POS.west(),Blocks.OAK_FENCE.defaultBlockState());
                if(mode.equals("other_fence"))blocks.put(POS.west(),Blocks.NETHER_BRICK_FENCE.defaultBlockState());
                if(mode.startsWith("gate_"))blocks.put(POS.west(),Blocks.OAK_FENCE_GATE.defaultBlockState().setValue(FenceGateBlock.FACING,mode.equals("gate_north")?Direction.NORTH:Direction.EAST));
            }
            ShortList[] marks={new ShortArrayList()};marks[0].add(ProtoChunk.packOffsetCoordinates(POS));
            set(chunk,"postProcessing",marks);set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
            blockTicks=new LevelTicks<>(p->true);fluidTicks=new LevelTicks<>(p->true);
            var bs=new LevelChunkTicks<Block>();var fs=new LevelChunkTicks<Fluid>();
            blockTicks.addContainer(new ChunkPos(0,0),bs);fluidTicks.addContainer(new ChunkPos(0,0),fs);
            random=RandomSource.create(42);counter=0;writes=0;lights=0;chunk.postProcessGeneration(level);
            var state=get(POS);var saved=fs.pack(100);
            out.printf("F %s %s %s %s %s %s %d %d %d %d %d %d%n",mode,state.getValue(FenceBlock.NORTH),state.getValue(FenceBlock.EAST),
                state.getValue(FenceBlock.SOUTH),state.getValue(FenceBlock.WEST),state.getValue(FenceBlock.WATERLOGGED),
                bs.pack(100).size(),saved.size(),saved.isEmpty()?-1:saved.get(0).delay(),counter,writes,lights);
        }
        out.println("END_RESULT");
    }
}
