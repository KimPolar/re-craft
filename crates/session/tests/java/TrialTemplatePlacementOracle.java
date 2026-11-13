import java.lang.reflect.Proxy;
import java.nio.file.Path;
import java.util.*;
import java.util.zip.ZipFile;
import com.google.gson.JsonObject;
import com.mojang.serialization.DynamicOps;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class TrialTemplatePlacementOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var components = DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE, 64).build();
        for (var id : List.of("brick", "string", "trial_key", "ominous_trial_key", "flow_pottery_sherd", "guster_pottery_sherd", "scrape_pottery_sherd"))
            BuiltInRegistries.ITEM.getValue(Identifier.parse("minecraft:" + id)).builtInRegistryHolder().bindComponents(components);
        var lookup = VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        RegistryAccess access = new RegistryAccess() {
            public <E> Optional<Registry<E>> lookup(ResourceKey<? extends Registry<? extends E>> key) { return Optional.empty(); }
            public java.util.stream.Stream<RegistryAccess.RegistryEntry<?>> registries() { return java.util.stream.Stream.empty(); }
            public <T> RegistryOps<T> createSerializationContext(DynamicOps<T> ops) { return lookup.createSerializationContext(ops); }
        };
        var template = new StructureTemplate();
        try (var jar = new ZipFile(args[0]); var input = jar.getInputStream(jar.getEntry("data/minecraft/structure/trial_chambers/" + args[1] + ".nbt"))) {
            if (input == null) throw new AssertionError("template missing");
            var nbt = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap());
            if (!nbt.getListOrEmpty("entities").isEmpty()) throw new AssertionError("entity templates need comparison");
            template.load(BuiltInRegistries.BLOCK, nbt);
        }
        var states = new HashMap<BlockPos, BlockState>();
        var entities = new HashMap<BlockPos, BlockEntity>();
        var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
            new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, arguments) -> switch (method.getName()) {
                case "holderLookup" -> BuiltInRegistries.BLOCK;
                case "registryAccess" -> access;
                case "getBlockState" -> states.getOrDefault((BlockPos)arguments[0],
                    ((BlockPos)arguments[0]).getY() < 8 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState());
                case "getBlockEntity" -> entities.get((BlockPos)arguments[0]);
                case "setBlock" -> {
                    var pos = ((BlockPos)arguments[0]).immutable(); var state = (BlockState)arguments[1];
                    states.put(pos, state); entities.remove(pos);
                    if (state.getBlock() instanceof EntityBlock block) {
                        var entity = block.newBlockEntity(pos, state);
                        if (entity != null) entities.put(pos, entity);
                    }
                    yield true;
                }
                default -> throw new UnsupportedOperationException(method.toString());
            });
        var settings = new StructurePlaceSettings().setBoundingBox(new BoundingBox(0, 0, 0, 15, 15, 15))
            .setRotation(Rotation.valueOf(args[2])).setKnownShape(true).setIgnoreEntities(true)
            .setLiquidSettings(LiquidSettings.IGNORE_WATERLOGGING)
            .addProcessor(BlockIgnoreProcessor.STRUCTURE_BLOCK).addProcessor(JigsawReplacementProcessor.INSTANCE);
        var random = new WorldgenRandom(new XoroshiroRandomSource(42));
        var origin = new BlockPos(6, 8, 6);
        if (!template.placeInWorld(world, origin, origin, settings, random, 18)) throw new AssertionError("template placement failed");
        for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++) {
            var pos = new BlockPos(x, y, z);
            var state = states.getOrDefault(pos, y < 8 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState());
            var properties = new JsonObject();
            state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
            out.printf("BLOCK\t%d\t%d\t%d\t%s\t%s%n", x, y, z, BuiltInRegistries.BLOCK.getKey(state.getBlock()), properties);
        }
        int index = 0;
        for (var entry : entities.entrySet()) {
            var pos = entry.getKey(); String filename = "entity-" + index++ + ".nbt";
            NbtIo.write(entry.getValue().saveWithFullMetadata(lookup), Path.of(args[3], filename));
            out.printf("ENTITY\t%d\t%d\t%d\t%s%n", pos.getX(), pos.getY(), pos.getZ(), filename);
        }
        out.printf("RANDOM\t%d%n", random.nextLong());
    }
}
