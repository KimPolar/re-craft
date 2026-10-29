import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import com.google.gson.reflect.TypeToken;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.sounds.SoundManager;
import net.minecraft.client.resources.sounds.SoundEventRegistration;
import net.minecraft.util.RandomSource;
import net.minecraft.resources.Identifier;
import net.minecraft.client.sounds.WeighedSoundEvents;
import net.minecraft.client.sounds.Weighted;
import net.minecraft.client.resources.sounds.Sound;

public class SoundDefinitionOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var field=SoundManager.class.getDeclaredField("GSON"); field.setAccessible(true);
    var gson=(Gson)field.get(null);
    var type=new TypeToken<Map<String,SoundEventRegistration>>(){}.getType();
    Map<String,SoundEventRegistration> events=gson.fromJson(Files.readString(Path.of(args[0])),type);
    var out=new JsonObject();
    for (String id:new String[]{"block.stone_button.click_on","block.stone_button.click_off"}) {
      var event=events.get(id); if(event==null || event.getSounds().size()!=1) throw new AssertionError(id);
      var sound=event.getSounds().get(0); var row=new JsonObject();
      row.addProperty("name",sound.getLocation().toString());
      row.addProperty("volume",Float.floatToRawIntBits(sound.getVolume().sample(RandomSource.create(1))));
      row.addProperty("pitch",Float.floatToRawIntBits(sound.getPitch().sample(RandomSource.create(1))));
      row.addProperty("weight",sound.getWeight()); row.addProperty("stream",sound.shouldStream());
      row.addProperty("preload",sound.shouldPreload()); row.addProperty("attenuation",sound.getAttenuationDistance());
      row.addProperty("subtitle",event.getSubtitle()); out.add(id,row);
    }
    String fixture="""
      {"test.outer":{"sounds":[{"name":"test/a","weight":2,"volume":0.7},
       {"name":"test.inner","type":"event","weight":19,"volume":0.5,"pitch":0.8,"stream":true}]},
       "test.inner":{"sounds":[{"name":"test/b","weight":1,"volume":0.3,"preload":true,"attenuation_distance":9},
       {"name":"test/c","weight":3,"pitch":1.2}]}}
      """;
    Map<String,SoundEventRegistration> parsed=gson.fromJson(fixture,type);
    Class<?> prepClass=Class.forName("net.minecraft.client.sounds.SoundManager$Preparations");
    var prepConstructor=prepClass.getDeclaredConstructor(); prepConstructor.setAccessible(true);
    Object prep=prepConstructor.newInstance();
    var registryField=prepClass.getDeclaredField("registry"); registryField.setAccessible(true);
    @SuppressWarnings("unchecked") var registry=(Map<Identifier,WeighedSoundEvents>)registryField.get(prep);
    var innerId=Identifier.parse("minecraft:test.inner");
    var outerId=Identifier.parse("minecraft:test.outer");
    var inner=new WeighedSoundEvents(innerId,null); var outer=new WeighedSoundEvents(outerId,null);
    registry.put(innerId,inner); registry.put(outerId,outer);
    for (Sound sound:parsed.get("test.inner").getSounds()) inner.addSound(sound);
    var outerSounds=parsed.get("test.outer").getSounds(); outer.addSound(outerSounds.get(0));
    Class<?> wrapper=Class.forName("net.minecraft.client.sounds.SoundManager$Preparations$1");
    var wrapperConstructor=wrapper.getDeclaredConstructors()[0]; wrapperConstructor.setAccessible(true);
    @SuppressWarnings("unchecked") var reference=(Weighted<Sound>)wrapperConstructor.newInstance(prep,innerId,outerSounds.get(1));
    outer.addSound(reference);
    var choices=new JsonArray();
    for (long seed=0;seed<128;seed++) {
      var random=RandomSource.create(seed); var sound=outer.getSound(random); var row=new JsonObject();
      row.addProperty("name",sound.getLocation().toString());
      row.addProperty("volume",Float.floatToRawIntBits(sound.getVolume().sample(random)));
      row.addProperty("pitch",Float.floatToRawIntBits(sound.getPitch().sample(random)));
      row.addProperty("weight",sound.getWeight()); row.addProperty("stream",sound.shouldStream());
      row.addProperty("preload",sound.shouldPreload()); row.addProperty("attenuation",sound.getAttenuationDistance());
      row.addProperty("next",random.nextInt()); choices.add(row);
    }
    out.add("selection",choices);
    var values=new String[]{"0.5", "1", "0", "-1", "1e-100", "1e100", "null", "true", "\"0.5\"", "{}", "{\"type\":\"uniform\",\"min_inclusive\":0.5,\"max_exclusive\":1.0}"};
    var validation=new JsonArray();
    for (String fieldName:new String[]{"volume", "pitch"}) for (String value:values) {
      var row=new JsonObject(); row.addProperty("field",fieldName); row.addProperty("value",value);
      try {
        String json="{\"test\":{\"sounds\":[{\"name\":\"random/a\",\""+fieldName+"\":"+value+"}]}}";
        Map<String,SoundEventRegistration> result=gson.fromJson(json,type);
        var sound=result.get("test").getSounds().get(0);
        float number=(fieldName.equals("volume")?sound.getVolume():sound.getPitch()).sample(RandomSource.create(1));
        row.addProperty("accepted",true); row.addProperty("bits",Float.floatToRawIntBits(number));
      } catch (RuntimeException ex) { row.addProperty("accepted",false); }
      validation.add(row);
    }
    out.add("validation",validation);
    Files.writeString(Path.of(args[1]),new Gson().toJson(out));
  }
}
