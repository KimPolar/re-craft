import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.properties.StairsShape;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.structure.*;

public class StructureFrameOracle {
    static class Piece extends StructurePiece {
        Piece(Direction direction){super(null,0,new BoundingBox(-8,-64,10,12,70,20));setOrientation(direction);}
        BlockPos world(int x,int y,int z){return getWorldPos(x,y,z).immutable();}
        Direction facing(Direction input){
            var state=Blocks.SPRUCE_STAIRS.defaultBlockState().setValue(StairBlock.FACING,input);
            return state.mirror(getMirror()).rotate(getRotation()).getValue(StairBlock.FACING);
        }
        @Override protected void addAdditionalSaveData(net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext c,net.minecraft.nbt.CompoundTag tag){}
        @Override public void postProcess(WorldGenLevel level,StructureManager manager,ChunkGenerator generator,RandomSource random,BoundingBox clip,ChunkPos pos,BlockPos origin){}
    }
    public static void main(String[] args){
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var directions=new Direction[]{null,Direction.NORTH,Direction.EAST,Direction.SOUTH,Direction.WEST};
        var faces=new Direction[]{Direction.NORTH,Direction.EAST,Direction.SOUTH,Direction.WEST};
        var locals=new int[][]{{0,0,0},{1,2,3},{-4,-5,-6},{15,319,16},
            {Integer.MAX_VALUE,Integer.MIN_VALUE,Integer.MAX_VALUE},{Integer.MIN_VALUE,Integer.MAX_VALUE,Integer.MIN_VALUE}};
        out.println("BEGIN_RESULT");
        for(int orientation=0;orientation<directions.length;orientation++){
            var piece=new Piece(directions[orientation]);
            for(int local=0;local<locals.length;local++){
                var p=locals[local];var world=piece.world(p[0],p[1],p[2]);
                for(int face=0;face<faces.length;face++)
                    out.printf("T %d %d %d %d %d %d %s%n",orientation,local,face,world.getX(),world.getY(),world.getZ(),piece.facing(faces[face]).name());
            }
            for(int face=0;face<faces.length;face++)for(var shape:StairsShape.values()){
                var state=Blocks.SPRUCE_STAIRS.defaultBlockState().setValue(StairBlock.FACING,faces[face]).setValue(StairBlock.SHAPE,shape);
                var transformed=state.mirror(piece.getMirror()).rotate(piece.getRotation());
                out.printf("S %d %d %s %s %s%n",orientation,face,shape.getSerializedName(),
                    transformed.getValue(StairBlock.FACING).name(),transformed.getValue(StairBlock.SHAPE).getSerializedName());
            }
        }
        out.println("END_RESULT");
    }
}
