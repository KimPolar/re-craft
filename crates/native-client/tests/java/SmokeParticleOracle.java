import com.google.gson.*;
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.concurrent.atomic.AtomicLong;
import java.util.List;
import net.minecraft.client.particle.*;
import net.minecraft.client.renderer.texture.*;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import com.mojang.blaze3d.platform.Transparency;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;

public class SmokeParticleOracle {
  static class Sprite extends TextureAtlasSprite {
    Sprite() { super(null,null,1,1,0,0,0); }
    @Override public Transparency transparency() { return Transparency.NONE; }
    @Override public Identifier atlasLocation() { return TextureAtlas.LOCATION_PARTICLES; }
  }
  static class Sprites implements SpriteSet {
    TextureAtlasSprite sprite; int index;
    public TextureAtlasSprite first() { return sprite; }
    public TextureAtlasSprite get(RandomSource random) { return sprite; }
    public TextureAtlasSprite get(int age, int lifetime) { index=age*7/lifetime; return sprite; }
  }
  static class World extends net.minecraft.client.multiplayer.ClientLevel {
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public Iterable<VoxelShape> getBlockCollisionsFromContext(CollisionContext context,AABB bounds) {
      return List.of(Shapes.create(new AABB(-100,4,-100,100,5,100)));
    }
  }
  static Field field(Class<?> type,String name) throws Exception { var result=type.getDeclaredField(name); result.setAccessible(true); return result; }
  static JsonArray values(Particle p,Sprites sprites) throws Exception {
    var out=new JsonArray();
    for(String key:new String[]{"x","y","z","xd","yd","zd"}) out.add(Double.doubleToRawLongBits(field(Particle.class,key).getDouble(p)));
    out.add(field(Particle.class,"age").getInt(p)); out.add(!p.isAlive());
    out.add(Float.floatToRawIntBits(((BaseAshSmokeParticle)p).getQuadSize(0.5f))); out.add(sprites.index);
    return out;
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var sprite=(Sprite)unsafe.allocateInstance(Sprite.class); var world=(World)unsafe.allocateInstance(World.class);
    var ctor=LargeSmokeParticle.class.getDeclaredConstructor(net.minecraft.client.multiplayer.ClientLevel.class,double.class,double.class,double.class,double.class,double.class,double.class,SpriteSet.class); ctor.setAccessible(true);
    var out=new JsonArray(); long mask=(1L<<48)-1, inverse=0xdfe05bcb1365L, multiplier=0x5deece66dL;
    for(boolean ceiling:new boolean[]{false,true}) for(int i=0;i<100;i++) {
      var sprites=new Sprites(); sprites.sprite=sprite;
      var p=(Particle)ctor.newInstance(ceiling?world:null,1.25,3.5,-2.25,0.0,0.0,0.0,sprites);
      var random=field(Particle.class,"random").get(p); var state=((AtomicLong)field(random.getClass(),"seed").get(random)).get();
      for(int j=0;j<9;j++) state=((state-11)*inverse)&mask;
      var row=new JsonObject(); row.addProperty("seed",state^multiplier); row.addProperty("ceiling",ceiling);
      row.addProperty("lifetime",field(Particle.class,"lifetime").getInt(p));
      row.addProperty("size_bits",Float.floatToRawIntBits(field(SingleQuadParticle.class,"quadSize").getFloat(p)));
      row.addProperty("color_bits",Float.floatToRawIntBits(field(SingleQuadParticle.class,"rCol").getFloat(p)));
      if(!ceiling && i==0) { var packetRandom=RandomSource.create(123); var positions=new JsonArray(); for(int j=0;j<24;j++) positions.add(Double.doubleToRawLongBits(packetRandom.nextDouble())); row.add("packet_positions",positions); }
      if(!ceiling) field(Particle.class,"hasPhysics").setBoolean(p,false);
      var states=new JsonArray(); states.add(values(p,sprites));
      for(int j=0;j<102 && p.isAlive();j++) { p.tick(); states.add(values(p,sprites)); }
      row.add("states",states); out.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
