import com.google.gson.*;
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;
import java.util.concurrent.atomic.AtomicLong;
import net.minecraft.client.particle.*;
import net.minecraft.client.renderer.texture.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;

public class SplashParticleOracle {
    static class World extends ClientLevel {
        int mode;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public BlockState getBlockState(BlockPos pos) {
            if (pos.getY()==2 && mode==1) return Blocks.STONE.defaultBlockState();
            if (pos.getY()==2 && mode==2) return Blocks.WATER.defaultBlockState();
            return Blocks.AIR.defaultBlockState();
        }
        @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        @Override public Iterable<VoxelShape> getBlockCollisionsFromContext(CollisionContext context,AABB bounds) {
            return mode==1 ? List.of(Shapes.create(new AABB(-100,2,-100,100,3,100))) : List.of();
        }
    }
    static class Sprite extends TextureAtlasSprite { Sprite() { super(null,null,1,1,0,0,0); } }
    static Field field(Class<?> type,String name) throws Exception { var f=type.getDeclaredField(name); f.setAccessible(true); return f; }
    static JsonArray state(Particle p) throws Exception {
        var result=new JsonArray();
        for(String key:new String[]{"x","y","z","xd","yd","zd"}) result.add(Double.doubleToRawLongBits(field(Particle.class,key).getDouble(p)));
        result.add(field(Particle.class,"lifetime").getInt(p)); result.add(p.isAlive()); return result;
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var bind=net.minecraft.core.Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        bind.invoke(Fluids.WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var sprite=(Sprite)unsafe.allocateInstance(Sprite.class);
        var ctor=SplashParticle.class.getDeclaredConstructor(ClientLevel.class,double.class,double.class,double.class,double.class,double.class,double.class,TextureAtlasSprite.class); ctor.setAccessible(true);
        long mask=(1L<<48)-1, inverse=0xdfe05bcb1365L, multiplier=0x5deece66dL;
        var rows=new JsonArray();
        for(int mode=0;mode<4;mode++) for(int i=0;i<100;i++) {
            world.mode=mode==3?0:mode;
            double vy=mode==3?0:0.8;
            var p=(Particle)ctor.newInstance(world,1.25,3.5,-2.25,0.2,vy,-0.4,sprite);
            var random=field(Particle.class,"random").get(p); var seed=((AtomicLong)field(random.getClass(),"seed").get(random)).get();
            for(int j=0;j<9;j++) seed=((seed-11)*inverse)&mask;
            var row=new JsonObject(); row.addProperty("seed",seed^multiplier); row.addProperty("mode",mode);
            row.addProperty("size_bits",Float.floatToRawIntBits(field(SingleQuadParticle.class,"quadSize").getFloat(p)));
            var states=new JsonArray(); states.add(state(p));
            for(int tick=0;tick<45 && p.isAlive();tick++) { p.tick(); states.add(state(p)); }
            row.add("states",states); rows.add(row);
        }
        var random=new LegacyRandomSource(77); var packets=new JsonArray();
        for(int i=0;i<100;i++) {
            boolean bubble=(i%2)==1;
            float yd=bubble?0.01f:0.0f, speed=bubble?0.2f:1.0f;
            double x=random.nextGaussian()*0.0f, y=random.nextGaussian()*yd, z=random.nextGaussian()*0.0f;
            double vx=random.nextGaussian()*speed, vy=random.nextGaussian()*speed, vz=random.nextGaussian()*speed;
            var row=new JsonArray(); for(double v:new double[]{1.25+x,3.5+y,-2.25+z,vx,vy,vz}) row.add(v); packets.add(row);
        }
        var result=new JsonObject(); result.add("particles",rows); result.add("packets",packets); result.addProperty("next",random.nextInt());
        Files.writeString(Path.of(args[0]),result.toString());
    }
}
