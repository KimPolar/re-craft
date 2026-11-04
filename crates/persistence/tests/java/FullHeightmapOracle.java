import java.io.DataInput;
import java.io.DataInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.IdMapper;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.Strategy;
import net.minecraft.world.level.chunk.UpgradeData;
import net.minecraft.world.level.levelgen.Heightmap;

public class FullHeightmapOracle {
    static PalettedContainerFactory factory;
    static int minY, height;
    static class TestLevel extends ServerLevel {
        TestLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getMinY() { return minY; }
        @Override public int getHeight() { return height; }
        @Override public PalettedContainerFactory palettedContainerFactory() { return factory; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var biome = Holder.direct((Biome) unsafe.allocateInstance(Biome.class));
        var ids = new IdMapper<Holder<Biome>>();
        ids.add(biome);
        factory = new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(ids), biome, null);
        var level = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        CompoundTag root;
        try (var stream = new DataInputStream(Files.newInputStream(Path.of(args[0])))) {
            root = NbtIo.read((DataInput) stream);
        }
        var cases = (ListTag) root.get("cases");
        for (var tag : cases) {
            var test = (CompoundTag) tag;
            minY = test.getIntOr("min_y", 0);
            height = test.getIntOr("height", 0);
            var source = test.getCompoundOrEmpty("source");
            var expected = test.getCompoundOrEmpty("expected");
            var proto = new ProtoChunk(new ChunkPos(0, 0), UpgradeData.EMPTY, LevelHeightAccessor.create(minY, height), factory, null);
            for (var type : Heightmap.Types.values()) {
                var raw = source.getLongArray(type.getSerializationKey());
                if (raw.isPresent()) proto.setHeightmap(type, raw.get());
            }
            var full = new LevelChunk(level, proto, null);
            int count = 0;
            for (var entry : full.getHeightmaps()) {
                var name = entry.getKey().getSerializationKey();
                var raw = expected.getLongArray(name).orElseThrow();
                if (!Arrays.equals(raw, entry.getValue().getRawData())) throw new AssertionError("heightmap differs: " + name);
                count++;
            }
            if (count != 4 || expected.size() != 4) throw new AssertionError("expected four final heightmaps");
        }
    }
}
