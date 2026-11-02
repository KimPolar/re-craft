import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.placement.*;

public class FeaturePlacementOracle {
  static java.io.PrintStream out;
  static class Context extends PlacementContext {
    Context(){super(null,null,Optional.empty());}
    @Override public int getMinY(){return -64;}
    @Override public int getHeight(Heightmap.Types type,int x,int z){return ((x^z)&7)==0?-64:70+(x&7);}
  }
  static void emit(List<PlacementModifier> mods,int depth,Context context,RandomSource random,BlockPos p) {
    if(depth==mods.size()){out.printf("P %d %d %d %d%n",p.getX(),p.getY(),p.getZ(),random.nextInt(101));return;}
    mods.get(depth).modify(context,random,p,next->emit(mods,depth+1,context,random,next));
  }
  public static void main(String[] args)throws Exception {
    out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var context=(Context)unsafe.allocateInstance(Context.class);
    for(var field:List.of("minY","height")){Field v=WorldGenerationContext.class.getDeclaredField(field);v.setAccessible(true);v.setInt(context,field.equals("minY")?-64:384);}
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();int n=0;
    for(var entry:cases){
      var data=entry.getAsJsonObject();var mods=new ArrayList<PlacementModifier>();
      for(var item:data.getAsJsonArray("placement"))mods.add(PlacementModifier.CODEC.parse(JsonOps.INSTANCE,item).getOrThrow());
      for(long seed:new long[]{0,1,-1,987654321}){
        var random=new WorldgenRandom(new XoroshiroRandomSource(0));long deco=random.setDecorationSeed(seed,-32,48);random.setFeatureSeed(deco,7,3);
        out.printf("C %d %d%n",n,seed);emit(mods,0,context,random,new BlockPos(-32,0,48));out.println("R "+random.nextLong());
      }n++;
    }
  }
}
