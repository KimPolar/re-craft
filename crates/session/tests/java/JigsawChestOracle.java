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
            "reinforced_deepslate", "basalt", "polished_basalt", "smooth_basalt"};
        for (var name : stones) {
            var state = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + name)).defaultBlockState();
            out.printf("STONE\t%s\t%s\t%s%n", name, state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO), state.hasBlockEntity());
        }
        for (var axis : Direction.Axis.values()) for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
            var state = Blocks.DEEPSLATE.defaultBlockState().setValue(RotatedPillarBlock.AXIS, axis).mirror(mirror).rotate(rotation);
            out.printf("AXIS\t%s\t%s\t%s\t%s%n", axis.getSerializedName(), mirror.name(), rotation.name(), state.getValue(RotatedPillarBlock.AXIS).getSerializedName());
        }
    }
}
