import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.RandomSequences;

public class FreshRandomSequencesOracle {
    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var root = NbtIo.readCompressed(Path.of(args[0]), NbtAccounter.unlimitedHeap());
        var sequences = RandomSequences.CODEC.parse(NbtOps.INSTANCE, root.getCompoundOrEmpty("data")).getOrThrow();
        var random = sequences.get(Identifier.parse(args[1]), Long.parseLong(args[2]));
        output.println("RESULT " + random.nextLong());
    }
}
