import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.ticks.*;

public class GeneratedInertPostprocessingOracle {
    static final BlockPos POS = new BlockPos(8,8,8);
    static BlockState state, neighbor;
    static LevelTicks<Block> blocks;
    static LevelTicks<Fluid> fluids;
    static RandomSource random;
    static int writes;
    static void set(Object object,String name,Object value)throws Exception {
        for(Class<?> type=object.getClass();type!=null;type=type.getSuperclass()) {
            try {var field=type.getDeclaredField(name);field.setAccessible(true);field.set(object,value);return;}
            catch(NoSuchFieldException ignored) {}
        }
        throw new NoSuchFieldException(name);
    }
    static class FakeLevel extends ServerLevel {
        FakeLevel(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
        @Override public BlockState getBlockState(BlockPos p){return p.equals(POS)?state:neighbor;}
        @Override public RandomSource getRandom(){return random;}
        @Override public LevelTicks<Block> getBlockTicks(){return blocks;}
        @Override public LevelTicks<Fluid> getFluidTicks(){return fluids;}
        @Override public long getGameTime(){return 100;}
        @Override public long nextSubTickCount(){throw new AssertionError("unexpected scheduled tick");}
        @Override public boolean setBlock(BlockPos p,BlockState s,int flags,int depth){writes++;return true;}
    }
    static class FakeChunk extends LevelChunk {
        FakeChunk(){super((Level)null,new ChunkPos(0,0));}
        @Override public ChunkPos getPos(){return new ChunkPos(0,0);}
        @Override public int getMinY(){return 0;}
        @Override public int getHeight(){return 16;}
        @Override public BlockState getBlockState(BlockPos p){return p.equals(POS)?state:neighbor;}
    }
    static class FakeUpgrade extends UpgradeData {
        FakeUpgrade(){super(new CompoundTag(),LevelHeightAccessor.create(0,16));}
        @Override public void upgrade(LevelChunk chunk){}
    }
    public static void main(String[] args)throws Exception {
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var level=(FakeLevel)unsafe.allocateInstance(FakeLevel.class);
        var chunk=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class));
        var names=List.of("stone","granite","diorite","andesite","tuff","calcite","dripstone_block",
            "cobblestone","mossy_cobblestone","dirt","coarse_dirt","rooted_dirt","clay","bedrock",
            "netherrack","end_stone","blackstone","obsidian","crying_obsidian","coal_ore","iron_ore",
            "copper_ore","gold_ore","lapis_ore","diamond_ore","emerald_ore","deepslate_coal_ore",
            "deepslate_iron_ore","deepslate_copper_ore","deepslate_gold_ore","deepslate_lapis_ore",
            "deepslate_diamond_ore","deepslate_emerald_ore","nether_gold_ore","nether_quartz_ore","ancient_debris");
        out.println("BEGIN_RESULT");
        for(var name:names) for(var adjacent:List.of(Blocks.STONE,Blocks.AIR,Blocks.WATER)) {
            var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name));
            state=block.defaultBlockState();neighbor=adjacent.defaultBlockState();
            if(block.getStateDefinition().getPossibleStates().size()!=1 || !state.getFluidState().isEmpty()
                || !Block.isShapeFullBlock(state.getCollisionShape(EmptyBlockGetter.INSTANCE,POS)))
                throw new AssertionError("not an inert full-cube candidate: "+name);
            ShortList[] marks={new ShortArrayList()};marks[0].add(ProtoChunk.packOffsetCoordinates(POS));
            set(chunk,"postProcessing",marks);set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
            blocks=new LevelTicks<>(p->true);fluids=new LevelTicks<>(p->true);
            var blockTicks=new LevelChunkTicks<Block>();var fluidTicks=new LevelChunkTicks<Fluid>();
            blocks.addContainer(new ChunkPos(0,0),blockTicks);
            fluids.addContainer(new ChunkPos(0,0),fluidTicks);
            random=RandomSource.create(42);writes=0;
            chunk.postProcessGeneration(level);
            out.printf("%s %s %d %d %d %d%n",name,BuiltInRegistries.BLOCK.getKey(adjacent).getPath(),
                writes,blockTicks.pack(100).size(),fluidTicks.pack(100).size(),random.nextLong());
        }
        out.println("END_RESULT");
    }
}
