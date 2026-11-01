import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.resources.Identifier;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.Player;

public class MovementEffectApplyOracle {
  // duration, amplifier, ambient, visible, icon; first existing case is absent.
  static int[][] old={{0,0,0,1,1},{100,0,0,1,1},{100,2,1,0,0},{50,3,1,1,0},{-1,1,0,1,1}};
  static int[][] incoming={{40,0,0,0,0},{1800,0,0,1,1},{400,3,0,1,1},{100,2,0,1,1},{-1,1,1,0,0},{0,0,0,1,1}};
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var hiddenField=MobEffectInstance.class.getDeclaredField("hiddenEffect"); hiddenField.setAccessible(true);
    var rows=new JsonArray();
    for(int kind=0;kind<2;kind++)for(int existing=0;existing<old.length;existing++)for(int modifier=0;modifier<2;modifier++)for(int add=0;add<incoming.length;add++) {
      var holder=kind==0?MobEffects.SPEED:MobEffects.SLOWNESS; var type=holder.value();
      var attrs=new AttributeMap(Player.createAttributes().build()); var movement=attrs.getInstance(Attributes.MOVEMENT_SPEED);
      movement.setBaseValue((double)0.1f);
      var o=old[existing]; var n=incoming[add];
      MobEffectInstance current=existing==0?null:new MobEffectInstance(holder,o[0],o[1],o[2]!=0,o[3]!=0,o[4]!=0);
      if(modifier!=0)type.addAttributeModifiers(attrs,existing==0?0:o[1]);
      var next=new MobEffectInstance(holder,n[0],n[1],n[2]!=0,n[3]!=0,n[4]!=0);
      boolean changed;
      if(current==null) { current=next; type.addAttributeModifiers(attrs,current.getAmplifier()); changed=true; }
      else { changed=current.update(next); if(changed) { type.removeAttributeModifiers(attrs); type.addAttributeModifiers(attrs,current.getAmplifier()); } }
      var row=new JsonObject();row.addProperty("kind",kind);row.addProperty("existing",existing);row.addProperty("modifier",modifier);row.addProperty("add",add);row.addProperty("changed",changed);
      row.addProperty("duration",current.getDuration());row.addProperty("amplifier",current.getAmplifier());row.addProperty("ambient",current.isAmbient());row.addProperty("visible",current.isVisible());row.addProperty("icon",current.showIcon());
      var hidden=(MobEffectInstance)hiddenField.get(current);
      if(hidden!=null) { row.addProperty("hidden_duration",hidden.getDuration()); row.addProperty("hidden_amplifier",hidden.getAmplifier()); row.addProperty("hidden_ambient",hidden.isAmbient()); row.addProperty("hidden_visible",hidden.isVisible()); row.addProperty("hidden_icon",hidden.showIcon()); }
      var m=movement.getModifier(Identifier.parse(kind==0?"minecraft:effect.speed":"minecraft:effect.slowness"));
      if(m!=null)row.addProperty("amount",m.amount());row.addProperty("value",movement.getValue());rows.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
  }
}
