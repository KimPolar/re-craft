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
import net.minecraft.world.level.block.entity.TrialSpawnerBlockEntity;
import net.minecraft.world.level.storage.TagValueInput;

public class JigsawTrialSpawnerOracle {
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
                        if (nbt.getStringOr("id", "").equals("minecraft:trial_spawner")) inputs.put(nbt.toString(), nbt);
                    }
                }
            }
        }
        if (inputs.size() != 14) throw new AssertionError("unexpected trial-spawner templates: " + inputs.size());
        int index = 0;
        for (var input : inputs.values()) {
            var reporter = new ProblemReporter.Collector();
            var entity = new TrialSpawnerBlockEntity(new BlockPos(3, 7, -2), Blocks.TRIAL_SPAWNER.defaultBlockState());
            entity.loadWithComponents(TagValueInput.create(reporter, registries, input));
            if (!reporter.isEmpty()) throw new AssertionError("trial-spawner input failed to load: " + reporter.getReport());
            var saved = entity.saveWithFullMetadata(registries);
            NbtIo.write(input, Path.of(args[1], "input-" + index + ".nbt"));
            NbtIo.write(saved, Path.of(args[1], "saved-" + index + ".nbt"));
            System.out.println("INPUT " + input + " SAVED " + saved);
            index++;
        }
    }
}
