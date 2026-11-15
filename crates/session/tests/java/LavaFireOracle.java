import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.Vec3;

public class LavaFireOracle {
    static class World extends ServerLevel {
        int mode;
        boolean enabled;
        Map<BlockPos, BlockState> changed;
        List<BlockPos> placed;

        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 16; }
        @Override public boolean canSpreadFireAround(BlockPos p) { return enabled; }
        @Override public boolean isLoaded(BlockPos p) {
            return p.getY() >= 0 && p.getY() < 16 && (mode != 3 || p.getX() < 9);
        }
        @Override public boolean hasChunkAt(BlockPos p) { return mode != 3 || p.getX() < 9; }
        @Override public BlockState getBlockState(BlockPos p) {
            var changedState = changed.get(p);
            if (changedState != null) return changedState;
            if (p.getY() < 0 || p.getY() >= 16) return Blocks.AIR.defaultBlockState();
            if (mode == 0) return Blocks.STONE.defaultBlockState();
            if (mode == 1) {
                if (p.getY() > 5) return Blocks.AIR.defaultBlockState();
                return Math.floorMod(p.getX() + p.getZ(), 3) == 0
                    ? Blocks.OAK_PLANKS.defaultBlockState() : Blocks.STONE.defaultBlockState();
            }
            return switch (Math.floorMod(p.getX() * 7 + p.getY() * 11 + p.getZ() * 13, 4)) {
                case 0 -> Blocks.AIR.defaultBlockState();
                case 1 -> Blocks.OAK_PLANKS.defaultBlockState();
                case 2 -> Blocks.STONE.defaultBlockState();
                default -> Blocks.WATER.defaultBlockState();
            };
        }
        @Override public boolean setBlockAndUpdate(BlockPos p, BlockState state) {
            placed.add(p.immutable());
            changed.put(p.immutable(), state);
            return true;
        }
    }

    static class Player extends ServerPlayer {
        boolean spectator;
        Vec3 at;
        Player() { super(null, null, null, null); }
        @Override public boolean isSpectator() { return spectator; }
        @Override public Vec3 position() { return at; }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", java.util.Collection.class);
        bind.setAccessible(true);
        var rows = new JsonArray();
        var lava = Blocks.LAVA.defaultBlockState();
        for (int mode = 0; mode < 5; mode++) {
            for (boolean enabled : new boolean[] { false, true }) {
                for (boolean blocked : new boolean[] { false, true }) {
                    var tags = blocked ? List.of(BlockTags.BLOCKS_LAVA_FIRE_SPREAD) : List.of();
                    bind.invoke(Blocks.STONE.builtInRegistryHolder(), tags);
                    bind.invoke(Blocks.OAK_PLANKS.builtInRegistryHolder(), tags);
                    for (int seed = -48; seed <= 48; seed++) {
                        var world = (World) unsafe.allocateInstance(World.class);
                        world.mode = mode;
                        world.enabled = enabled;
                        world.changed = new HashMap<>();
                        world.placed = new ArrayList<>();
                        var position = new BlockPos(8, mode == 4 ? 15 : 5, 8);
                        world.changed.put(position, lava);
                        var random = RandomSource.create(seed);
                        lava.randomTick(world, position, random);
                        lava.getFluidState().randomTick(world, position, random);
                        var row = new JsonObject();
                        row.addProperty("mode", mode);
                        row.addProperty("enabled", enabled);
                        row.addProperty("blocked", blocked);
                        row.addProperty("seed", seed);
                        row.addProperty("next", random.nextInt());
                        var placed = new JsonArray();
                        for (var p : world.placed) {
                            var pos = new JsonArray();
                            pos.add(p.getX()); pos.add(p.getY()); pos.add(p.getZ());
                            placed.add(pos);
                        }
                        row.add("placed", placed);
                        rows.add(row);
                    }
                }
            }
        }
        var radius = new JsonArray();
        var map = (ChunkMap) unsafe.allocateInstance(ChunkMap.class);
        var player = (Player) unsafe.allocateInstance(Player.class);
        var close = ChunkMap.class.getDeclaredMethod("playerIsCloseEnoughTo", ServerPlayer.class, Vec3.class, int.class);
        close.setAccessible(true);
        for (boolean spectator : new boolean[] { false, true }) {
            player.spectator = spectator;
            for (int r : new int[] { 0, 1, 5, 128 }) {
                for (double x : new double[] { 0, 0.5, 1, 4.999999, 5, 5.000001, 127.999999, 128 }) {
                    player.at = new Vec3(x, 0, 0);
                    var row = new JsonArray();
                    row.add(spectator); row.add(r); row.add(x);
                    row.add((boolean) close.invoke(map, player, new Vec3(0, 0, 0), r));
                    radius.add(row);
                }
            }
        }
        var result = new JsonObject();
        result.addProperty("max_chunk_coordinate", net.minecraft.world.level.chunk.status.ChunkPyramid.MAX_CHUNK_COORDINATE_VALUE);
        result.add("ticks", rows);
        result.add("radius", radius);
        Files.writeString(Path.of(args[0]), result.toString());
    }
}
