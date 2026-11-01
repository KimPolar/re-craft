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
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.storage.loot.*;

public class ChestLootOracle {
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var lookup=VanillaRegistries.createWorldLookup();var registries=VanillaRegistries.createReloadableLookup(lookup);
  for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries))pending.apply();
  var json=JsonParser.parseString(Files.readString(Path.of(args[0])));var ops=RegistryOps.create(JsonOps.INSTANCE,registries);
  var table=LootTable.DIRECT_CODEC.parse(ops,json).getOrThrow();var nbtops=RegistryOps.create(NbtOps.INSTANCE,registries);
  for(var item:List.of(Items.HEART_OF_THE_SEA,Items.TNT,Items.PRISMARINE_CRYSTALS,Items.POTION,Items.IRON_SPEAR,Items.COPPER_NAUTILUS_ARMOR,Items.IRON_NAUTILUS_ARMOR,Items.GOLDEN_NAUTILUS_ARMOR,Items.DIAMOND_NAUTILUS_ARMOR)){
   var stack=new ItemStack(item);out.printf("V %s %d %d %s%n",BuiltInRegistries.ITEM.getKey(item),stack.getMaxStackSize(),stack.getMaxDamage(),item.getClass().getName());
  }
  var ctor=LootContext.class.getDeclaredConstructors()[0];ctor.setAccessible(true);
  var slots=LootTable.class.getDeclaredMethod("getAvailableSlots",net.minecraft.world.Container.class,RandomSource.class);slots.setAccessible(true);
  var split=LootTable.class.getDeclaredMethod("shuffleAndSplitItems",ObjectArrayList.class,int.class,RandomSource.class);split.setAccessible(true);
  for(int mode=0;mode<2;mode++)for(long seed:new long[]{1,2,3,7,42,12345,-1,-12345,Long.MIN_VALUE,Long.MAX_VALUE}){
   RandomSource random=mode==0?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
   var params=new LootParams(null,ContextMap.EMPTY,Map.of(),0.0f);var context=(LootContext)ctor.newInstance(params,random,registries);
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
