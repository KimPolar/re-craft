import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.chat.*;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.TooltipDisplay;
import net.minecraft.world.item.component.ItemLore;
import net.minecraft.world.item.alchemy.*;
import net.minecraft.core.Holder;

public class ItemTooltipOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup(); var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var result=new JsonArray();
    for(var element:cases) {
      var input=element.getAsJsonObject();
      if(input.has("all")) {
        for(var current:BuiltInRegistries.ITEM) {
          var row=new JsonObject();
          row.addProperty("id",BuiltInRegistries.ITEM.getKey(current).toString());
          row.addProperty("description",current.getDescriptionId());
          row.addProperty("rarity",new ItemStack(current).getRarity().name());
          result.add(row);
        }
        continue;
      }
      var item=BuiltInRegistries.ITEM.getValue(Identifier.parse(input.get("id").getAsString()));
      var stack=new ItemStack(item);
      if(input.has("potion")) {
        @SuppressWarnings("unchecked")
        var holder=(Holder<Potion>)Potions.class.getField(input.get("potion").getAsString().toUpperCase(java.util.Locale.ROOT)).get(null);
        stack.set(DataComponents.POTION_CONTENTS,new PotionContents(holder));
      }
      if(input.has("custom")) stack.set(DataComponents.CUSTOM_NAME,Component.literal(input.get("custom").getAsString()));
      if(input.has("item_name")) stack.set(DataComponents.ITEM_NAME,Component.literal(input.get("item_name").getAsString()));
      if(input.has("rarity")) stack.set(DataComponents.RARITY,Rarity.valueOf(input.get("rarity").getAsString()));
      if(input.has("lore")) {
        var lore=new java.util.ArrayList<Component>();
        for(var line:input.getAsJsonArray("lore")) lore.add(Component.literal(line.getAsString()));
        stack.set(DataComponents.LORE,new ItemLore(lore));
      }
      if(input.has("hide") && input.get("hide").getAsBoolean()) stack.set(DataComponents.TOOLTIP_DISPLAY,new TooltipDisplay(true,new java.util.LinkedHashSet<>()));
      var lines=new JsonArray();
      for(var component:stack.getTooltipLines(Item.TooltipContext.EMPTY,null,TooltipFlag.NORMAL)) {
        lines.add(ComponentSerialization.CODEC.encodeStart(JsonOps.INSTANCE,component).getOrThrow());
      }
      var row=new JsonObject();row.add("lines",lines);row.addProperty("description",item.getDescriptionId());
      var spans=new JsonArray();
      for(var component:stack.getTooltipLines(Item.TooltipContext.EMPTY,null,TooltipFlag.NORMAL)) {
        var line=new JsonArray();
        component.visit((style,text)->{
          if(!text.isEmpty()) {
            var part=new JsonObject();part.addProperty("text",text);part.addProperty("italic",style.isItalic());
            if(style.getColor()!=null)part.addProperty("color",style.getColor().getValue());
            line.add(part);
          }
          return java.util.Optional.empty();
        },Style.EMPTY);
        spans.add(line);
      }
      row.add("spans",spans);result.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(result));
  }
}
