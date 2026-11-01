import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.Proxy;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.*;
import net.minecraft.world.level.chunk.CarvingMask;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.carver.WorldCarver;

public class CarverScheduleOracle {
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var root=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
    var lists=new ArrayList<List<WorldCarver>>();
    for(var list:root.getAsJsonArray("biomes")) {
      var carvers=new ArrayList<WorldCarver>();
      for(var config:list.getAsJsonArray()) carvers.add((WorldCarver)WorldCarver.DIRECT_CODEC.parse(JsonOps.INSTANCE,config).getOrThrow());
      lists.add(carvers);
    }
    var ctor=WorldGenerationContext.class.getDeclaredConstructor(LevelAccessor.class); ctor.setAccessible(true);
    for(var element:root.getAsJsonArray("cases")) {
      var c=element.getAsJsonObject(); int min=c.get("min_y").getAsInt(),height=c.get("height").getAsInt();
      LevelAccessor level=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(o,m,a)->switch(m.getName()) {
        case "getMinY" -> min; case "getHeight" -> height; case "getSeaLevel" -> 63;
        default -> throw new UnsupportedOperationException(m.getName());
      });
      var context=ctor.newInstance(level); var target=new ChunkPos(c.get("x").getAsInt(),c.get("z").getAsInt());
      var mask=new CarvingMask(min+1,min+height-8); var random=new WorldgenRandom(new LegacyRandomSource(0));
      long seed=c.get("seed").getAsLong();
      for(int dx=-8;dx<=8;dx++) for(int dz=-8;dz<=8;dz++) {
        var source=new ChunkPos(target.x()+dx,target.z()+dz);
        int biome=Math.floorMod(source.x()+2*source.z(),lists.size()); int index=0;
        for(var carver:lists.get(biome)) {
          random.setLargeFeatureSeed(seed+index,source.x(),source.z());
          if(carver.isStartChunk(random)) carver.carve(context,random,target,source,mask);
          index++;
        }
      }
      out.println("S"); mask.visit((x,z,lo,hi)->out.printf("P %d %d %d %d%n",x,z,lo,hi));
    }
  }
}
