import com.mojang.serialization.Lifecycle;
import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalPiece;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class RuinedPortalTemplateOracle {
    static String key(BlockState state) {
        var values = new TreeMap<String, String>();
        state.getValues().forEach(e -> values.put(e.property().getName(), e.valueName()));
        var out = new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
        for (var e : values.entrySet()) out.append(e.getKey()).append('=').append(e.getValue()).append(';');
        return out.toString();
    }
    public static void main(String[] args) throws Exception {
        var out = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        for (var b : BuiltInRegistries.BLOCK) {
            if (b instanceof StairBlock) bind.invoke(b.builtInRegistryHolder(), List.of(BlockTags.STAIRS));
            else if (b instanceof SlabBlock) bind.invoke(b.builtInRegistryHolder(), List.of(BlockTags.SLABS));
            else if (b instanceof WallBlock) bind.invoke(b.builtInRegistryHolder(), List.of(BlockTags.WALLS));
        }
        var blocks = new MappedRegistry<Block>(Registries.BLOCK, Lifecycle.stable());
        blocks.bindTags(Map.of(BlockTags.FEATURES_CANNOT_REPLACE, List.of())); blocks.freeze();
        var registries = new RegistryAccess.ImmutableRegistryAccess(List.of(blocks));
        var method = RuinedPortalPiece.class.getDeclaredMethod("makeSettings", HolderLookup.Provider.class,
            Mirror.class, Rotation.class, RuinedPortalPiece.VerticalPlacement.class, BlockPos.class, RuinedPortalPiece.Properties.class);
        method.setAccessible(true);
        var paletteField = StructureTemplate.class.getDeclaredField("palettes"); paletteField.setAccessible(true);
        var world = (ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
            new Class<?>[]{ServerLevelAccessor.class}, (proxy, m, a) -> {
                if (m.getName().equals("getBlockState")) return Blocks.STONE.defaultBlockState();
                throw new UnsupportedOperationException(m.toString());
            });
        var origin = new BlockPos(-37, 72, 51);
        var clip = new BoundingBox(-43, 70, 47, -32, 82, 58);
        var names = List.of("portal_1", "portal_5", "giant_portal_1");
        for (int t = 0; t < names.size(); t++) {
            var template = new StructureTemplate();
            var path = "data/minecraft/structure/ruined_portal/" + names.get(t) + ".nbt";
            try (var stream = RuinedPortalTemplateOracle.class.getClassLoader().getResourceAsStream(path)) {
                if (stream == null) throw new AssertionError("missing template " + path);
                template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap()));
            }
            @SuppressWarnings("unchecked") var palettes = (List<StructureTemplate.Palette>)paletteField.get(template);
            var classified = new HashSet<String>();
            for (var info : palettes.getFirst().blocks()) if (info.nbt() == null && classified.add(key(info.state()))) {
                var state = info.state();
                var full = !state.getBlock().hasDynamicShape() && state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                out.printf("C\t%s\t%s%n", key(state), full);
            }
            var pivot = new BlockPos(template.getSize().getX() / 2, 0, template.getSize().getZ() / 2);
            for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) for (int flags : new int[]{0, 7}) {
                var properties = new RuinedPortalPiece.Properties((flags & 1) != 0, 0.8f, (flags & 2) != 0, false, false, (flags & 4) != 0);
                var settings = (StructurePlaceSettings)method.invoke(null, registries, mirror, rotation,
                    RuinedPortalPiece.VerticalPlacement.ON_LAND_SURFACE, pivot, properties);
                settings.setBoundingBox(clip);
                var list = StructureTemplate.processBlockInfos(world, origin, origin, settings,
                    settings.getRandomPalette(palettes, origin).blocks());
                out.printf("BEGIN\t%d\t%d\t%d\t%d\t%d%n", t, mirror.ordinal(), rotation.ordinal(), flags, list.size());
                for (var info : list) out.printf("B\t%d\t%d\t%d\t%s\t%s%n", info.pos().getX(), info.pos().getY(), info.pos().getZ(), key(info.state()), info.nbt() != null);
            }
        }
    }
}
