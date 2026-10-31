import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.server.level.ServerPlayer;
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

public class BucketFluidReplaceOracle {
  static final BlockPos CLICKED=BlockPos.ZERO, ADJACENT=CLICKED.south();
  static class Player extends ServerPlayer {
    boolean sneak;
    Player() { super(null,null,null,null); }
    @Override public boolean isShiftKeyDown() { return sneak; }
  }
  static class World extends ClientLevel {
    Map<BlockPos,BlockState> blocks; EnvironmentAttributeSystem environment; RandomSource rng;
    SoundEvent sound; SoundSource category; float volume,pitch; BlockPos soundPosition; boolean destroyed;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState()); }
    @Override public boolean isClientSide() { return false; }
    @Override public boolean setBlock(BlockPos pos,BlockState state,int flags,int depth) { if(getBlockState(pos)==state) return false; blocks.put(pos,state); return true; }
    @Override public boolean destroyBlock(BlockPos pos,boolean drops,Entity entity,int depth) { destroyed=true; blocks.put(pos,Blocks.AIR.defaultBlockState()); return true; }
    @Override public EnvironmentAttributeSystem environmentAttributes() { return environment; }
    @Override public RandomSource getRandom() { return rng; }
    @Override public void playSound(Entity e,BlockPos p,SoundEvent s,SoundSource c,float v,float pitch) { sound=s; category=c; volume=v; this.pitch=pitch; soundPosition=p; }
    @Override public void scheduleTick(BlockPos pos,Fluid fluid,int delay) {}
    @Override public void gameEvent(Holder<GameEvent> event,Vec3 pos,GameEvent.Context context) {}
  }
  static String describe(BlockState state) {
    if(state.is(Blocks.AIR)) return "air";
    if(state.is(Blocks.STONE)) return "stone";
    if(state.is(Blocks.WATER)) return "water_"+state.getValue(LiquidBlock.LEVEL);
    if(state.is(Blocks.LAVA)) return "lava_"+state.getValue(LiquidBlock.LEVEL);
    if(state.is(Blocks.STONE_SLAB)) return "slab_"+state.getValue(SlabBlock.WATERLOGGED);
    if(state.is(Blocks.SHORT_GRASS)) return "short_grass";
    if(state.is(Blocks.FERN)) return "fern";
    if(state.is(Blocks.DEAD_BUSH)) return "dead_bush";
    if(state.is(Blocks.CHEST) || state.is(Blocks.TRAPPED_CHEST)) return (state.is(Blocks.CHEST)?"chest_":"trapped_chest_")+state.getValue(ChestBlock.TYPE).getSerializedName()+"_"+state.getValue(ChestBlock.WATERLOGGED);
    throw new AssertionError(state);
  }
  static void run(JsonArray out,World world,Player player,boolean lava,boolean evaporates,boolean sneak,String initial,int level) {
    player.sneak=sneak; world.blocks=new HashMap<>(); world.rng=RandomSource.create(123); world.sound=null; world.soundPosition=null; world.destroyed=false;
    boolean slab=initial.startsWith("slab_");
    boolean chest=initial.startsWith("chest_") || initial.startsWith("trapped_chest_");
    BlockState clicked=slab?Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.TOP).setValue(SlabBlock.WATERLOGGED,initial.equals("slab_wet")):chest?(initial.startsWith("trapped_")?Blocks.TRAPPED_CHEST:Blocks.CHEST).defaultBlockState().setValue(ChestBlock.TYPE,new ChestType[]{ChestType.SINGLE,ChestType.LEFT,ChestType.RIGHT}[level]).setValue(ChestBlock.WATERLOGGED,initial.endsWith("_wet")):Blocks.STONE.defaultBlockState();
    world.blocks.put(CLICKED,clicked);
    if(!slab && !chest) world.blocks.put(ADJACENT,switch(initial) { case "short_grass" -> Blocks.SHORT_GRASS.defaultBlockState(); case "fern" -> Blocks.FERN.defaultBlockState(); case "dead_bush" -> Blocks.DEAD_BUSH.defaultBlockState(); default -> (initial.equals("water")?Blocks.WATER:Blocks.LAVA).defaultBlockState().setValue(LiquidBlock.LEVEL,level); });
    world.environment=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributes.WATER_EVAPORATES, base -> evaporates).build();
    var bucket=(BucketItem)(lava?Items.LAVA_BUCKET:Items.WATER_BUCKET);
    var hit=new BlockHitResult(new Vec3(0.5,0.75,1.0),Direction.SOUTH,CLICKED,false);
    var target=!lava && clicked.getBlock() instanceof LiquidBlockContainer?CLICKED:ADJACENT;
    boolean success=bucket.emptyContents(player,world,target,hit);
    var row=new JsonObject(); row.addProperty("lava",lava); row.addProperty("evaporates",evaporates); row.addProperty("sneak",sneak); row.addProperty("initial",initial); row.addProperty("level",level); row.addProperty("success",success);
    row.addProperty("clicked",describe(world.getBlockState(CLICKED))); row.addProperty("adjacent",describe(world.getBlockState(ADJACENT))); row.addProperty("next_random",world.rng.nextInt());
    row.addProperty("destroyed",world.destroyed);
    if(world.sound!=null) { row.addProperty("sound",world.sound.location().toString()); row.addProperty("sound_z",world.soundPosition.getZ()); row.addProperty("volume_bits",Float.floatToRawIntBits(world.volume)); row.addProperty("pitch_bits",Float.floatToRawIntBits(world.pitch)); }
    out.add(row);
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
    for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var bindTags=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bindTags.setAccessible(true);
    bindTags.invoke(net.minecraft.core.registries.BuiltInRegistries.FLUID.wrapAsHolder(net.minecraft.world.level.material.Fluids.WATER),List.of(net.minecraft.tags.FluidTags.WATER));
    bindTags.invoke(net.minecraft.core.registries.BuiltInRegistries.FLUID.wrapAsHolder(net.minecraft.world.level.material.Fluids.LAVA),List.of(net.minecraft.tags.FluidTags.LAVA));
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var player=(Player)unsafe.allocateInstance(Player.class); var out=new JsonArray();
    for(boolean lava:new boolean[]{false,true}) for(boolean evaporates:new boolean[]{false,true}) for(boolean sneak:new boolean[]{false,true})
      for(String initial:new String[]{"water","lava"}) for(int level=0;level<16;level++) run(out,world,player,lava,evaporates,sneak,initial,level);
    for(boolean evaporates:new boolean[]{false,true}) for(boolean sneak:new boolean[]{false,true}) for(String initial:new String[]{"slab_dry","slab_wet"})
      run(out,world,player,false,evaporates,sneak,initial,0);
    for(boolean evaporates:new boolean[]{false,true}) for(boolean sneak:new boolean[]{false,true}) for(String initial:new String[]{"chest_dry","chest_wet","trapped_chest_dry","trapped_chest_wet"}) for(int type=0;type<3;type++)
      run(out,world,player,false,evaporates,sneak,initial,type);
    for(boolean lava:new boolean[]{false,true}) for(boolean evaporates:new boolean[]{false,true}) for(boolean sneak:new boolean[]{false,true}) for(String initial:new String[]{"short_grass","fern","dead_bush"})
      run(out,world,player,lava,evaporates,sneak,initial,0);
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
