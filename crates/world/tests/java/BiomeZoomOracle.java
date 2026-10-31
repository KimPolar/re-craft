import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.world.level.biome.BiomeManager;

public class BiomeZoomOracle {
  public static void main(String[] args) throws Exception {
    var output=new JsonArray(); var random=new Random(92017);
    for(long seed:new long[]{0,1,-1,123456789,-987654321,Long.MIN_VALUE,Long.MAX_VALUE}) {
      long zoom=BiomeManager.obfuscateSeed(seed); int[] selected=new int[3];
      var manager=new BiomeManager((x,y,z)->{selected[0]=x; selected[1]=y; selected[2]=z; return null;},zoom);
      for(int i=0;i<500;i++) {
        int x=random.nextInt(), y=random.nextInt(), z=random.nextInt();
        if(i<6) { int[] values={Integer.MIN_VALUE,Integer.MIN_VALUE+1,-1,0,Integer.MAX_VALUE-1,Integer.MAX_VALUE}; x=values[i]; y=values[5-i]; z=values[(i+3)%6]; }
        manager.getBiome(x,y,z);
        var row=new JsonArray(); row.add(seed); row.add(zoom); row.add(x); row.add(y); row.add(z);
        row.add(selected[0]); row.add(selected[1]); row.add(selected[2]); output.add(row);
      }
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(output));
  }
}
