import java.nio.file.Path;
import java.lang.reflect.Method;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.nbt.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;

public class CombinedPostprocessingOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var biome=Holder.direct((Biome)((sun.misc.Unsafe)field.get(null)).allocateInstance(Biome.class));
        var ids=new IdMapper<Holder<Biome>>(); ids.add(biome);
        var factory=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
        var chunk=new ProtoChunk(new ChunkPos(-1,1),UpgradeData.EMPTY,LevelHeightAccessor.create(0,16),factory,null);
        for(var p:new BlockPos[]{new BlockPos(-16,0,16),new BlockPos(-1,15,31),new BlockPos(-16,0,16),
                new BlockPos(-1,15,31),new BlockPos(-1,15,31),new BlockPos(-8,8,24),new BlockPos(-8,-1,24)})
            chunk.markPosForPostProcessing(p);
        Method pack=SerializableChunkData.class.getDeclaredMethod("packOffsets",it.unimi.dsi.fastutil.shorts.ShortList[].class);
        pack.setAccessible(true);
        var expected=(ListTag)pack.invoke(null,(Object)chunk.getPostProcessing());
        var root=NbtIo.read(Path.of(args[0]));
        if(!expected.equals(root.get("PostProcessing"))) throw new AssertionError("expected "+expected+" got "+root);
    }
}
