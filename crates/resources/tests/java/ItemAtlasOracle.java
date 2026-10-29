import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Optional;
import java.util.Set;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.client.Minecraft;
import net.minecraft.client.Options;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.TextureFilteringMethod;
import net.minecraft.client.renderer.texture.SpriteLoader;
import net.minecraft.data.AtlasIds;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.packs.PackLocationInfo;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.PathPackResources;
import net.minecraft.server.packs.VanillaPackResourcesBuilder;
import net.minecraft.server.packs.repository.PackSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import java.util.List;

// Runs only in the ignored installed-Java test. No game window or world is opened.
public class ItemAtlasOracle {
    static void set(Object target, Class<?> type, String name, Object value) throws Exception {
        var field = type.getDeclaredField(name);
        field.setAccessible(true);
        field.set(target, value);
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        net.minecraft.client.renderer.texture.atlas.SpriteSources.bootstrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var client = (Minecraft) unsafe.allocateInstance(Minecraft.class);
        var options = (Options) unsafe.allocateInstance(Options.class);
        var filtering = (OptionInstance<?>) unsafe.allocateInstance(OptionInstance.class);
        set(filtering, OptionInstance.class, "value", TextureFilteringMethod.NONE);
        set(options, Options.class, "textureFiltering", filtering);
        set(client, Minecraft.class, "options", options);
        var instance = Minecraft.class.getDeclaredField("instance");
        instance.setAccessible(true);
        instance.set(null, client);

        var info = new PackLocationInfo("vanilla", Component.literal("Vanilla"), PackSource.BUILT_IN, Optional.empty());
        var vanilla = new VanillaPackResourcesBuilder().exposeNamespace("minecraft").pushJarResources().build(info);
        var resources = args.length > 1 && !args[1].equals("-")
            ? new MultiPackResourceManager(PackType.CLIENT_RESOURCES, List.of(vanilla.fullResources(),
                new PathPackResources(new PackLocationInfo("test", Component.literal("Test"), PackSource.BUILT_IN, Optional.empty()), Path.of(args[1]))))
            : vanilla.asResourceManager();
        var blocks = args.length > 2 && args[2].equals("blocks");
        int mip = args.length > 3 ? Integer.parseInt(args[3]) : 0;
        var loader = new SpriteLoader(Identifier.withDefaultNamespace(blocks ? "textures/atlas/blocks.png" : "textures/atlas/items.png"), 16384);
        var atlas = loader.loadAndStitch(resources, blocks ? AtlasIds.BLOCKS : AtlasIds.ITEMS, mip, Runnable::run, Set.of()).join();
        var output = new JsonObject();
        output.addProperty("width", atlas.width());
        output.addProperty("height", atlas.height());
        var sprites = new JsonObject();
        atlas.regions().forEach((id, sprite) -> {
            var values = new JsonArray();
            values.add(sprite.getX());
            values.add(sprite.getY());
            values.add(sprite.contents().width());
            values.add(sprite.contents().height());
            sprites.add(id.toString(), values);
        });
        output.add("sprites", sprites);
        Files.writeString(Path.of(args[0]), output.toString());
    }
}
