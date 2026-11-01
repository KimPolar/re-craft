import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.util.Prediction;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.*;

public class BucketInventoryOracle {
  static class Player extends ServerPlayer {
    Inventory inventory; boolean creative; ItemStack dropped;
    Player() { super(null,null,null,null); }
    @Override public Inventory getInventory() { return inventory; }
    @Override public boolean hasInfiniteMaterials() { return creative; }
    @Override public void onEquipItem(EquipmentSlot slot, ItemStack before, ItemStack after) {}
    @Override public ItemEntity drop(ItemStack stack, boolean trace, Prediction prediction) { dropped=stack.copy(); return null; }
  }
  static String describe(ItemStack stack) {
    if (stack==null || stack.isEmpty()) return "-";
    if (stack.is(Items.STONE)) return "s"+stack.getCount();
    if (stack.is(Items.BUCKET)) return "b"+stack.getCount();
    if (stack.is(Items.WATER_BUCKET)) return "w"+stack.getCount();
    if (stack.is(Items.LAVA_BUCKET)) return "l"+stack.getCount();
    if (stack.is(Items.GLASS_BOTTLE)) return "g"+stack.getCount();
    if (stack.is(Items.POTION)) return "p"+stack.getCount();
    throw new AssertionError(stack);
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
    for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var player=(Player)unsafe.allocateInstance(Player.class); var output=new JsonArray();
    for(boolean creative:new boolean[]{false,true}) for(boolean offhand:new boolean[]{false,true})
    for(boolean full:new boolean[]{false,true}) for(boolean existing:new boolean[]{false,true})
    for(boolean lava:new boolean[]{false,true}) for(boolean bottle:new boolean[]{false,true}) for(int count:new int[]{1,2}) {
      player.creative=creative; player.dropped=null; player.inventory=new Inventory(player,new EntityEquipment());
      if(full) for(int i=0;i<36;i++) player.inventory.setItem(i,new ItemStack(Items.STONE,64));
      var result=lava?Items.LAVA_BUCKET:Items.WATER_BUCKET;
      var filled=bottle ? net.minecraft.world.item.alchemy.PotionContents.createItemStack(Items.POTION,net.minecraft.world.item.alchemy.Potions.WATER) : new ItemStack(result);
      var emptied=BucketItem.getEmptySuccessItem(new ItemStack(result),player);
      if(existing) player.inventory.setItem(5,filled.copy());
      int slot=offhand?40:0; var held=new ItemStack(bottle?Items.GLASS_BOTTLE:Items.BUCKET,count); player.inventory.setItem(slot,held);
      player.inventory.setItem(slot,ItemUtils.createFilledResult(held,player,filled));
      var row=new JsonObject(); row.addProperty("creative",creative); row.addProperty("offhand",offhand); row.addProperty("full",full); row.addProperty("existing",existing); row.addProperty("lava",lava); row.addProperty("bottle",bottle); row.addProperty("count",count); row.addProperty("drop",describe(player.dropped)); row.addProperty("empty",describe(emptied));
      var slots=new JsonArray(); for(int i=0;i<36;i++) slots.add(describe(player.inventory.getItem(i))); row.add("slots",slots); row.addProperty("off",describe(player.inventory.getItem(40))); output.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(output));
  }
}
