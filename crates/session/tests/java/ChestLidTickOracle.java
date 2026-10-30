import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.world.level.block.entity.ChestLidController;

public class ChestLidTickOracle {
    public static void main(String[] args) throws Exception {
        var controller = new ChestLidController();
        var rows = new JsonArray();
        for (int tick=0; tick<43; tick++) {
            boolean open = tick < 12 || (tick >= 17 && tick < 23);
            controller.shouldBeOpen(open); controller.tickLid();
            var row = new JsonArray();
            for (float partial : new float[]{0.0f,0.25f,0.5f,1.0f}) {
                float openness=controller.getOpenness(partial);
                float remaining=1.0f-openness;
                float angle=-(1.0f-remaining*remaining*remaining)*1.5707964f;
                var sample=new JsonArray(); sample.add(openness); sample.add(angle); row.add(sample);
            }
            rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
