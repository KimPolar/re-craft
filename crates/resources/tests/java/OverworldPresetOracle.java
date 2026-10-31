import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList;

public class OverworldPresetOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var list=MultiNoiseBiomeSourceParameterList.knownPresets().get(MultiNoiseBiomeSourceParameterList.Preset.OVERWORLD);
    for(var entry:list.values()) {
      var p=entry.getFirst();
      var a=new net.minecraft.world.level.biome.Climate.Parameter[]{p.temperature(),p.humidity(),p.continentalness(),p.erosion(),p.depth(),p.weirdness()};
      out.print("P");for(var value:a)out.printf(" %d %d",value.min(),value.max());
      out.printf(" %d %s%n",p.offset(),entry.getSecond().identifier());
    }
  }
}
