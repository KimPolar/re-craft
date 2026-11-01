import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.Vec3;

public class PlayerFireKnockbackOracle {
    static class TestPlayer extends Player {
        double resistance; boolean grounded;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return resistance; }
        @Override public boolean onGround() { return grounded; }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        var randomField=Entity.class.getDeclaredField("random"); randomField.setAccessible(true);
        var rows=new JsonArray();
        for(double resistance:new double[]{-2.0,-0.5,0.0,0.4,1.0}) for(boolean grounded:new boolean[]{false,true})
        for(long seed:new long[]{0,1,17,1234567}) for(double[] velocity:new double[][]{{0,0,0},{0.2,-0.1,-0.3},{2,1,-4}}) {
            player.resistance=resistance; player.grounded=grounded;
            var random=RandomSource.create(seed); randomField.set(player,random);
            player.setDeltaMovement(new Vec3(velocity[0],velocity[1],velocity[2]));
            player.knockback((double)0.4f,0.0,0.0,null,4.0f);
            var actual=player.getDeltaMovement(); var row=new JsonObject();
            row.addProperty("resistance",resistance); row.addProperty("grounded",grounded); row.addProperty("seed",seed);
            var initial=new JsonArray(); for(double n:velocity) initial.add(n); row.add("initial",initial);
            var result=new JsonArray(); result.add(actual.x); result.add(actual.y); result.add(actual.z); row.add("result",result);
            row.addProperty("burn_pitch_bits",Float.floatToRawIntBits(2.0f+random.nextFloat()*0.4f));
            row.addProperty("next",random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
