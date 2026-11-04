import java.io.*;
import java.nio.file.*;
import com.mojang.serialization.Codec;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.nbt.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import java.util.Arrays;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;

public class GeneratedChunkOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var biome=Holder.direct((Biome)((sun.misc.Unsafe)field.get(null)).allocateInstance(Biome.class));
        var ids=new IdMapper<Holder<Biome>>(); ids.add(biome);
        var blocks=Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        var biomes=Strategy.createForBiomes(ids);
        Codec<Holder<Biome>> biomeCodec=Codec.STRING.xmap(name -> {
            if (!name.equals("minecraft:plains")) throw new IllegalArgumentException(name);
            return biome;
        }, value -> "minecraft:plains");
        var factory=new PalettedContainerFactory(blocks, Blocks.AIR.defaultBlockState(),
            PalettedContainer.codecRW(BlockState.CODEC, blocks, Blocks.AIR.defaultBlockState()),
            biomes, biome, PalettedContainer.codecRO(biomeCodec, biomes, biome));
        CompoundTag input; try(var stream=new DataInputStream(Files.newInputStream(Path.of(args[0])))) { input=NbtIo.read((DataInput)stream); }
        int minY=Integer.parseInt(args[1]);
        var parsed=SerializableChunkData.parse(LevelHeightAccessor.create(minY,16),factory,input);
        if(parsed==null) throw new AssertionError("parse returned null");
        var output=parsed.write();
        input.remove("sections"); var outputSections=output.remove("sections");
        if(!input.equals(output)) throw new AssertionError("chunk metadata or component mismatch");
        output.put("sections", outputSections);
        var reread=SerializableChunkData.parse(LevelHeightAccessor.create(minY,16),factory,output);
        if(parsed.sectionData().size()!=1 || reread.sectionData().size()!=1) throw new AssertionError("section count");
        var a=parsed.sectionData().get(0); var b=reread.sectionData().get(0);
        if(a.y()!=minY/16 || b.y()!=minY/16 || !Arrays.equals(a.blockLight().getData(),b.blockLight().getData()) || !Arrays.equals(a.skyLight().getData(),b.skyLight().getData())) throw new AssertionError("section light");
        var expectedChest=Blocks.CHEST.defaultBlockState().setValue(BlockStateProperties.HORIZONTAL_FACING,Direction.WEST);
        for(int y=0;y<16;y++) for(int z=0;z<16;z++) for(int x=0;x<16;x++) {
            var expected=x==1 && y==4 && z==2 ? expectedChest : Blocks.STONE.defaultBlockState();
            if(a.chunkSection().getBlockState(x,y,z)!=expected || b.chunkSection().getBlockState(x,y,z)!=expected) throw new AssertionError("block state");
        }
        for(int y=0;y<4;y++) for(int z=0;z<4;z++) for(int x=0;x<4;x++) {
            if(a.chunkSection().getNoiseBiome(x,y,z)!=biome || b.chunkSection().getNoiseBiome(x,y,z)!=biome) throw new AssertionError("biome");
        }
    }
}
