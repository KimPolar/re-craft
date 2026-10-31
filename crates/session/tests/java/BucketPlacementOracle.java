import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.item.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.phys.*;
import net.minecraft.sounds.*;

public class BucketPlacementOracle {
  static final BlockPos CLICKED=new BlockPos(0,0,0), ADJACENT=CLICKED.south();
  static class World extends ClientLevel {
    Map<BlockPos,BlockState> blocks; EnvironmentAttributeSystem environment; RandomSource rng;
    SoundEvent sound; SoundSource soundSource; float volume, pitch; BlockPos soundPosition;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
    @Override public boolean isClientSide() { return false; }
    @Override public boolean setBlock(BlockPos pos, BlockState value, int flags, int depth) { blocks.put(pos,value); return true; }
    @Override public EnvironmentAttributeSystem environmentAttributes() { return environment; }
    @Override public RandomSource getRandom() { return rng; }
    @Override public void playSound(Entity entity, BlockPos pos, SoundEvent sound, SoundSource source, float volume, float pitch) { this.sound=sound; this.soundSource=source; this.volume=volume; this.pitch=pitch; this.soundPosition=pos; }
    @Override public void scheduleTick(BlockPos pos, Fluid fluid, int delay) {}
    @Override public void gameEvent(Holder<GameEvent> event, Vec3 pos, GameEvent.Context context) {}
  }
  static String describe(BlockState state) {
    if(state.is(Blocks.AIR)) return "air";
    if(state.is(Blocks.WATER)) return "water";
    if(state.is(Blocks.LAVA)) return "lava";
    if(state.is(Blocks.STONE)) return "stone";
    if(state.is(Blocks.STONE_SLAB)) return "slab_"+state.getValue(SlabBlock.TYPE)+"_"+state.getValue(SlabBlock.WATERLOGGED);
    throw new AssertionError(state);
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
    for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var bindTags=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bindTags.setAccessible(true);
    bindTags.invoke(net.minecraft.core.registries.BuiltInRegistries.FLUID.wrapAsHolder(net.minecraft.world.level.material.Fluids.WATER),List.of(net.minecraft.tags.FluidTags.WATER));
    bindTags.invoke(net.minecraft.core.registries.BuiltInRegistries.FLUID.wrapAsHolder(net.minecraft.world.level.material.Fluids.LAVA),List.of(net.minecraft.tags.FluidTags.LAVA));
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var output=new JsonArray();
    for(boolean lava:new boolean[]{false,true}) for(boolean evaporates:new boolean[]{false,true})
    for(String shape:new String[]{"stone","slab_dry","slab_wet","slab_double"}) {
      // This fake level does not run the neighbor updates that turn lava next
      // to a waterlogged slab into obsidian.
      if(lava && shape.equals("slab_wet")) continue;
      BlockState clicked=shape.equals("stone")?Blocks.STONE.defaultBlockState():Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,shape.equals("slab_double")?SlabType.DOUBLE:SlabType.TOP).setValue(SlabBlock.WATERLOGGED,shape.equals("slab_wet"));
      world.blocks=new HashMap<>(); world.blocks.put(CLICKED,clicked); world.rng=RandomSource.create(123); world.sound=null; world.soundPosition=null;
      world.environment=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributes.WATER_EVAPORATES, base -> evaporates).build();
      var bucket=(BucketItem)(lava?Items.LAVA_BUCKET:Items.WATER_BUCKET);
      var hit=new BlockHitResult(new Vec3(0.5,0.75,1.0), Direction.SOUTH, CLICKED, false);
      var target=!lava && clicked.getBlock() instanceof LiquidBlockContainer?CLICKED:ADJACENT;
      boolean success=bucket.emptyContents(null,world,target,hit);
      var row=new JsonObject(); row.addProperty("lava",lava); row.addProperty("evaporates",evaporates); row.addProperty("shape",shape); row.addProperty("success",success); row.addProperty("clicked",describe(world.getBlockState(CLICKED))); row.addProperty("adjacent",describe(world.getBlockState(ADJACENT))); row.addProperty("next_random",world.rng.nextInt()); row.addProperty("sound",world.sound.location().toString()); row.addProperty("category",world.soundSource.getName()); row.addProperty("volume_bits",Float.floatToRawIntBits(world.volume)); row.addProperty("pitch_bits",Float.floatToRawIntBits(world.pitch)); row.addProperty("sound_z",world.soundPosition.getZ()); output.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(output));
  }
}
