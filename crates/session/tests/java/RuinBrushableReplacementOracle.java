import java.lang.reflect.Field;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.BrushableBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.ticks.*;
import sun.misc.Unsafe;

/** Run the real WorldGenRegion and ProtoChunk replacement methods. */
public class RuinBrushableReplacementOracle {
    static final class Level extends ServerLevel {
        int poiNotifications;
        Level() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public void updatePOIOnBlockStateChange(BlockPos p, BlockState old, BlockState next) {
            poiNotifications++;
        }
    }
    static final class Region extends WorldGenRegion {
        ProtoChunk chunk;
        Region() { super(null, null, null, null); }
        @Override public boolean ensureCanWrite(BlockPos p) { return true; }
        @Override public ChunkAccess getChunk(int x, int z) { return chunk; }
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field unsafeField = Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (Unsafe) unsafeField.get(null);
        var biome = Holder.direct((Biome) unsafe.allocateInstance(Biome.class));
        var biomes = new IdMapper<Holder<Biome>>(); biomes.add(biome);
        var palette = new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),
            Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(biomes), biome, null);
        var pos = new BlockPos(8, 8, 8);
        var other = new BlockPos(9, 8, 8);
        var sources = List.of(Blocks.SUSPICIOUS_SAND, Blocks.SUSPICIOUS_GRAVEL);
        var targets = List.of(Blocks.STONE_BRICKS, Blocks.MOSSY_STONE_BRICKS,
            Blocks.STONE_BRICK_STAIRS, Blocks.AIR);
        var levelField = WorldGenRegion.class.getDeclaredField("level"); levelField.setAccessible(true);
        for (Block source : sources) for (Block target : targets) for (boolean pending : List.of(false, true)) {
            var chunk = new ProtoChunk(new ChunkPos(0, 0), UpgradeData.EMPTY,
                LevelHeightAccessor.create(0, 16), palette, null);
            chunk.setPersistedStatus(ChunkStatus.FEATURES);
            var before = source.defaultBlockState();
            if (net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(before)
                    || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(target.defaultBlockState()))
                throw new AssertionError("unexpected POI state");
            chunk.setBlockState(pos, before, 2);
            chunk.setBlockState(other, before, 2);
            if (pending) {
                var tag = new CompoundTag(); tag.putInt("x", pos.getX()); tag.putInt("y", pos.getY());
                tag.putInt("z", pos.getZ()); tag.putString("id", "DUMMY"); chunk.setBlockEntityNbt(tag);
            } else {
                BlockEntity blockEntity = ((EntityBlock) source).newBlockEntity(pos, before);
                if (blockEntity == null) throw new AssertionError("missing brushable entity");
                chunk.setBlockEntity(blockEntity);
            }
            chunk.setBlockEntity(((EntityBlock) source).newBlockEntity(other, before));
            chunk.getBlockTicks().schedule(new ScheduledTick<>(source, pos, 2, 0L));
            var level = (Level) unsafe.allocateInstance(Level.class);
            var region = (Region) unsafe.allocateInstance(Region.class); region.chunk = chunk; levelField.set(region, level);
            var next = target.defaultBlockState();
            if (!region.setBlock(pos, next, 2, 512)) throw new AssertionError("write failed");
            var ticks = ((ProtoChunkTicks<Block>) chunk.getBlockTicks()).pack(0);
            if (ticks.size() != 1 || ticks.get(0).type() != source || !ticks.get(0).pos().equals(pos))
                throw new AssertionError("scheduled tick was not preserved");
            if (chunk.getBlockEntity(pos) != null || chunk.getBlockEntityNbt(pos) != null || chunk.getBlockEntity(other) == null)
                throw new AssertionError("brushable entity removal is wrong");
            if (chunk.getBlockState(pos) != next || level.poiNotifications != 1)
                throw new AssertionError("unexpected block write");
            out.printf("R %s %s %s %d %d %d%n", BuiltInRegistries.BLOCK.getKey(source),
                BuiltInRegistries.BLOCK.getKey(target), pending, ticks.size(), ticks.get(0).delay(),
                ticks.get(0).priority().getValue());
        }
        var lookup = VanillaRegistries.createWorldLookup();
        var table = ResourceKey.create(Registries.LOOT_TABLE, Identifier.withDefaultNamespace("archaeology/ocean_ruin_warm"));
        var pendingField = ChunkAccess.class.getDeclaredField("pendingBlockEntities"); pendingField.setAccessible(true);
        for (Block source : sources) for (Block target : sources) for (boolean pending : List.of(false, true)) {
            var chunk = new ProtoChunk(new ChunkPos(0, 0), UpgradeData.EMPTY,
                LevelHeightAccessor.create(0, 16), palette, null);
            chunk.setPersistedStatus(ChunkStatus.FEATURES);
            var before = source.defaultBlockState();
            chunk.setBlockState(pos, before, 2);
            chunk.setBlockState(other, before, 2);
            for (int x = 0; x < 12; x++) {
                var p = new BlockPos(x, 6, 0);
                chunk.setBlockState(p, Blocks.CHEST.defaultBlockState(), 2);
                var tag = new CompoundTag(); tag.putInt("x", x); tag.putInt("y", 6);
                tag.putInt("z", 0); tag.putString("id", "DUMMY"); chunk.setBlockEntityNbt(tag);
            }
            if (pending) {
                var tag = new CompoundTag(); tag.putInt("x", pos.getX()); tag.putInt("y", pos.getY());
                tag.putInt("z", pos.getZ()); tag.putString("id", "DUMMY"); chunk.setBlockEntityNbt(tag);
            } else {
                chunk.setBlockEntity(((EntityBlock) source).newBlockEntity(pos, before));
            }
            chunk.setBlockEntity(((EntityBlock) source).newBlockEntity(other, before));
            chunk.getBlockTicks().schedule(new ScheduledTick<>(source, pos, 2, 0L));
            var level = (Level) unsafe.allocateInstance(Level.class);
            var region = (Region) unsafe.allocateInstance(Region.class); region.chunk = chunk; levelField.set(region, level);
            var oldEntity = region.getBlockEntity(pos);
            if (!(oldEntity instanceof BrushableBlockEntity)) throw new AssertionError("old entity was not resolved");
            if (!region.setBlock(pos, Blocks.BARRIER.defaultBlockState(), 20, 512)) throw new AssertionError("barrier write failed");
            if (chunk.getBlockEntity(pos) != null || chunk.getBlockEntityNbt(pos) != null)
                throw new AssertionError("old entity survived barrier");
            var next = target.defaultBlockState();
            if (!region.setBlock(pos, next, 2, 512)) throw new AssertionError("new block write failed");
            var newEntity = region.getBlockEntity(pos);
            if (!(newEntity instanceof BrushableBlockEntity brushable) || newEntity == oldEntity)
                throw new AssertionError("new brushable entity was not materialized");
            brushable.setLootTable(table, 79L);
            var data = brushable.saveWithFullMetadata(lookup);
            if (!data.getStringOr("LootTable", "").equals("minecraft:archaeology/ocean_ruin_warm")
                    || data.getLongOr("LootTableSeed", 0L) != 79L)
                throw new AssertionError("new loot data differs");
            var ticks = ((ProtoChunkTicks<Block>) chunk.getBlockTicks()).pack(0);
            if (ticks.size() != 1 || ticks.get(0).type() != source || chunk.getBlockEntity(other) == null
                    || chunk.getBlockEntityNbt(pos) != null || chunk.getBlockState(pos) != next || level.poiNotifications != 2)
                throw new AssertionError("brushable replacement lost state");
            var pendingMap = (Map<BlockPos, CompoundTag>) pendingField.get(chunk);
            if (pendingMap.size() != 12) throw new AssertionError("other pending entities were lost");
            var order = pendingMap.keySet().stream().map(p -> p.getX()+","+p.getY()+","+p.getZ()).toList();
            out.printf("A %s %s %s %d %d %d %d %s%n", BuiltInRegistries.BLOCK.getKey(source),
                BuiltInRegistries.BLOCK.getKey(target), pending, ticks.size(), ticks.get(0).delay(),
                ticks.get(0).priority().getValue(), data.getLongOr("LootTableSeed", 0L), String.join(";", order));
        }
    }
}
