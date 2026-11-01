import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.RegistryOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;

public class WeightedLootOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var ops=RegistryOps.create(JsonOps.INSTANCE,registries);
    var definitions=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var ctor=LootContext.class.getDeclaredConstructors()[0]; ctor.setAccessible(true);
    var params=new LootParams(null,ContextMap.builder().set(LootContextParams.TOOL,ItemStack.EMPTY).build(),Map.of(),0.0f);
    var output=new JsonArray();
    for(var definition:definitions) {
      var table=LootTable.DIRECT_CODEC.parse(ops,definition).getOrThrow();
      for(int mode=0;mode<2;mode++) for(long seed=-32;seed<32;seed++) {
        net.minecraft.util.RandomSource random=mode==0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
        var items=new ArrayList<ItemStack>(); var context=(LootContext)ctor.newInstance(params,random,registries);
        table.getRandomItemsRaw(context,items::add);
        var row=new JsonObject(); var drops=new JsonArray();
        for(var stack:items) if(!stack.isEmpty()) { var item=new JsonArray(); item.add(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString()); item.add(stack.getCount()); drops.add(item); }
        row.add("drops",drops); row.addProperty("next",random.nextLong()); output.add(row);
      }
    }
    Files.writeString(Path.of(args[1]),output.toString());
  }
}
