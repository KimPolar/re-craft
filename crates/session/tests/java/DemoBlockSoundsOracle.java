import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;

public class DemoBlockSoundsOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var inputs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var rows=new JsonArray();
        for(var value:inputs) {
            var id=Identifier.parse(value.getAsString());
            var block=BuiltInRegistries.BLOCK.getValue(id);
            if(block==null) throw new IllegalArgumentException(id.toString());
            var sound=block.defaultBlockState().getSoundType();
            var row=new JsonObject(); row.addProperty("id",id.toString());
            row.addProperty("event",sound.getFallSound().location().toString());
            row.addProperty("volume_bits",Float.floatToRawIntBits(sound.getVolume()*0.5f));
            row.addProperty("pitch_bits",Float.floatToRawIntBits(sound.getPitch()*0.75f)); rows.add(row);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(rows));
    }
}
