import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.util.FileUtil;
import net.minecraft.util.DirectoryLock;

public class WorldFolderOracle {
    public static void main(String[] args) throws Exception {
        if (args[0].equals("--lock")) {
            Files.writeString(Path.of(args[2]), Boolean.toString(DirectoryLock.isLocked(Path.of(args[1]))));
            return;
        }
        Path saves = Path.of(args[0]);
        var cases = JsonParser.parseString(Files.readString(Path.of(args[1]))).getAsJsonArray();
        var results = new JsonArray();
        for (var value : cases) {
            String name = value.getAsString().trim();
            if (name.isEmpty()) name = "New World";
            try {
                results.add(FileUtil.findAvailableName(saves, name, ""));
            } catch (Exception exception) {
                results.add(FileUtil.findAvailableName(saves, "World", ""));
            }
        }
        Files.writeString(Path.of(args[2]), results.toString());
    }
}
