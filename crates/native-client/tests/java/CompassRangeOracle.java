import java.nio.file.*;
import java.util.zip.ZipFile;
import com.google.gson.*;
import net.minecraft.client.renderer.item.RangeSelectItemModel;

public class CompassRangeOracle {
    public static void main(String[] args) throws Exception {
        var method=RangeSelectItemModel.class.getDeclaredMethod("lastIndexLessOrEqual",float[].class,float.class);
        method.setAccessible(true);
        var out=new JsonArray();
        try (var jar=new ZipFile(args[0])) {
            for (String id:new String[]{"compass","recovery_compass"}) {
                var definition=JsonParser.parseString(new String(jar.getInputStream(jar.getEntry("assets/minecraft/items/"+id+".json")).readAllBytes(),java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject().getAsJsonObject("model");
                for (boolean linked:new boolean[]{false,true}) {
                    if(id.equals("recovery_compass") && linked) continue;
                    var range=id.equals("compass")?definition.getAsJsonObject(linked?"on_true":"on_false"):definition;
                    var entries=range.getAsJsonArray("entries"); float[] thresholds=new float[entries.size()];
                    for(int i=0;i<thresholds.length;i++) thresholds[i]=entries.get(i).getAsJsonObject().get("threshold").getAsFloat();
                    for(float threshold:thresholds) for(float angle:new float[]{Math.nextDown(threshold/32.0f),threshold/32.0f,Math.nextUp(threshold/32.0f)}) {
                        int index=(int)method.invoke(null,thresholds,angle*32.0f);
                        var row=new JsonObject(); row.addProperty("id",id); row.addProperty("linked",linked); row.addProperty("bits",Float.floatToRawIntBits(angle));
                        if(index<0) row.add("model",JsonNull.INSTANCE);
                        else row.addProperty("model",entries.get(index).getAsJsonObject().getAsJsonObject("model").get("model").getAsString());
                        out.add(row);
                    }
                }
            }
        }
        Files.writeString(Path.of(args[1]),new GsonBuilder().serializeNulls().create().toJson(out));
    }
}
