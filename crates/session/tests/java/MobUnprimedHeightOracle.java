import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.levelgen.Heightmap;

public final class MobUnprimedHeightOracle {
    @SuppressWarnings({"rawtypes", "unchecked"})
    static String value(BlockState state, Property property) {
        return property.getName(state.getValue(property));
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bind.setAccessible(true);
        bind.invoke(Blocks.STONE.builtInRegistryHolder(), List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP, BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES));
        bind.invoke(Blocks.OAK_LEAVES.builtInRegistryHolder(), List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP));
        var types = new Heightmap.Types[]{Heightmap.Types.WORLD_SURFACE, Heightmap.Types.OCEAN_FLOOR,
            Heightmap.Types.MOTION_BLOCKING, Heightmap.Types.MOTION_BLOCKING_NO_LEAVES};
        var output = new JsonArray();
        for (var block : List.of(Blocks.AIR, Blocks.STONE, Blocks.WATER, Blocks.SHORT_GRASS,
                Blocks.GRASS_BLOCK, Blocks.REDSTONE_ORE, Blocks.SEAGRASS, Blocks.OAK_LEAVES)) {
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var entry = new JsonObject();
                entry.addProperty("name", BuiltInRegistries.BLOCK.getKey(block).toString());
                var props = new JsonObject();
                for (var property : state.getProperties()) props.addProperty(property.getName(), value(state, property));
                entry.add("properties", props);
                var matches = new JsonArray();
                for (var type : types) matches.add(type.isOpaque().test(state));
                entry.add("matches", matches);
                output.add(entry);
            }
        }
        Files.writeString(Path.of(args[0]), output.toString());
    }
}
