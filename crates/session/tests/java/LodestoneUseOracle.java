import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.util.Prediction;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.LodestoneTracker;
import net.minecraft.world.item.context.UseOnContext;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;

public class LodestoneUseOracle {
  static final BlockPos POS = new BlockPos(-20, 64, 47);
  static LodestoneTracker tracker(BlockPos pos) { return new LodestoneTracker(Optional.of(GlobalPos.of(Level.OVERWORLD, pos)), true); }
  static class World extends ClientLevel {
    int sounds;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return Blocks.LODESTONE.defaultBlockState(); }
    @Override public net.minecraft.resources.ResourceKey<Level> dimension() { return Level.OVERWORLD; }
    @Override public void playSound(Entity entity, BlockPos pos, net.minecraft.sounds.SoundEvent event, net.minecraft.sounds.SoundSource source, float volume, float pitch) {
      if(entity!=null || !POS.equals(pos) || event!=net.minecraft.sounds.SoundEvents.LODESTONE_COMPASS_LOCK || source!=net.minecraft.sounds.SoundSource.PLAYERS || volume!=1.0f || pitch!=1.0f) throw new AssertionError("compass sound differs");
      sounds++;
    }
  }
  static class Player extends ServerPlayer {
    Inventory inventory; boolean creative; ItemStack dropped;
    Player() { super(null,null,null,null); }
    @Override public Inventory getInventory() { return inventory; }
    @Override public boolean hasInfiniteMaterials() { return creative; }
    @Override public void onEquipItem(EquipmentSlot slot, ItemStack before, ItemStack after) {}
    @Override public ItemEntity drop(ItemStack stack, boolean trace, Prediction prediction) { if (trace) throw new AssertionError(); dropped=stack.copy(); return null; }
  }
  static class Context extends UseOnContext {
    World world; Player player; ItemStack item;
    Context() { super(null,null,null,null,null); }
    @Override public BlockPos getClickedPos() { return POS; }
    @Override public Level getLevel() { return world; }
    @Override public Player getPlayer() { return player; }
    @Override public ItemStack getItemInHand() { return item; }
  }
  static String describe(ItemStack stack) {
    if (stack == null || stack.isEmpty()) return "-";
    if (stack.is(Items.STONE)) return "s" + stack.getCount();
    if (!stack.is(Items.COMPASS)) throw new AssertionError(stack);
    var t=stack.get(DataComponents.LODESTONE_TRACKER);
    String state = t == null ? "none" : t.equals(tracker(POS)) ? "new" : "old";
    return "c" + stack.getCount() + state;
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
    for (var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var player=(Player)unsafe.allocateInstance(Player.class);
    var context=(Context)unsafe.allocateInstance(Context.class); context.world=world; context.player=player;
    var output=new JsonArray();
    for (boolean creative:new boolean[]{false,true}) for (boolean offhand:new boolean[]{false,true})
    for (boolean full:new boolean[]{false,true}) for (boolean old:new boolean[]{false,true})
    for (boolean matching:new boolean[]{false,true}) for (int count:new int[]{1,2}) {
      player.creative=creative; player.dropped=null; world.sounds=0;
      player.inventory=new Inventory(player,new EntityEquipment());
      if(full) for(int i=0;i<36;i++) player.inventory.setItem(i,new ItemStack(Items.STONE,64));
      if(matching) { var existing=new ItemStack(Items.COMPASS,12); existing.set(DataComponents.LODESTONE_TRACKER,tracker(POS)); player.inventory.setItem(5,existing); }
      var held=new ItemStack(Items.COMPASS,count);
      if(old) held.set(DataComponents.LODESTONE_TRACKER,tracker(new BlockPos(1,2,3)));
      player.inventory.setItem(offhand?40:0,held); context.item=held;
      var result=Items.COMPASS.useOn(context);
      var row=new JsonObject(); row.addProperty("creative",creative); row.addProperty("offhand",offhand); row.addProperty("full",full); row.addProperty("old",old); row.addProperty("matching",matching); row.addProperty("count",count);
      row.addProperty("success",result instanceof net.minecraft.world.InteractionResult.Success); row.addProperty("sounds",world.sounds); row.addProperty("drop",describe(player.dropped));
      var slots=new JsonArray(); for(int i=0;i<36;i++) slots.add(describe(player.inventory.getItem(i)));
      row.add("slots",slots); row.addProperty("off",describe(player.inventory.getItem(40))); output.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(output));
  }
}
