import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.ChestBlock;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.state.properties.ChestType;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.RotatedPillarBlock;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.storage.TagValueInput;

public class JigsawChestOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var chest = new ChestBlockEntity(new BlockPos(1, 8, 1), Blocks.CHEST.defaultBlockState());
        var tag = new CompoundTag();
        tag.putString("id", "minecraft:chest");
        tag.putString("LootTable", "minecraft:chests/village/village_armorer");
        tag.putLong("LootTableSeed", 123);
        var random = new WorldgenRandom(new XoroshiroRandomSource(42L));
        tag.putLong("LootTableSeed", random.nextLong());
        chest.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING, registries, tag));
        var saved = chest.saveWithFullMetadata(registries);
        out.printf("CHEST\t%s\t%d\t%d%n", chest.getLootTable().identifier(), chest.getLootTableSeed(), random.nextLong());
        if (!saved.contains("LootTable") || !saved.contains("LootTableSeed")) throw new AssertionError(saved);
        for (var facing : new Direction[]{Direction.NORTH, Direction.EAST, Direction.SOUTH, Direction.WEST})
            for (var type : ChestType.values()) for (var wet : new boolean[]{false, true})
                for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var state = Blocks.CHEST.defaultBlockState().setValue(ChestBlock.FACING, facing)
                        .setValue(ChestBlock.TYPE, type).setValue(ChestBlock.WATERLOGGED, wet)
                        .mirror(mirror).rotate(rotation);
                    out.printf("STATE\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n",
                        facing.getSerializedName(), type.getSerializedName(), wet, mirror.name(), rotation.name(),
                        state.getValue(ChestBlock.FACING).getSerializedName(), state.getValue(ChestBlock.TYPE).getSerializedName(), state.getValue(ChestBlock.WATERLOGGED));
                }
        String[] stones = {"stone", "deepslate", "cobbled_deepslate", "polished_deepslate", "deepslate_bricks",
            "cracked_deepslate_bricks", "deepslate_tiles", "cracked_deepslate_tiles", "chiseled_deepslate",
            "reinforced_deepslate", "basalt", "polished_basalt", "smooth_basalt",
            "white_wool", "orange_wool", "magenta_wool", "light_blue_wool", "yellow_wool", "lime_wool",
            "pink_wool", "gray_wool", "light_gray_wool", "cyan_wool", "purple_wool", "blue_wool",
            "brown_wool", "green_wool", "red_wool", "black_wool"};
        for (var name : stones) {
            var state = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + name)).defaultBlockState();
            out.printf("STONE\t%s\t%s\t%s\t%d%n", name, state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO), state.hasBlockEntity(), state.getBlock().getStateDefinition().getPossibleStates().size());
        }
        for (var family : new String[]{"oak","spruce","birch","jungle","acacia","dark_oak","mangrove","cherry","pale_oak","bamboo","crimson","warped","poplar"}) {
            var names = new java.util.ArrayList<String>();
            names.add(family + "_planks");
            if (family.equals("bamboo")) {
                names.add("bamboo_block"); names.add("stripped_bamboo_block");
            } else {
                var a = family.equals("crimson") || family.equals("warped") ? "stem" : "log";
                var b = family.equals("crimson") || family.equals("warped") ? "hyphae" : "wood";
                for (var part : new String[]{a,b}) for (var prefix : new String[]{"","stripped_"}) names.add(prefix + family + "_" + part);
            }
            for (var name : names) {
                var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + name));
                for (var original : block.getStateDefinition().getPossibleStates()) for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var rotated = original.mirror(mirror).rotate(rotation);
                    var axis = original.hasProperty(RotatedPillarBlock.AXIS) ? original.getValue(RotatedPillarBlock.AXIS).getSerializedName() : "-";
                    var nextAxis = rotated.hasProperty(RotatedPillarBlock.AXIS) ? rotated.getValue(RotatedPillarBlock.AXIS).getSerializedName() : "-";
                    out.printf("WOOD\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n", name, axis, mirror.name(), rotation.name(), nextAxis,
                        original.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO), original.hasBlockEntity(),
                        net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(original), original.getPostProcessPos(null, BlockPos.ZERO) != null);
                }
            }
        }
        for (var axis : Direction.Axis.values()) for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
            var state = Blocks.DEEPSLATE.defaultBlockState().setValue(RotatedPillarBlock.AXIS, axis).mirror(mirror).rotate(rotation);
            out.printf("AXIS\t%s\t%s\t%s\t%s%n", axis.getSerializedName(), mirror.name(), rotation.name(), state.getValue(RotatedPillarBlock.AXIS).getSerializedName());
        }
        for (var name : new String[]{"cobbled_deepslate_slab", "polished_deepslate_slab", "deepslate_brick_slab", "deepslate_tile_slab"}) {
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + name));
            if (block.getClass() != SlabBlock.class) throw new AssertionError(name);
            for (var type : SlabType.values()) for (var wet : new boolean[]{false, true})
                for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var state = block.defaultBlockState().setValue(SlabBlock.TYPE, type).setValue(SlabBlock.WATERLOGGED, wet);
                    var transformed = state.mirror(mirror).rotate(rotation);
                    out.printf("SLAB\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n", name, type.getSerializedName(), wet,
                        mirror.name(), rotation.name(), state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO),
                        state.getFluidState().isSource(), transformed.getValue(SlabBlock.TYPE).getSerializedName(), transformed.getValue(SlabBlock.WATERLOGGED));
                }
        }
    }
}
