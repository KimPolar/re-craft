import java.nio.file.Path;
import java.util.TreeMap;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.DecoratedPotBlockEntity;
import net.minecraft.world.level.storage.TagValueInput;

public class JigsawDecoratedPotOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var components = DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE, 64).build();
        for (var id : new String[]{"brick", "string", "flow_pottery_sherd", "guster_pottery_sherd", "scrape_pottery_sherd"})
            BuiltInRegistries.ITEM.getValue(Identifier.parse("minecraft:" + id)).builtInRegistryHolder().bindComponents(components);
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        var inputs = new TreeMap<String, CompoundTag>();
        try (var zip = new ZipFile(args[0])) {
            for (var entries = zip.entries(); entries.hasMoreElements();) {
                var entry = entries.nextElement();
                if (!entry.getName().startsWith("data/minecraft/structure/trial_chambers/") || !entry.getName().endsWith(".nbt")) continue;
                try (var input = zip.getInputStream(entry)) {
                    var blocks = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap()).getListOrEmpty("blocks");
                    for (int i = 0; i < blocks.size(); i++) {
                        var nbt = blocks.getCompoundOrEmpty(i).getCompoundOrEmpty("nbt");
                        if (nbt.getStringOr("id", "").equals("minecraft:decorated_pot")) inputs.put(nbt.toString(), nbt);
                    }
                }
            }
        }
        if (inputs.size() != 5) throw new AssertionError("unexpected decorated-pot templates: " + inputs.size());
        int index = 0;
        for (var original : inputs.values()) {
            if (args.length > 1) NbtIo.write(original, Path.of(args[1], "input-" + index + ".nbt"));
            long[] seeds = {0, 0x123456789abcdefL, -1};
            for (int seedIndex = 0; seedIndex < seeds.length; seedIndex++) {
                var nbt = original.copy();
                nbt.putLong("LootTableSeed", seeds[seedIndex]);
                var entity = new DecoratedPotBlockEntity(new BlockPos(3, 7, -2), Blocks.DECORATED_POT.defaultBlockState());
                entity.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING, registries, nbt));
                var saved = entity.saveWithFullMetadata(registries);
                if (args.length > 1) NbtIo.write(saved, Path.of(args[1], "saved-" + index + "-" + seedIndex + ".nbt"));
            }
            index++;
        }
    }
}
