import java.io.*;
import java.nio.file.*;
import com.mojang.serialization.Codec;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.nbt.*;
import net.minecraft.core.IdMapper;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.BeehiveBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.core.Direction;
import net.minecraft.world.level.chunk.*;

public class GeneratedSectionOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        CompoundTag root; try(var input = new DataInputStream(Files.newInputStream(Path.of(args[0])))) { root = NbtIo.read((DataInput)input); }
        var section = root.getListOrEmpty("sections").getCompoundOrEmpty(0);
        var blockCodec = PalettedContainer.codecRW(BlockState.CODEC, Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState());
        var blocks = blockCodec.parse(NbtOps.INSTANCE, section.get("block_states")).getOrThrow();
        var ids = new IdMapper<String>(); ids.add("minecraft:plains"); ids.add("minecraft:desert");
        var biomeCodec = PalettedContainer.codecRW(Codec.STRING, Strategy.createForBiomes(ids), "minecraft:plains");
        var biomes = biomeCodec.parse(NbtOps.INSTANCE, section.get("biomes")).getOrThrow();
        var expected = new BlockState[]{Blocks.STONE.defaultBlockState(), Blocks.OAK_LOG.defaultBlockState().setValue(BlockStateProperties.AXIS,Direction.Axis.X), Blocks.AIR.defaultBlockState(), Blocks.OAK_LOG.defaultBlockState().setValue(BlockStateProperties.AXIS,Direction.Axis.Z), Blocks.DIRT.defaultBlockState(), Blocks.COBBLESTONE.defaultBlockState(), Blocks.GRANITE.defaultBlockState(), Blocks.DIORITE.defaultBlockState(), Blocks.ANDESITE.defaultBlockState(), Blocks.GRAVEL.defaultBlockState(), Blocks.SAND.defaultBlockState(), Blocks.SANDSTONE.defaultBlockState(), Blocks.NETHERRACK.defaultBlockState(), Blocks.OBSIDIAN.defaultBlockState(), Blocks.BEDROCK.defaultBlockState(), Blocks.COAL_ORE.defaultBlockState(), Blocks.IRON_ORE.defaultBlockState(), Blocks.GOLD_ORE.defaultBlockState(), Blocks.DIAMOND_ORE.defaultBlockState(), Blocks.BEE_NEST.defaultBlockState().setValue(BlockStateProperties.HORIZONTAL_FACING,Direction.SOUTH).setValue(BeehiveBlock.HONEY_LEVEL,2), Blocks.CHEST.defaultBlockState().setValue(BlockStateProperties.HORIZONTAL_FACING,Direction.WEST), Blocks.SPAWNER.defaultBlockState()};
        for(int y=0;y<16;y++) for(int z=0;z<16;z++) for(int x=0;x<16;x++) {
            int i=x+16*z+256*y;
            if (blocks.get(x,y,z)!=expected[i%expected.length]) throw new AssertionError("block "+x+","+y+","+z);
        }
        for(int y=0;y<4;y++) for(int z=0;z<4;z++) for(int x=0;x<4;x++) {
            int i=x+4*z+16*y; String expectedBiome=i%2==0?"minecraft:plains":"minecraft:desert";
            if(!biomes.get(x,y,z).equals(expectedBiome)) throw new AssertionError("biome "+x+","+y+","+z);
        }
    }
}
