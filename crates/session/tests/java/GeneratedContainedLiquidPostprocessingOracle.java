import java.lang.reflect.*;
import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.FluidTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.attribute.*;

public class GeneratedContainedLiquidPostprocessingOracle {
    static final BlockPos pos = new BlockPos(8,8,8);
    static BlockState liquid;
    static RandomSource random;
    static int writes;
    static EnvironmentAttributeSystem attributes;
    static void set(Object object, String name, Object value) throws Exception {
        for (Class<?> type = object.getClass(); type != null; type = type.getSuperclass()) {
            try { var field=type.getDeclaredField(name);field.setAccessible(true);field.set(object,value);return; }
            catch(NoSuchFieldException ignored) {}
        }
        throw new NoSuchFieldException(name);
    }
    static class FakeLevel extends ServerLevel {
        FakeLevel(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
        @Override public BlockState getBlockState(BlockPos p){return p.equals(pos)?liquid:Blocks.STONE.defaultBlockState();}
        @Override public FluidState getFluidState(BlockPos p){return getBlockState(p).getFluidState();}
        @Override public RandomSource getRandom(){return random;}
        @Override public EnvironmentAttributeSystem environmentAttributes(){return attributes;}
        @Override public boolean setBlock(BlockPos p, BlockState state, int flags, int depth){
            if(!state.equals(getBlockState(p)))throw new AssertionError("unexpected block change");writes++;return false;
        }
    }
    static class FakeChunk extends LevelChunk {
        FakeChunk(){super((Level)null,new ChunkPos(0,0));}
        @Override public ChunkPos getPos(){return new ChunkPos(0,0);}
        @Override public int getMinY(){return 0;}
        @Override public int getHeight(){return 16;}
        @Override public BlockState getBlockState(BlockPos p){return p.equals(pos)?liquid:Blocks.STONE.defaultBlockState();}
    }
    static class FakeUpgrade extends UpgradeData {
        FakeUpgrade(){super(new CompoundTag(),LevelHeightAccessor.create(0,16));}
        @Override public void upgrade(LevelChunk chunk){}
    }
    public static void main(String[] args) throws Exception {
        try { run(); } catch (Throwable error) { error.printStackTrace(System.err); throw error; }
    }
    static void run()throws Exception {
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
        var level=(FakeLevel)unsafe.allocateInstance(FakeLevel.class);
        var chunk=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class));
        out.println("BEGIN_RESULT");
        for(var block:List.of(Blocks.WATER,Blocks.LAVA))for(boolean occupy:new boolean[]{false,true})for(boolean fast:new boolean[]{false,true}) {
            attributes=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,fast).build()).build();
            liquid=block.defaultBlockState();
            bind.invoke(liquid.getFluidState().getType().builtInRegistryHolder(),occupy?List.of(FluidTags.BUBBLE_COLUMN_CAN_OCCUPY):List.of());
            ShortList[] marks={new ShortArrayList()};marks[0].add(ProtoChunk.packOffsetCoordinates(pos));
            set(chunk,"postProcessing",marks);set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
            random=RandomSource.create(42);writes=0;
            chunk.postProcessGeneration(level);
            if(random.nextLong()!=RandomSource.create(42).nextLong())throw new AssertionError("unexpected random use");
            if(writes!=(occupy ? 1 : 0))throw new AssertionError("unexpected bubble update count: "+writes);
            out.printf("%s 0 %s %s %s%n",BuiltInRegistries.BLOCK.getKey(block),occupy,BuiltInRegistries.BLOCK.getKey(liquid.getBlock()),fast);
            bind.invoke(liquid.getFluidState().getType().builtInRegistryHolder(),List.of());
        }
        out.println("END_RESULT");
    }
}
