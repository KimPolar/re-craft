import com.google.gson.JsonParser;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BrushableBlockEntity;
import net.minecraft.world.level.storage.TagValueInput;

public class BrushableBlockEntityOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var tests = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for (var entry : tests) {
            var test = entry.getAsJsonObject();
            var state = (test.get("sand").getAsBoolean() ? Blocks.SUSPICIOUS_SAND : Blocks.SUSPICIOUS_GRAVEL).defaultBlockState();
            var block = new BrushableBlockEntity(new BlockPos(test.get("x").getAsInt(), test.get("y").getAsInt(), test.get("z").getAsInt()), state);
            var data = new CompoundTag();
            data.putString("LootTable", test.get("table").getAsString());
            data.putLong("LootTableSeed", test.get("seed").getAsLong());
            block.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING, lookup, data));
            var actual = block.saveWithFullMetadata(lookup);
            var expected = NbtIo.read(Path.of(test.get("path").getAsString()));
            if (!actual.equals(expected)) throw new AssertionError("case=" + test + " java=" + actual + " rust=" + expected);
        }
        out.println("OK\t" + tests.size());
    }
}
