import java.io.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.Heightmap;

public class FullHeightmapUpdateOracle {
    public static void main(String[] args)throws Exception {
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
        var ids=new IdMapper<Holder<Biome>>();ids.add(biome);
        var factory=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
        var types=new Heightmap.Types[]{Heightmap.Types.WORLD_SURFACE,Heightmap.Types.OCEAN_FLOOR,Heightmap.Types.MOTION_BLOCKING,Heightmap.Types.MOTION_BLOCKING_NO_LEAVES};
        var blocks=new Block[]{Blocks.AIR,Blocks.STONE,Blocks.OAK_LEAVES,Blocks.WATER,Blocks.RED_MUSHROOM};
        CompoundTag root;
        try(var stream=new DataInputStream(Files.newInputStream(Path.of(args[0])))){root=NbtIo.read((DataInput)stream);}
        for(var item:(ListTag)root.get("cases")) {
            var test=(CompoundTag)item; int min=test.getIntOr("min",0),height=test.getIntOr("height",0);
            boolean airMotion=test.getBooleanOr("air_motion",false);
            bind.invoke(Blocks.AIR.builtInRegistryHolder(),airMotion?List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES):List.of());
            bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES));
            bind.invoke(Blocks.OAK_LEAVES.builtInRegistryHolder(),List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP));
            var chunk=new ProtoChunk(new ChunkPos(0,0),UpgradeData.EMPTY,LevelHeightAccessor.create(min,height),factory,null);
            byte[] cells=test.getByteArray("blocks").orElseThrow();
            for(int y=0;y<height;y++)for(int z=0;z<16;z++)for(int x=0;x<16;x++) {
                int kind=cells[x+z*16+y*256];
                chunk.getSection(y/16).setBlockState(x,y%16,z,blocks[kind].defaultBlockState(),false);
            }
            var source=test.getCompoundOrEmpty("source");
            var maps=new Heightmap[4];
            for(int i=0;i<4;i++) {
                maps[i]=new Heightmap(chunk,types[i]);
                maps[i].setRawData(chunk,types[i],source.getLongArray(types[i].getSerializationKey()).orElseThrow());
            }
            int step=0;
            for(var editTag:(ListTag)test.get("edits")) {
                var edit=(CompoundTag)editTag;
                int x=edit.getIntOr("x",0),y=edit.getIntOr("y",0),z=edit.getIntOr("z",0),kind=edit.getIntOr("kind",0);
                var state=blocks[kind].defaultBlockState();
                chunk.getSection((y-min)/16).setBlockState(x,(y-min)%16,z,state,false);
                for(var map:maps)map.update(x,y,z,state);
                var expected=edit.getCompoundOrEmpty("expected");
                for(int i=0;i<4;i++)if(!Arrays.equals(maps[i].getRawData(),expected.getLongArray(types[i].getSerializationKey()).orElseThrow()))
                    throw new AssertionError("heightmap mismatch "+types[i]+" step "+step+" min "+min+" air "+airMotion);
                step++;
            }
            bind.invoke(Blocks.AIR.builtInRegistryHolder(),List.of());
        }
    }
}
