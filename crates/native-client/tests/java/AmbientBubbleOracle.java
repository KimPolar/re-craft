import com.google.gson.*;
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;
import java.util.concurrent.atomic.AtomicLong;
import net.minecraft.client.particle.*;
import net.minecraft.client.renderer.texture.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.core.particles.*;
import net.minecraft.tags.FluidTags;
import net.minecraft.sounds.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;

public class AmbientBubbleOracle {
    static class Probe extends RuntimeException { }
    static class World extends ClientLevel {
        boolean water,ceiling,probe; JsonArray events,positions;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public FluidState getFluidState(BlockPos pos) { return water ? Fluids.WATER.defaultFluidState() : Fluids.EMPTY.defaultFluidState(); }
        @Override public BlockState getBlockState(BlockPos pos) {
            if(probe) { var value=new JsonArray(); value.add(pos.getX()); value.add(pos.getY()); value.add(pos.getZ()); positions.add(value); throw new Probe(); }
            return Blocks.AIR.defaultBlockState();
        }
        @Override public Iterable<VoxelShape> getBlockCollisionsFromContext(CollisionContext context,AABB bounds) { return ceiling ? List.of(Shapes.create(new AABB(-100,4,-100,100,5,100))) : List.of(); }
        @Override public void addAlwaysVisibleParticle(ParticleOptions type,double x,double y,double z,double vx,double vy,double vz) {
            var e=new JsonObject(); e.addProperty("kind",type==ParticleTypes.CURRENT_DOWN?"down":type==ParticleTypes.BUBBLE_COLUMN_UP?"up":"unexpected");
            e.addProperty("x",Double.doubleToRawLongBits(x)); e.addProperty("y",Double.doubleToRawLongBits(y)); e.addProperty("z",Double.doubleToRawLongBits(z)); e.addProperty("vx",Double.doubleToRawLongBits(vx)); e.addProperty("vy",Double.doubleToRawLongBits(vy)); e.addProperty("vz",Double.doubleToRawLongBits(vz)); events.add(e);
        }
        @Override public void playLocalSound(double x,double y,double z,SoundEvent sound,SoundSource category,float volume,float pitch,boolean delay) {
            var e=new JsonObject(); e.addProperty("sound",sound.location().toString()); e.addProperty("x",x); e.addProperty("y",y); e.addProperty("z",z);
            e.addProperty("category",category.toString()); e.addProperty("volume",volume); e.addProperty("pitch",pitch); e.addProperty("delay",delay); events.add(e);
        }
    }
    static class Sprite extends TextureAtlasSprite { Sprite() { super(null,null,1,1,0,0,0); } }
    static Field field(Class<?> type,String name) throws Exception { var f=type.getDeclaredField(name); f.setAccessible(true); return f; }
    static JsonArray state(Particle particle) throws Exception {
        var result=new JsonArray(); for(String key:new String[]{"x","y","z","xd","yd","zd"}) result.add(Double.doubleToRawLongBits(field(Particle.class,key).getDouble(particle)));
        result.add(field(Particle.class,"age").getInt(particle)); result.add(field(Particle.class,"lifetime").getInt(particle)); result.add(particle.isAlive()); return result;
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var bind=net.minecraft.core.Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true); bind.invoke(Fluids.WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var sprite=(Sprite)unsafe.allocateInstance(Sprite.class);
        var positions=new JsonArray();
        for(long seed:new long[]{0,77,-1}) {
            world.probe=true; world.positions=new JsonArray();
            var random=new LegacyRandomSource(seed); var randomField=Level.class.getDeclaredField("random"); unsafe.putObject(world,unsafe.objectFieldOffset(randomField),random);
            var visual=new LegacyRandomSource(1234); var mutable=new BlockPos.MutableBlockPos();
            for(int i=0;i<667;i++) for(int range:new int[]{16,32}) {
                try { world.doAnimateTick(-123,70,456,range,visual,null,mutable); } catch(Probe expected) { }
            }
            var row=new JsonObject(); row.addProperty("seed",seed); row.add("positions",world.positions); row.addProperty("next",random.nextInt()); positions.add(row);
        }
        world.probe=false;
        var animate=new JsonArray();
        for(boolean drag:new boolean[]{false,true}) for(int seed=0;seed<1000;seed++) {
            world.events=new JsonArray(); var random=new LegacyRandomSource(seed);
            Blocks.BUBBLE_COLUMN.animateTick(Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN,drag),world,new BlockPos(-8,10,17),random);
            var row=new JsonObject(); row.addProperty("drag",drag); row.addProperty("seed",seed); row.add("events",world.events); row.addProperty("next",random.nextInt()); animate.add(row);
        }
        var up=BubbleColumnUpParticle.class.getDeclaredConstructor(ClientLevel.class,double.class,double.class,double.class,double.class,double.class,double.class,TextureAtlasSprite.class); up.setAccessible(true);
        var down=WaterCurrentDownParticle.class.getDeclaredConstructor(ClientLevel.class,double.class,double.class,double.class,TextureAtlasSprite.class); down.setAccessible(true);
        long mask=(1L<<48)-1, inverse=0xdfe05bcb1365L, multiplier=0x5deece66dL;
        var particles=new JsonArray();
        for(boolean rising:new boolean[]{false,true}) for(int mode=0;mode<3;mode++) for(int i=0;i<100;i++) {
            world.water=true; world.ceiling=mode==1;
            var p=(Particle)(rising ? up.newInstance(world,1.25,3.5,-2.25,0.0,0.04,0.0,sprite) : down.newInstance(world,1.25,3.5,-2.25,sprite));
            var random=field(Particle.class,"random").get(p); var seed=((AtomicLong)field(random.getClass(),"seed").get(random)).get();
            for(int j=0;j<(rising?7:4);j++) seed=((seed-11)*inverse)&mask;
            var row=new JsonObject(); row.addProperty("up",rising); row.addProperty("mode",mode); row.addProperty("seed",seed^multiplier);
            row.addProperty("size_bits",Float.floatToRawIntBits(field(SingleQuadParticle.class,"quadSize").getFloat(p)));
            var states=new JsonArray(); states.add(state(p)); for(int tick=0;tick<120 && p.isAlive();tick++) { if(mode==2 && tick==3) world.water=false; p.tick(); states.add(state(p)); }
            row.add("states",states); particles.add(row);
        }
        var result=new JsonObject(); result.add("positions",positions); result.add("animate",animate); result.add("particles",particles); Files.writeString(Path.of(args[0]),result.toString());
    }
}
