import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.nbt.*;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.RegistryOps;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.item.*;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;

public class BannerLootOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries))pending.apply();
    var state=Blocks.BANNER.white().defaultBlockState();
    if(state.getDestroySpeed(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)!=1.0f)throw new AssertionError("unexpected banner hardness");
    var pattern=lookup.lookupOrThrow(Registries.BANNER_PATTERN).getOrThrow(BannerPatterns.CROSS);
    var source=new ItemStack(Items.BANNER.white());source.set(DataComponents.BANNER_PATTERNS,new BannerPatternLayers(List.of(new BannerPatternLayers.Layer(pattern,DyeColor.BLUE))));source.set(DataComponents.CUSTOM_NAME,Component.literal("Named"));
    var custom=new CompoundTag();custom.putInt("future",42);source.set(DataComponents.CUSTOM_DATA,net.minecraft.world.item.component.CustomData.of(custom));
    var entity=new BannerBlockEntity(BlockPos.ZERO,state);entity.applyComponentsFromItemStack(source);
    var ctor=LootContext.class.getDeclaredConstructor(LootParams.class,net.minecraft.util.RandomSource.class,HolderGetter.Provider.class);ctor.setAccessible(true);
    var params=new LootParams(null,ContextMap.builder().set(LootContextParams.BLOCK_ENTITY,entity).set(LootContextParams.BLOCK_STATE,state).set(LootContextParams.TOOL,ItemStack.EMPTY).build(),Map.of(),0f);
    var jsonops=RegistryOps.create(JsonOps.INSTANCE,registries);var nbtops=RegistryOps.create(NbtOps.INSTANCE,registries);
    var tables=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();var output=new CompoundTag();
    for(int i=0;i<tables.size();i++){
      var table=LootTable.DIRECT_CODEC.parse(jsonops,tables.get(i)).getOrThrow();var context=ctor.newInstance(params,new LegacyRandomSource(1),registries);var items=new ArrayList<ItemStack>();table.getRandomItemsRaw(context,items::add);
      if(items.size()!=1)throw new AssertionError("unexpected drops "+items.size());
      output.put("case"+i,ItemStack.CODEC.encodeStart(nbtops,items.get(0)).getOrThrow());
    }
    NbtIo.write(output,Path.of(args[1]));
  }
}
