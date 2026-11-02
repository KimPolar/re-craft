import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.feature.stateproviders.*;
import net.minecraft.world.level.levelgen.placement.*;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.util.valueproviders.UniformInt;

public class DiskChunkOracle {
  static java.io.PrintStream out;
  static final int CX=-1,CZ=2;
  static long key(int x,int z){return ((long)x<<32)^(z&0xffffffffL);}
  static BlockState initial(int x,int y,int z) {
    int n=Math.floorMod(x*31+y*17+z*13,8);
    if(y<0)return Blocks.STONE.defaultBlockState();
    if(y==0)return switch(n%4){case 0->Blocks.MUD.defaultBlockState();case 1->Blocks.CLAY.defaultBlockState();case 2->Blocks.GRASS_BLOCK.defaultBlockState();default->Blocks.DIRT.defaultBlockState();};
    if(y==1)return switch(n%4){case 0->Blocks.AIR.defaultBlockState();case 1->Blocks.GRASS_BLOCK.defaultBlockState();case 2->Blocks.MUD.defaultBlockState();default->Blocks.DIRT.defaultBlockState();};
    if(y==2)return switch(n%4){case 0->Blocks.WATER.defaultBlockState();case 1->Blocks.DIRT.defaultBlockState();case 2->Blocks.AIR.defaultBlockState();default->Blocks.STONE.defaultBlockState();};
    if(y==3&&n<2)return Blocks.STONE.defaultBlockState();
    return Blocks.AIR.defaultBlockState();
  }
  static DiskFeature feature(int kind) {
    BlockPredicate target;
    BlockStateProvider provider;
    if(kind==0) {
      target=BlockPredicate.matchesBlocks(Blocks.DIRT,Blocks.CLAY);
      provider=BlockStateProvider.of(Blocks.CLAY);
    } else if(kind==1) {
      target=BlockPredicate.matchesBlocks(Blocks.DIRT,Blocks.GRASS_BLOCK);
      provider=RuleBasedStateProvider.builder(BlockStateProvider.of(Blocks.SAND))
        .ifTrueThenProvide(BlockPredicate.matchesBlocks(Direction.DOWN,Blocks.AIR),Blocks.SANDSTONE).build();
    } else if(kind==2) {
      target=BlockPredicate.matchesBlocks(Blocks.DIRT,Blocks.MUD);
      var pred=BlockPredicate.not(BlockPredicate.anyOf(BlockPredicate.solid(Direction.UP),BlockPredicate.matchesFluids(Direction.UP,Fluids.WATER)));
      provider=RuleBasedStateProvider.builder(BlockStateProvider.of(Blocks.DIRT)).ifTrueThenProvide(pred,Blocks.GRASS_BLOCK).build();
    } else {
      target=BlockPredicate.matchesBlocks(Blocks.DIRT,Blocks.GRASS_BLOCK);
      provider=RuleBasedStateProvider.ifTrueThenProvide(BlockPredicate.matchesBlocks(Direction.DOWN,Blocks.AIR),Blocks.CLAY);
    }
    return new DiskFeature(Holder.direct(provider),target,UniformInt.of(2,kind==0?3:6),kind==0?1:2);
  }
  public static void main(String[]args)throws Exception {
    out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var generator=WorldPresets.getNormalOverworld(VanillaRegistries.createWorldLookup()).generator();
    var safe=List.of(Blocks.AIR,Blocks.CAVE_AIR,Blocks.VOID_AIR,Blocks.WATER,Blocks.LAVA,Blocks.STONE,Blocks.DEEPSLATE,Blocks.NETHERRACK,Blocks.BASALT,Blocks.BLACKSTONE,Blocks.ANCIENT_DEBRIS,Blocks.DIRT,Blocks.GRASS_BLOCK,Blocks.MUD,Blocks.CLAY,Blocks.SAND,Blocks.SANDSTONE,Blocks.GRAVEL,Blocks.PACKED_ICE,Blocks.SNOW_BLOCK,Blocks.ICE,Blocks.PODZOL,Blocks.COARSE_DIRT,Blocks.MYCELIUM,Blocks.TUFF,Blocks.CALCITE,Blocks.GRANITE,Blocks.DIORITE,Blocks.ANDESITE,Blocks.BEDROCK,Blocks.RED_SAND,Blocks.MOSS_BLOCK);
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    for(var b:safe) {
      boolean solid=!(b==Blocks.AIR||b==Blocks.CAVE_AIR||b==Blocks.VOID_AIR||b==Blocks.WATER||b==Blocks.LAVA);
      for(var s:b.getStateDefinition().getPossibleStates()) if(s.isSolid()!=solid||s.hasBlockEntity()||PoiTypes.hasPoi(s)||s.getPostProcessPos(null,BlockPos.ZERO)!=null)throw new AssertionError("unsafe disk state "+s);
      if(solid)bind.invoke(b.builtInRegistryHolder(),List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES));
    }
    out.println("E safe");
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
    var ids=new IdMapper<Holder<Biome>>();ids.add(biome);
    var palette=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
    var height=LevelHeightAccessor.create(-16,32);
    for(int kind=0;kind<8;kind++)for(int restricted=0;restricted<2;restricted++)for(long seed=0;seed<4;seed++) {
      final int limit=restricted;
      var chunks=new HashMap<Long,ProtoChunk>();
      for(int cz=CZ-1;cz<=CZ+1;cz++)for(int cx=CX-1;cx<=CX+1;cx++) {
        var c=new ProtoChunk(new ChunkPos(cx,cz),UpgradeData.EMPTY,height,palette,null);c.setPersistedStatus(ChunkStatus.TERRAIN);
        for(int y=-16;y<16;y++)for(int z=0;z<16;z++)for(int x=0;x<16;x++) {var s=initial(cx*16+x,y,cz*16+z);if(!s.isAir())c.getSection((y+16)>>4).setBlockState(x,y&15,z,s,false);}
        Heightmap.primeHeightmaps(c,EnumSet.of(Heightmap.Types.WORLD_SURFACE_WG,Heightmap.Types.OCEAN_FLOOR_WG,Heightmap.Types.WORLD_SURFACE,Heightmap.Types.OCEAN_FLOOR,Heightmap.Types.MOTION_BLOCKING,Heightmap.Types.MOTION_BLOCKING_NO_LEAVES));
        chunks.put(key(cx,cz),c);
      }
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        if(m.getName().equals("getMinY"))return -16;
        if(m.getName().equals("getHeight")&&(a==null||a.length==0))return 32;
        var p=(BlockPos)a[0];int cx=p.getX()>>4,cz=p.getZ()>>4;var c=chunks.get(key(cx,cz));if(c==null)throw new AssertionError("missing chunk");
        switch(m.getName()) {
          case "getBlockState":return c.getBlockState(p);
          case "getFluidState":return c.getBlockState(p).getFluidState();
          case "getChunk":return c;
          case "setBlock":if(limit==1&&(cx!=CX||cz!=CZ))return false;c.setBlockState(p,(BlockState)a[1],(int)a[2]);return true;
          default:throw new UnsupportedOperationException(m.getName());
        }
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-16,32);random.setFeatureSeed(deco,7,3);
      out.printf("C %d %d %d%n",kind,restricted,seed);
      var disk=feature(kind%4);
      boolean result;
      try {
        result=kind<4?disk.place(world,null,random,new BlockPos(-16,0,32)):
          new PlacedFeature(Holder.direct(disk),List.of(CountPlacement.of(4),InSquarePlacement.spread(),BlockPredicateFilter.forPredicate(BlockPredicate.matchesBlocks(Blocks.DIRT,Blocks.MUD,Blocks.GRASS_BLOCK)))).place(world,generator,random,new BlockPos(-16,0,32));
      } catch(RuntimeException e) { e.printStackTrace(out);throw e; }
      out.println("R "+result+" "+random.nextLong());
      for(int cz=CZ-1;cz<=CZ+1;cz++)for(int cx=CX-1;cx<=CX+1;cx++) {
        var c=chunks.get(key(cx,cz));
        for(int y=-3;y<=4;y++)for(int z=0;z<16;z++)for(int x=0;x<16;x++) {
          int wx=cx*16+x,wz=cz*16+z;var actual=c.getBlockState(new BlockPos(wx,y,wz));
          if(actual!=initial(wx,y,wz))out.printf("B %d %d %d %s%n",wx,y,wz,actual.getBlock().getDescriptionId().substring(16));
        }
        var marks=c.getPostProcessing();
        for(int section=0;section<marks.length;section++)if(marks[section]!=null)for(short packed:marks[section])out.printf("M %d %d %d %d%n",cx,cz,section,(int)packed);
      }
    }
  }
}
