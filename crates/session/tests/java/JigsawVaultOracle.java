import java.util.TreeMap;
import java.util.zip.ZipFile;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.vault.VaultBlockEntity;
import net.minecraft.world.level.storage.TagValueInput;

public class JigsawVaultOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var itemComponents = DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE, 64).build();
        Items.TRIAL_KEY.builtInRegistryHolder().bindComponents(itemComponents);
        Items.OMINOUS_TRIAL_KEY.builtInRegistryHolder().bindComponents(itemComponents);
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
                        if (nbt.getStringOr("id", "").equals("minecraft:vault")) inputs.put(nbt.toString(), nbt);
                    }
                }
            }
        }
        if (inputs.size() != 2) throw new AssertionError("unexpected vault templates: " + inputs.size());
        for (var nbt : inputs.values()) {
            var entity = new VaultBlockEntity(new BlockPos(3, 7, -2), Blocks.VAULT.defaultBlockState());
            entity.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING, registries, nbt));
            System.out.println("INPUT\t" + nbt);
            var saved = entity.saveWithFullMetadata(registries);
            System.out.println("SAVED\t" + saved);
            if (args.length > 1) {
                boolean ominous = nbt.toString().contains("ominous_trial_key");
                NbtIo.write(saved, Path.of(args[1], ominous ? "ominous.nbt" : "normal.nbt"));
            }
        }
    }
}
