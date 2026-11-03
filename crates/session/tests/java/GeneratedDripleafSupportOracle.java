import java.util.ArrayList;
import java.util.List;
import java.util.TreeMap;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.EmptyBlockGetter;

public class GeneratedDripleafSupportOracle {
    public static void main(String[] args) {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (var name : List.of("big_dripleaf", "big_dripleaf_stem", "small_dripleaf")) {
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + name));
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var properties = new TreeMap<String, String>();
                state.getValues().forEach(value -> properties.put(value.property().getName(), value.valueName()));
                var pairs = new ArrayList<String>();
                properties.forEach((key, value) -> pairs.add(key + "=" + value));
                System.out.printf("%s %s %s", name, String.join(",", pairs), state.isSolidRender());
                for (var direction : Direction.values()) {
                    System.out.printf(" %s", state.isFaceSturdy(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, direction));
                }
                System.out.println();
            }
        }
    }
}
