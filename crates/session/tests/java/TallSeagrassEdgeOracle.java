import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.material.*;

/** Run actual double-plant callbacks with independently selected live states and tags. */
public class TallSeagrassEdgeOracle {
    public static void main(String[] args) {
        var out=System.out;
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var pos=new BlockPos(8,8,8);
        var lower=Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF,DoubleBlockHalf.LOWER);
        var upper=lower.setValue(DoublePlantBlock.HALF,DoubleBlockHalf.UPPER);
        var supports=List.of(Blocks.STONE.defaultBlockState(),Blocks.MAGMA_BLOCK.defaultBlockState(),
            Blocks.AIR.defaultBlockState(),Blocks.WATER.defaultBlockState(),
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.BOTTOM),
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.TOP),
            lower,upper,Blocks.KELP_PLANT.defaultBlockState(),Blocks.SUSPICIOUS_GRAVEL.defaultBlockState());
        var live=List.of(lower,Blocks.WATER.defaultBlockState(),
            Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1),
            Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,8),
            Blocks.LAVA.defaultBlockState(),Blocks.AIR.defaultBlockState(),upper);
        var neighbors=List.of(Blocks.AIR.defaultBlockState(),Blocks.STONE.defaultBlockState(),lower,upper);
        out.println("BEGIN_RESULT");
        for(int tagMask=0;tagMask<4;tagMask++) {
            var supportTags=new ArrayList<Holder<Block>>();
            if((tagMask&1)!=0)supportTags.add(Blocks.STONE.builtInRegistryHolder());
            if((tagMask&2)!=0)supportTags.add(Blocks.MAGMA_BLOCK.builtInRegistryHolder());
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,
                Map.of(BlockTags.CANNOT_SUPPORT_SEAGRASS,supportTags))).apply();
            for(int fluidMask:new int[]{0,1,3,4,15}) {
                var water=new ArrayList<Holder<Fluid>>();
                if((fluidMask&1)!=0)water.add(Fluids.WATER.builtInRegistryHolder());
                if((fluidMask&2)!=0)water.add(Fluids.FLOWING_WATER.builtInRegistryHolder());
                if((fluidMask&4)!=0)water.add(Fluids.LAVA.builtInRegistryHolder());
                if((fluidMask&8)!=0)water.add(Fluids.FLOWING_LAVA.builtInRegistryHolder());
                BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,
                    Map.of(FluidTags.WATER,water))).apply();
                for(int kind=0;kind<2;kind++)for(Direction direction:Direction.values())
                    for(int support=0;support<supports.size();support++)for(int current=0;current<live.size();current++)
                    for(int n=0;n<neighbors.size();n++) {
                        if(kind==1 && (fluidMask!=3 || current!=0))continue;
                        if(direction!=(kind==0?Direction.UP:Direction.DOWN) && n!=0)continue;
                        var below=supports.get(support);var currentState=live.get(current);
                        var world=(ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                            new Class<?>[]{ServerLevelAccessor.class},(proxy,method,values)->switch(method.getName()) {
                                case "getBlockState" -> {
                                    if(!pos.below().equals(values[0]))throw new AssertionError("unexpected block read " + values[0]);
                                    yield below;
                                }
                                case "getFluidState" -> {
                                    if(!pos.equals(values[0]))throw new AssertionError("unexpected fluid read " + values[0]);
                                    yield currentState.getFluidState();
                                }
                                case "scheduleTick" -> throw new AssertionError("unexpected tick");
                                default -> throw new UnsupportedOperationException(method.toString());
                            });
                        var state=kind==0?lower:upper;var random=RandomSource.create(17);
                        var first=state.updateShape(world,world,pos,direction,pos.relative(direction),neighbors.get(n),random);
                        var second=state.updateShape(world,world,pos,direction,pos.relative(direction),neighbors.get(n),random);
                        out.printf("T %d %s %d %d %d %d %d %s %s %d%n",kind,direction.getSerializedName(),support,current,n,tagMask,fluidMask,
                            BuiltInRegistries.BLOCK.getKey(first.getBlock()),BuiltInRegistries.BLOCK.getKey(second.getBlock()),random.nextInt());
                    }
            }
        }
        out.println("END_RESULT");
    }
}
