import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.BonusChestFeature;
import net.minecraft.world.level.redstone.NeighborUpdater;

public class BonusChestOracle {
    static String pos(BlockPos p) { return p.getX()+","+p.getY()+","+p.getZ(); }
    static BlockState initial(BlockPos p, int mode) {
        int x=p.getX(), z=p.getZ();
        if (p.getY()==7) {
            if(mode==5) return Blocks.GRASS_BLOCK.defaultBlockState();
            if(mode==6) return Blocks.PODZOL.defaultBlockState();
            if(mode==7) return Blocks.MYCELIUM.defaultBlockState();
            if(mode==8) return Blocks.GRASS_BLOCK.defaultBlockState().setValue(BlockStateProperties.SNOWY,true);
            if(mode==9) return Blocks.PODZOL.defaultBlockState().setValue(BlockStateProperties.SNOWY,true);
            if(mode==10) return Blocks.MYCELIUM.defaultBlockState().setValue(BlockStateProperties.SNOWY,true);
            return mode!=4 && ((x*31+z*17)&3)==0 ? Blocks.AIR.defaultBlockState() : Blocks.STONE.defaultBlockState();
        }
        if (p.getY()!=8) return Blocks.AIR.defaultBlockState();
        if (mode==1 && (x&15)==7 && (z&15)==3) return Blocks.AIR.defaultBlockState();
        if (mode==2 && ((x^z)&3)==0) return Blocks.WATER.defaultBlockState();
        if (mode>=3) return Blocks.AIR.defaultBlockState();
        return Blocks.STONE.defaultBlockState();
    }
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        boolean flat=args.length>0 && args[0].startsWith("flat");
        int flatMode=flat ? switch(args[0]) {case "flat_grass" -> 5;case "flat_podzol" -> 6;case "flat_mycelium" -> 7;case "flat_snowy_grass" -> 8;case "flat_snowy_podzol" -> 9;case "flat_snowy_mycelium" -> 10;default -> 4;} : 0;
        long[] seeds=flat ? new long[]{42} : new long[]{0,1,-1,987654321,Long.MIN_VALUE,Long.MAX_VALUE};
        BlockPos[] origins={new BlockPos(0,20,0),new BlockPos(-17,-50,-33),new BlockPos(Integer.MAX_VALUE,8,Integer.MIN_VALUE)};
        for (int type=0;type<(flat?1:3);type++) for (long seed:seeds) for (int mode=flat?flatMode:0;mode<(flat?flatMode+1:4);mode++) for (boolean hasEntity:flat?new boolean[]{true}:new boolean[]{false,true}) {
            var origin=flat?origins[0]:origins[(mode+type)%origins.length];
            final int scenario=mode;
            var blocks=new HashMap<BlockPos,BlockState>(); var entities=new HashMap<BlockPos,ChestBlockEntity>();
            var probes=new ArrayList<String>(); var writes=new ArrayList<String>();
            var shapeRandom=RandomSource.create(13);
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,method,values)->{
                switch(method.getName()) {
                    case "getHeightmapPos": {var p=(BlockPos)values[1]; probes.add(p.getX()+","+p.getZ()); return new BlockPos(p.getX(),8,p.getZ());}
                    case "isEmptyBlock": {var p=(BlockPos)values[0]; return blocks.getOrDefault(p,initial(p,scenario)).isAir();}
                    case "getBlockState": {var p=(BlockPos)values[0]; return blocks.getOrDefault(p,initial(p,scenario));}
                    case "setBlock": {var p=((BlockPos)values[0]).immutable();var s=(BlockState)values[1];
                        if ((int)values[2]!=2) throw new AssertionError("flags"); blocks.put(p,s);
                        writes.add(pos(p)+","+net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey(s.getBlock()).getPath());
                        if (s.is(Blocks.CHEST) && hasEntity) entities.put(p,new ChestBlockEntity(p,s));
                        if(scenario>=8) s.updateNeighbourShapes((WorldGenLevel)proxy,p,2,511); return true;}
                    case "getBlockEntity": return entities.get((BlockPos)values[0]);
                    case "getRandom": return shapeRandom;
                    case "neighborShapeChanged": {NeighborUpdater.executeShapeUpdate((WorldGenLevel)proxy,(net.minecraft.core.Direction)values[0],(BlockPos)values[1],(BlockPos)values[2],(BlockState)values[3],(int)values[4],(int)values[5]);return null;}
                    default: throw new UnsupportedOperationException(method.toString());
                }
            });
            RandomSource random=switch(type) {case 0 -> RandomSource.create(seed);case 1 -> new XoroshiroRandomSource(seed);default -> new WorldgenRandom(new XoroshiroRandomSource(seed));};
            boolean placed=new BonusChestFeature().place(world,null,random,origin);
            String loot="-"; for(var entry:entities.entrySet()) if(entry.getValue().getLootTable()!=null) loot=pos(entry.getKey())+","+entry.getValue().getLootTableSeed();
            out.printf("C %d %d %d %s %s P %s W %s L %s R %d%n",type,seed,mode,hasEntity,placed,String.join(";",probes),String.join(";",writes),loot,random.nextLong());
            if(scenario>=8) {
                var center=entities.keySet().iterator().next();
                out.print("G");
                for(int[] d:new int[][]{{0,0},{0,-1},{1,0},{0,1},{-1,0}}) {
                    var p=center.offset(d[0],-1,d[1]);
                    out.print(" "+blocks.getOrDefault(p,initial(p,scenario)).getValue(BlockStateProperties.SNOWY));
                }
                out.println(); out.println("T "+shapeRandom.nextLong());
            }
        }
    }
}
