import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.placement.*;
import net.minecraft.world.level.levelgen.heightproviders.UniformHeight;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class ScatteredOreOracle {
  static java.io.PrintStream out;
  static BlockState initial(BlockPos p){
    int key=p.getX()*31+p.getY()*17+p.getZ()*13;
    if((key&7)==0)return Blocks.AIR.defaultBlockState();
    return (p.getY()<0?Blocks.DEEPSLATE:Blocks.STONE).defaultBlockState();
  }
  static WorldGenLevel world(Map<BlockPos,BlockState> blocks){
    return (WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
      switch(m.getName()){
        case "getBlockState": {var p=(BlockPos)a[0];return blocks.getOrDefault(p,initial(p));}
        case "setBlock": {var p=((BlockPos)a[0]).immutable();var state=(BlockState)a[1];out.printf("W %d %d %d %s %d%n",p.getX(),p.getY(),p.getZ(),state.getBlock().builtInRegistryHolder().key().identifier(),(int)a[2]);blocks.put(p,state);return true;}
        default:throw new UnsupportedOperationException(m.getName());
      }
    });
  }
  static void place(List<PlacementModifier> modifiers,int depth,PlacementContext context,WorldgenRandom random,BlockPos pos,WorldGenLevel world,ScatteredOreFeature feature){
    if(depth==modifiers.size()){feature.place(world,null,random,pos);return;}
    modifiers.get(depth).modify(context,random,pos,next->place(modifiers,depth+1,context,random,next,world,feature));
  }
  public static void main(String[] args)throws Exception{
    out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    for(var block:BuiltInRegistries.BLOCK)out.println("B "+BuiltInRegistries.BLOCK.getKey(block));
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.BASE_STONE_OVERWORLD));
    bind.invoke(Blocks.DEEPSLATE.builtInRegistryHolder(),List.of(BlockTags.BASE_STONE_OVERWORLD));
    var first=new AllOfRuleTest(List.of(new TagMatchTest(BlockTags.BASE_STONE_OVERWORLD),new HeightMatchTest(-2,2),new NotRuleTest(new RandomBlockMatchTest(Blocks.STONE,0.25f))));
    var second=new AnyOfRuleTest(List.of(new BlockMatchTest(Blocks.DEEPSLATE),new RandomBlockMatchTest(Blocks.STONE,0.5f)));
    var targets=List.of(new BlockReplacement(first,Blocks.ANCIENT_DEBRIS.defaultBlockState()),new BlockReplacement(second,Blocks.GOLD_ORE.defaultBlockState()));
    for(int size:new int[]{0,1,3,12,64})for(float discard:new float[]{0,0.5f,1})for(long seed=0;seed<16;seed++){
      var blocks=new HashMap<BlockPos,BlockState>();
      var level=world(blocks);
      var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-32,48);random.setFeatureSeed(deco,7,3);
      out.printf("C %d %s %d%n",size,discard,seed);
      boolean result=new ScatteredOreFeature(targets,size,discard).place(level,null,random,new BlockPos(-16,0,32));
      out.println("R "+result+" "+random.nextLong());
    }
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var context=(PlacementContext)unsafe.allocateInstance(PlacementContext.class);
    for(var field:List.of("minY","height")){var v=WorldGenerationContext.class.getDeclaredField(field);v.setAccessible(true);v.setInt(context,field.equals("minY")?-64:384);}
    var modifiers=List.<PlacementModifier>of(CountPlacement.of(4),InSquarePlacement.spread(),HeightRangePlacement.of(UniformHeight.of(VerticalAnchor.absolute(-5),VerticalAnchor.absolute(5))));
    for(long seed=0;seed<16;seed++){
      var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-32,48);random.setFeatureSeed(deco,7,3);
      out.println("I "+seed);place(modifiers,0,context,random,new BlockPos(-32,0,48),world(new HashMap<>()),new ScatteredOreFeature(targets,12,0.5f));out.println("T "+random.nextLong());
    }
  }
}
