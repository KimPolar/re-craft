import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.RegistrationInfo;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.chat.*;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.TooltipDisplay;
import net.minecraft.world.item.component.ItemLore;
import net.minecraft.world.item.alchemy.*;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.effect.MobEffects;
import net.minecraft.world.item.enchantment.Enchantment;
import net.minecraft.world.item.enchantment.ItemEnchantments;
import net.minecraft.tags.EnchantmentTags;
import com.mojang.serialization.Lifecycle;
import net.minecraft.core.Holder;

public class ItemTooltipOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup(); var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var enchantments=new MappedRegistry<Enchantment>(Registries.ENCHANTMENT,Lifecycle.stable());
    registries.lookupOrThrow(Registries.ENCHANTMENT).listElements().forEach(holder -> enchantments.register(holder.key(),holder.value(),RegistrationInfo.BUILT_IN));
    var tags=new java.util.HashMap<net.minecraft.tags.TagKey<Enchantment>,java.util.List<Holder<Enchantment>>>();
    for(var tag:java.util.List.of(EnchantmentTags.TOOLTIP_ORDER,EnchantmentTags.CURSE)) {
      var path="data/minecraft/tags/enchantment/"+tag.location().getPath()+".json";
      try(var stream=ItemTooltipOracle.class.getClassLoader().getResourceAsStream(path)) {
        var json=JsonParser.parseString(new String(stream.readAllBytes(),java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject();
        var holders=new java.util.ArrayList<Holder<Enchantment>>();
        for(var value:json.getAsJsonArray("values")) holders.add(enchantments.getOrThrow(ResourceKey.create(Registries.ENCHANTMENT,Identifier.parse(value.getAsString()))));
        tags.put(tag,holders);
      }
    }
    enchantments.bindTags(tags); enchantments.freeze();
    var tooltipRegistries=HolderLookup.Provider.create(java.util.stream.Stream.concat(registries.listRegistries().filter(registry -> !registry.key().equals(Registries.ENCHANTMENT)),java.util.stream.Stream.of(enchantments)));
    var context=Item.TooltipContext.of(tooltipRegistries);
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
      if(input.has("custom_effects")) {
        var contents=stack.getOrDefault(DataComponents.POTION_CONTENTS,PotionContents.EMPTY);
        for(var value:input.getAsJsonArray("custom_effects")) {
          var effect=value.getAsJsonObject();
          @SuppressWarnings("unchecked")
          var holder=(Holder<MobEffect>)MobEffects.class.getField(effect.get("id").getAsString().toUpperCase(java.util.Locale.ROOT)).get(null);
          int duration=effect.has("duration")?effect.get("duration").getAsInt():0;
          int amplifier=effect.has("amplifier")?effect.get("amplifier").getAsInt():0;
          boolean ambient=effect.has("ambient")&&effect.get("ambient").getAsBoolean();
          boolean visible=!effect.has("show_particles")||effect.get("show_particles").getAsBoolean();
          boolean icon=!effect.has("show_icon")||effect.get("show_icon").getAsBoolean();
          contents=contents.withEffectAdded(new MobEffectInstance(holder,duration,amplifier,ambient,visible,icon));
        }
        stack.set(DataComponents.POTION_CONTENTS,contents);
      }
      if(input.has("scale")) stack.set(DataComponents.POTION_DURATION_SCALE,input.get("scale").getAsFloat());
      for(var key:java.util.List.of("enchantments","stored_enchantments")) if(input.has(key)) {
        var mutable=new ItemEnchantments.Mutable(ItemEnchantments.EMPTY);
        for(var entry:input.getAsJsonObject(key).entrySet()) {
          var holder=enchantments.getOrThrow(ResourceKey.create(Registries.ENCHANTMENT,Identifier.parse(entry.getKey())));
          mutable.set(holder,entry.getValue().getAsInt());
        }
        stack.set(key.equals("enchantments")?DataComponents.ENCHANTMENTS:DataComponents.STORED_ENCHANTMENTS,mutable.toImmutable());
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
      if(input.has("hide_potion") && input.get("hide_potion").getAsBoolean()) stack.set(DataComponents.TOOLTIP_DISPLAY,new TooltipDisplay(false,new java.util.LinkedHashSet<>(java.util.Set.of(DataComponents.POTION_CONTENTS))));
      if(input.has("hide_enchantments") && input.get("hide_enchantments").getAsBoolean()) stack.set(DataComponents.TOOLTIP_DISPLAY,new TooltipDisplay(false,new java.util.LinkedHashSet<>(java.util.Set.of(DataComponents.ENCHANTMENTS))));
      if(input.has("hide_stored") && input.get("hide_stored").getAsBoolean()) stack.set(DataComponents.TOOLTIP_DISPLAY,new TooltipDisplay(false,new java.util.LinkedHashSet<>(java.util.Set.of(DataComponents.STORED_ENCHANTMENTS))));
      var lines=new JsonArray();
      for(var component:stack.getTooltipLines(context,null,TooltipFlag.NORMAL)) {
        lines.add(ComponentSerialization.CODEC.encodeStart(JsonOps.INSTANCE,component).getOrThrow());
      }
      var row=new JsonObject();row.add("lines",lines);row.addProperty("description",item.getDescriptionId());
      if(input.has("custom_effects")) row.add("contents",PotionContents.CODEC.encodeStart(registries.createSerializationContext(JsonOps.INSTANCE),stack.get(DataComponents.POTION_CONTENTS)).getOrThrow());
      var spans=new JsonArray();
      for(var component:stack.getTooltipLines(context,null,TooltipFlag.NORMAL)) {
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
