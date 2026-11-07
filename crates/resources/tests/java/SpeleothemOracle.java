import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.LevelSimulatedReader;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.feature.SpeleothemUtils;
import net.minecraft.world.level.levelgen.Column;
import net.minecraft.world.level.levelgen.feature.SpeleothemClusterFeature;
import net.minecraft.world.level.levelgen.feature.SpeleothemFeature;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.util.valueproviders.ConstantFloat;
import net.minecraft.util.valueproviders.UniformInt;
import net.minecraft.util.valueproviders.UniformFloat;
import net.minecraft.util.valueproviders.TrapezoidFloat;
import net.minecraft.util.valueproviders.ClampedNormalFloat;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.level.material.Fluids;

public class SpeleothemOracle {
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var grow=SpeleothemUtils.class.getDeclaredMethod("growSpeleothem", LevelAccessor.class, BlockPos.class, Direction.class, int.class, boolean.class, Block.class, Block.class, HolderSet.class);
        grow.setAccessible(true);
        var base=SpeleothemUtils.class.getDeclaredMethod("placeBaseBlockIfPossible", LevelAccessor.class, BlockPos.class, Block.class, HolderSet.class);
        base.setAccessible(true);
        var replaceable=HolderSet.direct(Blocks.STONE.builtInRegistryHolder());
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.BASE_STONE_OVERWORLD));
        bind.invoke(Fluids.WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        for(var block:List.of(Blocks.SULFUR,Blocks.SULFUR_SPIKE,Blocks.WATER,Blocks.CINNABAR)) for(var state:block.getStateDefinition().getPossibleStates()) {
            out.printf("E %s %s %s %s %s %s%n",block,state.hasBlockEntity(),net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state),state.getPostProcessPos(null,BlockPos.ZERO),state.isSolid(),state.liquid());
        }
        for (int d=0;d<2;d++) for(int height=-1;height<=6;height++) for(int merge=0;merge<2;merge++) for(int support=0;support<4;support++) {
            var direction=d==0?Direction.UP:Direction.DOWN; int step=d==0?1:-1;
            var start=new BlockPos(4,10,7); var supportPos=start.relative(direction.getOpposite());
            BlockState supportState=switch(support){case 0->Blocks.SULFUR.defaultBlockState();case 1->Blocks.STONE.defaultBlockState();case 2->Blocks.DIRT.defaultBlockState();default->Blocks.AIR.defaultBlockState();};
            var events=new ArrayList<String>(); var map=new HashMap<BlockPos,BlockState>(); map.put(supportPos,supportState);
            var world=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(o,m,a)->{
                var p=(BlockPos)a[0]; var current=map.getOrDefault(p,Math.floorMod(p.getY(),2)==0?Blocks.WATER.defaultBlockState():Blocks.AIR.defaultBlockState());
                return switch(m.getName()) {
                    case "getBlockState" -> current;
                    case "isWaterAt" -> current.is(Blocks.WATER);
                    case "setBlock" -> { var state=(BlockState)a[1]; map.put(p.immutable(),state);
                        var thickness=state.getValue(PointedDripstoneBlock.THICKNESS).getSerializedName();
                        var facing=state.getValue(PointedDripstoneBlock.TIP_DIRECTION).getSerializedName();
                        var wet=state.getValue(BlockStateProperties.WATERLOGGED);
                        events.add(p.getY()+":"+facing+":"+thickness+":"+wet+":"+a[2]); yield true; }
                    default -> throw new UnsupportedOperationException(m.toString());
                };
            });
            grow.invoke(null,world,start,direction,height,merge!=0,Blocks.SULFUR,Blocks.SULFUR_SPIKE,replaceable);
            out.printf("C %d %d %d %d %s%n",d,height,merge,support,events.isEmpty()?"-":String.join(",",events));
        }
        for(int supported=0;supported<2;supported++) {
            int choice=supported;
            var events=new ArrayList<String>();
            var world=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(o,m,a)-> switch(m.getName()) {
                case "getBlockState" -> choice==1?Blocks.STONE.defaultBlockState():Blocks.DIRT.defaultBlockState();
                case "setBlock" -> { events.add(a[2].toString()); yield false; }
                default -> throw new UnsupportedOperationException(m.toString());
            });
            out.printf("B %d %s %s%n",supported,base.invoke(null,world,new BlockPos(4,10,7),Blocks.SULFUR,replaceable),events.isEmpty()?"-":String.join(",",events));
        }
        for (int profile=0;profile<5;profile++) for (int range=0;range<=6;range++) {
            int scenario=profile; var visited=new ArrayList<Integer>();
            var world=(LevelSimulatedReader)Proxy.newProxyInstance(LevelSimulatedReader.class.getClassLoader(),new Class[]{LevelSimulatedReader.class},(o,m,a)->{
                if(!m.getName().equals("isStateAtPosition")) throw new UnsupportedOperationException(m.toString());
                int y=((BlockPos)a[0]).getY(); visited.add(y);
                BlockState state=switch(scenario) {
                    case 0 -> y<=7 || y>=14 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
                    case 1 -> Blocks.WATER.defaultBlockState();
                    case 2 -> y==10 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
                    case 3 -> y<=8 ? Blocks.DIRT.defaultBlockState() : Blocks.AIR.defaultBlockState();
                    default -> y>=12 ? Blocks.LAVA.defaultBlockState() : Blocks.WATER.defaultBlockState();
                };
                return ((java.util.function.Predicate<BlockState>)a[1]).test(state);
            });
            var column=Column.scan(world,new BlockPos(4,10,7),range,s->s.isAir()||s.is(Blocks.WATER),s->!s.isAir()&&!s.is(Blocks.WATER));
            String floor=column.isEmpty()?"absent":column.get().getFloor().isPresent()?Integer.toString(column.get().getFloor().getAsInt()):"_";
            String ceiling=column.isEmpty()?"absent":column.get().getCeiling().isPresent()?Integer.toString(column.get().getCeiling().getAsInt()):"_";
            out.printf("S %d %d %s %s %s%n",profile,range,floor,ceiling,visited.toString().replace(" ",""));
        }
        var cluster=new SpeleothemClusterFeature(Blocks.SULFUR.defaultBlockState(),Blocks.SULFUR_SPIKE.defaultBlockState(),replaceable,12,ConstantInt.of(4),ConstantInt.of(3),1,3,ConstantInt.of(2),ConstantFloat.of(.5f),ConstantFloat.of(0f),.1f,3,8);
        var chance=SpeleothemClusterFeature.class.getDeclaredMethod("getChanceOfStalagmiteOrStalactite",int.class,int.class,int.class,int.class); chance.setAccessible(true);
        var height=SpeleothemClusterFeature.class.getDeclaredMethod("getSpeleothemHeight",RandomSource.class,int.class,int.class,float.class,int.class); height.setAccessible(true);
        for(int rx:new int[]{0,2,8}) for(int rz:new int[]{0,2,8}) for(int x:new int[]{-9,-4,0,3,8}) for(int z:new int[]{-9,-4,0,3,8}) {
            double value=(double)chance.invoke(cluster,rx,rz,x,z);
            out.printf("P %d %d %d %d %d%n",rx,rz,x,z,Double.doubleToRawLongBits(value));
        }
        for(int x:new int[]{-9,-4,0,3,8}) for(int z:new int[]{-9,-4,0,3,8}) for(int h:new int[]{0,3,8}) for(int density=0;density<3;density++) for(long seed=0;seed<16;seed++) {
            float value=density==0?0f:density==1?.3f:1f;
            var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
            int result=(int)height.invoke(cluster,random,x,z,value,h);
            out.printf("H %d %d %d %d %d %d %d%n",x,z,h,density,seed,result,random.nextLong());
        }
        for(int mode=0;mode<4;mode++) for(int profile=0;profile<4;profile++) for(long seed=0;seed<32;seed++) {
            float[] chances=switch(mode){case 0->new float[]{.2f,.7f,.5f,.5f};case 1->new float[]{0f,0f,0f,0f};case 2->new float[]{1f,1f,1f,1f};default->new float[]{.8f,.3f,.9f,.2f};};
            var feature=new SpeleothemFeature(Blocks.SULFUR.defaultBlockState(),Blocks.SULFUR_SPIKE.defaultBlockState(),replaceable,chances[0],chances[1],chances[2],chances[3]);
            int scenario=profile; var states=new HashMap<BlockPos,BlockState>(); var events=new ArrayList<String>();
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
                var pos=(BlockPos)a[0]; int y=pos.getY();
                var fallback=(y>10 && (scenario&1)!=0)||(y<10 && (scenario&2)!=0)?Blocks.STONE.defaultBlockState()
                    : y==10 && Math.floorMod(pos.getX()+pos.getZ(),3)==0?Blocks.WATER.defaultBlockState():Blocks.AIR.defaultBlockState();
                var state=states.getOrDefault(pos,fallback);
                return switch(m.getName()) {
                    case "getBlockState" -> state;
                    case "isWaterAt" -> state.getFluidState().is(FluidTags.WATER);
                    case "setBlock" -> {var next=(BlockState)a[1];states.put(pos.immutable(),next);
                        String detail=next.is(Blocks.SULFUR)?"base":next.getValue(PointedDripstoneBlock.TIP_DIRECTION).getSerializedName()+":"+next.getValue(PointedDripstoneBlock.THICKNESS).getSerializedName()+":"+next.getValue(BlockStateProperties.WATERLOGGED);
                        events.add(pos.getX()+":"+y+":"+pos.getZ()+":"+detail+":"+a[2]);yield true;}
                    default -> throw new UnsupportedOperationException(m.toString());
                };
            });
            var random=new WorldgenRandom(new XoroshiroRandomSource(seed)); boolean result=feature.place(world,null,random,new BlockPos(0,10,0));
            out.printf("I %d %d %d %s %d %s%n",mode,profile,seed,result,random.nextLong(),events.isEmpty()?"-":String.join(",",events));
        }
        for(int variable=0;variable<5;variable++) for(int wet=0;wet<2;wet++) for(int profile=0;profile<4;profile++) for(long seed=0;seed<16;seed++) {
            var densityProvider=switch(variable){case 0->ConstantFloat.of(.7f);case 1->UniformFloat.of(.3f,.7f);case 2->TrapezoidFloat.of(.3f,.7f,.1f);case 3->TrapezoidFloat.of(.3f,.7f,-.1f);default->ClampedNormalFloat.of(.5f,-.2f,.3f,.7f);};
            var cluster2=new SpeleothemClusterFeature(Blocks.SULFUR.defaultBlockState(),Blocks.SULFUR_SPIKE.defaultBlockState(),replaceable,12,variable==0?ConstantInt.of(4):UniformInt.of(1,4),variable==0?ConstantInt.of(2):UniformInt.of(1,3),1,3,variable==0?ConstantInt.of(2):UniformInt.of(1,3),densityProvider,ConstantFloat.of((float)wet),.1f,3,8);
            int scenario=profile; var states=new HashMap<BlockPos,BlockState>(); var events=new ArrayList<String>();
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
                var pos=(BlockPos)a[0]; int y=pos.getY();
                var fallback=scenario==1 && y==10 ? Blocks.STONE.defaultBlockState()
                    : y<=5 ? Blocks.STONE.defaultBlockState()
                    : scenario!=2 && y>=14 ? (scenario==3?Blocks.LAVA.defaultBlockState():Blocks.STONE.defaultBlockState())
                    : Blocks.AIR.defaultBlockState();
                var state=states.getOrDefault(pos,fallback);
                return switch(m.getName()) {
                    case "getBlockState" -> state;
                    case "isStateAtPosition" -> ((java.util.function.Predicate<BlockState>)a[1]).test(state);
                    case "isWaterAt" -> state.getFluidState().is(FluidTags.WATER);
                    case "setBlock" -> { var next=(BlockState)a[1]; states.put(pos.immutable(),next);
                        String detail=next.is(Blocks.SULFUR)?"base":next.is(Blocks.WATER)?"water":next.getValue(PointedDripstoneBlock.TIP_DIRECTION).getSerializedName()+":"+next.getValue(PointedDripstoneBlock.THICKNESS).getSerializedName()+":"+next.getValue(BlockStateProperties.WATERLOGGED);
                        events.add(pos.getX()+":"+y+":"+pos.getZ()+":"+detail+":"+a[2]); yield true; }
                    default -> throw new UnsupportedOperationException(m.toString());
                };
            });
            var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
            boolean result=cluster2.place(world,null,random,new BlockPos(0,10,0));
            out.printf("G %d %d %d %d %s %d %s%n",variable,wet,profile,seed,result,random.nextLong(),events.isEmpty()?"-":String.join(",",events));
        }
    }
}
