import com.mojang.serialization.Codec;
import java.io.DataInput;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Objects;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.level.chunk.status.ChunkStatus;

/** Read and rewrite a disposable resumed light draft with Java 26.3. */
public class ResumedLightOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup().lookupOrThrow(Registries.BIOME);
        Holder<Biome> defaultBiome = lookup.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse("minecraft:plains")));
        var ids = new IdMapper<Holder<Biome>>();
        lookup.listElements().forEach(ids::add);
        var blockStrategy = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        var biomeStrategy = Strategy.createForBiomes(ids);
        Codec<Holder<Biome>> biomeCodec = Codec.STRING.xmap(
            name -> lookup.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse(name))),
            biome -> biome.unwrapKey().orElseThrow().identifier().toString());
        var blocksCodec = PalettedContainer.codecRW(BlockState.CODEC, blockStrategy, Blocks.AIR.defaultBlockState());
        var biomesCodec = PalettedContainer.codecRO(biomeCodec, biomeStrategy, defaultBiome);
        var factory = new PalettedContainerFactory(blockStrategy, Blocks.AIR.defaultBlockState(),
            blocksCodec, biomeStrategy, defaultBiome, biomesCodec);
        CompoundTag original;
        try (var input = new java.io.DataInputStream(Files.newInputStream(Path.of(args[0])))) {
            original = NbtIo.read((DataInput) input);
        }
        original.putString("Status", "minecraft:light");
        var height = LevelHeightAccessor.create(Integer.parseInt(args[1]), Integer.parseInt(args[2]));
        var parsed = SerializableChunkData.parse(height, factory, original);
        if (parsed == null || parsed.chunkStatus() != ChunkStatus.LIGHT) throw new AssertionError("status");
        var rewritten = parsed.write();
        var source = original.getListOrEmpty("sections");
        var output = rewritten.getListOrEmpty("sections");
        int checked = 0;
        for (int i = 0; i < source.size(); i++) {
            var before = source.getCompoundOrEmpty(i);
            byte y = before.getByteOr("Y", (byte) 127);
            CompoundTag after = null;
            for (int j = 0; j < output.size(); j++) {
                var candidate = output.getCompoundOrEmpty(j);
                if (candidate.getByteOr("Y", (byte) 127) == y) { after = candidate; break; }
            }
            if (after == null) throw new AssertionError("section missing: " + y);
            for (var key : new String[]{"BlockLight", "SkyLight"}) {
                if (!Objects.equals(before.get(key), after.get(key))) throw new AssertionError(key + " changed at " + y);
                if (before.get(key) != null) checked++;
            }
        }
        if (checked == 0) throw new AssertionError("no light layers checked");
    }
}
