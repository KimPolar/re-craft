import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.densityfunction.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pools.*;
import net.minecraft.util.context.ContextMap;
import com.mojang.serialization.JsonOps;

public class BeardifierOracle {
  static int[] vector(JsonElement v){var a=v.getAsJsonArray();return new int[]{a.get(0).getAsInt(),a.get(1).getAsInt(),a.get(2).getAsInt()};}
  static BoundingBox box(JsonElement v){var o=v.getAsJsonObject();var a=vector(o.get("min"));var b=vector(o.get("max"));return new BoundingBox(a[0],a[1],a[2],b[0],b[1],b[2]);}
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var groups=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    for(var element:groups) {
      var group=element.getAsJsonObject();var pieces=new ArrayList<Beardifier.Rigid>();var junctions=new ArrayList<JigsawJunction>();
      for(var entry:group.getAsJsonArray("pieces")){var v=entry.getAsJsonObject();pieces.add(new Beardifier.Rigid(box(v.get("box")),TerrainAdjustment.valueOf(v.get("adjustment").getAsString()),v.get("delta").getAsInt()));}
      for(var item:group.getAsJsonArray("junctions")){var p=vector(item);junctions.add(new JigsawJunction(p[0],p[1],p[2],0,StructureTemplatePool.Projection.RIGID));}
      var affected=group.get("affected").isJsonNull()?null:box(group.get("affected"));var beardifier=new Beardifier(pieces,junctions,affected);
      out.println("S");
      for(var item:group.getAsJsonArray("points")){var p=vector(item);out.println("P "+Float.floatToRawIntBits(beardifier.sampleValue(SamplerContext.EMPTY_UNCACHED,p[0],p[1],p[2])));}
      for(var item:group.getAsJsonArray("volumes")){var v=item.getAsJsonObject();var min=vector(v.get("min"));var size=vector(v.get("size"));var step=vector(v.get("step"));
        var volume=new DensityVolume(size[0],size[1],size[2],min[0],min[1],min[2],step[0],step[1],step[2]);var buffer=DensityBuffer.createUnpooled(volume.size());
        beardifier.sampleVolume(SamplerContext.EMPTY_UNCACHED,buffer,volume);var line=new StringBuilder("V");for(int i=0;i<volume.size();i++)line.append(' ').append(Float.floatToRawIntBits(buffer.get(i)));out.println(line);
      }
      var expression=JsonParser.parseString("{\"type\":\"minecraft:add\",\"left\":-0.25,\"right\":{\"type\":\"minecraft:beardifier\"}}");
      var sampler=new DensityFunctionCompiler(null).getSampler(DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE,expression).getOrThrow());
      var context=SamplerContext.builder().setUserFields(ContextMap.builder().set(Beardifier.CONTEXT_KEY,beardifier).build()).build();
      for(var item:group.getAsJsonArray("points")){var p=vector(item);out.println("Q "+Float.floatToRawIntBits(sampler.sampleValue(context,p[0],p[1],p[2])));}
      for(var item:group.getAsJsonArray("volumes")){var v=item.getAsJsonObject();var min=vector(v.get("min"));var size=vector(v.get("size"));var step=vector(v.get("step"));
        var volume=new DensityVolume(size[0],size[1],size[2],min[0],min[1],min[2],step[0],step[1],step[2]);var buffer=DensityBuffer.createUnpooled(volume.size());
        sampler.sampleVolume(context,buffer,volume);var line=new StringBuilder("W");for(int i=0;i<volume.size();i++)line.append(' ').append(Float.floatToRawIntBits(buffer.get(i)));out.println(line);
      }
    }
  }
}
