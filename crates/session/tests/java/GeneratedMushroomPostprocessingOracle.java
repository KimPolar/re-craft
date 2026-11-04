import java.lang.reflect.*;
import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;

public class GeneratedMushroomPostprocessingOracle {
    static final Map<BlockPos,BlockState> blocks=new HashMap<>();
    static int brightness, writes, flags;
    static void set(Object object,String name,Object value)throws Exception {
        for(Class<?> type=object.getClass();type!=null;type=type.getSuperclass()) {
            try { var field=type.getDeclaredField(name);field.setAccessible(true);field.set(object,value);return; }
            catch(NoSuchFieldException ignored) {}
        }
        throw new NoSuchFieldException(name);
    }
    static class FakeLevel extends ServerLevel {
        FakeLevel(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
        @Override public BlockState getBlockState(BlockPos pos){return blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState());}
        @Override public int getRawBrightness(BlockPos pos,int skyDarken){if(skyDarken!=0)throw new AssertionError();return brightness;}
        @Override public RandomSource getRandom(){return RandomSource.create(42);}
        @Override public boolean setBlock(BlockPos pos,BlockState state,int updateFlags,int recursion){writes++;flags=updateFlags;blocks.put(pos.immutable(),state);return true;}
    }
    static class FakeChunk extends LevelChunk {
        FakeChunk(){super((Level)null,new ChunkPos(0,0));}
        @Override public ChunkPos getPos(){return new ChunkPos(0,0);}
        @Override public int getMinY(){return 0;}
        @Override public int getHeight(){return 16;}
        @Override public BlockState getBlockState(BlockPos pos){return blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState());}
    }
    static class FakeUpgrade extends UpgradeData {
        FakeUpgrade(){super(new CompoundTag(),LevelHeightAccessor.create(0,16));}
        @Override public void upgrade(LevelChunk chunk){}
    }
    public static void main(String[] args)throws Exception {
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");unsafeField.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
        var level=(FakeLevel)unsafe.allocateInstance(FakeLevel.class);
        var chunk=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class));
        var p=new BlockPos(8,8,8);
        var supports=List.of(Blocks.STONE,Blocks.DIRT,Blocks.GRASS_BLOCK,Blocks.PODZOL,Blocks.MYCELIUM,Blocks.CRIMSON_NYLIUM,Blocks.WARPED_NYLIUM,Blocks.WATER,Blocks.AIR,Blocks.CHEST,Blocks.SPAWNER,Blocks.DEEPSLATE);
        out.println("BEGIN_RESULT");
        for(var mushroom:List.of(Blocks.BROWN_MUSHROOM,Blocks.RED_MUSHROOM)) for(var support:supports) for(boolean override:new boolean[]{false,true}) for(int light:new int[]{0,12,13,15}) {
            bind.invoke(support.builtInRegistryHolder(),override?List.of(BlockTags.OVERRIDES_MUSHROOM_LIGHT_REQUIREMENT):List.of());
            blocks.clear();blocks.put(p,mushroom.defaultBlockState());blocks.put(p.below(),support.defaultBlockState());
            brightness=light;writes=0;flags=-1;
            ShortList[] lists={new ShortArrayList()};lists[0].add(ProtoChunk.packOffsetCoordinates(p));
            set(chunk,"postProcessing",lists);set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
            chunk.postProcessGeneration(level);
            out.printf("%s %s %s %d %s %d %d%n",BuiltInRegistries.BLOCK.getKey(mushroom),BuiltInRegistries.BLOCK.getKey(support),override,light,BuiltInRegistries.BLOCK.getKey(blocks.get(p).getBlock()),writes,flags);
            bind.invoke(support.builtInRegistryHolder(),List.of());
        }
        out.println("END_RESULT");
    }
}
