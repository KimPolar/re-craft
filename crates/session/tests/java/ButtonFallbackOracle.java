import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.entity.player.Player;

public class ButtonFallbackOracle {
  static class World extends ClientLevel {
    int mask;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) {
      for (var direction : Direction.values())
        if (BlockPos.ZERO.relative(direction).equals(pos))
          return ((mask & (1 << direction.ordinal())) != 0 ? Blocks.STONE : Blocks.AIR).defaultBlockState();
      return Blocks.AIR.defaultBlockState();
    }
  }
  static class TestPlayer extends Player {
    float yaw, pitch;
    TestPlayer() { super(null,null); }
    @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
    @Override public float getViewXRot(float partial) { return pitch; }
    @Override public float getViewYRot(float partial) { return yaw; }
  }
  static class Context extends BlockPlaceContext {
    Level world; Direction face; TestPlayer player; net.minecraft.world.item.ItemStack item;
    Context() { super(null,null,null,null,null); }
    void setReplacement(boolean value) { replaceClicked=value; }
    @Override public Direction getClickedFace() { return face; }
    @Override public Player getPlayer() { return player; }
    @Override public Direction getHorizontalDirection() { return Direction.fromYRot(player.yaw); }
    @Override public Level getLevel() { return world; }
    @Override public BlockPos getClickedPos() { return BlockPos.ZERO; }
    @Override public net.minecraft.world.item.ItemStack getItemInHand() { return item; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    net.minecraft.world.item.Items.STONE_BUTTON.builtInRegistryHolder().bindComponents(net.minecraft.core.component.DataComponentMap.builder().set(net.minecraft.core.component.DataComponents.MAX_STACK_SIZE,64).build());
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var world=(World)unsafe.allocateInstance(World.class);
    var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    var context=(Context)unsafe.allocateInstance(Context.class); context.world=world; context.player=player;
    context.item = new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.STONE_BUTTON,1);
    for (var block : new net.minecraft.world.level.block.Block[]{Blocks.AIR,Blocks.CAVE_AIR,Blocks.VOID_AIR,Blocks.WATER,Blocks.LAVA,Blocks.FIRE,Blocks.SOUL_FIRE,Blocks.SHORT_GRASS,Blocks.FERN,Blocks.DEAD_BUSH})
      if (!block.defaultBlockState().canBeReplaced(context)) throw new AssertionError("expected replaceable " + block);
    for (var block : new net.minecraft.world.level.block.Block[]{Blocks.STONE,Blocks.STONE_SLAB,Blocks.STONE_BUTTON})
      if (block.defaultBlockState().canBeReplaced(context)) throw new AssertionError("unexpected replaceable " + block);
    var result=new JsonArray();
    for (var item:JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray()) {
      var input=item.getAsJsonObject(); context.face=Direction.valueOf(input.get("face").getAsString());
      player.yaw=input.get("yaw").getAsFloat(); player.pitch=input.get("pitch").getAsFloat(); world.mask=input.get("mask").getAsInt(); context.setReplacement(input.get("replace").getAsBoolean());
      var row=new JsonObject(); var order=new JsonArray();
      for (var direction:context.getNearestLookingDirections()) order.add(direction.name());
      row.add("order",order);
      var state=Blocks.STONE_BUTTON.getStateForPlacement(context);
      if (state==null) row.add("state",JsonNull.INSTANCE);
      else { var properties=new JsonObject(); state.getValues().forEach(value -> properties.addProperty(value.property().getName(),value.valueName())); row.add("state",properties); }
      result.add(row);
    }
    Files.writeString(Path.of(args[1]),new GsonBuilder().serializeNulls().create().toJson(result));
  }
}
