import java.nio.file.*;
import java.util.concurrent.atomic.AtomicLong;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.server.level.ServerLevel;

public class ItemRandomOracle {
  static class World extends ServerLevel {
    World() { super(null,null,null,null,null,null,false,0L,null,false); }
    @Override public int getNextEntityId() { return 7; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registry=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
    var seedField = LegacyRandomSource.class.getDeclaredField("seed");
    seedField.setAccessible(true);
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var world=(World)((sun.misc.Unsafe)f.get(null)).allocateInstance(World.class);
    var out = new JsonArray();
    for (int mode=0; mode<2; mode++) for (int i=0; i<32; i++) {
      var item = mode==0
        ? new ItemEntity(world,3.5,5.8,4.5,new ItemStack(Items.STONE))
        : new ItemEntity(world,3.5,5.8,4.5,new ItemStack(Items.STONE),0.1,0.2,0.3);
      var random=item.getRandom();
      long state=((AtomicLong)seedField.get(random)).get();
      for(int step=0;step<(mode==0?10:6);step++)
        state=((state-11L)*0xdfe05bcb1365L)&((1L<<48)-1);
      long seed=state^0x5deece66dL;
      var row=new JsonObject();
      row.addProperty("mode",mode); row.addProperty("seed",seed);
      row.addProperty("bob",Float.floatToRawIntBits(item.bobOffs));
      row.addProperty("yaw",Float.floatToRawIntBits(item.getYRot()));
      row.addProperty("x",Double.doubleToRawLongBits(item.getDeltaMovement().x));
      row.addProperty("z",Double.doubleToRawLongBits(item.getDeltaMovement().z));
      row.addProperty("next",random.nextInt()); out.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
