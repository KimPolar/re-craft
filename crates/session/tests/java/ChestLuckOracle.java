import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.resources.Identifier;
import net.minecraft.world.effect.MobEffects;
import net.minecraft.world.entity.ai.attributes.*;

public class ChestLuckOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var out=new JsonArray();
    for(double base:new double[]{-2048,-5,0,1.25,2048})
      for(double persistent:new double[]{-2,0,3})
        for(int luck:new int[]{-1,0,2}) for(int unluck:new int[]{-1,1})
          for(double equipment:new double[]{-3,0,5}) {
            var attribute=new AttributeInstance(Attributes.LUCK, value -> {});
            attribute.setBaseValue(base);
            attribute.addPermanentModifier(new AttributeModifier(Identifier.parse("example:persistent"),persistent,AttributeModifier.Operation.ADD_VALUE));
            if(luck>=0) MobEffects.LUCK.value().createModifiers(luck,(id,modifier) -> { if(id.equals(Attributes.LUCK)) attribute.addTransientModifier(modifier); });
            if(unluck>=0) MobEffects.UNLUCK.value().createModifiers(unluck,(id,modifier) -> { if(id.equals(Attributes.LUCK)) attribute.addTransientModifier(modifier); });
            attribute.addTransientModifier(new AttributeModifier(Identifier.parse("example:equipment"),equipment,AttributeModifier.Operation.ADD_VALUE));
            var row=new JsonObject(); row.addProperty("base",base); row.addProperty("persistent",persistent); row.addProperty("luck",luck); row.addProperty("unluck",unluck); row.addProperty("equipment",equipment); row.addProperty("bits",Float.floatToRawIntBits((float)attribute.getValue())); out.add(row);
          }
    Files.writeString(Path.of(args[0]),out.toString());
  }
}
