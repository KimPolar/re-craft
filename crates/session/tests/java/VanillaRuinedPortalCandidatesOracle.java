import com.google.gson.JsonParser;
import java.nio.file.*;
import java.util.*;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.*;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.*;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.level.validation.DirectoryValidator;

public class VanillaRuinedPortalCandidatesOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        for (var block : List.of(Blocks.STONE, Blocks.DEEPSLATE, Blocks.NETHERRACK, Blocks.END_STONE))
            bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP));
        var registries = VanillaRegistries.createWorldLookup();
        var biomeLookup = registries.lookupOrThrow(Registries.BIOME);
        var repo = ServerPacksSource.createPackRepository(Path.of(args[0]), new DirectoryValidator(path -> false));
        MinecraftServer.configurePackRepository(repo,
                new WorldDataConfiguration(new DataPackConfig(List.of("vanilla"), List.of()), FeatureFlags.DEFAULT_FLAGS), false, false);
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var templateManager = (StructureTemplateManager) unsafe.allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var templates = new HashMap<Identifier, Optional<StructureTemplate>>();
        try (var zip = new ZipFile(args[2])) {
            for (int giant = 0; giant < 2; giant++) for (int n = 1; n <= (giant == 0 ? 10 : 3); n++) {
                String name = "ruined_portal/" + (giant == 0 ? "" : "giant_") + "portal_" + n;
                var entry = zip.getEntry("data/minecraft/structure/" + name + ".nbt");
                try (var input = zip.getInputStream(entry)) {
                    var data = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap());
                    var template = new StructureTemplate(); template.load(BuiltInRegistries.BLOCK, data);
                    templates.put(Identifier.withDefaultNamespace(name), Optional.of(template));
                }
            }
        }
        repository.set(templateManager, templates);
        try (var manager = new MultiPackResourceManager(PackType.SERVER_DATA, repo.openAllSelected())) {
            TagLoader.ElementLookup<Holder<Biome>> lookup = (id, required) ->
                    biomeLookup.get(ResourceKey.create(Registries.BIOME, id)).map(holder -> (Holder<Biome>) holder);
            var tags = TagLoader.loadTagsForRegistry(manager, Registries.BIOME, lookup);
            var generator = (NoiseBasedChunkGenerator) WorldPresets.getNormalOverworld(registries).generator();
            long seed = -1; var random = RandomState.create(registries.lookupOrThrow(Registries.NOISE), seed, generator.generatorSettings().value());
            var accessor = LevelHeightAccessor.create(-64, 384);
            var cases = JsonParser.parseString(Files.readString(Path.of(args[1]))).getAsJsonArray();
            for (var item : cases) {
                var obj = item.getAsJsonObject(); String id = obj.get("structure").getAsString();
                int x = obj.get("x").getAsInt(), z = obj.get("z").getAsInt();
                var structure = registries.lookupOrThrow(Registries.STRUCTURE)
                        .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse(id))).value();
                var allowed = structure.biomes().unwrapKey().map(key -> Set.copyOf(Objects.requireNonNull(tags.get(key))))
                        .orElseGet(() -> Set.copyOf(structure.biomes().stream().toList()));
                var climate = random.createClimateSampler(SamplerContext.builder().enableCaches().build());
                var ctx = new Structure.GenerationContext(null, generator, generator.getBiomeSource(), climate,
                        random, templateManager, seed, new ChunkPos(x, z), accessor, allowed::contains);
                var stub = structure.findValidGenerationPoint(ctx);
                out.println("P " + id + " " + x + " " + z + " " + (stub.isPresent() ? "S" : "X"));
            }
        }
    }
}
