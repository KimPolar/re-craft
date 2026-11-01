import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.RandomSequences;

public class RandomSequencesNbtOracle {
 public static void main(String[] args) throws Exception {
  var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
  var data=NbtIo.read(Path.of(args[0])).get("data");
  var sequences=RandomSequences.CODEC.parse(NbtOps.INSTANCE,data).getOrThrow();
  var random=sequences.get(Identifier.parse(args[1]),Long.parseLong(args[2]));
  out.printf("R %d %d %d%n",random.nextLong(),random.nextLong(),random.nextLong());
 }
}
