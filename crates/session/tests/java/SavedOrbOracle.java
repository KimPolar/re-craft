import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.util.ExtraCodecs;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.storage.TagValueInput;

public class SavedOrbOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registry = VanillaRegistries.createWorldLookup();
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
    }
  }
}
