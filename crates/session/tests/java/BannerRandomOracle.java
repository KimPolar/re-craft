import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.RandomSequence;
import net.minecraft.world.item.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.BannerBlockEntity;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;

public class BannerRandomOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var json=JsonParser.parseString(Files.readString(Path.of(args[0])));
    var table=LootTable.DIRECT_CODEC.parse(RegistryOps.create(JsonOps.INSTANCE,registries),json).getOrThrow();
    var ctor=LootContext.class.getDeclaredConstructors()[0]; ctor.setAccessible(true);
    var output=new JsonArray();
    for(long seed:new long[]{0,1,42,-17}) for(int step=0;step<5;step++) {
      var random=new RandomSequence(seed,Identifier.parse("example:banner")).random();
      var items=new ArrayList<ItemStack>();
      for(int n=0;n<=step;n++) {
        var state=Blocks.BANNER.white().defaultBlockState();
        var entity=new BannerBlockEntity(BlockPos.ZERO,state);
        var source=new ItemStack(Items.BANNER.white()); source.set(DataComponents.CUSTOM_NAME,Component.literal(new String[]{"west","east","north","south","up"}[n]));
        entity.applyComponentsFromItemStack(source);
        var params=new LootParams(null,ContextMap.builder().set(LootContextParams.BLOCK_ENTITY,entity).set(LootContextParams.BLOCK_STATE,state).set(LootContextParams.TOOL,ItemStack.EMPTY).build(),Map.of(),0.0f);
        var context=(LootContext)ctor.newInstance(params,random,registries);
        items.clear(); table.getRandomItemsRaw(context,items::add);
      }
      var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("step",step); row.addProperty("drop",!items.isEmpty());
      if(!items.isEmpty()) row.addProperty("name",items.get(0).get(DataComponents.CUSTOM_NAME).getString());
      row.addProperty("next",random.nextLong()); output.add(row);
    }
    Files.writeString(Path.of(args[1]),output.toString());
  }
}
