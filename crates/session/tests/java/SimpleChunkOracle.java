import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.DoubleBlockHalf;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.feature.stateproviders.*;
import net.minecraft.util.random.WeightedList;
import net.minecraft.world.ticks.*;

public class SimpleChunkOracle {
  static java.io.PrintStream out;
  static BlockState initial(int x,int y,int z) {
    int n=Math.floorMod(x*7+z*11,8);
    if(y<0)return Blocks.STONE.defaultBlockState();
    if(y==0)return switch(n){case 0,6->Blocks.DIRT.defaultBlockState();case 1->Blocks.GRASS_BLOCK.defaultBlockState();case 2->Blocks.SAND.defaultBlockState();case 3->Blocks.WATER.defaultBlockState();case 4->Blocks.STONE.defaultBlockState();case 5->Blocks.SUNFLOWER.defaultBlockState();default->Blocks.AIR.defaultBlockState();};
    if(y==1)return switch(n%5){case 0->Blocks.AIR.defaultBlockState();case 1->Blocks.SHORT_GRASS.defaultBlockState();case 2->Blocks.POPPY.defaultBlockState();case 3->Blocks.WATER.defaultBlockState();default->Blocks.STONE.defaultBlockState();};
    if(y==2)return switch((n+2)%6){case 0->Blocks.AIR.defaultBlockState();case 1->Blocks.SHORT_GRASS.defaultBlockState();case 2->Blocks.POPPY.defaultBlockState();case 3->Blocks.WATER.defaultBlockState();case 4->Blocks.STONE.defaultBlockState();default->Blocks.TALL_GRASS.defaultBlockState();};
    return Blocks.AIR.defaultBlockState();
  }
  static String key(BlockState state) {
    var props=new TreeMap<String,String>();state.getValues().forEach(e->props.put(e.property().getName(),e.valueName()));
    var s=new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
    for(var e:props.entrySet())s.append(e.getKey()).append('=').append(e.getValue()).append(';');
    return s.toString();
  }
  static SimpleBlockFeature feature(int kind,boolean schedule) {
    BlockStateProvider p=switch(kind) {
      case 0 -> BlockStateProvider.of(Blocks.SHORT_GRASS);
      case 1 -> new WeightedStateProvider(WeightedList.<BlockState>builder().add(Blocks.SHORT_GRASS.defaultBlockState(),3).add(Blocks.FERN.defaultBlockState(),1));
      case 2 -> new WeightedStateProvider(WeightedList.<BlockState>builder().add(Blocks.SHORT_DRY_GRASS.defaultBlockState(),1).add(Blocks.TALL_DRY_GRASS.defaultBlockState(),1));
      case 3 -> BlockStateProvider.of(Blocks.TALL_GRASS);
      case 4 -> BlockStateProvider.of(Blocks.SUNFLOWER.defaultBlockState().setValue(DoublePlantBlock.HALF,DoubleBlockHalf.UPPER));
      case 5 -> BlockStateProvider.of(Blocks.SWEET_BERRY_BUSH.defaultBlockState().setValue(SweetBerryBushBlock.AGE,3));
      case 6 -> BlockStateProvider.of(Blocks.MELON);
      case 7 -> BlockStateProvider.of(Blocks.CLOSED_EYEBLOSSOM);
      default -> throw new AssertionError();
    };
    return new SimpleBlockFeature(Holder.direct(p),schedule);
  }
  public static void main(String[]args)throws Exception {
    out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    for(var b:List.of(Blocks.STONE,Blocks.DIRT,Blocks.GRASS_BLOCK))bind.invoke(b.builtInRegistryHolder(),List.of(BlockTags.SUPPORTS_VEGETATION,BlockTags.SUPPORTS_DRY_VEGETATION,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES));
    bind.invoke(Blocks.SAND.builtInRegistryHolder(),List.of(BlockTags.SUPPORTS_DRY_VEGETATION,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES));
    bind.invoke(Blocks.MELON.builtInRegistryHolder(),List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES));
    var safe=List.of(Blocks.SHORT_GRASS,Blocks.FERN,Blocks.BUSH,Blocks.DANDELION,Blocks.POPPY,Blocks.BLUE_ORCHID,Blocks.ALLIUM,Blocks.AZURE_BLUET,Blocks.RED_TULIP,Blocks.ORANGE_TULIP,Blocks.WHITE_TULIP,Blocks.PINK_TULIP,Blocks.OXEYE_DAISY,Blocks.CORNFLOWER,Blocks.LILY_OF_THE_VALLEY,Blocks.FIREFLY_BUSH,Blocks.CLOSED_EYEBLOSSOM,Blocks.SWEET_BERRY_BUSH,Blocks.SHORT_DRY_GRASS,Blocks.TALL_DRY_GRASS,Blocks.DEAD_BUSH,Blocks.SUNFLOWER,Blocks.TALL_GRASS,Blocks.LARGE_FERN,Blocks.ROSE_BUSH,Blocks.PEONY,Blocks.LILAC,Blocks.MELON,Blocks.PUMPKIN);
    for(var b:safe) for(var s:b.getStateDefinition().getPossibleStates()) if(s.hasBlockEntity()||PoiTypes.hasPoi(s)||s.getPostProcessPos(null,BlockPos.ZERO)!=null)throw new AssertionError("unsafe plant "+s);
    out.println("E safe");
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));var ids=new IdMapper<Holder<Biome>>();ids.add(biome);
    var palette=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
    var height=LevelHeightAccessor.create(-16,32);
    for(int kind=0;kind<8;kind++)for(int restricted=0;restricted<2;restricted++)for(int n=0;n<8;n++)for(long seed:new long[]{0,7})for(int scheduled=0;scheduled<2;scheduled++) {
      int x=-16+n,z=32;final int limit=restricted;
      var c=new ProtoChunk(new ChunkPos(-1,2),UpgradeData.EMPTY,height,palette,null);c.setPersistedStatus(ChunkStatus.TERRAIN);
      for(int y=-16;y<16;y++)for(int dz=0;dz<16;dz++)for(int dx=0;dx<16;dx++){var s=initial(-16+dx,y,32+dz);if(!s.isAir())c.getSection((y+16)>>4).setBlockState(dx,y&15,dz,s,false);}
      Heightmap.primeHeightmaps(c,EnumSet.of(Heightmap.Types.WORLD_SURFACE_WG,Heightmap.Types.OCEAN_FLOOR_WG,Heightmap.Types.WORLD_SURFACE,Heightmap.Types.OCEAN_FLOOR,Heightmap.Types.MOTION_BLOCKING,Heightmap.Types.MOTION_BLOCKING_NO_LEAVES));
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        var p=(BlockPos)a[0];if((p.getX()>>4)!=-1||(p.getZ()>>4)!=2)throw new AssertionError("missing chunk");
        return switch(m.getName()) {case "getBlockState" -> c.getBlockState(p);case "setBlock" -> {if(limit==1)yield false;c.setBlockState(p,(BlockState)a[1],(int)a[2]);yield true;}case "scheduleTick" -> {c.getBlockTicks().schedule(new ScheduledTick<>((Block)a[1],p,1L,0L));yield null;}default -> throw new UnsupportedOperationException(m.getName());};
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-16,32);random.setFeatureSeed(deco,7,3);
      out.printf("C %d %d %d %d %d%n",kind,restricted,n,seed,scheduled);
      var placed=feature(kind,scheduled==1);
      boolean result=placed.place(world,null,random,new BlockPos(x,1,z));
      boolean second=placed.place(world,null,random,new BlockPos(x,1,z));
      out.println("R "+result+" "+second+" "+random.nextLong());
      out.println("B 1 "+key(c.getBlockState(new BlockPos(x,1,z))));out.println("B 2 "+key(c.getBlockState(new BlockPos(x,2,z))));
      out.printf("H %d %d %d %d%n",c.getHeight(Heightmap.Types.WORLD_SURFACE,x,z)+1,c.getHeight(Heightmap.Types.OCEAN_FLOOR,x,z)+1,c.getHeight(Heightmap.Types.MOTION_BLOCKING,x,z)+1,c.getHeight(Heightmap.Types.MOTION_BLOCKING_NO_LEAVES,x,z)+1);
      for(var tick:((ProtoChunkTicks<Block>)c.getBlockTicks()).scheduledTicks())out.printf("T %s %d %d %d %d %d%n",BuiltInRegistries.BLOCK.getKey(tick.type()),tick.pos().getX(),tick.pos().getY(),tick.pos().getZ(),tick.delay(),tick.priority().getValue());
    }
  }
}
