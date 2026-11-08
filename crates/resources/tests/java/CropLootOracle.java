import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.component.DataComponents;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.RegistryOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.enchantment.Enchantments;
import net.minecraft.world.item.enchantment.ItemEnchantments;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;

public class CropLootOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var ops=RegistryOps.create(JsonOps.INSTANCE,registries);
        var ctor=LootContext.class.getDeclaredConstructors()[0]; ctor.setAccessible(true);
        var fortune=registries.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.FORTUNE);
        var output=new JsonArray();
        var names=List.of("wheat","carrots","potatoes","beetroots");
        var blocks=List.of(Blocks.WHEAT,Blocks.CARROTS,Blocks.POTATOES,Blocks.BEETROOTS);
        try (var jar=new ZipFile(args[0])) {
            for(int kind=0;kind<names.size();kind++) {
                var entry=jar.getEntry("data/minecraft/loot_table/blocks/"+names.get(kind)+".json");
                var json=JsonParser.parseString(new String(jar.getInputStream(entry).readAllBytes(),java.nio.charset.StandardCharsets.UTF_8));
                var table=LootTable.DIRECT_CODEC.parse(ops,json).getOrThrow();
                int max=kind==3 ? 3 : 7;
                var property=kind==3 ? BeetrootBlock.AGE : CropBlock.AGE;
                for(int age=0;age<=max;age++) for(int level:new int[]{0,1,3}) for(int mode=0;mode<2;mode++) for(long seed=-16;seed<16;seed++) {
                    var state=blocks.get(kind).defaultBlockState().setValue(property,age);
                    var tool=level==0 ? ItemStack.EMPTY : new ItemStack(Items.DIAMOND_HOE);
                    if(level>0) { var enchantments=new ItemEnchantments.Mutable(ItemEnchantments.EMPTY); enchantments.set(fortune,level); tool.set(DataComponents.ENCHANTMENTS,enchantments.toImmutable()); }
                    var params=new LootParams(null,ContextMap.builder().set(LootContextParams.TOOL,tool).set(LootContextParams.BLOCK_STATE,state).build(),Map.of(),0.0f);
                    net.minecraft.util.RandomSource random=mode==0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                    var context=(LootContext)ctor.newInstance(params,random,registries);
                    var items=new ArrayList<ItemStack>(); table.getRandomItemsRaw(context,items::add);
                    var row=new JsonObject(); var drops=new JsonArray();
                    for(var stack:items) if(!stack.isEmpty()) {var item=new JsonArray(); item.add(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString()); item.add(stack.getCount()); drops.add(item);}
                    row.addProperty("kind",kind); row.addProperty("age",age); row.addProperty("level",level); row.addProperty("mode",mode); row.addProperty("seed",seed);
                    row.add("drops",drops); row.addProperty("next",random.nextLong()); output.add(row);
                }
            }
        }
        Files.writeString(Path.of(args[1]),output.toString());
    }
}
