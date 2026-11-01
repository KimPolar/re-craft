import com.google.gson.*;
import com.mojang.serialization.Dynamic;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.world.level.storage.LevelSummary;
import net.minecraft.world.level.storage.LevelVersion;

public class WorldListOrderOracle {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        String[] ids={"Zebra","apple","Apple","\uE000","\uD83D\uDE00","한글","old","no-date","zero","future"};
        Long[] times={50L,50L,50L,50L,50L,50L,-10L,null,0L,100L};
        LevelSummary[] summaries=new LevelSummary[ids.length];
        for(int i=0;i<ids.length;i++) {
            var data=new JsonObject();
            if(times[i]!=null) data.addProperty("LastPlayed",times[i]);
            var version=LevelVersion.parse(new Dynamic<>(JsonOps.INSTANCE,data));
            summaries[i]=new LevelSummary(null,version,ids[i],false,false,false,false,null);
        }
        var rows=new JsonArray();
        for(int i=0;i<ids.length;i++) for(int j=0;j<ids.length;j++) {
            var row=new JsonObject(); row.addProperty("left",i); row.addProperty("right",j);
            row.addProperty("order",Integer.signum(summaries[i].compareTo(summaries[j]))); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
