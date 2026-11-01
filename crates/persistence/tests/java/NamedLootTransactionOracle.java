import com.google.gson.JsonParser;
import it.unimi.dsi.fastutil.objects.ObjectArrayList;
import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.RegistryOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.RandomSequence;
import net.minecraft.world.RandomSequences;
import net.minecraft.world.SimpleContainer;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.storage.loot.*;
import com.mojang.serialization.JsonOps;

public class NamedLootTransactionOracle {
    static final String NAME = "minecraft:chests/buried_treasure";
    static final long SEED = 987654321L;

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        Path world = Path.of(args[1]);
        Path dir = world.resolve("dimensions/minecraft/overworld/region");
        Path dataDir = world.resolve("data/minecraft");
        Path playerPath = world.resolve("players/data/00000000-0000-0000-0000-000000000001.dat");
        boolean existing = Boolean.parseBoolean(args[3]);
        if (args[0].equals("prepare")) {
            RegionWriteOracle.main(new String[]{"seedBlockEntity", world.toString()});
            RegionWriteOracle.main(new String[]{"seedTransferPlayer", world.toString()});
            CompoundTag chunk;
            try (var region = RegionWriteOracle.open(dir); var in = region.getChunkDataInputStream(new ChunkPos(0, 0))) {
                chunk = NbtIo.read((DataInput)in);
            }
            var chest = chunk.getListOrEmpty("block_entities").getCompoundOrEmpty(0);
            chest.remove("Items");
            chest.putString("LootTable", NAME);
            try (var region = RegionWriteOracle.open(dir); var out = region.getChunkDataOutputStream(new ChunkPos(0, 0))) {
                NbtIo.write(chunk, (DataOutput)out);
            }
            Files.createDirectories(dataDir);
            var settings = new CompoundTag(); settings.putInt("DataVersion", 5023);
            var settingsData = new CompoundTag(); settingsData.putLong("seed", SEED); settings.put("data", settingsData);
            NbtIo.writeCompressed(settings, dataDir.resolve("world_gen_settings.dat"));
            if (existing) {
                var initial = new CompoundTag(); initial.putInt("salt", 0); initial.put("sequences", new CompoundTag());
                var sequences = RandomSequences.CODEC.parse(NbtOps.INSTANCE, initial).getOrThrow();
                sequences.get(Identifier.parse(NAME), SEED).nextLong();
                var document = new CompoundTag(); document.putInt("DataVersion", 5023);
                document.put("data", RandomSequences.CODEC.encodeStart(NbtOps.INSTANCE, sequences).getOrThrow());
                NbtIo.writeCompressed(document, dataDir.resolve("random_sequences.dat"));
            }
            System.out.println("PREPARED");
            return;
        }
        var table = LootTable.DIRECT_CODEC.parse(RegistryOps.create(JsonOps.INSTANCE, registries),
            JsonParser.parseString(Files.readString(Path.of(args[2])))).getOrThrow();
        RandomSource random = new RandomSequence(SEED, Identifier.parse(NAME)).random();
        if (existing) random.nextLong();
        var params = new LootParams(null, ContextMap.EMPTY, Map.of(), 0.0f);
        var ctor = LootContext.class.getDeclaredConstructors()[0]; ctor.setAccessible(true);
        var context = (LootContext)ctor.newInstance(params, random, registries);
        var items = new ObjectArrayList<ItemStack>(); table.getRandomItemsRaw(context, items::add);
        var expected = new SimpleContainer(27);
        var availableMethod = LootTable.class.getDeclaredMethod("getAvailableSlots", net.minecraft.world.Container.class, RandomSource.class);
        var splitMethod = LootTable.class.getDeclaredMethod("shuffleAndSplitItems", ObjectArrayList.class, int.class, RandomSource.class);
        availableMethod.setAccessible(true); splitMethod.setAccessible(true);
        @SuppressWarnings("unchecked") var available = (List<Integer>)availableMethod.invoke(table, expected, random);
        splitMethod.invoke(table, items, available.size(), random);
        for (var item : items) { if (available.isEmpty()) break; expected.setItem(available.remove(available.size()-1), item); }
        int moved = 0; while (moved < 27 && expected.getItem(moved).isEmpty()) moved++;
        if (moved == 27) throw new AssertionError("empty generated loot");
        CompoundTag chunk;
        try (var region = RegionWriteOracle.open(dir); var in = region.getChunkDataInputStream(new ChunkPos(0, 0))) {
            chunk = NbtIo.read((DataInput)in);
        }
        var tags = chunk.getListOrEmpty("block_entities");
        var tag = tags.getCompoundOrEmpty(0);
        if (tag.contains("LootTable") || tag.contains("LootTableSeed") || tag.getLongOr("future_data", 0) != 123)
            throw new AssertionError("loot tags or unknown chest data differ");
        var block = BlockEntity.loadStatic(new BlockPos(0,0,0), Blocks.CHEST.defaultBlockState(), tag, registries);
        if (!(block instanceof ChestBlockEntity chest)) throw new AssertionError("not a chest");
        for (int slot = 0; slot < 27; slot++) {
            var want = slot == moved ? ItemStack.EMPTY : expected.getItem(slot);
            var actual = chest.getItem(slot);
            if (want.getCount() != actual.getCount() || !ItemStack.isSameItemSameComponents(want, actual))
                throw new AssertionError("chest slot " + slot + " differs: " + want + " / " + actual);
        }
        var player = NbtIo.readCompressed(playerPath, NbtAccounter.unlimitedHeap());
        var inventory = player.getListOrEmpty("Inventory");
        if (inventory.size() != 1 || player.getLongOr("future_player", 0) != 55) throw new AssertionError("player metadata");
        var movedStack = ItemStack.CODEC.parse(RegistryOps.create(NbtOps.INSTANCE, registries), inventory.getCompoundOrEmpty(0)).getOrThrow();
        if (movedStack.getCount() != expected.getItem(moved).getCount() || !ItemStack.isSameItemSameComponents(movedStack, expected.getItem(moved)))
            throw new AssertionError("player stack differs");
        var seqDocument = NbtIo.readCompressed(dataDir.resolve("random_sequences.dat"), NbtAccounter.unlimitedHeap());
        var sequences = RandomSequences.CODEC.parse(NbtOps.INSTANCE, seqDocument.get("data")).getOrThrow();
        var savedRandom = sequences.get(Identifier.parse(NAME), SEED);
        for (int i = 0; i < 3; i++) if (random.nextLong() != savedRandom.nextLong()) throw new AssertionError("sequence differs");
        chest.setItem(moved, movedStack);
        var restored = chest.saveWithFullMetadata(registries); restored.putLong("future_data", 123);
        tags.set(0, restored); player.put("Inventory", new ListTag());
        try (var region = RegionWriteOracle.open(dir); var out = region.getChunkDataOutputStream(new ChunkPos(0, 0))) {
            NbtIo.write(chunk, (DataOutput)out);
        }
        NbtIo.writeCompressed(player, playerPath);
        System.out.println("VERIFIED " + moved);
    }
}
