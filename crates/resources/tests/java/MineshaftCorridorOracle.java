import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.*;

public class MineshaftCorridorOracle {
    static final List<String> events=new ArrayList<>();
    static final Map<BlockPos,BlockState> changed=new HashMap<>();
    static int scenario;
    static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
    static String facing(Direction d){return d.getSerializedName();}
    static BlockState at(BlockPos p){
        var changedState=changed.get(p);if(changedState!=null)return changedState;
        int h=p.getX()*31+p.getY()*17+p.getZ()*13;
        if(scenario==1 && p.getY()==33)return Blocks.AIR.defaultBlockState();
        if(scenario==3 && Math.floorMod(h,3)==0)return Blocks.AIR.defaultBlockState();
        if(Math.floorMod(h,11)==0)return Blocks.OAK_PLANKS.defaultBlockState();
        if(Math.floorMod(h,13)==0)return Blocks.DARK_OAK_PLANKS.defaultBlockState();
        if(Math.floorMod(h,17)==0)return Blocks.IRON_CHAIN.defaultBlockState();
        return Blocks.STONE.defaultBlockState();
    }
    static String describe(BlockState s){
        if(s.getBlock()==Blocks.OAK_PLANKS||s.getBlock()==Blocks.DARK_OAK_PLANKS)return "planks";
        if(s.getBlock()==Blocks.COBWEB)return "web";
        if(s.getBlock()==Blocks.WALL_TORCH)return "torch:"+facing(s.getValue(WallTorchBlock.FACING));
        if(s.getBlock() instanceof FenceBlock){
            for(var d:new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST})
                if(s.getValue(FenceBlock.PROPERTY_BY_DIRECTION.get(d)))return "fence:"+facing(d);
        }
        throw new AssertionError("unexpected block "+s);
    }
    static class TracedChunk extends ProtoChunk {
        TracedChunk(){super(new ChunkPos(0,0),null,(LevelHeightAccessor)null,null,null);}
        @Override public void markPosForPostProcessing(BlockPos p){events.add("M "+pos(p));}
    }
    public static void main(String[]args)throws Exception{
        var out=System.out;var err=System.err;
        try{
            SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
            var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");unsafeField.setAccessible(true);
            var chunk=(TracedChunk)((sun.misc.Unsafe)unsafeField.get(null)).allocateInstance(TracedChunk.class);
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
                switch(m.getName()){
                    case "getBlockState": {var p=(BlockPos)a[0];events.add("R "+pos(p));return at(p);}
                    case "getHeight": {int x=(int)a[1],z=(int)a[2];int height=scenario==2?31:40;events.add("H "+x+",0,"+z+" "+height);return height;}
                    case "setBlock": {var p=((BlockPos)a[0]).immutable();var s=(BlockState)a[1];
                        if(!a[2].equals(2))throw new AssertionError("wrong flags");events.add("W "+pos(p)+" "+describe(s));changed.put(p,s);return true;}
                    case "getFluidState": {var p=(BlockPos)a[0];events.add("F "+pos(p));return at(p).getFluidState();}
                    case "getChunk": return chunk;
                    default:throw new AssertionError("world callback "+m);
                }
            });
            var beam=MineshaftPieces.MineShaftCorridor.class.getDeclaredMethod("placeSupport",WorldGenLevel.class,BoundingBox.class,int.class,int.class,int.class,int.class,int.class,RandomSource.class);beam.setAccessible(true);
            var web=MineshaftPieces.MineShaftCorridor.class.getDeclaredMethod("maybePlaceCobWeb",WorldGenLevel.class,BoundingBox.class,RandomSource.class,float.class,int.class,int.class,int.class);web.setAccessible(true);
            var dirs=new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
            for(int kind=0;kind<2;kind++)for(int direction=0;direction<4;direction++)for(int clipId=0;clipId<3;clipId++)
                for(scenario=0;scenario<4;scenario++)for(long seed:new long[]{-3,0,1,2,42,99,12345,123456789})for(var variant:MineshaftStructure.Type.values()){
                    var box=new BoundingBox(0,30,0,9,32,9);
                    var piece=new MineshaftPieces.MineShaftCorridor(1,RandomSource.create(5),box,dirs[direction],variant);
                    var clip=switch(clipId){case 0->new BoundingBox(-5,25,-5,15,40,15);case 1->new BoundingBox(0,30,0,7,32,7);default->new BoundingBox(0,30,0,4,34,9);};
                    var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
                    changed.clear();events.clear();
                    float chance=switch(scenario){case 0->0.1F;case 1->1.0F;case 2->0.0F;default->0.05F;};
                    if(kind==0)beam.invoke(piece,world,clip,0,0,2,2,2,random);
                    else web.invoke(piece,world,clip,random,chance,1,1,3);
                    out.printf("C %d %d %d %d %d %s %d %d%n",kind,direction,clipId,scenario,seed,variant.name(),events.size(),random.nextLong());
                    for(var event:events)out.println(event);
                }
        }catch(Throwable error){error.printStackTrace(err);System.exit(1);}
    }
}
