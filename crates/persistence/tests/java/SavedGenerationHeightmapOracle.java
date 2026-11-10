import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.IdMapper;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.Heightmap;

public class SavedGenerationHeightmapOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var biome = Holder.direct((Biome)((sun.misc.Unsafe)field.get(null)).allocateInstance(Biome.class));
        var ids = new IdMapper<Holder<Biome>>(); ids.add(biome);
        var factory = new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(ids), biome, null);
        var root = NbtIo.read(Path.of(args[0]));
        for (var value : (ListTag)root.get("cases")) {
            var test = (CompoundTag)value;
            var proto = new ProtoChunk(new ChunkPos(0,0), UpgradeData.EMPTY,
                LevelHeightAccessor.create(test.getIntOr("min_y",0),test.getIntOr("height",0)),factory,null);
            var source = test.getCompoundOrEmpty("source");
            var expected = test.getCompoundOrEmpty("expected");
            for (var type : Heightmap.Types.values()) {
                var raw = source.getLongArray(type.getSerializationKey());
                if (raw.isPresent()) proto.setHeightmap(type, raw.get());
            }
            int count = 0;
            for (var entry : proto.getHeightmaps()) {
                var values = expected.getIntArray(entry.getKey().getSerializationKey()).orElseThrow();
                if (values.length != 256) throw new AssertionError("invalid expected map");
                for (int z=0;z<16;z++) for (int x=0;x<16;x++)
                    if (entry.getValue().getFirstAvailable(x,z) != values[x+z*16]) throw new AssertionError("height mismatch: " + entry.getKey());
                count++;
            }
            if (count != expected.size()) throw new AssertionError("heightmap set differs");
        }
    }
}
