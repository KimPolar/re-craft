import java.lang.reflect.*;
import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.ticks.*;

public class GeneratedFallingPostprocessingOracle {
    static final Map<BlockPos,BlockState> blocks=new HashMap<>();
    static LevelTicks<Block> tickAccess;
    static long counter=2;
    static void set(Object object,String name,Object value)throws Exception {
        for(Class<?> type=object.getClass();type!=null;type=type.getSuperclass()) {
            try { var field=type.getDeclaredField(name);field.setAccessible(true);field.set(object,value);return; }
            catch(NoSuchFieldException ignored) {}
        }
        throw new NoSuchFieldException(name);
    }
    static class FakeLevel extends ServerLevel {
        FakeLevel(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
        @Override public BlockState getBlockState(BlockPos pos){return blocks.getOrDefault(pos,Blocks.STONE.defaultBlockState());}
        @Override public RandomSource getRandom(){return RandomSource.create(42);}
        @Override public LevelTicks<Block> getBlockTicks(){return tickAccess;}
        @Override public long getGameTime(){return 100L;}
        @Override public long nextSubTickCount(){return counter++;}
    }
    static class FakeChunk extends LevelChunk {
        FakeChunk(){super((Level)null,new ChunkPos(0,0));}
        @Override public ChunkPos getPos(){return new ChunkPos(0,0);}
        @Override public int getMinY(){return 0;}
        @Override public int getHeight(){return 16;}
        @Override public BlockState getBlockState(BlockPos pos){return blocks.getOrDefault(pos,Blocks.STONE.defaultBlockState());}
    }
    static class FakeUpgrade extends UpgradeData {
        FakeUpgrade(){super(new CompoundTag(),LevelHeightAccessor.create(0,16));}
        @Override public void upgrade(LevelChunk chunk){}
    }
    public static void main(String[] args)throws Exception {
        var out=System.out;
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var level=(FakeLevel)unsafe.allocateInstance(FakeLevel.class);
        var chunk=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        var a=new BlockPos(8,8,8);var b=new BlockPos(9,8,8);var c=new BlockPos(10,8,8);
        blocks.put(a,Blocks.SAND.defaultBlockState());blocks.put(b,Blocks.RED_SAND.defaultBlockState());blocks.put(c,Blocks.GRAVEL.defaultBlockState());
        ShortList[] lists={new ShortArrayList()};
        for(var p:List.of(a,a,b,c))lists[0].add(ProtoChunk.packOffsetCoordinates(p));
        set(chunk,"postProcessing",lists);set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
        set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class));
        var pending=List.of(new SavedTick<>(Blocks.SAND,a,0,TickPriority.NORMAL));
        var ticks=new LevelChunkTicks<Block>(pending);tickAccess=new LevelTicks<Block>(p->true);
        tickAccess.addContainer(new ChunkPos(0,0),ticks);
        chunk.postProcessGeneration(level);
        out.println("BEGIN_RESULT");
        for(var tick:ticks.pack(100L)) {
            var p=tick.pos();
            out.printf("%s %d,%d,%d %d %d%n",BuiltInRegistries.BLOCK.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay(),tick.priority().getValue());
        }
        out.println("counter "+counter);
        out.println("END_RESULT");
    }
}
