import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.decoration.ArmorStand;
import net.minecraft.world.item.enchantment.LevelBasedValue;
import net.minecraft.world.item.enchantment.effects.ReplaceDisk;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.phys.Vec3;

public class ReplaceDiskGeometryOracle {
    static class World extends ServerLevel {
        JsonArray visited;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public boolean setBlockAndUpdate(BlockPos position, BlockState state) {
            JsonArray point = new JsonArray();
            point.add(position.getX()); point.add(position.getY()); point.add(position.getZ());
            visited.add(point);
            return true;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)unsafeField.get(null);
        var world = (World)unsafe.allocateInstance(World.class);
        var entity = (ArmorStand)unsafe.allocateInstance(ArmorStand.class);
        var randomField = Entity.class.getDeclaredField("random");
        unsafe.putObject(entity, unsafe.objectFieldOffset(randomField), new LegacyRandomSource(5));
        JsonArray results = new JsonArray();
        double[][] positions = {{0.5, 10.0, 0.5}, {0.1, 10.9, -0.2}, {-14.5, -3.25, 8.5}, {4.999, 0.0, -8.001}};
        int[][] offsets = {{0, -1, 0}, {2, 1, -3}};
        for (int radius : new int[]{-3, -1, 0, 1, 2, 3, 4, 6})
        for (int height : new int[]{-2, 0, 1, 2})
        for (double[] pos : positions) for (int[] off : offsets) {
            world.visited = new JsonArray();
            var effect = new ReplaceDisk(LevelBasedValue.constant(radius), LevelBasedValue.constant(height),
                new Vec3i(off[0], off[1], off[2]), Optional.empty(), BlockStateProvider.holderOf(Blocks.STONE), Optional.empty());
            effect.apply(world, 1, null, entity, new Vec3(pos[0], pos[1], pos[2]));
            JsonObject row = new JsonObject(); row.addProperty("radius", radius); row.addProperty("height", height);
            JsonArray origin = new JsonArray(); for (double coordinate : pos) origin.add(coordinate);
            JsonArray offset = new JsonArray(); for (int coordinate : off) offset.add(coordinate);
            row.add("origin", origin); row.add("offset", offset); row.add("positions", world.visited); results.add(row);
        }
        Files.writeString(Path.of(args[0]), results.toString());
    }
}
