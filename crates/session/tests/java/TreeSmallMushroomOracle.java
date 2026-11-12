import java.lang.reflect.Proxy;
import java.util.Collection;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Holder;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

public class TreeSmallMushroomOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bind.setAccessible(true);
        var position = new BlockPos(8, 8, 8);
        for (var mushroom : List.of(Blocks.BROWN_MUSHROOM, Blocks.RED_MUSHROOM)) {
            for (var support : List.of(Blocks.STONE, Blocks.AIR)) {
                for (boolean override : new boolean[] {false, true}) {
                    bind.invoke(support.builtInRegistryHolder(), override
                        ? List.of(BlockTags.OVERRIDES_MUSHROOM_LIGHT_REQUIREMENT) : List.of());
                    for (int brightness : new int[] {0, 12, 13, 15}) {
                        var world = (LevelAccessor) Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),
                            new Class[] {LevelAccessor.class}, (proxy, method, values) -> {
                                if (method.getName().equals("getBlockState")) return support.defaultBlockState();
                                if (method.getName().equals("getRawBrightness")) return brightness;
                                throw new UnsupportedOperationException(method.toString());
                            });
                        for (Direction direction : Direction.values()) {
                            var updated = mushroom.defaultBlockState().updateShape(world, world, position,
                                direction, position.relative(direction), Blocks.STONE.defaultBlockState(),
                                new WorldgenRandom(new XoroshiroRandomSource(0)));
                            out.println("R " + (updated.getBlock() == mushroom));
                        }
                    }
                    bind.invoke(support.builtInRegistryHolder(), List.of());
                }
            }
        }
    }
}
