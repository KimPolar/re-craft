import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class ScatteredChunkOracle {
  static java.io.PrintStream out;
  static final int CX=-1,CZ=2;
  static long key(int x,int z){return ((long)x<<32)^(z&0xffffffffL);}
  static BlockState initial(int x,int y,int z) {
    int n=x*31+y*17+z*13;
    if((n&7)==0)return Blocks.AIR.defaultBlockState();
    return switch(Math.floorMod(n,3)){case 0->Blocks.NETHERRACK.defaultBlockState();case 1->Blocks.BASALT.defaultBlockState();default->Blocks.BLACKSTONE.defaultBlockState();};
  }
  public static void main(String[] args)throws Exception {
    out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for(var b:List.of(Blocks.AIR,Blocks.CAVE_AIR,Blocks.VOID_AIR,Blocks.NETHERRACK,Blocks.BASALT,Blocks.BLACKSTONE,Blocks.ANCIENT_DEBRIS,Blocks.STONE,Blocks.DEEPSLATE,Blocks.WATER,Blocks.LAVA))
      for(var s:b.getStateDefinition().getPossibleStates()) if(s.hasBlockEntity()||PoiTypes.hasPoi(s))throw new AssertionError("unsupported state effects "+s);
    if(Blocks.ANCIENT_DEBRIS.defaultBlockState().getPostProcessPos(null,BlockPos.ZERO)!=null)throw new AssertionError("debris postprocessing");
    out.println("E safe");
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    for(var b:List.of(Blocks.NETHERRACK,Blocks.BASALT,Blocks.BLACKSTONE))bind.invoke(b.builtInRegistryHolder(),List.of(BlockTags.BASE_STONE_NETHER,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP));
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
    var ids=new IdMapper<Holder<Biome>>();ids.add(biome);
    var palette=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
    var height=LevelHeightAccessor.create(-16,32);
    var target=List.of(new BlockReplacement(new TagMatchTest(BlockTags.BASE_STONE_NETHER),Blocks.ANCIENT_DEBRIS.defaultBlockState()));
    for(int mode=0;mode<2;mode++) {
      bind.invoke(Blocks.ANCIENT_DEBRIS.builtInRegistryHolder(),mode==0?List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP):List.of());
      for(int size:new int[]{3,12})for(float discard:new float[]{0.0f,1.0f})for(long seed=0;seed<4;seed++) {
        var chunks=new HashMap<Long,ProtoChunk>();
        for(int cz=CZ-1;cz<=CZ+1;cz++)for(int cx=CX-1;cx<=CX+1;cx++) {
          var chunk=new ProtoChunk(new ChunkPos(cx,cz),UpgradeData.EMPTY,height,palette,null);chunk.setPersistedStatus(ChunkStatus.TERRAIN);
          for(int y=-16;y<16;y++)for(int z=0;z<16;z++)for(int x=0;x<16;x++) {
            var s=initial(cx*16+x,y,cz*16+z);if(!s.isAir())chunk.getSection((y+16)>>4).setBlockState(x,y&15,z,s,false);
          }
          Heightmap.primeHeightmaps(chunk,EnumSet.of(Heightmap.Types.OCEAN_FLOOR_WG,Heightmap.Types.WORLD_SURFACE_WG));
          chunks.put(key(cx,cz),chunk);
        }
        var changed=new TreeSet<Long>();
        var level=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
          var p=(BlockPos)a[0];int cx=p.getX()>>4,cz=p.getZ()>>4;var chunk=chunks.get(key(cx,cz));
          switch(m.getName()) {
            case "getBlockState":if(chunk==null)throw new AssertionError("missing chunk");return chunk.getBlockState(p);
            case "setBlock":
              if(Math.abs(cx-CX)>1||Math.abs(cz-CZ)>1)return false;
              if(chunk==null)throw new AssertionError("missing chunk");
              var s=(BlockState)a[1];chunk.setBlockState(p,s,(int)a[2]);
              changed.add(key(p.getX(),p.getZ()));out.printf("W %d %d %d%n",p.getX(),p.getY(),p.getZ());return true;
            default:throw new UnsupportedOperationException(m.getName());
          }
        });
        var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-16,32);random.setFeatureSeed(deco,7,3);
        out.printf("C %d %d %.1f %d%n",mode,size,discard,seed);
        boolean result=new ScatteredOreFeature(target,size,discard).place(level,null,random,new BlockPos(-16,14,32));
        out.println("R "+result+" "+random.nextLong());
        for(long column:changed) {
          int x=(int)(column>>32),z=(int)column;var chunk=chunks.get(key(x>>4,z>>4));
          out.printf("H %d %d %d %d%n",x,z,chunk.getHeight(Heightmap.Types.OCEAN_FLOOR_WG,x,z)+1,chunk.getHeight(Heightmap.Types.WORLD_SURFACE_WG,x,z)+1);
        }
      }
    }
  }
}
