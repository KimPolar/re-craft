import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.storage.TagValueInput;

public class HungerSaveOracle {
  static class TestPlayer extends Player {
    TestPlayer() { super(null,null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public void causeFoodExhaustion(float value) { }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)unsafeField.get(null); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    var lookup=RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
    var ops=lookup.createSerializationContext(NbtOps.INSTANCE);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[1]))).getAsJsonArray();
    for(int index=0;index<cases.size();index++) {
      var c=cases.get(index).getAsJsonObject();
      var original=NbtIo.read(Path.of(args[0],"source"+index+".nbt"));
      var draft=NbtIo.read(Path.of(args[0],"draft"+index+".nbt"));
      var before=TagValueInput.create(ProblemReporter.DISCARDING,lookup,original);
      var after=TagValueInput.create(ProblemReporter.DISCARDING,lookup,draft);
      var effects=new ArrayList<>(before.read("active_effects",MobEffectInstance.CODEC.listOf()).orElse(List.of()));
      var updates=c.getAsJsonArray("events"); int event=0; int ticks=c.get("ticks").getAsInt();
      for(int tick=0;tick<=ticks;tick++) {
        while(event<updates.size() && (updates.get(event).isJsonPrimitive()?updates.get(event).getAsInt():updates.get(event).getAsJsonObject().get("tick").getAsInt())==tick) {
          var entry=updates.get(event);
          var object=entry.isJsonObject()?entry.getAsJsonObject():null;
          var incoming=object==null?new MobEffectInstance(MobEffects.HUNGER,600,0):new MobEffectInstance(MobEffects.HUNGER,object.get("duration").getAsInt(),object.get("amplifier").getAsInt(),object.get("ambient").getAsBoolean(),object.get("visible").getAsBoolean(),object.get("show_icon").getAsBoolean());
          var existing=effects.stream().filter(e->e.getEffect()==MobEffects.HUNGER).findFirst();
          if(existing.isPresent()) existing.get().update(incoming);
          else effects.add(incoming);
          event++;
        }
        if(tick<ticks) {
          var iterator=effects.iterator();
          while(iterator.hasNext()) if(!iterator.next().tickServer(null,player,()->{})) iterator.remove();
        }
      }
      var expected=MobEffectInstance.CODEC.listOf().encodeStart(ops,effects).getOrThrow();
      var actual=MobEffectInstance.CODEC.listOf().encodeStart(ops,after.read("active_effects",MobEffectInstance.CODEC.listOf()).orElse(List.of())).getOrThrow();
      if(!expected.equals(actual)) throw new AssertionError("case "+index+" expected="+expected+" actual="+actual);
    }
  }
}
