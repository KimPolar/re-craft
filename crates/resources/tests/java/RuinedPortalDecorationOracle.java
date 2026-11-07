import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalPiece;

public class RuinedPortalDecorationOracle {
    static String key(BlockState state) {
        var values = new TreeMap<String, String>();
        state.getValues().forEach(e -> values.put(e.property().getName(), e.valueName()));
        var out = new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
        for (var e : values.entrySet()) out.append(e.getKey()).append('=').append(e.getValue()).append(';');
        return out.toString();
    }
    static int surface(int x, int z, Heightmap.Types type) {
        return 63 + Math.floorMod(x * 3 + z * 5, 7) + (type == Heightmap.Types.OCEAN_FLOOR_WG ? -2 : 0);
    }
    static BlockState initial(BlockPos p) {
        if (p.getY() > surface(p.getX(), p.getZ(), Heightmap.Types.WORLD_SURFACE_WG)) return Blocks.AIR.defaultBlockState();
        return switch (Math.floorMod(p.getX() * 31 + p.getY() * 13 + p.getZ() * 17, 19)) {
            case 0 -> Blocks.LAVA.defaultBlockState();
            case 1 -> Blocks.OBSIDIAN.defaultBlockState();
            case 2 -> Blocks.BEDROCK.defaultBlockState();
            case 3 -> Blocks.CAVE_AIR.defaultBlockState();
            case 4 -> Blocks.NETHERRACK.defaultBlockState();
            default -> Blocks.STONE.defaultBlockState();
        };
    }
    static Method method(String name, Class<?>... args) throws Exception {
        var m = RuinedPortalPiece.class.getDeclaredMethod(name, args); m.setAccessible(true); return m;
    }
    static void field(Class<?> owner, Object target, String name, Object value) throws Exception {
        var f = owner.getDeclaredField(name); f.setAccessible(true); f.set(target, value);
    }
    public static void main(String[] args) throws Exception {
        var out = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        bind.invoke(Blocks.BEDROCK.builtInRegistryHolder(), List.of(BlockTags.FEATURES_CANNOT_REPLACE));
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)unsafeField.get(null);
        var spread = method("spreadNetherrack", RandomSource.class, LevelAccessor.class);
        var columns = method("addNetherrackDripColumnsBelowPortal", RandomSource.class, LevelAccessor.class);
        var vine = method("maybeAddVines", RandomSource.class, LevelAccessor.class, BlockPos.class);
        var leaves = method("maybeAddLeavesAbove", RandomSource.class, LevelAccessor.class, BlockPos.class);
        var bounds = new BoundingBox(-3, 64, -2, 3, 67, 2);
        for (var placement : RuinedPortalPiece.VerticalPlacement.values()) for (int flags = 0; flags < 8; flags++)
            for (long seed : new long[]{0, 42, -1}) {
                var piece = (RuinedPortalPiece)unsafe.allocateInstance(RuinedPortalPiece.class);
                field(StructurePiece.class, piece, "boundingBox", bounds);
                field(RuinedPortalPiece.class, piece, "verticalPlacement", placement);
                field(RuinedPortalPiece.class, piece, "properties", new RuinedPortalPiece.Properties(
                    (flags & 1) != 0, 0.8f, false, (flags & 2) != 0, (flags & 4) != 0, false));
                var changes = new HashMap<BlockPos, BlockState>();
                var events = new ArrayList<String>();
                var world = (LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(), new Class<?>[]{LevelAccessor.class},
                    (proxy, m, a) -> switch (m.getName()) {
                        case "getBlockState" -> changes.getOrDefault((BlockPos)a[0], initial((BlockPos)a[0]));
                        case "getHeight" -> surface((int)a[1], (int)a[2], (Heightmap.Types)a[0]) + 1;
                        case "setBlockAndUpdate" -> {
                            var pos = ((BlockPos)a[0]).immutable(); var state = (BlockState)a[1];
                            changes.put(pos, state);
                            events.add(pos.getX()+"\t"+pos.getY()+"\t"+pos.getZ()+"\t"+key(state));
                            yield true;
                        }
                        default -> throw new UnsupportedOperationException(m.toString());
                    });
                var random = new WorldgenRandom(new XoroshiroRandomSource(seed));
                spread.invoke(piece, random, world);
                columns.invoke(piece, random, world);
                if ((flags & 6) != 0) for (var pos : BlockPos.betweenClosed(bounds.minX(), bounds.minY(), bounds.minZ(), bounds.maxX(), bounds.maxY(), bounds.maxZ())) {
                    if ((flags & 4) != 0) vine.invoke(piece, random, world, pos);
                    if ((flags & 2) != 0) leaves.invoke(piece, random, world, pos);
                }
                out.printf("CASE\t%d\t%d\t%d\t%d%n", placement.ordinal(), flags, seed, events.size());
                for (var e : events) out.println("B\t" + e);
                out.printf("END\t%d%n", random.nextLong());
            }
    }
}
