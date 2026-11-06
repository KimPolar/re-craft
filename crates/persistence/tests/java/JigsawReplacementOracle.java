import java.lang.reflect.Proxy;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.structure.templatesystem.JigsawReplacementProcessor;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate.StructureBlockInfo;

public class JigsawReplacementOracle {
    static final String[] VALUES = {
        "minecraft:air", "stone", "minecraft:structure_void", "oak_stairs[facing=east]",
        "oak_stairs[waterlogged=true,half=top]", "oak_stairs[shape=outer_left,facing=west]",
        "oak_stairs[facing=sideways]", "oak_stairs[facing=east,facing=west]",
        "oak_stairs[unknown=x]", "oak_stairs[facing=east", "stone trailing data",
        "oak_stairs[facing='east',]", "oak_stairs[]", "stone[]", "stone[foo=bar]",
        "water[level=15]", "water[level=16]", "grass_block[snowy=true]",
        "oak_stairs[ facing = \"south\" , waterlogged = true ]",
        "oak_stairs[facing=east, ]", "stone{broken"
    };

    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var world = (LevelReader) Proxy.newProxyInstance(LevelReader.class.getClassLoader(),
            new Class<?>[]{LevelReader.class}, (proxy, method, arguments) -> {
                if (method.getName().equals("holderLookup")) return BuiltInRegistries.BLOCK;
                throw new UnsupportedOperationException(method.toString());
            });
        var pos = new BlockPos(3, -4, 5);
        for (int i = 0; i < VALUES.length; i++) {
            var nbt = new CompoundTag();
            nbt.putString("final_state", VALUES[i]);
            var info = new StructureBlockInfo(pos, Blocks.JIGSAW.defaultBlockState(), nbt);
            var result = JigsawReplacementProcessor.INSTANCE.processBlock(
                world, pos, pos, pos, info, new StructurePlaceSettings());
            out.printf("RESULT\t%d\t%s\t%s%n", i, result == null ? "<skip>" : result.state(),
                result == null ? "-" : result.nbt() == null ? "none" : "present");
        }
    }
}
