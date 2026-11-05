import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.SwampHutPiece;

public class SwampHutPieceOracle {
    static final List<String> events=new ArrayList<>();
    static final Map<BlockPos,BlockState> blocks=new HashMap<>();
    static int surfaceHeight=64;
    static void set(Object obj,String name,Object value)throws Exception{
        for(var type=obj.getClass();type!=null;type=type.getSuperclass())try{
            var field=type.getDeclaredField(name);field.setAccessible(true);field.set(obj,value);return;
        }catch(NoSuchFieldException ignored){}
        throw new NoSuchFieldException(name);
    }
    static Object get(Object obj,String name)throws Exception{
        for(var type=obj.getClass();type!=null;type=type.getSuperclass())try{
            var field=type.getDeclaredField(name);field.setAccessible(true);return field.get(obj);
        }catch(NoSuchFieldException ignored){}
        throw new NoSuchFieldException(name);
    }
    static String position(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
    static String block(BlockState state){
        var id=BuiltInRegistries.BLOCK.getKey(state.getBlock()).getPath();
        if(state.getBlock()==Blocks.SPRUCE_STAIRS)
            return id+":"+state.getValue(StairBlock.FACING).name()+":"+state.getValue(StairBlock.SHAPE).getSerializedName();
        return id;
    }
    static BlockState blockAt(BlockPos p){
        return blocks.getOrDefault(p,p.getY()<=59?Blocks.STONE.defaultBlockState():
            p.getY()<=62?Blocks.WATER.defaultBlockState():Blocks.AIR.defaultBlockState());
    }
    static class TracedChunk extends ProtoChunk {
        TracedChunk(){super(new ChunkPos(0,0),null,(LevelHeightAccessor)null,null,null);}
        @Override public void markPosForPostProcessing(BlockPos p){events.add("M "+position(p));}
    }
    public static void main(String[] args)throws Exception{
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        if(args.length>0 && args[0].equals("natural-height"))surfaceHeight=63;
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var chunk=(TracedChunk)unsafe.allocateInstance(TracedChunk.class);
        var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
            switch(m.getName()){
                case "getHeightmapPos": {var p=(BlockPos)a[1];return new BlockPos(p.getX(),surfaceHeight,p.getZ());}
                case "getMinY": return -64;
                case "setBlock": {var p=((BlockPos)a[0]).immutable();var state=(BlockState)a[1];
                    if(!a[2].equals(2))throw new AssertionError("incorrect update flags");
                    events.add("W "+position(p)+" "+block(state));blocks.put(p,state);return true;}
                case "getBlockState": return blockAt((BlockPos)a[0]);
                case "getFluidState": return blockAt((BlockPos)a[0]).getFluidState();
                case "getChunk": return chunk;
                default: throw new AssertionError("unexpected world callback "+m);
            }
        });
        var orientations=new Direction[]{Direction.NORTH,Direction.EAST,Direction.SOUTH,Direction.WEST};
        var clips=new BoundingBox[]{new BoundingBox(0,-64,0,15,319,15),new BoundingBox(0,-64,0,7,319,15),
            new BoundingBox(8,-64,0,15,319,15),new BoundingBox(0,-64,0,15,63,15)};
        out.println("BEGIN_RESULT");
        for(int direction=0;direction<4;direction++)for(int c=0;c<clips.length;c++){
            var piece=new SwampHutPiece(RandomSource.create(0),3,3);piece.setOrientation(orientations[direction]);
            var max=direction%2==0?new int[]{9,11}:new int[]{11,9};
            set(piece,"boundingBox",new BoundingBox(3,64,3,max[0],70,max[1]));
            set(piece,"spawnedWitch",true);set(piece,"spawnedCat",true);blocks.clear();
            for(int repeat=0;repeat<2;repeat++){
                events.clear();var random=RandomSource.create(42);
                piece.postProcess(world,null,null,random,clips[c],new ChunkPos(0,0),BlockPos.ZERO);
                var box=piece.getBoundingBox();
                out.printf("C %d %d %d %d %d %d %d %d %d%n",direction,c,repeat,(int)get(piece,"heightPosition"),
                    box.minY(),box.maxY(),events.size(),random.nextInt(),blocks.size());
                for(var event:events)out.println(event);
            }
        }
        out.println("END_RESULT");
    }
}
