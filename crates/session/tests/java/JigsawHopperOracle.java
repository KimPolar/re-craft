import java.nio.file.Path;
import java.util.TreeMap;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.HopperBlockEntity;
import net.minecraft.world.level.storage.TagValueInput;

public class JigsawHopperOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var registries = VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        var inputs = new TreeMap<String, CompoundTag>();
        try (var zip = new ZipFile(args[0])) {
            for (var entries = zip.entries(); entries.hasMoreElements();) {
                var entry = entries.nextElement();
                if (!entry.getName().startsWith("data/minecraft/structure/trial_chambers/") || !entry.getName().endsWith(".nbt")) continue;
                try (var input = zip.getInputStream(entry)) {
                    var blocks = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap()).getListOrEmpty("blocks");
                    for (int i = 0; i < blocks.size(); i++) {
                        var nbt = blocks.getCompoundOrEmpty(i).getCompoundOrEmpty("nbt");
                        if (nbt.getStringOr("id", "").equals("minecraft:hopper")) inputs.put(nbt.toString(), nbt);
                    }
                }
            }
        }
        if (inputs.size() != 1) throw new AssertionError("unexpected hopper templates: " + inputs.size());
        var missingCooldown = inputs.firstEntry().getValue().copy();
        missingCooldown.remove("TransferCooldown");
        inputs.put(missingCooldown.toString(), missingCooldown);
        var loot = missingCooldown.copy();
        loot.remove("Items");
        loot.putString("LootTable", "minecraft:chests/trial_chambers/corridor");
        loot.putInt("TransferCooldown", 8);
        inputs.put(loot.toString(), loot);
        int index = 0;
        for (var original : inputs.values()) {
            NbtIo.write(original, Path.of(args[1], "input-" + index + ".nbt"));
            long[] seeds = {0, 0x123456789abcdefL, -1};
            for (int seedIndex = 0; seedIndex < seeds.length; seedIndex++) {
                var nbt = original.copy();
                nbt.putLong("LootTableSeed", seeds[seedIndex]);
                var entity = new HopperBlockEntity(new BlockPos(3, 7, -2), Blocks.HOPPER.defaultBlockState());
                entity.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING, registries, nbt));
                NbtIo.write(entity.saveWithFullMetadata(registries), Path.of(args[1], "saved-" + index + "-" + seedIndex + ".nbt"));
            }
            index++;
        }
    }
}
