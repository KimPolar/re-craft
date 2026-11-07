import com.google.gson.JsonObject;
import java.lang.reflect.Proxy;
import java.util.HashMap;
import java.util.List;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class VillageHouseOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        try (var jar = new ZipFile(args[0])) {
            for (var name : List.of("plains_small_house_1", "plains_small_house_2", "plains_small_house_3")) {
                var template = new StructureTemplate();
                var entry = jar.getEntry("data/minecraft/structure/village/plains/houses/" + name + ".nbt");
                try (var input = jar.getInputStream(entry)) {
                    template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap()));
                }
                var states = new HashMap<BlockPos, BlockState>();
                var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                    new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, arguments) -> switch (method.getName()) {
                        case "holderLookup" -> BuiltInRegistries.BLOCK;
                        case "getBlockState" -> states.getOrDefault((BlockPos) arguments[0],
                            ((BlockPos) arguments[0]).getY() < 8 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState());
                        case "setBlock" -> { states.put(((BlockPos) arguments[0]).immutable(), (BlockState) arguments[1]); yield true; }
                        case "getBlockEntity" -> null;
                        default -> throw new UnsupportedOperationException(method.toString());
                    });
                var settings = new StructurePlaceSettings().setBoundingBox(new BoundingBox(0, 0, 0, 15, 15, 15))
                    .setKnownShape(true).setIgnoreEntities(true).setLiquidSettings(LiquidSettings.IGNORE_WATERLOGGING)
                    .addProcessor(BlockIgnoreProcessor.STRUCTURE_AND_AIR)
                    .addProcessor(JigsawReplacementProcessor.INSTANCE)
                    .addProcessor(new RuleProcessor(List.of(new ProcessorRule(
                        new RandomBlockMatchTest(Blocks.COBBLESTONE, 0.1F), AlwaysTrueTest.INSTANCE, Blocks.MOSSY_COBBLESTONE.defaultBlockState()))));
                var random = new WorldgenRandom(new XoroshiroRandomSource(42));
                var origin = new BlockPos(1, 8, 1);
                if (!template.placeInWorld(world, origin, origin, settings, random, 18)) throw new AssertionError(name);
                for (int y = 8; y < 15; y++) for (int z = 1; z < 8; z++) for (int x = 1; x < 8; x++) {
                    var state = states.getOrDefault(new BlockPos(x, y, z), Blocks.AIR.defaultBlockState());
                    var props = new JsonObject();
                    state.getValues().forEach(value -> props.addProperty(value.property().getName(), value.valueName()));
                    out.printf("BLOCK\t%s\t%d\t%d\t%d\t%s\t%s%n", name, x, y, z,
                        BuiltInRegistries.BLOCK.getKey(state.getBlock()), props);
                }
                out.printf("RANDOM\t%s\t%d%n", name, random.nextLong());
            }
        }
    }
}
