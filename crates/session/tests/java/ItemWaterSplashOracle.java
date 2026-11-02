import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.Holder;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.core.particles.ParticleTypes;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;

public class ItemWaterSplashOracle {
    static class World extends ClientLevel {
        int bubbles, splashes;
        World() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public void addParticle(ParticleOptions type, double x, double y, double z, double dx, double dy, double dz) {
            if (type == ParticleTypes.BUBBLE) bubbles++;
            else if (type == ParticleTypes.SPLASH) splashes++;
            else throw new AssertionError("unexpected particle");
        }
    }
    static class Item extends ItemEntity {
        World world; Vec3 velocity; float volume, pitch; String sound; int events;
        Item() { super(null, 0, 0, 0, ItemStack.EMPTY); }
        @Override public World level() { return world; }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public LivingEntity getControllingPassenger() { return null; }
        @Override public void playSound(SoundEvent event, float volume, float pitch) {
            this.sound = event.location().toString(); this.volume = volume; this.pitch = pitch;
        }
        @Override public void gameEvent(Holder<GameEvent> event) { events++; }
        void splash() { doWaterSplashEffect(); }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        var item = (Item) unsafe.allocateInstance(Item.class); item.world = world;
        var randomField = Entity.class.getDeclaredField("random");
        var dimensionsField = Entity.class.getDeclaredField("dimensions");
        var positionField = Entity.class.getDeclaredField("position");
        unsafe.putObject(item, unsafe.objectFieldOffset(dimensionsField), EntityDimensions.scalable(0.25f, 0.25f));
        unsafe.putObject(item, unsafe.objectFieldOffset(positionField), new Vec3(1.25, 4.5, -3.75));
        var rows = new JsonArray();
        for (int seed = 0; seed < 64; seed++) for (double y : new double[]{-3.0, -1.25, -0.1, 0, 1.5}) {
            var random = new LegacyRandomSource(seed);
            unsafe.putObject(item, unsafe.objectFieldOffset(randomField), random);
            item.velocity = new Vec3(0.3, y, -0.4); item.events = 0;
            world.bubbles = 0; world.splashes = 0; item.splash();
            var row = new JsonObject(); row.addProperty("seed", seed); row.addProperty("y", y);
            row.addProperty("event", item.sound); row.addProperty("volume", Float.floatToRawIntBits(item.volume));
            row.addProperty("pitch", Float.floatToRawIntBits(item.pitch)); row.addProperty("next", random.nextInt());
            row.addProperty("bubbles", world.bubbles); row.addProperty("splashes", world.splashes);
            row.addProperty("events", item.events); rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
