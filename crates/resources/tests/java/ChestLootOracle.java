import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import it.unimi.dsi.fastutil.objects.ObjectArrayList;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.RegistryOps;
import net.minecraft.nbt.NbtOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.SimpleContainer;
import net.minecraft.world.RandomSequence;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.storage.loot.*;

public class ChestLootOracle {
 static Set<String> readTag(String kind,String name,Set<String> visiting)throws Exception{
  if(!visiting.add(name))throw new IllegalStateException("cyclic tag");
  var id=Identifier.parse(name);String path="data/"+id.getNamespace()+"/tags/"+kind+"/"+id.getPath()+".json";
  var result=new LinkedHashSet<String>();
  try(var stream=ChestLootOracle.class.getClassLoader().getResourceAsStream(path)){
   if(stream==null)throw new IllegalStateException("missing tag "+path);
   var values=JsonParser.parseString(new String(stream.readAllBytes(),java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject().getAsJsonArray("values");
   for(var value:values){String entry=value.isJsonObject()?value.getAsJsonObject().get("id").getAsString():value.getAsString();
    if(entry.startsWith("#"))result.addAll(readTag(kind,entry.substring(1),visiting));else result.add(Identifier.parse(entry).toString());}
  }visiting.remove(name);return result;
 }
 static void expandEnchantmentOptions(com.google.gson.JsonElement value)throws Exception{
  if(value.isJsonArray()){for(var child:value.getAsJsonArray())expandEnchantmentOptions(child);return;}
  if(!value.isJsonObject())return;var object=value.getAsJsonObject();
  if(object.has("type")&&object.get("type").getAsString().equals("minecraft:enchant_randomly")&&object.has("options")&&object.get("options").isJsonPrimitive()&&object.get("options").getAsString().startsWith("#")){
   var list=new com.google.gson.JsonArray();for(var id:readTag("enchantment",object.get("options").getAsString().substring(1),new HashSet<>()))list.add(id);object.add("options",list);
  }
  for(var child:object.entrySet())expandEnchantmentOptions(child.getValue());
 }
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var lookup=VanillaRegistries.createWorldLookup();var registries=VanillaRegistries.createReloadableLookup(lookup);
  for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries))pending.apply();
  var json=JsonParser.parseString(Files.readString(Path.of(args[0])));var ops=RegistryOps.create(JsonOps.INSTANCE,registries);var nbtops=RegistryOps.create(NbtOps.INSTANCE,registries);
  if(args.length>1&&args[1].equals("enchant")){
   expandEnchantmentOptions(json);
   var holders=lookup.lookupOrThrow(Registries.ENCHANTMENT).listElements().toList();
   var tags=holders.stream().map(h->h.value().definition().supportedItems().unwrapKey()).flatMap(Optional::stream).distinct().toList();
   var membership=new HashMap<String,List<net.minecraft.tags.TagKey<net.minecraft.world.item.Item>>>();
   for(var tag:tags)for(var id:readTag("item",tag.location().toString(),new HashSet<>()))membership.computeIfAbsent(id,k->new ArrayList<>()).add(tag);
   var bind=net.minecraft.core.Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
   for(var item:BuiltInRegistries.ITEM){var id=BuiltInRegistries.ITEM.getKey(item).toString();bind.invoke(item.builtInRegistryHolder(),membership.getOrDefault(id,List.of()));}
  }
  var ctor=LootContext.class.getDeclaredConstructors()[0];ctor.setAccessible(true);
  var slots=LootTable.class.getDeclaredMethod("getAvailableSlots",net.minecraft.world.Container.class,RandomSource.class);slots.setAccessible(true);
  var split=LootTable.class.getDeclaredMethod("shuffleAndSplitItems",ObjectArrayList.class,int.class,RandomSource.class);split.setAccessible(true);
  if(json.isJsonArray()){
   if(args.length>1&&args[1].equals("damage"))for(var item:List.of(Items.DIAMOND_BLOCK,Items.EMERALD_BLOCK,Items.IRON_BLOCK,Items.AMETHYST_SHARD,Items.CAKE,Items.DIAMOND_AXE,Items.DIAMOND_PICKAXE,Items.STONE_PICKAXE,Items.STONE)){
    var stack=new ItemStack(item);out.printf("V %s %d %d%n",BuiltInRegistries.ITEM.getKey(item),stack.getMaxStackSize(),stack.getMaxDamage());
   }
   int index=0;
   for(var entry:json.getAsJsonArray()){
    var table=LootTable.DIRECT_CODEC.parse(ops,entry).getOrThrow();
    for(float luck:new float[]{-2.5f,0.0f,1.5f,4.0f})for(int mode=0;mode<2;mode++)for(long seed:new long[]{1,42,-1,Long.MIN_VALUE}){
     RandomSource random=mode==0?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
     var params=new LootParams(null,ContextMap.EMPTY,Map.of(),luck);var context=(LootContext)ctor.newInstance(params,random,registries);
     var items=new ObjectArrayList<ItemStack>();
     table.getRandomItemsRaw(context,stack->{
      int remaining=stack.getCount(), max=stack.getMaxStackSize();
      if(remaining<max)items.add(stack);
      else while(remaining>0){int count=Math.min(max,remaining);items.add(stack.copyWithCount(count));remaining-=count;}
     });
     var container=new SimpleContainer(27);@SuppressWarnings("unchecked") var available=(List<Integer>)slots.invoke(table,container,random);
     split.invoke(table,items,available.size(),random);
     for(var stack:items){if(available.isEmpty())break;container.setItem(available.remove(available.size()-1),stack);}
     out.printf("S %d %s %d %d%n",index,Float.toString(luck),mode,seed);
     for(int slot=0;slot<27;slot++){var stack=container.getItem(slot);if(!stack.isEmpty()){
      if(args.length>1&&args[1].equals("damage")){
       out.printf("P %d %s %d %d%n",slot,BuiltInRegistries.ITEM.getKey(stack.getItem()),stack.getCount(),stack.getDamageValue());
       String encoded=ItemStack.CODEC.encodeStart(nbtops,stack).getOrThrow().toString();
       String component="\"minecraft:damage\":";
       if(stack.getDamageValue()==0&&encoded.contains(component))throw new AssertionError(encoded);
       if(stack.getDamageValue()>0&&!encoded.contains(component+stack.getDamageValue()))throw new AssertionError(encoded);
      }
      else if(args.length>1&&args[1].equals("enchant")){
       boolean book=stack.is(Items.ENCHANTED_BOOK);var enchantments=stack.get(book?DataComponents.STORED_ENCHANTMENTS:DataComponents.ENCHANTMENTS);String id="-";int level=0;
       if(enchantments!=null&&!enchantments.isEmpty()){var values=new ArrayList<String>();for(var e:enchantments.entrySet())values.add(e.getKey().unwrapKey().orElseThrow().identifier().toString()+"="+e.getIntValue());Collections.sort(values);id=String.join(",",values);level=enchantments.size();}
       if(!id.equals("-")){String encoded=ItemStack.CODEC.encodeStart(nbtops,stack).getOrThrow().toString();if(!encoded.contains(book?"minecraft:stored_enchantments":"minecraft:enchantments"))throw new AssertionError(encoded);}
       out.printf("P %d %s %d %s %d%n",slot,BuiltInRegistries.ITEM.getKey(stack.getItem()),stack.getCount(),id,level);
      }
      else if(args.length>1&&args[1].equals("potion")){
       var contents=stack.get(DataComponents.POTION_CONTENTS);
       String potion=contents==null?"-":contents.potion().flatMap(Holder::unwrapKey).map(k->k.identifier().toString()).orElse("-");
       out.printf("P %d %s %d %s %d%n",slot,BuiltInRegistries.ITEM.getKey(stack.getItem()),stack.getCount(),potion,stack.getDamageValue());
      }
      else out.printf("P %d %s %d%n",slot,BuiltInRegistries.ITEM.getKey(stack.getItem()),stack.getCount());
     }}
     out.println("R "+random.nextLong());
    }
    index++;
   }
   return;
  }
  var table=LootTable.DIRECT_CODEC.parse(ops,json).getOrThrow();
  for(var item:List.of(Items.HEART_OF_THE_SEA,Items.TNT,Items.PRISMARINE_CRYSTALS,Items.POTION,Items.IRON_SPEAR,Items.COPPER_NAUTILUS_ARMOR,Items.IRON_NAUTILUS_ARMOR,Items.GOLDEN_NAUTILUS_ARMOR,Items.DIAMOND_NAUTILUS_ARMOR)){
   var stack=new ItemStack(item);out.printf("V %s %d %d %s%n",BuiltInRegistries.ITEM.getKey(item),stack.getMaxStackSize(),stack.getMaxDamage(),item.getClass().getName());
  }
  for(int mode=0;mode<3;mode++)for(long seed:new long[]{0,1,2,3,7,42,12345,-1,-12345,Long.MIN_VALUE,Long.MAX_VALUE}){
   if(mode==0&&seed==0)continue;
   RandomSource random=mode==0?new LegacyRandomSource(seed):mode==1?new XoroshiroRandomSource(seed):new RandomSequence(seed,Identifier.parse("minecraft:chests/buried_treasure")).random();
   var params=new LootParams(null,ContextMap.EMPTY,Map.of(),args.length>1?Float.parseFloat(args[1]):0.0f);var context=(LootContext)ctor.newInstance(params,random,registries);
   var items=new ObjectArrayList<ItemStack>();table.getRandomItemsRaw(context,items::add);
   var container=new SimpleContainer(27);@SuppressWarnings("unchecked") var available=(List<Integer>)slots.invoke(table,container,random);
   split.invoke(table,items,available.size(),random);
   for(var stack:items){if(available.isEmpty())break;container.setItem(available.remove(available.size()-1),stack);}
   out.printf("S %d %d%n",mode,seed);
   for(int slot=0;slot<27;slot++){var stack=container.getItem(slot);if(stack.isEmpty())continue;
    var contents=stack.get(DataComponents.POTION_CONTENTS);String potion=contents==null?"-":contents.potion().flatMap(Holder::unwrapKey).map(k->k.identifier().toString()).orElse("-");
    out.printf("P %d %s %d %s%n",slot,BuiltInRegistries.ITEM.getKey(stack.getItem()),stack.getCount(),potion);
    if(!potion.equals("-"))out.println("N "+ItemStack.CODEC.encodeStart(nbtops,stack).getOrThrow());
   }
   out.println("R "+random.nextLong());
  }
 }
}
