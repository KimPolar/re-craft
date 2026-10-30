import java.nio.file.Path;
import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.TagKey;
import java.util.List;
import net.minecraft.world.LockCode;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;

public class ChestLockOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var world = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(world);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        // This standalone lookup has no data-pack tag reload. Bind the test member
        // explicitly so the vanilla predicate exercises a populated named tag.
        var planks = TagKey.create(Registries.ITEM, Identifier.parse("minecraft:planks"));
        var oak = (Holder.Reference<?>) BuiltInRegistries.ITEM.wrapAsHolder(Items.OAK_PLANKS);
        var bindTags = Holder.Reference.class.getDeclaredMethod("bindTags", java.util.Collection.class);
        bindTags.setAccessible(true);
        bindTags.invoke(oak, List.of(planks));
        var named = registries.lookupOrThrow(Registries.ITEM).getOrThrow(planks);
        var bind = HolderSet.Named.class.getDeclaredMethod("bind", List.class);
        bind.setAccessible(true);
        bind.invoke(named, List.of(oak));
        var jsonOps = registries.createSerializationContext(JsonOps.INSTANCE);
        var nbtOps = registries.createSerializationContext(NbtOps.INSTANCE);
        var cases = new String[] {
            "{}", "{\"items\":\"minecraft:diamond\"}",
            "{\"items\":[\"minecraft:diamond\",\"minecraft:stone\"]}",
            "{\"count\":3}", "{\"count\":{\"min\":2,\"max\":4}}",
            "{\"items\":\"minecraft:stone\",\"count\":{\"min\":1}}",
            "{\"count\":{\"max\":0}}", "{\"items\":\"minecraft:air\"}",
            "{\"items\":\"#minecraft:planks\"}"
        };
        var items = new ItemStack[] { ItemStack.EMPTY, new ItemStack(Items.DIAMOND, 1),
            new ItemStack(Items.DIAMOND, 3), new ItemStack(Items.STONE, 2), new ItemStack(Items.OAK_PLANKS, 1) };
        var rows = new ListTag();
        for (var input : cases) {
            var lock = LockCode.CODEC.parse(jsonOps, JsonParser.parseString(input)).getOrThrow();
            var row = new CompoundTag();
            row.put("lock", LockCode.CODEC.encodeStart(nbtOps, lock).getOrThrow());
            byte[] answers = new byte[items.length];
            for (int i = 0; i < items.length; i++) answers[i] = (byte)(lock.unlocksWith(items[i]) ? 1 : 0);
            row.putByteArray("answers", answers);
            rows.add(row);
        }
        var root = new CompoundTag();
        root.put("cases", rows);
        NbtIo.write(root, Path.of(args[0]));
    }
}
