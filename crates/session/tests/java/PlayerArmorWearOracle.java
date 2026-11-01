import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.core.component.DataComponents;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.*;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.Level;

public class PlayerArmorWearOracle {
    static class TestPlayer extends Player {
        Level testLevel;
        ItemStack[] armor;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public Level level() { return testLevel; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) {
            if (attribute == Attributes.ARMOR_TOUGHNESS) return 0;
            if (attribute == Attributes.ARMOR) {
                int[] values={2,6,5,2}; int total=0;
                for (int i=0;i<4;i++) if (!armor[i].isEmpty()) total+=values[i];
                return total;
            }
            throw new IllegalStateException();
        }
        @Override public ItemStack getItemBySlot(EquipmentSlot slot) {
            return switch (slot) {
                case HEAD -> armor[0]; case CHEST -> armor[1];
                case LEGS -> armor[2]; case FEET -> armor[3];
                default -> ItemStack.EMPTY;
            };
        }
        @Override public void onEquippedItemBroken(ItemStack item, EquipmentSlot slot) { }
        float apply(DamageSource source, float amount) { return getDamageAfterArmorAbsorb(source, amount); }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var registries = net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
        for (var pending : net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var fireResistance = new ItemStack(Items.NETHERITE_HELMET).get(DataComponents.DAMAGE_RESISTANT);
        if (fireResistance == null || !fireResistance.types().unwrapKey().orElseThrow().toString().contains("is_fire")) throw new AssertionError("unexpected netherite damage resistance");
        if (new ItemStack(Items.ELYTRA).get(DataComponents.EQUIPPABLE).damageOnHurt()) throw new AssertionError("elytra unexpectedly takes armor damage");
        if (!new ItemStack(Items.SHIELD).get(DataComponents.EQUIPPABLE).damageOnHurt()) throw new AssertionError("shield unexpectedly avoids armor damage");
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        player.testLevel = (ServerLevel) unsafe.allocateInstance(ServerLevel.class);
        var source = new DamageSource(Holder.direct(new DamageType("test", 0.1f)));
        var rows = new JsonArray();
        Item[] items = {Items.IRON_HELMET, Items.IRON_CHESTPLATE, Items.IRON_LEGGINGS, Items.IRON_BOOTS};
        for (float amount : new float[]{0f, 0.5f, 1f, 3.9f, 4f, 7.9f, 8f, 20f, 100f}) {
            for (int initial : new int[]{0, 10, 160}) {
                player.armor = new ItemStack[4];
                for (int i=0; i<4; i++) {
                    player.armor[i] = new ItemStack(items[i]);
                    player.armor[i].set(DataComponents.DAMAGE, Math.min(initial, player.armor[i].getMaxDamage()-1));
                }
                float health = Math.max(100f - player.apply(source, amount), 0f);
                var row = new JsonObject(); row.addProperty("amount",amount); row.addProperty("initial",initial);
                row.addProperty("health_bits",Float.floatToRawIntBits(health));
                var damages = new JsonArray();
                for (var stack : player.armor) damages.add(stack.isEmpty() ? -1 : stack.getDamageValue());
                row.add("damage",damages); rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
    }
}
