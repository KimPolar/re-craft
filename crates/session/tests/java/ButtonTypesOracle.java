import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.ButtonBlock;
import net.minecraft.world.level.block.state.properties.BlockSetType;

public class ButtonTypesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var delay = ButtonBlock.class.getDeclaredField("ticksToStayPressed");
        var type = ButtonBlock.class.getDeclaredField("type");
        delay.setAccessible(true);
        type.setAccessible(true);
        var rows = new JsonObject();
        for (var entry : BuiltInRegistries.BLOCK.entrySet()) {
            if (!(entry.getValue() instanceof ButtonBlock button)) continue;
            var set = (BlockSetType) type.get(button);
            var row = new JsonObject();
            row.addProperty("duration", delay.getInt(button));
            row.addProperty("arrow", set.canButtonBeActivatedByArrows());
            row.addProperty("hardness", button.defaultBlockState().getDestroySpeed(EmptyBlockGetter.INSTANCE, BlockPos.ZERO));
            row.addProperty("on", set.buttonClickOn().location().toString());
            row.addProperty("off", set.buttonClickOff().location().toString());
            rows.add(entry.getKey().identifier().toString(), row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
