import java.lang.reflect.*;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.LakeFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.material.Fluids;

public class WaterLakeFreezeOracle {
    static long key(int x,int z){return ((long)x<<32)^(z&0xffffffffL);}
    static BlockState initial(BlockPos p,Set<BlockPos> water){
        if(water.contains(p))return Blocks.WATER.defaultBlockState();
        return (p.getY()<12?Blocks.STONE:Blocks.AIR).defaultBlockState();
    }
    static class Run {
        Map<BlockPos,BlockState> changes=new HashMap<>();
        Map<Long,ProtoChunk> chunks=new HashMap<>();
        Set<BlockPos> initialWater;
        boolean result;
        long next;
        Run(Set<BlockPos> water, float temperature, PalettedContainerFactory palette){
            initialWater=water;
            var height=LevelHeightAccessor.create(0,16);
            for(int z=-1;z<=0;z++)for(int x=-1;x<=0;x++){
                var c=new ProtoChunk(new ChunkPos(x,z),UpgradeData.EMPTY,height,palette,null);
                c.setPersistedStatus(ChunkStatus.TERRAIN);chunks.put(key(x,z),c);
            }
            var biome=new Biome.BiomeBuilder().temperature(temperature).downfall(0.5f)
                .hasPrecipitation(true).specialEffects(new BiomeSpecialEffects.Builder().waterColor(0).build())
                .generationSettings(BiomeGenerationSettings.EMPTY).build();
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
                switch(m.getName()){
                    case "getMinY":return 0;
                    case "getSeaLevel":return 8;
                    case "getBiome":return Holder.direct(biome);
                    case "isInsideBuildHeight":{int y=(int)a[0];return y>=0&&y<16;}
                    case "getBrightness":{var p=(BlockPos)a[1];return Math.floorMod(p.getX()+2*p.getZ(),5)==0?10:9;}
                }
                var p=(BlockPos)a[0];var state=changes.getOrDefault(p,initial(p,water));
                return switch(m.getName()){
                    case "getBlockState"->state;
                    case "getFluidState"->state.getFluidState();
                    case "isStateAtPosition"->((Predicate<BlockState>)a[1]).test(state);
                    case "getChunk"->{var c=chunks.get(key(p.getX()>>4,p.getZ()>>4));if(c==null)throw new AssertionError("chunk "+p);yield c;}
                    case "setBlock"->{changes.put(p.immutable(),(BlockState)a[1]);yield true;}
                    case "scheduleTick"->null;
                    default->throw new UnsupportedOperationException(m.toString());
                };
            });
            var replace=BlockPredicate.alwaysTrue();
            var feature=new LakeFeature(Holder.direct(BlockStateProvider.of(Blocks.WATER)),
                Holder.direct(BlockStateProvider.of(Blocks.STONE)),BlockPredicate.alwaysTrue(),replace,BlockPredicate.alwaysTrue());
            var random=new WorldgenRandom(new XoroshiroRandomSource(0));
            long seed=random.setDecorationSeed(0,0,0);random.setFeatureSeed(seed,0,0);
            result=feature.place(world,null,random,new BlockPos(0,12,0));next=random.nextLong();
        }
        BlockState at(int x,int y,int z){var p=new BlockPos(x,y,z);return changes.getOrDefault(p,initial(p,initialWater));}
    }
    public static void main(String[] args)throws Exception{
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
        bind.invoke(Fluids.WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        var biome=new Biome.BiomeBuilder().temperature(0).downfall(0.5f).hasPrecipitation(true)
            .specialEffects(new BiomeSpecialEffects.Builder().waterColor(0).build()).generationSettings(BiomeGenerationSettings.EMPTY).build();
        var holder=Holder.direct(biome);var ids=new IdMapper<Holder<Biome>>();ids.add(holder);
        var palette=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),holder,null);
        var probe=new Run(Set.of(),0.8f,palette);
        if(!probe.result)throw new AssertionError("probe lake failed");
        var water=new TreeSet<BlockPos>(Comparator.comparingInt((BlockPos p)->p.getX()).thenComparingInt(p->p.getZ()).thenComparingInt(p->p.getY()));
        for(int x:new int[]{-8,7})for(int z:new int[]{-8,7})water.add(new BlockPos(x,12,z));
        for(int x:new int[]{-8,7})for(int z=-7;z<7;z++){
            var adjacent=new BlockPos(x==-8?-7:6,12,z);
            if(!probe.changes.getOrDefault(adjacent,Blocks.AIR.defaultBlockState()).is(Blocks.CAVE_AIR))water.add(new BlockPos(x,12,z));
        }
        if(water.stream().noneMatch(p->Math.floorMod(p.getX()+2*p.getZ(),5)==0))throw new AssertionError("missing bright water");
        out.println("BEGIN_RESULT");
        for(var p:water)out.printf("P %d %d%n",p.getX(),p.getZ());
        for(float temperature:new float[]{0.0f,0.8f}){
            var run=new Run(water,temperature,palette);
            out.printf("R %s %s %d%n",temperature==0.0f?"cold":"warm",run.result,run.next);
            for(int x=-8;x<8;x++)for(int z=-8;z<8;z++)for(int y=8;y<16;y++){
                var state=run.at(x,y,z);
                var name=state.is(Blocks.ICE)?"ice":state.is(Blocks.WATER)?"water":state.is(Blocks.CAVE_AIR)?"cave_air":state.is(Blocks.STONE)?"stone":state.isAir()?"air":"other";
                out.printf("B %d %d %d %s%n",x,y,z,name);
            }
        }
        out.println("END_RESULT");
    }
}
