import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SnowLayerBlock;
import net.minecraft.util.RandomSource;

public class SnowMeltOracle {
  static class ReachedLoot extends RuntimeException { }
  static class World extends ServerLevel {
    int blockLight, skyLight; boolean sawBlock, sawSky;
    World() { super(null,null,null,null,null,null,false,0L,null,false); }
    @Override public int getBrightness(LightLayer layer, BlockPos pos) {
      if(layer==LightLayer.BLOCK) { sawBlock=true; return blockLight; }
      sawSky=true; return skyLight;
    }
    @Override public MinecraftServer getServer() { throw new ReachedLoot(); }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var world=(World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
    var rows=new JsonArray();
    for(int layers=1;layers<=8;layers++) for(int light=0;light<=15;light++) for(int sky:new int[]{0,15}) {
      world.blockLight=light; world.skyLight=sky; world.sawBlock=false; world.sawSky=false;
      boolean reachesLoot=false;
      try { Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS,layers).randomTick(world,BlockPos.ZERO,RandomSource.create(42L)); }
      catch(ReachedLoot expected) { reachesLoot=true; }
      var row=new JsonObject(); row.addProperty("layers",layers); row.addProperty("block",light); row.addProperty("sky",sky);
      row.addProperty("reached_loot",reachesLoot); row.addProperty("saw_block",world.sawBlock); row.addProperty("saw_sky",world.sawSky); rows.add(row);
    }
    Files.writeString(Path.of(args[0]),rows.toString());
  }
}
