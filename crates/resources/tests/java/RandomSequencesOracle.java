import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.RandomSequences;

public class RandomSequencesOracle {
 public static void main(String[] args) {
  var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
  for(long seed:new long[]{0,1,-1,123456789L,Long.MIN_VALUE,Long.MAX_VALUE})
   for(int salt:new int[]{0,12345,-31,Integer.MIN_VALUE})
    for(boolean world:new boolean[]{false,true})for(boolean includeId:new boolean[]{false,true})
     for(String name:new String[]{"minecraft:chests/buried_treasure","example:other/path"}) {
      var sequences=new RandomSequences(); sequences.setSeedDefaults(salt,world,includeId);
      var random=sequences.get(Identifier.parse(name),seed);
      out.printf("C %d %d %s %s %s%n",seed,salt,world,includeId,name);
      out.printf("R %d %d %d %d%n",random.nextLong(),random.nextLong(),random.nextLong(),random.nextLong());
      out.println("J "+RandomSequences.CODEC.encodeStart(JsonOps.INSTANCE,sequences).getOrThrow());
      if(seed==0 && salt==0 && world && includeId)out.println("N "+RandomSequences.CODEC.encodeStart(NbtOps.INSTANCE,sequences).getOrThrow());
     }
 }
}
