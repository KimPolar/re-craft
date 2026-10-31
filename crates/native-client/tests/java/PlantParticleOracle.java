import com.google.gson.*;
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;
import java.util.concurrent.atomic.AtomicLong;
import net.minecraft.client.particle.*;
import net.minecraft.client.renderer.texture.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.resources.Identifier;
import com.mojang.blaze3d.platform.Transparency;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.core.BlockPos;

public class PlantParticleOracle {
  static class Sprite extends TextureAtlasSprite {
    Sprite() { super(null,null,1,1,0,0,0); }
    @Override public Transparency transparency() { return Transparency.NONE; }
    @Override public Identifier atlasLocation() { return TextureAtlas.LOCATION_BLOCKS; }
  }
  static class World extends ClientLevel {
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return Blocks.AIR.defaultBlockState(); }
    @Override public Iterable<VoxelShape> getBlockCollisionsFromContext(CollisionContext context,AABB bounds) { return List.of(Shapes.create(new AABB(-100,2,-100,100,3,100))); }
  }
  // TerrainParticle's superclass initialization and fields; avoids creating a graphics device.
  static class Terrain extends SingleQuadParticle {
    final float uo,vo;
    Terrain(ClientLevel world,double x,double y,double z,double dx,double dy,double dz,TextureAtlasSprite sprite) {
      super(world,x,y,z,dx,dy,dz,sprite); gravity=1.0f; quadSize/=2.0f; uo=random.nextFloat()*3.0f; vo=random.nextFloat()*3.0f;
    }
    public Layer getLayer() { return Layer.bySprite(sprite); }
    protected float getU0() { return 0; } protected float getU1() { return 1; }
    protected float getV0() { return 0; } protected float getV1() { return 1; }
  }
  static Field field(Class<?> type,String name) throws Exception { var f=type.getDeclaredField(name); f.setAccessible(true); return f; }
  static JsonArray values(Particle p) throws Exception {
    var a=new JsonArray(); for(String key:new String[]{"x","y","z","xd","yd","zd"}) a.add(Double.doubleToRawLongBits(field(Particle.class,key).getDouble(p)));
    a.add(field(Particle.class,"age").getInt(p)); a.add(!p.isAlive()); return a;
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var sprite=(Sprite)unsafe.allocateInstance(Sprite.class); var world=(World)unsafe.allocateInstance(World.class);
    var result=new JsonObject(); var shapes=new JsonArray();
    for(Block block:new Block[]{Blocks.SHORT_GRASS,Blocks.FERN,Blocks.DEAD_BUSH}) for(BlockPos pos:new BlockPos[]{new BlockPos(0,0,1),new BlockPos(8,2,7),new BlockPos(-21,64,42),new BlockPos(16000008,2,-15999993)}) {
      var state=block.defaultBlockState(); var row=new JsonObject(); row.addProperty("name",net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey(block).toString());
      var coords=new JsonArray(); coords.add(pos.getX()); coords.add(pos.getY()); coords.add(pos.getZ()); row.add("position",coords); row.addProperty("enabled",state.shouldSpawnTerrainParticles());
      var fragments=new JsonArray(); state.getShape(world,pos).forAllBoxes((minX,minY,minZ,maxX,maxY,maxZ)->{
        double sx=Math.min(1.0,maxX-minX), sy=Math.min(1.0,maxY-minY), sz=Math.min(1.0,maxZ-minZ);
        int nx=Math.max(2,net.minecraft.util.Mth.ceil(sx/0.25)), ny=Math.max(2,net.minecraft.util.Mth.ceil(sy/0.25)), nz=Math.max(2,net.minecraft.util.Mth.ceil(sz/0.25));
        for(int x=0;x<nx;x++) for(int y=0;y<ny;y++) for(int z=0;z<nz;z++) {
          double fx=(x+0.5)/nx,fy=(y+0.5)/ny,fz=(z+0.5)/nz; var a=new JsonArray();
          for(double value:new double[]{pos.getX()+(fx*sx+minX),pos.getY()+(fy*sy+minY),pos.getZ()+(fz*sz+minZ),fx-0.5,fy-0.5,fz-0.5}) a.add(Double.doubleToRawLongBits(value));
          fragments.add(a);
        }
      }); row.add("fragments",fragments); shapes.add(row);
    }
    result.add("shapes",shapes); var physics=new JsonArray(); long mask=(1L<<48)-1,inverse=0xdfe05bcb1365L,multiplier=0x5deece66dL;
    for(boolean floor:new boolean[]{false,true}) for(int i=0;i<100;i++) {
      var p=new Terrain(floor?world:null,1.25,3.5,-2.25,0.1,0.15,-0.05,sprite);
      var random=field(Particle.class,"random").get(p); var state=((AtomicLong)field(random.getClass(),"seed").get(random)).get();
      for(int j=0;j<9;j++) state=((state-11)*inverse)&mask;
      var row=new JsonObject(); row.addProperty("seed",state^multiplier); row.addProperty("floor",floor); row.addProperty("lifetime",field(Particle.class,"lifetime").getInt(p));
      row.addProperty("size_bits",Float.floatToRawIntBits(field(SingleQuadParticle.class,"quadSize").getFloat(p))); row.addProperty("u_bits",Float.floatToRawIntBits(p.uo)); row.addProperty("v_bits",Float.floatToRawIntBits(p.vo));
      if(!floor) field(Particle.class,"hasPhysics").setBoolean(p,false);
      var states=new JsonArray(); states.add(values(p)); for(int j=0;j<42 && p.isAlive();j++) { p.tick(); states.add(values(p)); } row.add("states",states); physics.add(row);
    }
    result.add("physics",physics); Files.writeString(Path.of(args[0]),new Gson().toJson(result));
  }
}
