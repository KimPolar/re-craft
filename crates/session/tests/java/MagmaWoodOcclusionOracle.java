import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;

public class MagmaWoodOcclusionOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (String name : Files.readAllLines(Path.of(args[0]))) {
            var id = Identifier.parse(name);
            if (!BuiltInRegistries.BLOCK.containsKey(id)) {
                throw new IllegalArgumentException("missing block " + name);
            }
            var block = BuiltInRegistries.BLOCK.getValue(id);
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var axis = state.hasProperty(BlockStateProperties.AXIS)
                    ? state.getValue(BlockStateProperties.AXIS).getName() : "none";
                for (var direction : Direction.values()) {
                    System.out.printf("WOOD %s %s %s %s%n", name, axis,
                        direction.getName(), Block.isShapeFullBlock(state.getFaceOcclusionShape(direction)));
                }
            }
        }
    }
}
