import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.resources.RegistryOps;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.damagesource.DamageTypes;
import net.minecraft.world.item.component.DamageResistant;
import net.minecraft.tags.TagKey;
import net.minecraft.resources.Identifier;
import net.minecraft.core.Holder;

public class SavedItemOracle {
    static class TestItem extends ItemEntity {
        ItemStack item;
        TestItem() { super(null, 0, 0, 0, ItemStack.EMPTY); }
        @Override public void setItem(ItemStack item) { this.item = item; }
        @Override public ItemStack getItem() { return item; }
        void loadExtra(ValueInput input) { super.readAdditionalSaveData(input); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var world = VanillaRegistries.createWorldLookup();
        var registry = VanillaRegistries.createReloadableLookup(world);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var age = ItemEntity.class.getDeclaredField("age"); age.setAccessible(true);
        var delay = ItemEntity.class.getDeclaredField("pickupDelay"); delay.setAccessible(true);
        var health = ItemEntity.class.getDeclaredField("health"); health.setAccessible(true);
        var owner = ItemEntity.class.getDeclaredField("target"); owner.setAccessible(true);
        Tag[] values = {ShortTag.valueOf((short)-32768), ShortTag.valueOf((short)32767),
            IntTag.valueOf(65535), LongTag.valueOf(65538), FloatTag.valueOf(-1.25f),
            DoubleTag.valueOf(8.75), StringTag.valueOf("invalid")};
        JsonArray out = new JsonArray();
        for (int i=0; i<values.length; i++) {
            CompoundTag tag = new CompoundTag();
            tag.putString("id", "minecraft:item");
            tag.putIntArray("UUID", new int[]{1,2,3,4});
            tag.putIntArray("Owner", new int[]{9,8,7,6});
            ListTag pos = new ListTag(); for(double v:new double[]{1.25,64,-2.5}) pos.add(DoubleTag.valueOf(v));
            tag.put("Pos", pos);
            ListTag motion = new ListTag(); for(double v:new double[]{0.125,-0.25,15}) motion.add(DoubleTag.valueOf(v));
            tag.put("Motion", motion);
            tag.put("Age", values[i]); tag.put("PickupDelay", values[i]); tag.put("Health", values[i]);
            tag.put("Fire", values[i]); tag.put("Invulnerable", values[i]); tag.put("invulnerable_time", values[i]);
            var stack = new ItemStack(Items.DIAMOND_SWORD, 1);
            stack.set(net.minecraft.core.component.DataComponents.DAMAGE, 17);
            tag.put("Item", ItemStack.CODEC.encodeStart(NbtOps.INSTANCE, stack).getOrThrow());
            tag.putLong("unknown_plugin_field", 987654321L);
            var item = (TestItem) unsafe.allocateInstance(TestItem.class);
            var input = TagValueInput.create(ProblemReporter.DISCARDING, registry, tag);
            item.loadExtra(input);
            var row = new JsonObject(); row.addProperty("age", age.getInt(item));
            row.addProperty("delay", delay.getInt(item)); row.addProperty("health", health.getInt(item));
            row.addProperty("fire", input.getShortOr("Fire", (short)0));
            row.addProperty("invulnerable", input.getBooleanOr("Invulnerable", false));
            row.addProperty("invulnerable_time", input.getIntOr("invulnerable_time", 0));
            row.addProperty("owner_matches", new java.util.UUID((9L<<32)|8L, (7L<<32)|6L).equals(owner.get(item)));
            row.addProperty("item", BuiltInRegistries.ITEM.getKey(item.getItem().getItem()).toString());
            row.addProperty("damage", item.getItem().getDamageValue()); out.add(row);
            NbtIo.write(tag, Path.of(args[0], "item-"+i+".nbt"));
        }
        Files.writeString(Path.of(args[0], "expected.json"), new Gson().toJson(out));
        var ops = RegistryOps.create(NbtOps.INSTANCE, registry);
        var lava = registry.lookupOrThrow(Registries.DAMAGE_TYPE).getOrThrow(DamageTypes.LAVA);
        var fireTag = TagKey.create(Registries.DAMAGE_TYPE, Identifier.parse("minecraft:is_fire"));
        var bindTags = Holder.Reference.class.getDeclaredMethod("bindTags", java.util.Collection.class);
        bindTags.setAccessible(true); bindTags.invoke(lava, java.util.List.of(fireTag));
        ListTag lavaList = new ListTag(); lavaList.add(StringTag.valueOf("minecraft:lava"));
        ListTag fallList = new ListTag(); fallList.add(StringTag.valueOf("minecraft:fall"));
        ListTag unknownList = new ListTag(); unknownList.add(StringTag.valueOf("example:unknown"));
        Tag[] types = {lavaList, fallList, new ListTag(), StringTag.valueOf("minecraft:lava"), IntTag.valueOf(5),
            StringTag.valueOf("example:unknown"), unknownList, StringTag.valueOf("#minecraft:is_fire")};
        JsonArray resistance = new JsonArray();
        for (int i=0;i<types.length;i++) {
            var component = new CompoundTag(); component.put("types",types[i]);
            var parsed = DamageResistant.CODEC.parse(ops,component).result();
            var row = new JsonObject(); row.addProperty("valid",parsed.isPresent());
            if (parsed.isPresent()) row.addProperty("lava",parsed.get().types().contains(lava));
            resistance.add(row); NbtIo.write(component,Path.of(args[0],"resistance-"+i+".nbt"));
        }
        Files.writeString(Path.of(args[0], "resistance.json"),new Gson().toJson(resistance));
        if (args.length > 1) {
            var in = new java.io.DataInputStream(Files.newInputStream(Path.of(args[1])));
            var tag = NbtIo.read((java.io.DataInput)in); in.close();
            var input = TagValueInput.create(ProblemReporter.DISCARDING, registry, tag);
            var item = (TestItem) unsafe.allocateInstance(TestItem.class); item.loadExtra(input);
            if (age.getInt(item) != 42 || delay.getInt(item) != 9 || health.getInt(item) != 4 ||
                item.getItem().getItem() != Items.DIAMOND_SWORD || item.getItem().getDamageValue() != 17 ||
                !new java.util.UUID((9L<<32)|8L, (7L<<32)|6L).equals(owner.get(item)) ||
                !input.read("Pos",net.minecraft.world.phys.Vec3.CODEC).orElseThrow().equals(new net.minecraft.world.phys.Vec3(2,63,-2)) ||
                input.getShortOr("Fire", (short)0) != 12 || !input.getBooleanOr("Invulnerable", false) ||
                !LongTag.valueOf(987654321L).equals(tag.get("unknown_plugin_field")))
                throw new AssertionError("Rust item draft differs from Java decoding");
        }
        if (args.length > 2) {
            var tag = NbtIo.read(Path.of(args[2]));
            var input = TagValueInput.create(ProblemReporter.DISCARDING, registry, tag);
            var item = (TestItem) unsafe.allocateInstance(TestItem.class); item.loadExtra(input);
            var identity = net.minecraft.core.UUIDUtil.CODEC.parse(NbtOps.INSTANCE, tag.get("UUID")).getOrThrow();
            var thrower = net.minecraft.core.UUIDUtil.CODEC.parse(NbtOps.INSTANCE, tag.get("Thrower")).getOrThrow();
            var throwerField = ItemEntity.class.getDeclaredField("thrower"); throwerField.setAccessible(true);
            var decodedThrower = (net.minecraft.world.entity.EntityReference<?>)throwerField.get(item);
            var rotation = input.read("Rotation",net.minecraft.world.phys.Vec2.CODEC).orElseThrow();
            if (!identity.equals(new java.util.UUID((1L<<32)|2L,(3L<<32)|4L)) ||
                !thrower.equals(new java.util.UUID((5L<<32)|6L,(7L<<32)|8L)) ||
                decodedThrower == null || !decodedThrower.getUUID().equals(thrower) ||
                !new java.util.UUID((9L<<32)|8L,(7L<<32)|6L).equals(owner.get(item)) ||
                age.getInt(item)!=42 || delay.getInt(item)!=9 || health.getInt(item)!=4 ||
                item.getItem().getItem()!=Items.STONE || item.getItem().getCount()!=3 || item.getItem().getMaxStackSize()!=99 ||
                !input.read("Pos",net.minecraft.world.phys.Vec3.CODEC).orElseThrow().equals(new net.minecraft.world.phys.Vec3(2,63,-2)) ||
                !input.read("Motion",net.minecraft.world.phys.Vec3.CODEC).orElseThrow().equals(new net.minecraft.world.phys.Vec3(0.25,-0.5,0)) ||
                rotation.x!=30 || rotation.y!=-15 || input.getBooleanOr("OnGround",true) || input.getBooleanOr("NoGravity",true) ||
                input.getShortOr("Fire",(short)0)!=12 || !input.getBooleanOr("Invulnerable",false) || input.getIntOr("invulnerable_time",0)!=3 ||
                input.getDoubleOr("fall_distance",0)!=2.5 || input.getShortOr("Air",(short)0)!=280 || input.getIntOr("PortalCooldown",0)!=7)
                throw new AssertionError("new item document differs from Java decoding");
        }
    }
}
