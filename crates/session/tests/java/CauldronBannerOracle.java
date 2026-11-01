import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.core.cauldron.CauldronInteractions;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.util.Prediction;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.*;

public class CauldronBannerOracle {
  static class Player extends CauldronOracle.Player {
    ItemStack dropped;
    Player() { super(); }
    @Override public ItemEntity drop(ItemStack stack, boolean trace, Prediction prediction) { dropped=stack.copy(); return null; }
  }
  static com.mojang.serialization.DynamicOps<JsonElement> ops;
  static JsonElement describe(ItemStack stack) {
    if(stack==null || stack.isEmpty()) return JsonNull.INSTANCE;
    var obj=new JsonObject(); obj.addProperty("id",BuiltInRegistries.ITEM.getKey(stack.getItem()).toString()); obj.addProperty("count",stack.getCount());
    var patterns=stack.get(DataComponents.BANNER_PATTERNS); if(patterns!=null) obj.add("patterns",BannerPatternLayers.CODEC.encodeStart(ops,patterns).getOrThrow());
    return obj;
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup(); var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply(); ops=lookup.createSerializationContext(JsonOps.INSTANCE);
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(CauldronOracle.World)unsafe.allocateInstance(CauldronOracle.World.class); var player=(Player)unsafe.allocateInstance(Player.class);
    var patterns=lookup.lookupOrThrow(Registries.BANNER_PATTERN); var first=new BannerPatternLayers.Layer(patterns.getOrThrow(BannerPatterns.STRIPE_BOTTOM),DyeColor.RED); var second=new BannerPatternLayers.Layer(patterns.getOrThrow(BannerPatterns.CROSS),DyeColor.BLUE); var direct=new BannerPatternLayers.Layer(Holder.direct(new BannerPattern(net.minecraft.resources.Identifier.parse("example:custom"),"example.banner")),DyeColor.GREEN);
    var output=new JsonObject(); output.addProperty("max",new ItemStack(Items.BANNER.white()).getMaxStackSize()); output.addProperty("default_patterns",new ItemStack(Items.BANNER.white()).has(DataComponents.BANNER_PATTERNS));
    var rows=new JsonArray();
    for(int layers=0;layers<=3;layers++) for(int count:new int[]{1,4,16}) for(boolean creative:new boolean[]{false,true}) for(boolean off:new boolean[]{false,true}) for(boolean full:new boolean[]{false,true}) for(boolean existing:new boolean[]{false,true}) {
      player.creative=creative; player.dropped=null; player.inventory=new Inventory(player,new EntityEquipment()); if(full) for(int i=0;i<36;i++) player.inventory.setItem(i,new ItemStack(Items.STONE,64));
      var stack=new ItemStack(Items.BANNER.white(),count); if(layers>0) stack.set(DataComponents.BANNER_PATTERNS,new BannerPatternLayers(layers==1?List.of(first):layers==2?List.of(first,second):List.of(first,direct,second)));
      var cleaned=stack.copyWithCount(1); if(layers>0) cleaned.set(DataComponents.BANNER_PATTERNS,stack.get(DataComponents.BANNER_PATTERNS).removeLast()); if(existing) player.inventory.setItem(5,cleaned.copy());
      int slot=off?40:0; player.inventory.setItem(slot,stack); world.state=Blocks.WATER_CAULDRON.defaultBlockState().setValue(LayeredCauldronBlock.LEVEL,2); world.sound=null; world.aboveWater=false;
      var result=CauldronInteractions.WATER.get(stack).interact(world.state,world,BlockPos.ZERO,player,off?InteractionHand.OFF_HAND:InteractionHand.MAIN_HAND,stack);
      var row=new JsonObject(); row.addProperty("layers",layers); row.addProperty("count",count); row.addProperty("creative",creative); row.addProperty("off",off); row.addProperty("full",full); row.addProperty("existing",existing); row.addProperty("handled",result.consumesAction()); row.addProperty("level",world.state.getValue(LayeredCauldronBlock.LEVEL)); row.add("dropped",describe(player.dropped)); row.add("offhand",describe(player.inventory.getItem(40))); var slots=new JsonArray(); for(int i=0;i<36;i++) slots.add(describe(player.inventory.getItem(i))); row.add("slots",slots); rows.add(row);
    }
    output.add("rows",rows); Files.writeString(Path.of(args[0]),output.toString());
  }
}
