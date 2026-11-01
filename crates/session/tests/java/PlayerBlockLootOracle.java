import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.RandomSequence;
import net.minecraft.world.item.*;
import net.minecraft.world.item.enchantment.Enchantments;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;

public class PlayerBlockLootOracle {
  static class World extends net.minecraft.client.multiplayer.ClientLevel {
    net.minecraft.util.RandomSource seeded;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public net.minecraft.util.RandomSource getRandom() { return seeded; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var ops=RegistryOps.create(JsonOps.INSTANCE,registries);
    var predicate=JsonParser.parseString(Files.readString(Path.of(args[0])));
    var ctor=LootContext.class.getDeclaredConstructors()[0]; ctor.setAccessible(true);
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var output=new JsonArray();
    for(String name:new String[]{"coal_ore","stone_slab"}) {
      var json=JsonParser.parseString(Files.readString(Path.of(name.equals("coal_ore")?args[1]:args[2]))).getAsJsonObject();
      if(name.equals("coal_ore")) json.getAsJsonArray("pools").get(0).getAsJsonObject().getAsJsonArray("entries").get(0).getAsJsonObject().getAsJsonArray("children").get(0).getAsJsonObject().add("condition",predicate.deepCopy());
      var table=LootTable.DIRECT_CODEC.parse(ops,json).getOrThrow();
      for(int variant=0;variant<3;variant++) for(long seed:new long[]{0,42,-17}) for(int step=0;step<8;step++) {
        var tool=new ItemStack(Items.IRON_PICKAXE);
        var enchants=registries.lookupOrThrow(Registries.ENCHANTMENT);
        if(name.equals("coal_ore") && variant==1) tool.enchant(enchants.getOrThrow(Enchantments.FORTUNE),3);
        if(name.equals("coal_ore") && variant==2) tool.enchant(enchants.getOrThrow(Enchantments.SILK_TOUCH),1);
        var block=name.equals("coal_ore")?Blocks.COAL_ORE.defaultBlockState():Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,new SlabType[]{SlabType.BOTTOM,SlabType.TOP,SlabType.DOUBLE}[variant]);
        var random=new RandomSequence(seed,Identifier.parse("minecraft:blocks/"+name)).random();
        var world=(World)unsafe.allocateInstance(World.class); world.seeded=new net.minecraft.world.level.levelgen.LegacyRandomSource(9182);
        var params=new LootParams(null,ContextMap.builder().set(LootContextParams.TOOL,tool).set(LootContextParams.BLOCK_STATE,block).build(),Map.of(),0.0f);
        List<ItemStack> items=List.of();
        for(int n=0;n<=step;n++) {
          var context=(LootContext)ctor.newInstance(params,random,registries); var next=new ArrayList<ItemStack>();
          table.getRandomItemsRaw(context,next::add); items=next;
          for(var item:items) if(!item.isEmpty()) Block.popResource(world,net.minecraft.core.BlockPos.ZERO,item);
          if(name.equals("coal_ore")) net.minecraft.util.valueproviders.UniformInt.of(0,2).sample(world.seeded);
        }
        var drops=new JsonArray(); for(var stack:items) if(!stack.isEmpty()) {
          var item=new JsonArray(); item.add(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString()); item.add(stack.getCount()); drops.add(item);
        }
        var row=new JsonObject(); row.addProperty("name",name); row.addProperty("variant",variant); row.addProperty("seed",seed); row.addProperty("step",step); row.add("drops",drops); row.addProperty("next",random.nextLong()); row.addProperty("levelNext",world.seeded.nextDouble()); output.add(row);
      }
    }
    Files.writeString(Path.of(args[3]),output.toString());
  }
}
