import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate.StructureBlockInfo;

public class BlockAgeProcessorOracle {
    static String key(BlockState state) {
        var values = new TreeMap<String, String>();
        state.getValues().forEach(entry -> values.put(entry.property().getName(), entry.valueName()));
        var out = new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
        for (var entry : values.entrySet()) out.append(entry.getKey()).append('=').append(entry.getValue()).append(';');
        return out.toString();
    }
    public static void main(String[] args) throws Exception {
        var out = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        var stairs = List.of(Blocks.STONE_BRICK_STAIRS, Blocks.OAK_STAIRS);
        var slabs = List.of(Blocks.STONE_SLAB, Blocks.STONE_BRICK_SLAB);
        var walls = List.of(Blocks.STONE_BRICK_WALL);
        for (var block : stairs) bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.STAIRS));
        for (var block : slabs) bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.SLABS));
        for (var block : walls) bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.WALLS));
        var blocks = new ArrayList<Block>(List.of(Blocks.STONE, Blocks.STONE_BRICKS,
                Blocks.CHISELED_STONE_BRICKS, Blocks.OBSIDIAN, Blocks.NETHERRACK));
        blocks.addAll(stairs); blocks.addAll(slabs); blocks.addAll(walls);
        float[] mossiness = {0.0f, 0.2f, 0.8f, 1.0f};
        for (int m = 0; m < mossiness.length; m++) {
            var processor = new BlockAgeProcessor(mossiness[m]);
            for (var block : blocks) for (var state : block.getStateDefinition().getPossibleStates())
                for (int i = 0; i < 13; i++) {
                    var pos = new BlockPos(i * 17 - 95, i * 29 - 64, 113 - i * 23);
                    var nbt = new CompoundTag(); nbt.putInt("test", 17);
                    var info = new StructureBlockInfo(pos, state, nbt);
                    var result = processor.processBlock(null, pos, pos, pos, info, new StructurePlaceSettings());
                    if (result == null || !result.pos().equals(pos) || !nbt.equals(result.nbt()))
                        throw new AssertionError("aging changed position or NBT");
                    out.printf("A\t%d\t%d\t%d\t%d\t%s\t%s%n", m, pos.getX(), pos.getY(), pos.getZ(), key(state), key(result.state()));
                }
        }
        // Simulate data packs that add ordinary blocks to each aging tag.
        var tagged = List.of(Blocks.DIRT, Blocks.NETHERRACK, Blocks.OAK_LOG, Blocks.REDSTONE_ORE, Blocks.WATER);
        var tags = List.of(BlockTags.STAIRS, BlockTags.SLABS, BlockTags.WALLS);
        for (int category = 0; category < tags.size(); category++) {
            for (var block : tagged) bind.invoke(block.builtInRegistryHolder(), List.of(tags.get(category)));
            for (int m = 0; m < mossiness.length; m++) {
                var processor = new BlockAgeProcessor(mossiness[m]);
                for (var block : tagged) for (var state : block.getStateDefinition().getPossibleStates())
                    for (int i = 0; i < 13; i++) {
                        var pos = new BlockPos(i * 17 - 95, i * 29 - 64, 113 - i * 23);
                        var info = new StructureBlockInfo(pos, state, null);
                        var result = processor.processBlock(null, pos, pos, pos, info, new StructurePlaceSettings());
                        out.printf("B\t%d\t%d\t%d\t%d\t%d\t%s\t%s%n", category, m, pos.getX(), pos.getY(), pos.getZ(), key(state), key(result.state()));
                    }
            }
        }
        // Verify the property identity assumption across the whole Java block
        // registry, then exercise both waterlogged values on eligible blocks.
        var excluded = List.of(Set.of("facing", "half", "shape"), Set.of("type"),
                Set.of("north", "east", "south", "west", "up"));
        for (var block : BuiltInRegistries.BLOCK) {
            var properties = block.getStateDefinition().getProperties();
            for (var property : properties)
                if (property.getName().equals("waterlogged") && property != BlockStateProperties.WATERLOGGED)
                    throw new AssertionError("different waterlogged identity: " + block);
            var base = block.defaultBlockState();
            if (!base.hasProperty(BlockStateProperties.WATERLOGGED)) continue;
            for (int category = 0; category < tags.size(); category++) {
                final int c = category;
                if (properties.stream().anyMatch(p -> excluded.get(c).contains(p.getName()))) continue;
                bind.invoke(block.builtInRegistryHolder(), List.of(tags.get(category)));
                for (int m = 0; m < mossiness.length; m++) {
                    var processor = new BlockAgeProcessor(mossiness[m]);
                    for (boolean wet : new boolean[] {false, true}) {
                        var state = base.setValue(BlockStateProperties.WATERLOGGED, wet);
                        for (int i = 0; i < 7; i++) {
                            var pos = new BlockPos(i * 17 - 95, i * 29 - 64, 113 - i * 23);
                            var info = new StructureBlockInfo(pos, state, null);
                            var result = processor.processBlock(null, pos, pos, pos, info, new StructurePlaceSettings());
                            out.printf("C\t%d\t%d\t%d\t%d\t%d\t%s\t%s%n", category, m, pos.getX(), pos.getY(), pos.getZ(), key(state), key(result.state()));
                        }
                    }
                }
            }
        }
        // Vary every property value of each eligible registered block. Wall
        // sides represented by booleans are distinct from wall-side enums.
        for (var block : BuiltInRegistries.BLOCK) {
            var states = new LinkedHashMap<String, BlockState>();
            states.put("default", block.defaultBlockState());
            for (var state : block.getStateDefinition().getPossibleStates())
                state.getValues().forEach(entry ->
                    states.putIfAbsent(entry.property().getName() + "=" + entry.valueName(), state));
            for (int category = 1; category <= 2; category++) {
                bind.invoke(block.builtInRegistryHolder(), List.of(tags.get(category)));
                for (var state : states.values()) {
                    var props = new HashMap<String, String>();
                    state.getValues().forEach(e -> props.put(e.property().getName(), e.valueName()));
                    if (category == 1 && props.containsKey("type")
                            && !Set.of("single", "left", "right", "normal", "sticky").contains(props.get("type"))
                            && !(props.size() == 2 && Set.of("top", "bottom", "double").contains(props.get("type"))
                                && props.containsKey("waterlogged"))) continue;
                    if (category == 2) {
                        boolean boolSides = List.of("north", "east", "south", "west").stream()
                                .allMatch(k -> !props.containsKey(k) || Set.of("true", "false").contains(props.get(k)));
                        boolean wall = props.size() == 6 && props.containsKey("up") && props.containsKey("waterlogged")
                                && List.of("north", "east", "south", "west").stream()
                                    .allMatch(k -> Set.of("none", "low", "tall").contains(props.get(k)));
                        if (!boolSides && !wall) continue;
                    }
                    for (int m = 0; m < mossiness.length; m++) {
                        var processor = new BlockAgeProcessor(mossiness[m]);
                        for (int i = 0; i < 4; i++) {
                            var pos = new BlockPos(i * 17 - 95, i * 29 - 64, 113 - i * 23);
                            var info = new StructureBlockInfo(pos, state, null);
                            var result = processor.processBlock(null, pos, pos, pos, info, new StructurePlaceSettings());
                            out.printf("D\t%d\t%d\t%d\t%d\t%d\t%s\t%s%n", category, m, pos.getX(), pos.getY(), pos.getZ(), key(state), key(result.state()));
                        }
                    }
                }
            }
        }
    }
}
