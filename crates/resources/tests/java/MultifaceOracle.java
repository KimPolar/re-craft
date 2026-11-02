import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.feature.MultifaceGrowthFeature;

public class MultifaceOracle {
 static int bx=0,by=20,bz=0;
 static class Chunk extends ProtoChunk {
  List<String> marks;
  Chunk(){super(null,null,null,null,null);}
  public void markPosForPostProcessing(BlockPos p){marks.add(pos(p));}
 }
 static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
 static String state(BlockState s){int mask=0;var dirs=Direction.values();for(int i=0;i<6;i++)if(MultifaceBlock.hasFace(s,dirs[i]))mask|=1<<i;return mask+":"+s.getValue(BlockStateProperties.WATERLOGGED);}
 static BlockState initial(int mode,BlockPos p){
  int x=p.getX()-bx,y=p.getY()-by,z=p.getZ()-bz;
  if(mode==0)return Blocks.AIR.defaultBlockState();
  if(mode==1 && y>=1)return Blocks.STONE.defaultBlockState();
  if(mode>=2 && (x>=2||y>=2||z>=2||x<=-2||y<=-2||z<=-2))return Blocks.STONE.defaultBlockState();
  if(mode==3)return Blocks.WATER.defaultBlockState();
  if(mode==4)return Blocks.WATER.defaultBlockState().setValue(net.minecraft.world.level.block.LiquidBlock.LEVEL,5);
  if(mode==5 && x==1 && y==0 && z==0)return Blocks.GLOW_LICHEN.defaultBlockState().setValue(BlockStateProperties.EAST,true);
  return Blocks.AIR.defaultBlockState();
 }
 public static void main(String[]args)throws Exception {
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  if(args.length>0 && args[0].equals("chunk")){bx=8;by=8;bz=8;}
  for(var block:List.of(Blocks.GLOW_LICHEN,Blocks.STONE,Blocks.WATER))out.printf("E %s %s %s %s%n",block,block.defaultBlockState().hasBlockEntity(),net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(block.defaultBlockState()),block.defaultBlockState().getPostProcessPos(null,BlockPos.ZERO));
  var getter=(BlockGetter)Proxy.newProxyInstance(BlockGetter.class.getClassLoader(),new Class[]{BlockGetter.class},(o,m,a)->{throw new UnsupportedOperationException(m.toString());});
  for(var block:List.of(Blocks.STONE,Blocks.ANDESITE,Blocks.DIORITE,Blocks.GRANITE,Blocks.DRIPSTONE_BLOCK,Blocks.CALCITE,Blocks.TUFF,Blocks.DEEPSLATE,Blocks.SULFUR,Blocks.CINNABAR,Blocks.BEDROCK,Blocks.AMETHYST_BLOCK,Blocks.BUDDING_AMETHYST,Blocks.SMOOTH_BASALT))for(var direction:Direction.values())out.printf("S %s %s %s%n",block,direction,MultifaceBlock.canAttachTo(getter,direction,BlockPos.ZERO,block.defaultBlockState()));
  for(var block:List.of(Blocks.AIR,Blocks.WATER,Blocks.LAVA,Blocks.GLOW_LICHEN))for(var direction:Direction.values())out.printf("N %s %s %s%n",block,direction,MultifaceBlock.canAttachTo(getter,direction,BlockPos.ZERO,block.defaultBlockState()));
  var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
  for(int mode=0;mode<6;mode++)for(int config=0;config<4;config++)for(long seed:new long[]{0,1,-1,123456789}) {
   var blocks=new HashMap<BlockPos,BlockState>();var writes=new ArrayList<String>();var marks=new ArrayList<String>();
   var chunk=(Chunk)unsafe.allocateInstance(Chunk.class);chunk.marks=marks;final int scenario=mode;
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
    if(m.getName().equals("getBlockState")){var p=(BlockPos)a[0];return blocks.getOrDefault(p,initial(scenario,p));}
    if(m.getName().equals("getChunk"))return chunk;
    if(m.getName().equals("setBlockAndUpdate")||m.getName().equals("setBlock")) {
      var p=((BlockPos)a[0]).immutable();var s=(BlockState)a[1];boolean first=m.getName().equals("setBlockAndUpdate");
      if(!first && (int)a[2]!=2)throw new AssertionError("flags");blocks.put(p,s);writes.add(pos(p)+","+state(s)+","+first);return true;
    }
    throw new UnsupportedOperationException(m.toString());
   });
   var feature=new MultifaceGrowthFeature(Blocks.GLOW_LICHEN,config==3?3:20,config==2,config!=1,true,config==1?0f:config==2?1f:0.5f,HolderSet.direct(Blocks.STONE.builtInRegistryHolder()));
   var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
   boolean result=feature.place(world,null,random,new BlockPos(bx,by,bz));
   out.printf("C %d %d %d %s W %s M %s R %d%n",mode,config,seed,result,String.join(";",writes),String.join(";",marks),random.nextLong());
  }
 }
}
