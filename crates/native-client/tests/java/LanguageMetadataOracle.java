import com.google.gson.Gson;
import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.TreeMap;
import net.minecraft.client.resources.language.LanguageInfo;
import net.minecraft.client.resources.metadata.language.LanguageMetadataSection;

public class LanguageMetadataOracle {
    public static void main(String[] args) throws Exception {
        Map<String, LanguageInfo> languages = new TreeMap<>();
        for (String path : args) {
            try {
                var root = JsonParser.parseString(Files.readString(Path.of(path))).getAsJsonObject();
                var section = LanguageMetadataSection.CODEC.parse(JsonOps.INSTANCE, root.get("language")).getOrThrow();
                section.languages().forEach(languages::putIfAbsent);
            } catch (RuntimeException ignored) {
                // LanguageManager ignores invalid metadata sections individually.
            }
        }
        System.out.write(new Gson().toJson(languages).getBytes(StandardCharsets.UTF_8));
    }
}
