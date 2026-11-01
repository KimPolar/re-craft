import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;

public class PlayerArmorDamageOracle {
    static class TestPlayer extends Player {
        double armor, toughness;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) {
            if (attribute == Attributes.ARMOR) return armor;
            if (attribute == Attributes.ARMOR_TOUGHNESS) return toughness;
            throw new IllegalStateException();
        }
        @Override protected void hurtArmor(DamageSource source, float amount) { }
        float apply(DamageSource source, float amount) { return getDamageAfterArmorAbsorb(source, amount); }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var player = (TestPlayer)((sun.misc.Unsafe)f.get(null)).allocateInstance(TestPlayer.class);
        var source = new DamageSource(Holder.direct(new DamageType("test", 0.1f)));
        var rows = new JsonArray();
        for (double armor : new double[]{0, 0.1, 0.99, 1, 1.99, 3.99, 10.5, 19.99, 20, 29.99, 30})
            for (double toughness : new double[]{0, 0.5, 2, 8, 20})
                for (float amount : new float[]{0.25f, 1f, 3f, 4f, 20f}) {
                    player.armor = armor; player.toughness = toughness;
                    float health = Math.max(20f - player.apply(source, amount), 0f);
                    var row = new JsonObject(); row.addProperty("armor", armor); row.addProperty("toughness", toughness);
                    row.addProperty("amount", amount); row.addProperty("health_bits", Float.floatToRawIntBits(health)); rows.add(row);
                }
        Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
    }
}
