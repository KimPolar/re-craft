import com.google.gson.*;
import java.lang.reflect.Field;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.core.Holder;
import net.minecraft.tags.TagKey;
import net.minecraft.world.entity.EntityType;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.Blocks;

public class HoneyLandingOracle {
    static class TestLevel extends ServerLevel {
        boolean client;
        DamageSources sources;
        List<String> order;
        TestLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public boolean isClientSide() { return client; }
        @Override public DamageSources damageSources() { return sources; }
        @Override public void broadcastEntityEvent(Entity entity, byte event) { order.add("event:" + event); }
    }
    static class TestPlayer extends Player {
        TestLevel world;
        boolean result;
        double distance;
        float factor;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public void playSound(SoundEvent event, float volume, float pitch) {
            world.order.add(event.location().toString() + ":" + volume + ":" + pitch);
        }
        @Override public boolean causeFallDamage(double distance, float factor, DamageSource source) {
            this.distance = distance; this.factor = factor; world.order.add("damage"); return result;
        }
    }
    static class LivingPlayer extends Player {
        TestLevel world;
        boolean silent;
        double safe, multiplier;
        LivingPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean is(TagKey<EntityType<?>> tag) { return false; }
        @Override public boolean isVehicle() { return false; }
        @Override public boolean isIgnoringFallDamageFromCurrentImpulse() { return false; }
        @Override public boolean isSilent() { return silent; }
        @Override public double getAttributeValue(Holder<Attribute> attr) {
            if (attr == Attributes.SAFE_FALL_DISTANCE) return safe;
            if (attr == Attributes.FALL_DAMAGE_MULTIPLIER) return multiplier;
            throw new IllegalStateException();
        }
        @Override public void awardStat(Identifier id, int amount) { }
        @Override public void playSound(SoundEvent event, float volume, float pitch) {
            world.order.add(event.location().toString() + ":" + volume + ":" + pitch);
        }
        @Override protected void playBlockFallSound() { if (!silent) world.order.add("block_below"); }
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) { world.order.add("hurt:" + amount); return false; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        var world = (TestLevel)unsafe.allocateInstance(TestLevel.class);
        world.sources = (DamageSources)unsafe.allocateInstance(DamageSources.class);
        player.world = world;
        var rows = new JsonArray();
        for (boolean client : new boolean[]{false,true}) for (boolean damage : new boolean[]{false,true})
            for (double distance : new double[]{0.01, 3.0, 10.0, 42.5}) {
                world.client=client; world.order=new ArrayList<>(); player.result=damage;
                Blocks.HONEY_BLOCK.fallOn(world, Blocks.HONEY_BLOCK.defaultBlockState(), BlockPos.ZERO, player, distance);
                var row = new JsonObject(); row.addProperty("client",client); row.addProperty("result",damage); row.addProperty("distance",player.distance);
                row.addProperty("factor_bits",Float.floatToRawIntBits(player.factor)); row.add("order",new Gson().toJsonTree(world.order)); rows.add(row);
            }
        var player2 = (LivingPlayer)unsafe.allocateInstance(LivingPlayer.class); player2.world=world;
        Field entityLevel = Entity.class.getDeclaredField("level"); entityLevel.setAccessible(true); entityLevel.set(player2, world);
        Field abilityField = Player.class.getDeclaredField("abilities"); abilityField.setAccessible(true);
        var abilities = new Abilities(); abilityField.set(player2, abilities);
        var living = new JsonArray();
        for (boolean mayfly : new boolean[]{false,true}) for (boolean silent : new boolean[]{false,true})
            for (double distance : new double[]{0.01,3.0,7.99,8.1,22.99,23.1,42.5}) {
                world.client=false; world.order=new ArrayList<>(); abilities.mayfly=mayfly; player2.silent=silent;
                player2.safe=3.0; player2.multiplier=1.0;
                Blocks.HONEY_BLOCK.fallOn(world, Blocks.HONEY_BLOCK.defaultBlockState(), BlockPos.ZERO, player2, distance);
                var row=new JsonObject(); row.addProperty("mayfly",mayfly); row.addProperty("silent",silent); row.addProperty("distance",distance); row.add("order",new Gson().toJsonTree(world.order)); living.add(row);
            }
        var result = new JsonObject(); result.add("block",rows); result.add("living",living);
        Files.writeString(Path.of(args[0]),new Gson().toJson(result));
    }
}
