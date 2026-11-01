import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.Holder;
import net.minecraft.resources.Identifier;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.sounds.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.Vec3;

public class EquipmentBreakOracle {
  static class World extends ClientLevel {
    JsonArray particles, sounds;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public void addParticle(ParticleOptions option,double x,double y,double z,double dx,double dy,double dz) {
      var row=new JsonArray(); for(double value:new double[]{x,y,z,dx,dy,dz}) row.add(Double.doubleToRawLongBits(value)); particles.add(row);
    }
    @Override public void playLocalSound(double x,double y,double z,SoundEvent sound,SoundSource source,float volume,float pitch,boolean delay) {
      var row=new JsonObject(); row.addProperty("event",sound.location().toString()); row.addProperty("volume",Float.floatToRawIntBits(volume)); row.addProperty("pitch",Float.floatToRawIntBits(pitch)); sounds.add(row);
    }
  }
  static class TestPlayer extends Player {
    World world; float yaw,pitch; boolean silent;
    TestPlayer() { super(null,null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public Level level() { return world; }
    @Override public boolean isSilent() { return silent; }
    @Override public float getYRot() { return yaw; }
    @Override public float getXRot() { return pitch; }
    @Override public double getEyeY() { return 65.62; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
    for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var registered = net.minecraft.core.registries.BuiltInRegistries.SOUND_EVENT.wrapAsHolder(SoundEvents.ANVIL_BREAK);
    var inline = Holder.direct(SoundEvent.createFixedRangeEvent(Identifier.parse("example:hammer"),8f));
    var ops = net.minecraft.resources.RegistryOps.create(com.mojang.serialization.JsonOps.INSTANCE,registries);
    if(!SoundEvent.CODEC.encodeStart(ops,registered).getOrThrow().getAsString().equals("minecraft:block.anvil.break")) throw new AssertionError("registered sound codec");
    var inlineJson=SoundEvent.CODEC.encodeStart(ops,inline).getOrThrow().getAsJsonObject();
    if(!inlineJson.get("sound_id").getAsString().equals("example:hammer") || inlineJson.get("range").getAsFloat()!=8f) throw new AssertionError("inline sound codec");
    for(var type:new Item[]{Items.LEATHER_HELMET,Items.GOLDEN_HELMET,Items.COPPER_HELMET,Items.CHAINMAIL_HELMET,Items.IRON_HELMET,Items.DIAMOND_HELMET,Items.NETHERITE_HELMET,Items.TURTLE_HELMET,Items.STONE_PICKAXE}) {
      var sound=new ItemStack(type).get(DataComponents.BREAK_SOUND);
      if(sound==null || !sound.value().location().toString().equals("minecraft:entity.item.break")) throw new AssertionError("unexpected equipment break sound for "+type);
    }
    var shieldSound = new ItemStack(Items.SHIELD).get(DataComponents.BREAK_SOUND);
    if(shieldSound==null || !shieldSound.value().location().toString().equals("minecraft:item.shield.break")) throw new AssertionError("unexpected shield break sound: "+shieldSound);
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.world=world;
    var position=Entity.class.getDeclaredField("position"); position.setAccessible(true); position.set(player,new Vec3(12.25,64,-30.75));
    var random=Entity.class.getDeclaredField("random"); long randomOffset=unsafe.objectFieldOffset(random);
    var method=net.minecraft.world.entity.LivingEntity.class.getDeclaredMethod("breakItem",ItemStack.class); method.setAccessible(true);
    var out=new JsonArray();
    for(int seed=0;seed<8;seed++) for(boolean silent:new boolean[]{false,true}) for(int variant=0;variant<4;variant++) for(float yaw:new float[]{-37.5f,0f,90f}) for(float pitch:new float[]{-25.5f,0f,40f}) {
      player.yaw=yaw; player.pitch=pitch; player.silent=silent; unsafe.putObject(player,randomOffset,RandomSource.create(seed)); world.particles=new JsonArray(); world.sounds=new JsonArray();
      var item=new ItemStack(Items.IRON_HELMET);
      if(variant==1) item.remove(DataComponents.BREAK_SOUND);
      if(variant==2) item.set(DataComponents.BREAK_SOUND,registered);
      if(variant==3) item.set(DataComponents.BREAK_SOUND,inline);
      method.invoke(player,item);
      var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("silent",silent); row.addProperty("variant",variant); row.addProperty("yaw",yaw); row.addProperty("pitch",pitch);
      row.add("particles",world.particles); row.add("sounds",world.sounds); row.addProperty("next_bits",Float.floatToRawIntBits(player.getRandom().nextFloat())); out.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
