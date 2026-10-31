import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.Proxy;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.*;
import net.minecraft.world.level.chunk.CarvingMask;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.carver.WorldCarver;

public class CaveCarverOracle {
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var ctor=WorldGenerationContext.class.getDeclaredConstructor(LevelAccessor.class); ctor.setAccessible(true);
    for(var element:cases) {
      var item=element.getAsJsonObject();
      int min=item.get("min_y").getAsInt(),height=item.get("height").getAsInt();
      LevelAccessor level=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(o,m,a)->switch(m.getName()) {
        case "getMinY" -> min; case "getHeight" -> height; case "getSeaLevel" -> 63;
        default -> throw new UnsupportedOperationException(m.getName());
      });
      var context=ctor.newInstance(level);
      WorldCarver carver=(WorldCarver)WorldCarver.DIRECT_CODEC.parse(JsonOps.INSTANCE,item.get("config")).getOrThrow();
      var src=item.getAsJsonArray("source"); var dst=item.getAsJsonArray("target");
      var source=new ChunkPos(src.get(0).getAsInt(),src.get(1).getAsInt());
      var target=new ChunkPos(dst.get(0).getAsInt(),dst.get(1).getAsInt());
      var random=new WorldgenRandom(new LegacyRandomSource(0));
      random.setLargeFeatureSeed(item.get("seed").getAsLong()+item.get("index").getAsInt(),source.x(),source.z());
      var mask=new CarvingMask(min+1,min+height-8);
      boolean start=carver.isStartChunk(random);
      if(start) carver.carve(context,random,target,source,mask);
      out.printf("S %s%n",start);
      mask.visit((x,z,lo,hi)->out.printf("P %d %d %d %d%n",x,z,lo,hi));
    }
  }
}
