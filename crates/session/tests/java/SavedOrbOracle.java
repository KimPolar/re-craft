import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.util.ExtraCodecs;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.item.ItemStack;

public class SavedOrbOracle {
  static class TestLevel extends ClientLevel {
    TestLevel() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public int getNextEntityId() { return 1; }
    @Override public boolean noCollision(net.minecraft.world.phys.AABB box) { return true; }
  }
  static class TestOrb extends ExperienceOrb {
    TestOrb(TestLevel level) { super(level,0,0,0,7); }
    void loadExtra(ValueInput input) { super.readAdditionalSaveData(input); }
  }
  static class TestPlayer extends ServerPlayer {
    int gained;
    TestPlayer() { super(null,null,null,null); }
    @Override public ItemStack getItemBySlot(EquipmentSlot slot) { return ItemStack.EMPTY; }
    @Override public void take(Entity entity, int count) {}
    @Override public void giveExperiencePoints(int amount) { gained += amount; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registry = VanillaRegistries.createWorldLookup();
    if (args.length > 1 && args[1].equals("--pickup")) {
      var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
      var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
      var level=(TestLevel)unsafe.allocateInstance(TestLevel.class);
      var countField=ExperienceOrb.class.getDeclaredField("count"); countField.setAccessible(true);
      var rows=new JsonArray();
      for (int value : new int[]{1,7,17}) for (int count : new int[]{1,2,5}) for (int attempts : new int[]{1,2,3}) {
        var orb=new TestOrb(level); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        var tag=new CompoundTag(); tag.putShort("Value",(short)value); tag.putInt("Count",count);
        orb.loadExtra(TagValueInput.create(ProblemReporter.DISCARDING,registry,tag));
        for (int i=0;i<attempts && !orb.isRemoved();i++) { player.takeXpDelay=0; orb.playerTouch(player); }
        var row=new JsonObject(); row.addProperty("value",value); row.addProperty("count",count); row.addProperty("attempts",attempts);
        row.addProperty("remaining",countField.getInt(orb)); row.addProperty("gained",player.gained); row.addProperty("removed",orb.isRemoved()); rows.add(row);
      }
      Files.writeString(Path.of(args[0],"pickup.json"),new Gson().toJson(rows)); return;
    }
    if (args.length > 2 && args[2].equals("--live")) {
      var tag = NbtIo.read(Path.of(args[1]));
      var input=TagValueInput.create(ProblemReporter.DISCARDING,registry,tag);
      var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
      var level=(TestLevel)((sun.misc.Unsafe)unsafeField.get(null)).allocateInstance(TestLevel.class);
      var orb=new TestOrb(level); orb.loadExtra(input);
      var count=ExperienceOrb.class.getDeclaredField("count"); count.setAccessible(true);
      var age=ExperienceOrb.class.getDeclaredField("age"); age.setAccessible(true);
      if (orb.getValue()!=7 || count.getInt(orb)!=1 || age.getInt(orb)!=4 ||
          net.minecraft.core.UUIDUtil.CODEC.parse(NbtOps.INSTANCE,tag.get("UUID")).result().isEmpty() ||
          input.read("Pos",net.minecraft.world.phys.Vec3.CODEC).isEmpty() ||
          input.read("Motion",net.minecraft.world.phys.Vec3.CODEC).isEmpty())
        throw new AssertionError("live Rust orb draft differs from Java entity load");
      return;
    }
    Tag[] values = {ShortTag.valueOf((short)-32768), ShortTag.valueOf((short)32767),
      IntTag.valueOf(65535), LongTag.valueOf(65538), FloatTag.valueOf(-1.25f),
      DoubleTag.valueOf(8.75), StringTag.valueOf("invalid"), IntTag.valueOf(0),
      IntTag.valueOf(3), FloatTag.valueOf(1.9f), LongTag.valueOf(2147483648L)};
    var out = new JsonArray();
    for (int i=0; i<values.length; i++) {
      var tag = new CompoundTag(); tag.putString("id","minecraft:experience_orb");
      tag.putIntArray("UUID",new int[]{1,2,3,4});
      var pos = new ListTag(); for (double v : new double[]{1.25,64,-2.5}) pos.add(DoubleTag.valueOf(v));
      tag.put("Pos",pos); tag.put("Age",values[i]); tag.put("Value",values[i]);
      tag.put("Health",values[i]); tag.put("Count",values[i]);
      var input=TagValueInput.create(ProblemReporter.DISCARDING,registry,tag);
      var row = new JsonObject(); row.addProperty("age",input.getShortOr("Age",(short)0));
      row.addProperty("value",input.getShortOr("Value",(short)0));
      row.addProperty("health",input.getShortOr("Health",(short)5));
      row.addProperty("count",input.read("Count",ExtraCodecs.POSITIVE_INT).orElse(1));
      out.add(row); NbtIo.write(tag,Path.of(args[0],"orb-"+i+".nbt"));
    }
    Files.writeString(Path.of(args[0],"expected.json"),new Gson().toJson(out));
    if (args.length > 1) {
      var in=new java.io.DataInputStream(Files.newInputStream(Path.of(args[1])));
      var tag=NbtIo.read((java.io.DataInput)in); in.close();
      var input=TagValueInput.create(ProblemReporter.DISCARDING,registry,tag);
      if (input.getShortOr("Age",(short)0)!=42 || input.getShortOr("Value",(short)0)!=7 ||
          input.getShortOr("Health",(short)0)!=1 || input.getShortOr("Fire",(short)0)!=299 ||
          !input.getBooleanOr("Invulnerable",false) || input.getIntOr("invulnerable_time",-1)!=9 ||
          input.read("Count",ExtraCodecs.POSITIVE_INT).orElse(1)!=5 ||
          !input.read("Pos",net.minecraft.world.phys.Vec3.CODEC).orElseThrow().equals(new net.minecraft.world.phys.Vec3(2,63,-2)) ||
          !input.read("Motion",net.minecraft.world.phys.Vec3.CODEC).orElseThrow().equals(new net.minecraft.world.phys.Vec3(0.1,-0.2,0.3)) ||
          !input.getBooleanOr("OnGround",false) || input.getBooleanOr("NoGravity",true) ||
          !LongTag.valueOf(987654321L).equals(tag.get("unknown_plugin_field")))
        throw new AssertionError("Rust orb draft differs from Java decoding");
      var partial = NbtIo.read(Path.of(args[0], "partial-orb-chunk.nbt"));
      var remaining = (ListTag)partial.get("Entities");
      if (remaining.size()!=2) throw new AssertionError("partial entity count");
      var partialOrb=(CompoundTag)remaining.get(0);
      var partialInput=TagValueInput.create(ProblemReporter.DISCARDING,registry,partialOrb);
      if (partialInput.read("Count",ExtraCodecs.POSITIVE_INT).orElse(-1)!=2 ||
          partialInput.getShortOr("Value",(short)-1)!=7 ||
          !LongTag.valueOf(987654321L).equals(partialOrb.get("unknown_plugin_field")))
        throw new AssertionError("partial orb contents");
      var removed=NbtIo.read(Path.of(args[0], "removed-orb-chunk.nbt"));
      var after=(ListTag)removed.get("Entities");
      if (after.size()!=1 || !StringTag.valueOf("minecraft:pig").equals(((CompoundTag)after.get(0)).get("id")))
        throw new AssertionError("removed orb or unrelated entity");
      var created=NbtIo.read(Path.of(args[0], "created-orb.nbt"));
      var createdInput=TagValueInput.create(ProblemReporter.DISCARDING,registry,created);
      if (!StringTag.valueOf("minecraft:experience_orb").equals(created.get("id")) ||
          createdInput.getShortOr("Value",(short)-1)!=7 ||
          createdInput.read("Count",ExtraCodecs.POSITIVE_INT).orElse(-1)!=2 ||
          createdInput.getShortOr("Age",(short)-1)!=42 ||
          createdInput.getShortOr("Air",(short)-1)!=300 ||
          !createdInput.read("Pos",net.minecraft.world.phys.Vec3.CODEC).orElseThrow().equals(new net.minecraft.world.phys.Vec3(2,63,-2)) ||
          !DoubleTag.valueOf(0.75).equals(created.get("fall_distance")) ||
          !FloatTag.valueOf(123.5f).equals(((ListTag)created.get("Rotation")).get(0)) ||
          !new IntArrayTag(new int[]{9,8,7,6}).equals(created.get("UUID")))
        throw new AssertionError("created orb contents");
      var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
      var level=(TestLevel)((sun.misc.Unsafe)unsafeField.get(null)).allocateInstance(TestLevel.class);
      var createdOrb=new TestOrb(level); createdOrb.loadExtra(createdInput);
      var count=ExperienceOrb.class.getDeclaredField("count"); count.setAccessible(true);
      var age=ExperienceOrb.class.getDeclaredField("age"); age.setAccessible(true);
      var health=ExperienceOrb.class.getDeclaredField("health"); health.setAccessible(true);
      if (createdOrb.getValue()!=7 || count.getInt(createdOrb)!=2 || age.getInt(createdOrb)!=42 || health.getInt(createdOrb)!=1)
        throw new AssertionError("created orb Java entity load");
    }
  }
}
