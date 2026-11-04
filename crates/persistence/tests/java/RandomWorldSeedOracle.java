import java.io.*;
import java.nio.file.*;
import net.minecraft.util.RandomSource;

public class RandomWorldSeedOracle {
    public static void main(String[] args) throws Exception {
        try (var input = new DataInputStream(new BufferedInputStream(Files.newInputStream(Path.of(args[0]))));
             var output = new DataOutputStream(new BufferedOutputStream(Files.newOutputStream(Path.of(args[1]))))) {
            long uniquifier = 8682522807148012L;
            int count = input.readInt();
            for (int i = 0; i < count; i++) {
                uniquifier *= 1181783497276652981L;
                long clock = input.readLong();
                output.writeLong(RandomSource.create(uniquifier ^ clock).nextLong());
            }
        }
    }
}
