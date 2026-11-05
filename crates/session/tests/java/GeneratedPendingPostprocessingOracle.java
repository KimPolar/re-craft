import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.chunk.*;

public class GeneratedPendingPostprocessingOracle {
    static PalettedContainerFactory factory;
    static class FakeLevel extends ServerLevel {
        FakeLevel() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 16; }
        @Override public PalettedContainerFactory palettedContainerFactory() { return factory; }
        @Override public RegistryAccess registryAccess() { return RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY); }
    }
    @SuppressWarnings("unchecked")
    static Map<BlockPos, CompoundTag> pending(LevelChunk chunk) throws Exception {
        var field = ChunkAccess.class.getDeclaredField("pendingBlockEntities");
        field.setAccessible(true);
        return (Map<BlockPos, CompoundTag>) field.get(chunk);
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries = VanillaRegistries.createWorldLookup();
        var biome = registries.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.PLAINS);
        var ids = new IdMapper<Holder<net.minecraft.world.level.biome.Biome>>(); ids.add(biome);
        factory = new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),
            Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(ids), biome, null);
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var level = (FakeLevel)unsafe.allocateInstance(FakeLevel.class);
        var proto = new ProtoChunk(new ChunkPos(0,0), UpgradeData.EMPTY,
            LevelHeightAccessor.create(0,16), factory, null);
        for (int i=0; i<24; i++) {
            var pos = new BlockPos(i%16,8,i/16);
            proto.setBlockState(pos, Blocks.CHEST.defaultBlockState(), 3);
            var dummy = new CompoundTag(); dummy.putString("id", "DUMMY");
            dummy.putInt("x",pos.getX()); dummy.putInt("y",pos.getY()); dummy.putInt("z",pos.getZ());
            proto.setBlockEntityNbt(dummy);
        }
        for (int i=0; i<24; i+=2) {
            var pos = new BlockPos(i%16,8,i/16);
            proto.setBlockEntity(new ChestBlockEntity(pos, Blocks.CHEST.defaultBlockState()));
        }
        var full = new LevelChunk(level, proto, null);
        out.print("P");
        for (var pos : pending(full).keySet()) out.printf(" %d,%d,%d",pos.getX(),pos.getY(),pos.getZ());
        out.println();
        full.postProcessGeneration(level);
        if (!pending(full).isEmpty()) throw new AssertionError("pending entities remain");
        out.print("O");
        for (var pos : full.getBlockEntitiesPos()) {
            out.printf(" %d,%d,%d",pos.getX(),pos.getY(),pos.getZ());
            int index = pos.getX() + pos.getZ()*16;
            NbtIo.write(full.getBlockEntityNbtForSaving(pos, registries), Path.of(args[0]).resolve(index+".nbt"));
        }
        out.println();
    }
}
