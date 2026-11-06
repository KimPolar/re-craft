import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.*;
import net.minecraft.world.level.storage.loot.LootTable;

public class MineshaftFullCorridorOracle {
    static final List<String> events=new ArrayList<>();
    static final Map<BlockPos,BlockState> changed=new HashMap<>();
    static int mode;
    static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
    static BlockState at(BlockPos p){
        var s=changed.get(p);if(s!=null)return s;
        int h=p.getX()*31+p.getY()*17+p.getZ()*13;
        if(mode==2 && Math.floorMod(h,19)==0)return Blocks.WATER.defaultBlockState();
        if(Math.floorMod(h,29)==0)return Blocks.OAK_PLANKS.defaultBlockState();
        if(Math.floorMod(h,31)==0)return Blocks.DARK_OAK_PLANKS.defaultBlockState();
        if(Math.floorMod(h,37)==0)return Blocks.IRON_CHAIN.defaultBlockState();
        return Blocks.STONE.defaultBlockState();
    }
    static String describe(BlockState s){
        var b=s.getBlock();
        if(b==Blocks.CAVE_AIR)return "air";
        if(b==Blocks.OAK_PLANKS)return "oak_planks";
        if(b==Blocks.DARK_OAK_PLANKS)return "dark_planks";
        if(b==Blocks.OAK_LOG)return "oak_log";
        if(b==Blocks.DARK_OAK_LOG)return "dark_log";
        if(b==Blocks.IRON_CHAIN)return "chain";
        if(b==Blocks.SPAWNER)return "spawner";
        if(b==Blocks.COBWEB)return "web";
        if(b==Blocks.RAIL)return "rail:"+s.getValue(RailBlock.SHAPE).getSerializedName();
        if(b==Blocks.WALL_TORCH)return "torch:"+s.getValue(WallTorchBlock.FACING).getSerializedName();
        if(b instanceof FenceBlock){
            String type=b==Blocks.OAK_FENCE?"oak_fence":"dark_fence";
            for(var d:new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST})
                if(s.getValue(FenceBlock.PROPERTY_BY_DIRECTION.get(d)))return type+":"+d.getSerializedName();
            return type;
        }
        throw new AssertionError("unexpected write "+s);
    }
    static class Probe extends MineshaftPieces.MineShaftCorridor {
        Probe(BoundingBox b,Direction d,MineshaftStructure.Type t){super(1,RandomSource.create(5),b,d,t);}
        @Override protected boolean createChest(WorldGenLevel level,BoundingBox clip,RandomSource random,int x,int y,int z,ResourceKey<LootTable> table){events.add("C "+x+","+y+","+z);return false;}
    }
    static class TracedChunk extends ProtoChunk {
        TracedChunk(){super(new ChunkPos(0,0),null,(LevelHeightAccessor)null,null,null);}
        @Override public void markPosForPostProcessing(BlockPos p){events.add("M "+pos(p));}
    }
    static void field(Object obj,String name,Object value)throws Exception{
        var f=MineshaftPieces.MineShaftCorridor.class.getDeclaredField(name);f.setAccessible(true);f.set(obj,value);
    }
    static boolean flag(Object obj,String name)throws Exception{
        var f=MineshaftPieces.MineShaftCorridor.class.getDeclaredField(name);f.setAccessible(true);return f.getBoolean(obj);
    }
    public static void main(String[]args)throws Exception{
        var out=System.out;var err=System.err;
        try{
            SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
            var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
            var owner=new HolderOwner<net.minecraft.world.level.biome.Biome>(){};
            var clear=Holder.Reference.createStandAlone(owner,net.minecraft.world.level.biome.Biomes.PLAINS);
            var blocked=Holder.Reference.createStandAlone(owner,net.minecraft.world.level.biome.Biomes.PLAINS);
            bind.invoke(clear,List.of());bind.invoke(blocked,List.of(net.minecraft.tags.BiomeTags.MINESHAFT_BLOCKING));
            var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");unsafeField.setAccessible(true);
            var chunk=(TracedChunk)((sun.misc.Unsafe)unsafeField.get(null)).allocateInstance(TracedChunk.class);
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
                switch(m.getName()){
                    case "getBiome":{var p=(BlockPos)a[0];events.add("B "+pos(p));return mode==1?blocked:clear;}
                    case "getBlockState":{var p=(BlockPos)a[0];events.add("R "+pos(p));return at(p);}
                    case "getHeight":{int x=(int)a[1],z=(int)a[2];int h=Math.floorMod(x*7+z*3,5)==0?30:40;events.add("H "+x+",0,"+z+" "+h);return h;}
                    case "getMinY":return -64;
                    case "getMaxY":return 100;
                    case "setBlock":{var p=((BlockPos)a[0]).immutable();var s=(BlockState)a[1];if(!a[2].equals(2))throw new AssertionError("flags");events.add("W "+pos(p)+" "+describe(s));changed.put(p,s);return true;}
                    case "getFluidState":{var p=(BlockPos)a[0];events.add("F "+pos(p));return at(p).getFluidState();}
                    case "getBlockEntity":{var p=(BlockPos)a[0];events.add("E "+pos(p));return null;}
                    case "getChunk":return chunk;
                    default:throw new AssertionError("world callback "+m);
                }
            });
            var dirs=new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
            for(int direction=0;direction<4;direction++)for(int clipId=0;clipId<3;clipId++)for(int kind=0;kind<4;kind++)
                for(long seed:new long[]{-3,0,1,2,42,59,12345,123456789})for(var variant:MineshaftStructure.Type.values()){
                    mode=seed==0?1:seed==1?2:0;
                    int sections=seed==-3?1:seed==12345?4:2;
                    boolean zAxis=direction<2;
                    int end=sections*5-1;
                    var b=new BoundingBox(0,30,0,zAxis?2:end,32,zAxis?end:2);
                    var piece=new Probe(b,dirs[direction],variant);
                    field(piece,"hasRails",kind==1);field(piece,"spiderCorridor",kind>=2);field(piece,"hasPlacedSpider",kind==3);
                    var clip=switch(clipId){case 0->new BoundingBox(-5,25,-5,25,40,25);case 1->new BoundingBox(0,30,0,7,32,7);default->new BoundingBox(0,29,0,1,33,9);};
                    var random=new WorldgenRandom(new XoroshiroRandomSource(seed));changed.clear();events.clear();
                    piece.postProcess(world,null,null,random,clip,new ChunkPos(0,0),BlockPos.ZERO);
                    out.printf("C %d %d %d %d %d %s %b %d %d%n",direction,clipId,kind,seed,sections,variant.name(),flag(piece,"hasPlacedSpider"),events.size(),random.nextLong());
                    for(var event:events)out.println(event);
                }
        }catch(Throwable error){error.printStackTrace(err);System.exit(1);}
    }
}
