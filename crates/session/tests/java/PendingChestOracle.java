import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.block.state.properties.ChestType;
import net.minecraft.world.level.chunk.*;

public class PendingChestOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var registries = VanillaRegistries.createWorldLookup();
        var biome = registries.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.PLAINS);
        var ids = new IdMapper<Holder<net.minecraft.world.level.biome.Biome>>();
        ids.add(biome);
        var palette = new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),
                Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(ids), biome, null);
        var chunk = new ProtoChunk(new ChunkPos(0,0), UpgradeData.EMPTY,
                LevelHeightAccessor.create(0,16), palette, null);
        int index = 0;
        for (var facing : new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.EAST,Direction.WEST}) {
            for (var type : ChestType.values()) for (boolean waterlogged : new boolean[]{false,true}) {
                var pos = new BlockPos(index % 16, 8, index / 16);
                var state = Blocks.CHEST.defaultBlockState().setValue(ChestBlock.FACING, facing)
                        .setValue(ChestBlock.TYPE, type).setValue(ChestBlock.WATERLOGGED, waterlogged);
                chunk.setBlockState(pos, state, 3);
                var dummy = new CompoundTag();
                dummy.putString("id", "DUMMY");
                dummy.putInt("x", pos.getX()); dummy.putInt("y", pos.getY()); dummy.putInt("z", pos.getZ());
                chunk.setBlockEntityNbt(dummy);
                if (chunk.getBlockEntity(pos) != null || !dummy.equals(chunk.getBlockEntityNbtForSaving(pos, registries)))
                    throw new AssertionError("pending chest changed before access");
                var entity = new ChestBlockEntity(pos, state);
                chunk.setBlockEntity(entity);
                if (chunk.getBlockEntityNbt(pos) != null || chunk.getBlockEntity(pos) != entity)
                    throw new AssertionError("pending chest did not resolve");
                var saved = chunk.getBlockEntityNbtForSaving(pos, registries);
                NbtIo.write(saved, Path.of(args[0]).resolve(index + ".nbt"));
                if (index == 0) out.println("NBT " + saved);
                out.printf("P %d %s %s %s %d %d %d%n", index, facing.getName(), type.getSerializedName(), waterlogged,
                        pos.getX(), pos.getY(), pos.getZ());
                index++;
            }
        }
        out.print("O");
        for (var pos : chunk.getBlockEntitiesPos()) out.printf(" %d,%d,%d", pos.getX(), pos.getY(), pos.getZ());
        out.println();
    }
}
