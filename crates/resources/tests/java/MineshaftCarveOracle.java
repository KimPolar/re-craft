import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.*;

public class MineshaftCarveOracle {
    static final List<String> events = new ArrayList<>();
    static final Map<BlockPos,BlockState> changed = new HashMap<>();
    static int scenario;
    static String pos(BlockPos p) { return p.getX()+","+p.getY()+","+p.getZ(); }
    static BlockState at(BlockPos p) {
        var state = changed.get(p);
        if (state != null) return state;
        int h = p.getX()*31 + p.getY()*17 + p.getZ()*13;
        if (scenario == 2 && Math.floorMod(h, 23) == 0) return Blocks.WATER.defaultBlockState();
        if (Math.floorMod(h, 29) == 0) return Blocks.OAK_PLANKS.defaultBlockState();
        if (Math.floorMod(h, 31) == 0) return Blocks.DARK_OAK_LOG.defaultBlockState();
        if (Math.floorMod(h, 37) == 0) return Blocks.IRON_CHAIN.defaultBlockState();
        return Blocks.STONE.defaultBlockState();
    }
    static void set(Object obj,String name,Object value)throws Exception {
        for (var c=obj.getClass();c!=null;c=c.getSuperclass()) try {
            var f=c.getDeclaredField(name);f.setAccessible(true);f.set(obj,value);return;
        } catch (NoSuchFieldException ignored) {}
        throw new NoSuchFieldException(name);
    }
    @SuppressWarnings("unchecked")
    public static void main(String[] args)throws Exception {
        var out=System.out; var err=System.err;
        try {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var owner=new HolderOwner<net.minecraft.world.level.biome.Biome>() {};
        var clearBiome=Holder.Reference.createStandAlone(owner,net.minecraft.world.level.biome.Biomes.PLAINS);
        var blockingBiome=Holder.Reference.createStandAlone(owner,net.minecraft.world.level.biome.Biomes.PLAINS);
        var bindTags=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);
        bindTags.setAccessible(true);
        bindTags.invoke(clearBiome,List.of());
        bindTags.invoke(blockingBiome,List.of(net.minecraft.tags.BiomeTags.MINESHAFT_BLOCKING));
        var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
            switch(m.getName()) {
                case "getBiome": events.add("B "+pos((BlockPos)a[0])); return scenario==1?blockingBiome:clearBiome;
                case "getBlockState": { var p=(BlockPos)a[0]; events.add("R "+pos(p)); return at(p); }
                case "setBlock": { var p=((BlockPos)a[0]).immutable(); var state=(BlockState)a[1];
                    if(state.getBlock()!=Blocks.CAVE_AIR || !a[2].equals(2)) throw new AssertionError("unexpected write");
                    events.add("W "+pos(p)); changed.put(p,state); return true; }
                case "getFluidState": { var p=(BlockPos)a[0]; events.add("F "+pos(p)); return at(p).getFluidState(); }
                default: throw new AssertionError("world callback "+m);
            }
        });
        var dirs=new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
        for (int shift : new int[]{0,-20,16777216,-16777216}) for (int kind=0;kind<5;kind++)
            for (int c=0;c<3;c++) for (scenario=0;scenario<3;scenario++)
                for (var variant : MineshaftStructure.Type.values()) {
                    int x=shift, z=shift;
                    StructurePiece piece;
                    if(kind==0) {
                        var room=new MineshaftPieces.MineShaftRoom(0,RandomSource.create(2),x,z,variant);
                        set(room,"boundingBox",new BoundingBox(x,32,z,x+8,38,z+10));
                        var f=room.getClass().getDeclaredField("childEntranceBoxes");f.setAccessible(true);
                        var entrances=(List<BoundingBox>)f.get(room);
                        entrances.add(new BoundingBox(x+3,34,z,x+5,36,z+1));
                        entrances.add(new BoundingBox(x+7,33,z+4,x+8,35,z+6));
                        piece=room;
                    } else {
                        var d=dirs[kind-1]; boolean zAxis=d==Direction.NORTH||d==Direction.SOUTH;
                        var b=new BoundingBox(x+3,30,z+3,x+(zAxis?5:11),37,z+(zAxis?11:5));
                        piece=new MineshaftPieces.MineShaftStairs(3,b,d,variant);
                    }
                    var clip=switch(c) {
                        case 0 -> new BoundingBox(x-2,28,z-2,x+16,42,z+16);
                        case 1 -> new BoundingBox(x,30,z,x+7,35,z+7);
                        default -> new BoundingBox(x+8,31,z-2,x+16,42,z+16);
                    };
                    changed.clear(); events.clear();
                    var random=RandomSource.create(42);
                    piece.postProcess(world,null,null,random,clip,new ChunkPos(0,0),BlockPos.ZERO);
                    out.printf("C %d %d %d %d %s %d %d%n",shift,kind,c,scenario,variant.name(),events.size(),random.nextLong());
                    for(var event:events)out.println(event);
                }
        } catch (Throwable error) {
            error.printStackTrace(err);
            System.exit(1);
        }
    }
}
