import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.vehicle.minecart.MinecartChest;
import net.minecraft.world.flag.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.*;
import net.minecraft.world.level.storage.loot.BuiltInLootTables;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.storage.loot.LootTable;

public class MineshaftMinecartOracle {
    static final List<String> events=new ArrayList<>();
    static int mode; static BlockPos target;
    static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
    static class FakeLevel extends ServerLevel {
        FakeLevel(){super(null,null,null,null,null,null,false,0,List.of(),false);}
        @Override public FeatureFlagSet enabledFeatures(){return mode==3?FeatureFlagSet.of():FeatureFlags.VANILLA_SET;}
        @Override public int getNextEntityId(){return 100;}
    }
    public static void main(String[]args)throws Exception{
        var out=System.out;var err=System.err;
        try{
            SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
            var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");unsafeField.setAccessible(true);
            var fake=(FakeLevel)((sun.misc.Unsafe)unsafeField.get(null)).allocateInstance(FakeLevel.class);
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
                switch(m.getName()){
                    case "getBlockState":{
                        var p=(BlockPos)a[0];events.add("R "+pos(p));
                        if(p.equals(target))return mode==1?Blocks.STONE.defaultBlockState():Blocks.AIR.defaultBlockState();
                        if(p.equals(target.below()))return mode==2?Blocks.AIR.defaultBlockState():Blocks.STONE.defaultBlockState();
                        throw new AssertionError("unexpected read "+p);
                    }
                    case "setBlock":{var p=(BlockPos)a[0];var s=(BlockState)a[1];
                        if(s.getBlock()!=Blocks.RAIL || !a[2].equals(2))throw new AssertionError("unexpected write");
                        events.add("W "+pos(p)+" "+s.getValue(RailBlock.SHAPE).getSerializedName());return true;}
                    case "getFluidState":{var p=(BlockPos)a[0];events.add("F "+pos(p));return net.minecraft.world.level.material.Fluids.EMPTY.defaultFluidState();}
                    case "getLevel":events.add("L");return fake;
                    case "addFreshEntity":{
                        var e=(MinecartChest)a[0];
                        if(!BuiltInLootTables.ABANDONED_MINESHAFT.equals(e.getContainerLootTable()))throw new AssertionError("wrong loot table");
                        events.add("A "+e.getX()+","+e.getY()+","+e.getZ()+" "+e.getContainerLootTableSeed());return true;
                    }
                    default:throw new AssertionError("world callback "+m);
                }
            });
            var method=MineshaftPieces.MineShaftCorridor.class.getDeclaredMethod("createChest",WorldGenLevel.class,BoundingBox.class,RandomSource.class,int.class,int.class,int.class,ResourceKey.class);method.setAccessible(true);
            var coords=StructurePiece.class.getDeclaredMethod("getWorldPos",int.class,int.class,int.class);coords.setAccessible(true);
            var dirs=new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
            for(int d=0;d<4;d++)for(mode=0;mode<5;mode++)for(long seed:new long[]{-3,0,1,42,59,12345})for(var variant:MineshaftStructure.Type.values()){
                var box=new BoundingBox(0,30,0,9,32,9);var piece=new MineshaftPieces.MineShaftCorridor(1,RandomSource.create(5),box,dirs[d],variant);
                target=((BlockPos)coords.invoke(piece,1,0,3)).immutable();
                var clip=mode==4?new BoundingBox(20,25,20,30,40,30):new BoundingBox(-5,25,-5,15,40,15);
                var random=new WorldgenRandom(new XoroshiroRandomSource(seed));events.clear();
                boolean result=(boolean)method.invoke(piece,world,clip,random,1,0,3,BuiltInLootTables.ABANDONED_MINESHAFT);
                out.printf("C %d %d %d %s %b %d %d%n",d,mode,seed,variant.name(),result,events.size(),random.nextLong());
                for(var e:events)out.println(e);
            }
        }catch(Throwable error){error.printStackTrace(err);System.exit(1);}
    }
}
