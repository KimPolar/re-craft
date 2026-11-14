import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.phys.AABB;

public class FallingStalactiteDamageOracle {
    static class Sources extends DamageSources {
        DamageSource source;
        Sources() { super(null); }
        @Override public DamageSource fallingStalactite(Entity entity) { return source; }
    }
    static class World extends ServerLevel {
        Sources sources;
        Player target;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public DamageSources damageSources() { return sources; }
        @Override public List<Entity> getEntities(Entity except, AABB box, Predicate<? super Entity> predicate) {
            return List.of(target);
        }
    }
    static class Target extends Player {
        float amount;
        DamageSource source;
        int calls;
        Target() { super(null,null); }
        @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) {
            this.amount=amount; this.source=source; calls++; return true;
        }
    }
    static void set(Object object, Class<?> type, String name, Object value) throws Exception {
        var field=type.getDeclaredField(name);field.setAccessible(true);field.set(object,value);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var sources=(Sources)unsafe.allocateInstance(Sources.class);
        sources.source=(DamageSource)unsafe.allocateInstance(DamageSource.class);
        var world=(World)unsafe.allocateInstance(World.class);world.sources=sources;
        var target=(Target)unsafe.allocateInstance(Target.class);world.target=target;
        set(target,Entity.class,"level",world);
        var falling=(FallingBlockEntity)unsafe.allocateInstance(FallingBlockEntity.class);
        set(falling,Entity.class,"level",world);
        var output=new JsonArray();
        for(var block:List.of(Blocks.POINTED_DRIPSTONE,Blocks.SULFUR_SPIKE)) {
            set(falling,FallingBlockEntity.class,"blockState",block.defaultBlockState());
            for(int length:new int[]{1,2,5,6,7,15,40,100}) {
                falling.setHurtsEntities((float)Math.max(length,6),40);
                for(double distance:new double[]{-1,0,0.01,0.999,1,1.000001,1.5,2,2.000001,3.2,6,100}) {
                    target.calls=0;target.source=null;target.amount=-1;
                    falling.causeFallDamage(distance,1.0f,sources.source);
                    if(target.calls>1 || target.calls==1 && target.source!=sources.source)throw new AssertionError("unexpected damage");
                    var row=new JsonObject();row.addProperty("length",length);row.addProperty("distance",distance);
                    row.addProperty("calls",target.calls);row.addProperty("amount",target.amount);output.add(row);
                }
            }
        }
        Files.writeString(Path.of(args[0]),output.toString());
    }
}
