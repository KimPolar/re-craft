import java.nio.file.*;
import java.util.*;
import java.util.zip.ZipFile;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.context.UseOnContext;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;

public class ToolPassOracle {
  static class World extends ClientLevel {
    BlockState target;
    RandomSource rng;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return pos.equals(BlockPos.ZERO) ? target : Blocks.AIR.defaultBlockState(); }
    @Override public RandomSource getRandom() { return rng; }
  }
  static class TestPlayer extends Player {
    TestPlayer() { super(null,null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public ItemStack getOffhandItem() { return ItemStack.EMPTY; }
    @Override public boolean isSecondaryUseActive() { return false; }
  }
  static class Context extends UseOnContext {
    Context(World world,Player player,ItemStack stack,Direction face) {
      super(world,player,InteractionHand.MAIN_HAND,stack,new BlockHitResult(Vec3.atCenterOf(BlockPos.ZERO),face,BlockPos.ZERO,false));
    }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Map<TagKey<Block>,List<Holder<Block>>> tags=new HashMap<>();
    try(var jar=new ZipFile(args[1])) {
      for(String name:List.of("turns_into_dirt_path","turns_into_farmland","air")) {
        var entry=jar.getEntry("data/minecraft/tags/block/"+name+".json");
        var json=JsonParser.parseString(new String(jar.getInputStream(entry).readAllBytes(),java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject();
        List<Holder<Block>> values=new ArrayList<>();
        for(var element:json.getAsJsonArray("values")) {
          String id=element.getAsString(); if(id.startsWith("#")) throw new IllegalStateException("nested tag");
          values.add(BuiltInRegistries.BLOCK.getValue(Identifier.parse(id)).builtInRegistryHolder());
        }
        tags.put(TagKey.create(Registries.BLOCK,Identifier.withDefaultNamespace(name)),values);
      }
    }
    BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,tags)).apply();
    var lookup=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(lookup)) pending.apply();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); world.rng=RandomSource.create(1L);
    var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    var rows=new JsonArray();
    for(String material:List.of("wooden","stone","copper","iron","golden","diamond","netherite"))
      for(String kind:List.of("axe","shovel","hoe"))
        for(String target:List.of("stone","snow_block","obsidian","stone_bricks"))
          for(Direction face:Direction.values()) {
            var item=BuiltInRegistries.ITEM.getValue(Identifier.withDefaultNamespace(material+"_"+kind));
            var stack=new ItemStack(item,1);
            world.target=BuiltInRegistries.BLOCK.getValue(Identifier.withDefaultNamespace(target)).defaultBlockState();
            var result=item.useOn(new Context(world,player,stack,face));
            var row=new JsonObject(); row.addProperty("tool",material+"_"+kind); row.addProperty("target",target);
            row.addProperty("face",face.getName()); row.addProperty("passed",result==InteractionResult.PASS); rows.add(row);
          }
    Files.writeString(Path.of(args[0]),rows.toString());
  }
}
