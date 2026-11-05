import java.nio.file.Path;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;

public class StructureTemplateOracle {
    public static void main(String[] args) throws Exception {
        var output = Path.of(args[0]);
        for (int i = 1; i < args.length; i++) {
            var path = "data/minecraft/structure/" + args[i] + ".nbt";
            try (var stream = StructureTemplateOracle.class.getClassLoader().getResourceAsStream(path)) {
                if (stream == null) throw new AssertionError("missing " + path);
                var tag = NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap());
                NbtIo.write(tag, output.resolve((i - 1) + ".nbt"));
            }
        }
    }
}
