import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.placement.*;
import net.minecraft.world.level.levelgen.heightproviders.UniformHeight;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class OreVeinOracle {
  static java.io.PrintStream out;static sun.misc.Unsafe unsafe;static Map<BlockPos,BlockState> blocks;
  static BlockState initial(BlockPos p){int v=p.getX()*31+p.getY()*17+p.getZ()*13;if((v&7)==0)return Blocks.AIR.defaultBlockState();return (p.getY()<0?Blocks.DEEPSLATE:Blocks.STONE).defaultBlockState();}
  static class Section extends LevelChunkSection {
    int x,y,z;
    Section(){super((PalettedContainerFactory)null);}
    BlockPos at(int dx,int dy,int dz){return new BlockPos(x*16+dx,y*16+dy,z*16+dz);}
    @Override public void acquire(){} @Override public void release(){}
    @Override public BlockState getBlockState(int dx,int dy,int dz){var p=at(dx,dy,dz);return blocks.getOrDefault(p,initial(p));}
    @Override public BlockState setBlockState(int dx,int dy,int dz,BlockState state,boolean lock){
      var p=at(dx,dy,dz);var old=blocks.getOrDefault(p,initial(p));blocks.put(p,state);
      out.printf("W %d %d %d %s%n",p.getX(),p.getY(),p.getZ(),state.getBlock().builtInRegistryHolder().key().identifier());return old;
    }
  }
  static class Chunk extends ProtoChunk {
    int x,z;Map<Integer,Section> sections;
    Chunk(){super(null,null,null,null,null);}
    @Override public LevelChunkSection getSection(int index){return sections.computeIfAbsent(index,i->{try{var s=(Section)unsafe.allocateInstance(Section.class);s.x=x;s.y=i-1;s.z=z;return s;}catch(Exception e){throw new RuntimeException(e);}});}
  }
  static WorldGenLevel world(boolean ground,boolean restrict){
    var chunks=new HashMap<Long,Chunk>();
    return (WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
      switch(m.getName()){
        case "getHeight":return ground?20:-100;
        case "isOutsideBuildHeight":return (int)a[0]<-16||(int)a[0]>=16;
        case "getSectionIndex":return Math.floorDiv((int)a[0]+16,16);
        case "getSectionsCount":return 2;
        case "ensureCanWrite": {var p=(BlockPos)a[0];return !restrict||((p.getX()+p.getZ())&31)!=0;}
        case "getChunk": {int x=(int)a[0],z=(int)a[1];long key=((long)x<<32)^(z&0xffffffffL);return chunks.computeIfAbsent(key,k->{try{var c=(Chunk)unsafe.allocateInstance(Chunk.class);c.x=x;c.z=z;c.sections=new HashMap<>();return c;}catch(Exception e){throw new RuntimeException(e);}});}
        default:throw new UnsupportedOperationException(m.getName());
      }
    });
  }
  static void place(List<PlacementModifier> modifiers,int depth,PlacementContext context,WorldgenRandom random,BlockPos pos,WorldGenLevel world,OreFeature feature){
    if(depth==modifiers.size()){feature.place(world,null,random,pos);return;}
    modifiers.get(depth).modify(context,random,pos,next->place(modifiers,depth+1,context,random,next,world,feature));
  }
  public static void main(String[] args)throws Exception{
    out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);unsafe=(sun.misc.Unsafe)f.get(null);
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.BASE_STONE_OVERWORLD));bind.invoke(Blocks.DEEPSLATE.builtInRegistryHolder(),List.of(BlockTags.BASE_STONE_OVERWORLD));
    var first=new AllOfRuleTest(List.of(new TagMatchTest(BlockTags.BASE_STONE_OVERWORLD),new HeightMatchTest(-2,2),new NotRuleTest(new RandomBlockMatchTest(Blocks.STONE,0.25f))));
    var second=new AnyOfRuleTest(List.of(new BlockMatchTest(Blocks.DEEPSLATE),new RandomBlockMatchTest(Blocks.STONE,0.5f)));
    var targets=List.of(new BlockReplacement(first,Blocks.DIAMOND_ORE.defaultBlockState()),new BlockReplacement(second,Blocks.GOLD_ORE.defaultBlockState()));
    for(int size:new int[]{0,1,3,9,17,33,64})for(float chance:new float[]{0,0.5f,1})for(long seed=0;seed<8;seed++){
      blocks=new HashMap<>();var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-32,48);random.setFeatureSeed(deco,7,3);
      out.printf("C %d %s %d%n",size,chance,seed);boolean result=new OreFeature(targets,size,chance).place(world(seed%4!=0,seed%3==0),null,random,new BlockPos(-16,seed%2==0?-1:3,32));
      out.println("R "+result+" "+random.nextLong());
    }
    var context=(PlacementContext)unsafe.allocateInstance(PlacementContext.class);
    for(var field:List.of("minY","height")){var v=WorldGenerationContext.class.getDeclaredField(field);v.setAccessible(true);v.setInt(context,field.equals("minY")?-16:32);}
    var modifiers=List.<PlacementModifier>of(CountPlacement.of(4),InSquarePlacement.spread(),HeightRangePlacement.of(UniformHeight.of(VerticalAnchor.absolute(-5),VerticalAnchor.absolute(5))));
    for(long seed=0;seed<8;seed++){
      blocks=new HashMap<>();var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-32,48);random.setFeatureSeed(deco,7,3);
      out.println("I "+seed);place(modifiers,0,context,random,new BlockPos(-32,0,48),world(true,false),new OreFeature(targets,9,0.5f));out.println("T "+random.nextLong());
    }
    if(args.length>0 && args[0].equals("--batch")) out.println("Z "+ChunkPyramid.GENERATION_PYRAMID.getStepTo(ChunkStatus.FEATURES).blockStateWriteRadius());
    if(args.length>0 && args[0].equals("--batch")) for(long seed=0;seed<8;seed++){
      blocks=new HashMap<>();var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-32,48);
      out.println("B "+seed);
      for(int feature=0;feature<2;feature++){
        random.setFeatureSeed(deco,feature,0);
        place(modifiers,0,context,random,new BlockPos(-32,0,48),world(true,false),new OreFeature(targets,9,0.5f));
      }
      out.println("U "+random.nextLong());
    }
  }
}
