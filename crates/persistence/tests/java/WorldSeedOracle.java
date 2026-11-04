import java.io.*;
import java.nio.file.*;
import net.minecraft.world.level.levelgen.WorldOptions;

public class WorldSeedOracle {
    public static void main(String[] args) throws Exception {
        try (var input = new DataInputStream(new BufferedInputStream(Files.newInputStream(Path.of(args[0]))));
             var output = new DataOutputStream(new BufferedOutputStream(Files.newOutputStream(Path.of(args[1]))))) {
            int count = input.readInt();
            for (int i = 0; i < count; i++) {
                int length = input.readInt();
                char[] chars = new char[length];
                for (int j = 0; j < length; j++) chars[j] = input.readChar();
                var parsed = WorldOptions.parseSeed(new String(chars));
                output.writeBoolean(parsed.isPresent());
                output.writeLong(parsed.orElse(0));
            }
        }
    }
}
