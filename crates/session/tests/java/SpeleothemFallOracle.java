import com.google.gson.*;
import java.lang.reflect.Field;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.block.Blocks;

public class SpeleothemFallOracle {
    static class World extends ServerLevel {
        DamageSources sources;
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public DamageSources damageSources() { return sources; }
    }
    static class TestPlayer extends Player {
        DamageSources sources;
        DamageSource observed;
        double distance;
        float factor;
        TestPlayer() { super(null, null); }
        @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
        @Override public DamageSources damageSources() { return sources; }
        @Override public boolean causeFallDamage(double distance, float factor, DamageSource source) {
            this.distance=distance; this.factor=factor; this.observed=source; return true;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var sources=(DamageSources)unsafe.allocateInstance(DamageSources.class);
        var normal=(DamageSource)unsafe.allocateInstance(DamageSource.class);
        var stalagmite=(DamageSource)unsafe.allocateInstance(DamageSource.class);
        for (var pair : Map.of("fall", normal, "stalagmite", stalagmite).entrySet()) {
            Field field=DamageSources.class.getDeclaredField(pair.getKey()); field.setAccessible(true); field.set(sources,pair.getValue());
        }
        var world=(World)unsafe.allocateInstance(World.class); world.sources=sources;
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.sources=sources;
        var rows=new JsonArray();
        for (var block : new net.minecraft.world.level.block.Block[]{Blocks.POINTED_DRIPSTONE, Blocks.SULFUR_SPIKE})
            for (var state : block.getStateDefinition().getPossibleStates())
                for (double distance : new double[]{1.0,3.0,8.0}) {
                    player.observed=null;
                    block.fallOn(world,state,BlockPos.ZERO,player,distance);
                    if(player.observed!=normal && player.observed!=stalagmite) throw new AssertionError("unknown damage source");
                    var row=new JsonObject();
                    row.addProperty("id",BuiltInRegistries.BLOCK.getKey(block).toString());
                    var properties=new JsonObject(); state.getValues().forEach(v->properties.addProperty(v.property().getName(),v.valueName()));
                    row.add("properties",properties); row.addProperty("input",distance);
                    row.addProperty("distance",player.distance); row.addProperty("factor",player.factor);
                    row.addProperty("source",player.observed==stalagmite?"minecraft:stalagmite":"minecraft:fall");
                    rows.add(row);
                }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
