import net.minecraft.SharedConstants;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;

public class MagmaAquaticOcclusionOracle {
    public static void main(String[] args) {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (String name : new String[] {"seagrass", "tall_seagrass", "kelp", "kelp_plant"}) {
            var id = Identifier.parse("minecraft:" + name);
            if (!BuiltInRegistries.BLOCK.containsKey(id)) throw new IllegalArgumentException(name);
            for (var state : BuiltInRegistries.BLOCK.getValue(id).getStateDefinition().getPossibleStates()) {
                String value = "none";
                if (state.hasProperty(BlockStateProperties.AGE_25)) {
                    value = "age=" + state.getValue(BlockStateProperties.AGE_25);
                } else if (state.hasProperty(BlockStateProperties.DOUBLE_BLOCK_HALF)) {
                    value = "half=" + state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF).getSerializedName();
                }
                for (var direction : Direction.values()) {
                    System.out.printf("AQUA %s %s %s %s%n", id, value,
                        direction.getName(), Block.isShapeFullBlock(state.getFaceOcclusionShape(direction)));
                }
            }
        }
    }
}
