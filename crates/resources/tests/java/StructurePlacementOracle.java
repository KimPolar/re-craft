import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.placement.*;

public class StructurePlacementOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var registries=VanillaRegistries.createWorldLookup();var ops=registries.createSerializationContext(JsonOps.INSTANCE);
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var state=(ChunkGeneratorStructureState)unsafe.allocateInstance(ChunkGeneratorStructureState.class);
    var seedField=ChunkGeneratorStructureState.class.getDeclaredField("levelSeed");seedField.setAccessible(true);
    var groups=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    for(var item:groups) {
      var group=item.getAsJsonObject();
      var config=group.getAsJsonObject("set");
      var placement=(RandomSpreadStructurePlacement)StructurePlacement.CODEC.parse(ops,config.get("placement")).getOrThrow();
      out.println("S");
      for(var point:group.getAsJsonArray("points")) {
        var p=point.getAsJsonArray();long seed=p.get(0).getAsLong();int x=p.get(1).getAsInt(),z=p.get(2).getAsInt();seedField.setLong(state,seed);
        var potential=placement.getPotentialStructureChunk(seed,x,z);var locate=placement.getLocatePos(potential);
        boolean frequency=placement.applyAdditionalChunkRestrictions(x,z,seed);
        boolean source=placement.isStructureChunk(state,x,z),candidate=placement.isStructureChunk(state,potential.x(),potential.z());
        out.printf("P %d %d %d %d %s %s %s %d %d %d%n",x,z,potential.x(),potential.z(),frequency,source,candidate,locate.getX(),locate.getY(),locate.getZ());
        var options=new ArrayList<com.google.gson.JsonObject>();for(var entry:config.getAsJsonArray("structures"))options.add(entry.getAsJsonObject());
        var order=new ArrayList<String>();
        if(options.size()==1)order.add(options.get(0).get("structure").getAsString());
        else {var random=new WorldgenRandom(new LegacyRandomSource(0));random.setLargeFeatureSeed(seed,x,z);int total=options.stream().mapToInt(v->v.get("weight").getAsInt()).sum();
          while(!options.isEmpty()){int choice=random.nextInt(total),index=0;for(var option:options){choice-=option.get("weight").getAsInt();if(choice<0)break;index++;}
            var selected=options.remove(index);total-=selected.get("weight").getAsInt();order.add(selected.get("structure").getAsString());}}
        out.println("O "+String.join(",",order));
      }
    }
  }
}
