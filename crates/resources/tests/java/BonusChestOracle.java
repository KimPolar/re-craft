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
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.BonusChestFeature;

public class BonusChestOracle {
    static String pos(BlockPos p) { return p.getX()+","+p.getY()+","+p.getZ(); }
    static BlockState initial(BlockPos p, int mode) {
        int x=p.getX(), z=p.getZ();
        if (p.getY()==7) return mode!=4 && ((x*31+z*17)&3)==0 ? Blocks.AIR.defaultBlockState() : Blocks.STONE.defaultBlockState();
        if (p.getY()!=8) return Blocks.AIR.defaultBlockState();
        if (mode==1 && (x&15)==7 && (z&15)==3) return Blocks.AIR.defaultBlockState();
        if (mode==2 && ((x^z)&3)==0) return Blocks.WATER.defaultBlockState();
        if (mode==3 || mode==4) return Blocks.AIR.defaultBlockState();
        return Blocks.STONE.defaultBlockState();
    }
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        boolean flat=args.length>0 && args[0].equals("flat");
        long[] seeds=flat ? new long[]{42} : new long[]{0,1,-1,987654321,Long.MIN_VALUE,Long.MAX_VALUE};
        BlockPos[] origins={new BlockPos(0,20,0),new BlockPos(-17,-50,-33),new BlockPos(Integer.MAX_VALUE,8,Integer.MIN_VALUE)};
        for (int type=0;type<(flat?1:3);type++) for (long seed:seeds) for (int mode=flat?4:0;mode<(flat?5:4);mode++) for (boolean hasEntity:flat?new boolean[]{true}:new boolean[]{false,true}) {
            var origin=flat?origins[0]:origins[(mode+type)%origins.length];
            final int scenario=mode;
            var blocks=new HashMap<BlockPos,BlockState>(); var entities=new HashMap<BlockPos,ChestBlockEntity>();
            var probes=new ArrayList<String>(); var writes=new ArrayList<String>();
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,method,values)->{
                switch(method.getName()) {
                    case "getHeightmapPos": {var p=(BlockPos)values[1]; probes.add(p.getX()+","+p.getZ()); return new BlockPos(p.getX(),8,p.getZ());}
                    case "isEmptyBlock": {var p=(BlockPos)values[0]; return blocks.getOrDefault(p,initial(p,scenario)).isAir();}
                    case "getBlockState": {var p=(BlockPos)values[0]; return blocks.getOrDefault(p,initial(p,scenario));}
                    case "setBlock": {var p=((BlockPos)values[0]).immutable();var s=(BlockState)values[1];
                        if ((int)values[2]!=2) throw new AssertionError("flags"); blocks.put(p,s);
                        writes.add(pos(p)+","+(s.is(Blocks.CHEST)?"chest":"torch"));
                        if (s.is(Blocks.CHEST) && hasEntity) entities.put(p,new ChestBlockEntity(p,s)); return true;}
                    case "getBlockEntity": return entities.get((BlockPos)values[0]);
                    default: throw new UnsupportedOperationException(method.toString());
                }
            });
            RandomSource random=switch(type) {case 0 -> RandomSource.create(seed);case 1 -> new XoroshiroRandomSource(seed);default -> new WorldgenRandom(new XoroshiroRandomSource(seed));};
            boolean placed=new BonusChestFeature().place(world,null,random,origin);
            String loot="-"; for(var entry:entities.entrySet()) if(entry.getValue().getLootTable()!=null) loot=pos(entry.getKey())+","+entry.getValue().getLootTableSeed();
            out.printf("C %d %d %d %s %s P %s W %s L %s R %d%n",type,seed,mode,hasEntity,placed,String.join(";",probes),String.join(";",writes),loot,random.nextLong());
        }
    }
}
