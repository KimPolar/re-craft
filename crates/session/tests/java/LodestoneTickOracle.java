import java.nio.file.*;
import java.util.Optional;
import com.google.gson.*;
import net.minecraft.core.*;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.entity.ai.village.poi.*;
import net.minecraft.world.item.component.LodestoneTracker;
import net.minecraft.world.level.Level;

public class LodestoneTickOracle {
  static class Poi extends PoiManager {
    boolean exists; int calls;
    Poi() { super(null,null,null,false,null,null,null); }
    @Override public boolean existsAtPosition(ResourceKey<PoiType> type, BlockPos pos) { if (type != PoiTypes.LODESTONE) throw new AssertionError(); calls++; return exists; }
  }
  static class World extends ServerLevel {
    boolean bounds; ResourceKey<Level> dim; Poi poi;
    World() { super(null,null,null,null,null,null,false,0,null,false); }
    @Override public boolean isInWorldBounds(BlockPos pos) { return bounds; }
    @Override public ResourceKey<Level> dimension() { return dim; }
    @Override public PoiManager getPoiManager() { return poi; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); world.poi=(Poi)unsafe.allocateInstance(Poi.class);
    var rows=new JsonArray();
    for(boolean target:new boolean[]{false,true}) for(boolean tracked:new boolean[]{false,true})
    for(boolean same:new boolean[]{false,true}) for(boolean bounds:new boolean[]{false,true}) for(boolean exists:new boolean[]{false,true}) {
      world.dim=same?Level.OVERWORLD:Level.NETHER; world.bounds=bounds; world.poi.exists=exists; world.poi.calls=0;
      var before=new LodestoneTracker(target?Optional.of(GlobalPos.of(Level.OVERWORLD, new BlockPos(-9,64,7))):Optional.empty(),tracked);
      var after=before.tick(world); var row=new JsonObject();
      row.addProperty("target",target); row.addProperty("tracked",tracked); row.addProperty("same",same); row.addProperty("bounds",bounds); row.addProperty("exists",exists);
      row.addProperty("after_target",after.target().isPresent()); row.addProperty("after_tracked",after.tracked()); row.addProperty("changed",before!=after); row.addProperty("calls",world.poi.calls); rows.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
  }
}
