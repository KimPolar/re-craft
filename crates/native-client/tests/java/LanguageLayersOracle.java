import java.io.ByteArrayInputStream;
import java.lang.reflect.Proxy;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import net.minecraft.client.resources.language.ClientLanguage;
import net.minecraft.server.packs.resources.Resource;
import net.minecraft.server.packs.resources.ResourceManager;

public class LanguageLayersOracle {
    private static Resource resource(String json) {
        return new Resource(null, () -> new ByteArrayInputStream(json.getBytes(StandardCharsets.UTF_8)));
    }

    public static void main(String[] args) {
        Map<String, List<Resource>> layers = Map.of(
            "minecraft:lang/en_us.json", List.of(
                resource("{\"english\":\"English\",\"shared\":\"base\",\"fallback\":\"original\"}"),
                resource("{\"shared\":\"english pack\",\"pack_only\":\"present\"}")),
            "minecraft:lang/ko_kr.json", List.of(
                resource("{\"shared\":\"Korean\",\"korean_only\":\"retained\"}"),
                resource("{\"shared\":\"lower pack\",\"lower_only\":\"kept\"}"),
                resource("{\"shared\":\"higher pack\"}")),
            "example:lang/ko_kr.json", List.of(resource("{\"example.key\":\"custom namespace\"}"))
        );
        ResourceManager manager = (ResourceManager) Proxy.newProxyInstance(
            ResourceManager.class.getClassLoader(), new Class<?>[] {ResourceManager.class},
            (proxy, method, arguments) -> switch (method.getName()) {
                case "getNamespaces" -> new LinkedHashSet<>(List.of("example", "minecraft"));
                case "getResourceStack" -> layers.getOrDefault(arguments[0].toString(), List.of());
                default -> throw new UnsupportedOperationException(method.getName());
            });
        ClientLanguage language = ClientLanguage.loadFrom(manager, List.of("en_us", "ko_kr"), false);
        for (String key : List.of("english", "fallback", "pack_only", "korean_only", "lower_only", "shared", "example.key")) {
            System.out.println(key + "\t" + language.getOrDefault(key));
        }
    }
}
