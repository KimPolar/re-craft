import java.util.TreeMap;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;

/** Emit the actual 26.3 random-tick eligibility of every block state. */
public class RandomTickStateOracle {
    public static void main(String[] args) {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (var block : BuiltInRegistries.BLOCK) {
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var properties = new TreeMap<String, String>();
                state.getValues().forEach(value ->
                    properties.put(value.property().getName(), value.valueName()));
                var line = new StringBuilder("STATE\t")
                    .append(BuiltInRegistries.BLOCK.getKey(block)).append('\t')
                    .append(state.isRandomlyTicking() || state.getFluidState().isRandomlyTicking());
                properties.forEach((key, value) -> line.append('\t').append(key).append('=').append(value));
                output.println(line);
            }
        }
    }
}
