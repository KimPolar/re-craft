import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.*;

public class MineshaftSupportOracle {
    static int scenario, baseY;
    static final List<String> events = new ArrayList<>();
    static String pos(BlockPos p) { return p.getX()+","+p.getY()+","+p.getZ(); }
    static BlockState at(BlockPos p) {
        int d=p.getY()-baseY;
        return switch(scenario) {
            case 0 -> d==-1 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
            case 1 -> d==-4 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
            case 2 -> d==3 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
            case 3 -> d==-1 ? Blocks.LAVA.defaultBlockState() : d==5 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
            case 4 -> d==-4 ? Blocks.STONE.defaultBlockState() : d==2 ? Blocks.SAND.defaultBlockState() : Blocks.AIR.defaultBlockState();
            case 6 -> Math.abs(d)==3 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
            case 7 -> d<0 ? Blocks.WATER.defaultBlockState() : d==51 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
            case 8 -> d==-4 ? Blocks.STONE.defaultBlockState() : d==1 ? Blocks.IRON_CHAIN.defaultBlockState() : Blocks.AIR.defaultBlockState();
            case 9 -> d==-4 || d==1 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
            default -> Blocks.AIR.defaultBlockState();
        };
    }
    public static void main(String[] args)throws Exception {
        var out=System.out;var err=System.err;
        try {
            SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
            var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
            var method=MineshaftPieces.MineShaftCorridor.class.getDeclaredMethod("fillPillarDownOrChainUp",WorldGenLevel.class,BlockState.class,int.class,int.class,int.class,BoundingBox.class);
            method.setAccessible(true);
            var positionMethod=StructurePiece.class.getDeclaredMethod("getWorldPos",int.class,int.class,int.class);positionMethod.setAccessible(true);
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
                return switch(m.getName()) {
                    case "getMinY" -> scenario==10?43:-64;
                    case "getMaxY" -> scenario==10?47:100;
                    case "getBlockState" -> { var p=(BlockPos)a[0];events.add("R "+pos(p));yield at(p); }
                    case "setBlock" -> { var p=(BlockPos)a[0]; var s=(BlockState)a[1];
                        if(!a[2].equals(2))throw new AssertionError("wrong flags");
                        String block=s.getBlock()==Blocks.OAK_LOG?"oak_log":s.getBlock()==Blocks.DARK_OAK_LOG?"dark_log":
                            s.getBlock()==Blocks.OAK_FENCE?"oak_fence":s.getBlock()==Blocks.DARK_OAK_FENCE?"dark_fence":
                            s.getBlock()==Blocks.IRON_CHAIN?"chain":"unexpected";
                        if(block.equals("unexpected"))throw new AssertionError("wrong block "+s);
                        events.add("W "+pos(p)+" "+block); yield true; }
                    default -> throw new AssertionError("world callback "+m);
                };
            });
            var dirs=new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
            for(int direction=0;direction<4;direction++)for(scenario=0;scenario<11;scenario++)for(int clipped=0;clipped<2;clipped++)
                for(var variant:MineshaftStructure.Type.values()) {
                    bind.invoke(Blocks.STONE.builtInRegistryHolder(),scenario==9?List.of(BlockTags.UNSTABLE_BOTTOM_CENTER):List.of());
                    var b=new BoundingBox(0,46,0,9,48,9);
                    var piece=new MineshaftPieces.MineShaftCorridor(1,RandomSource.create(2),b,dirs[direction],variant);
                    var p=(BlockPos)positionMethod.invoke(piece,0,-1,2);baseY=p.getY();
                    var clip=clipped==0?new BoundingBox(-20,-64,-20,20,100,20):new BoundingBox(30,-64,30,40,100,40);
                    events.clear();
                    method.invoke(piece,world,variant.getWoodState(),0,-1,2,clip);
                    out.printf("C %d %d %d %s %d %d,%d,%d%n",direction,scenario,clipped,variant.name(),events.size(),p.getX(),p.getY(),p.getZ());
                    for(var event:events)out.println(event);
                }
        } catch(Throwable error) { error.printStackTrace(err);System.exit(1); }
    }
}
