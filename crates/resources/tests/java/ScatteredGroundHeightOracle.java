import java.lang.reflect.Proxy;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.structure.*;

public class ScatteredGroundHeightOracle {
    static int mode,reads,hash;
    static class Piece extends ScatteredFeaturePiece {
        Piece(){super(null,-4,40,9,7,7,9,Direction.SOUTH);boundingBox=new BoundingBox(-4,40,9,2,46,17);}
        boolean average(LevelAccessor level,BoundingBox clip,int offset){return updateAverageGroundHeight(level,clip,offset);}
        boolean lowest(LevelAccessor level,int offset){return updateHeightPositionToLowestGroundHeight(level,offset);}
        int hpos(){return heightPosition;}
        @Override public void postProcess(WorldGenLevel level,StructureManager manager,ChunkGenerator generator,RandomSource random,BoundingBox clip,ChunkPos pos,BlockPos origin){}
    }
    static int height(int x,int z){return new int[]{-45,0,75,319}[mode]+Math.floorMod(x*13+z*7,11)-5;}
    public static void main(String[] args){
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var level=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(obj,method,a)->{
            if(method.getName().equals("getHeightmapPos")){
                var p=(BlockPos)a[1];reads++;hash=31*hash+p.getX();hash=31*hash+p.getZ();
                return new BlockPos(p.getX(),height(p.getX(),p.getZ()),p.getZ());
            }
            if(method.getName().equals("getMaxY"))return 319;
            throw new AssertionError(method.toString());
        });
        var clips=new BoundingBox[]{new BoundingBox(-10,0,0,10,100,30),new BoundingBox(-3,64,10,-2,64,11),
            new BoundingBox(5,0,0,7,100,30),new BoundingBox(-10,0,0,10,63,30)};
        out.println("BEGIN_RESULT");
        for(mode=0;mode<4;mode++)for(int c=0;c<clips.length;c++)for(int offset:new int[]{-2,0,3})for(int method=0;method<2;method++){
            var piece=new Piece();
            for(int repeat=0;repeat<2;repeat++){
                reads=0;hash=0;
                boolean result=method==0?piece.average(level,clips[c],offset):piece.lowest(level,offset);
                var box=piece.getBoundingBox();
                out.printf("S %d %d %d %d %d %s %d %d %d %d %d%n",mode,c,offset,method,repeat,result,piece.hpos(),box.minY(),box.maxY(),reads,hash);
            }
        }
        out.println("END_RESULT");
    }
}
