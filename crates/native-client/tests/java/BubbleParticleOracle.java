import com.google.gson.*;
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;
import java.util.concurrent.atomic.AtomicLong;
import net.minecraft.client.particle.*;
import net.minecraft.client.renderer.texture.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.level.material.*;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;

public class BubbleParticleOracle {
    static class World extends ClientLevel {
        boolean water, ceiling;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public FluidState getFluidState(BlockPos pos) { return water ? Fluids.WATER.defaultFluidState() : Fluids.EMPTY.defaultFluidState(); }
        @Override public Iterable<VoxelShape> getBlockCollisionsFromContext(CollisionContext context,AABB bounds) {
            return ceiling ? List.of(Shapes.create(new AABB(-100,4,-100,100,5,100))) : List.of();
        }
    }
    static class Sprite extends TextureAtlasSprite { Sprite() { super(null,null,1,1,0,0,0); } }
    static Field field(Class<?> type,String name) throws Exception { var f=type.getDeclaredField(name); f.setAccessible(true); return f; }
    static JsonArray state(Particle particle) throws Exception {
        var result=new JsonArray();
        for(String key:new String[]{"x","y","z","xd","yd","zd"}) result.add(Double.doubleToRawLongBits(field(Particle.class,key).getDouble(particle)));
        result.add(field(Particle.class,"lifetime").getInt(particle)); result.add(particle.isAlive()); return result;
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var bind=net.minecraft.core.Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        bind.invoke(Fluids.WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var sprite=(Sprite)unsafe.allocateInstance(Sprite.class);
        var ctor=BubbleParticle.class.getDeclaredConstructor(ClientLevel.class,double.class,double.class,double.class,double.class,double.class,double.class,TextureAtlasSprite.class); ctor.setAccessible(true);
        long mask=(1L<<48)-1, inverse=0xdfe05bcb1365L, multiplier=0x5deece66dL;
        var rows=new JsonArray();
        for(int mode=0;mode<3;mode++) for(int i=0;i<100;i++) {
            world.water=true; world.ceiling=mode==1;
            var p=(Particle)ctor.newInstance(world,1.25,3.5,-2.25,0.2,0.8,-0.4,sprite);
            var random=field(Particle.class,"random").get(p); var state=((AtomicLong)field(random.getClass(),"seed").get(random)).get();
            for(int j=0;j<7;j++) state=((state-11)*inverse)&mask;
            var row=new JsonObject(); row.addProperty("seed",state^multiplier); row.addProperty("mode",mode);
            row.addProperty("size_bits",Float.floatToRawIntBits(field(SingleQuadParticle.class,"quadSize").getFloat(p)));
            var states=new JsonArray(); states.add(state(p));
            for(int tick=0;tick<45 && p.isAlive();tick++) {
                if(mode==2 && tick==3) world.water=false;
                p.tick(); states.add(state(p));
            }
            row.add("states",states); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
