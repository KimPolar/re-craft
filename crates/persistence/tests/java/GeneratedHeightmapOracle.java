import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.nbt.*;
import net.minecraft.util.SimpleBitStorage;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.Heightmap;

public class GeneratedHeightmapOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
        var ids=new IdMapper<Holder<Biome>>(); ids.add(biome);
        var factory=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(ids), biome, null);
        var chunk=new ProtoChunk(new ChunkPos(0,0),UpgradeData.EMPTY,LevelHeightAccessor.create(0,16),factory,null);
        chunk.setPersistedStatus(ChunkStatus.TERRAIN);
        chunk.getSection(0).setBlockState(0,3,0,Blocks.STONE.defaultBlockState(),false);
        if(!chunk.getHeightmaps().isEmpty()) throw new AssertionError("direct section write primed maps");
        chunk.getHeight(Heightmap.Types.OCEAN_FLOOR_WG,0,0);
        var kinds=new HashSet<Heightmap.Types>(); for(var entry:chunk.getHeightmaps()) kinds.add(entry.getKey());
        if(!kinds.equals(Set.of(Heightmap.Types.OCEAN_FLOOR_WG))) throw new AssertionError("unexpected lazy map creation "+kinds);
        chunk.setBlockState(new BlockPos(0,4,0),Blocks.STONE.defaultBlockState(),2);
        kinds.clear(); for(var entry:chunk.getHeightmaps()) kinds.add(entry.getKey());
        if(!kinds.equals(Set.of(Heightmap.Types.OCEAN_FLOOR_WG,Heightmap.Types.WORLD_SURFACE,Heightmap.Types.OCEAN_FLOOR,Heightmap.Types.MOTION_BLOCKING,Heightmap.Types.MOTION_BLOCKING_NO_LEAVES))) throw new AssertionError("unexpected maps after write "+kinds);
        CompoundTag root;
        try (var stream=new DataInputStream(Files.newInputStream(Path.of(args[0])))) { root=NbtIo.read((DataInput)stream); }
        int height=root.getIntOr("height",0); int bits=32-Integer.numberOfLeadingZeros(height);
        var maps=root.getCompoundOrEmpty("Heightmaps");
        var names=new String[]{"WORLD_SURFACE","OCEAN_FLOOR","MOTION_BLOCKING","MOTION_BLOCKING_NO_LEAVES","WORLD_SURFACE_WG","OCEAN_FLOOR_WG"};
        for(int kind=0;kind<names.length;kind++) {
            var expected=new SimpleBitStorage(bits,256);
            for(int i=0;i<256;i++) expected.set(i,(i*37+kind*13)%(height+1));
            long[] actual=maps.getLongArray(names[kind]).orElseThrow();
            if(!Arrays.equals(actual,expected.getRaw())) throw new AssertionError("packed "+names[kind]);
            var decoded=new SimpleBitStorage(bits,256,actual);
            for(int i=0;i<256;i++) if(decoded.get(i)!=expected.get(i)) throw new AssertionError("index "+i);
        }
    }
}
