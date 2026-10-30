import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.nbt.*;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.Strategy;

public class FallingLandingDraftOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var entities = (ListTag) NbtIo.read(Path.of(args[0])).get("Entities");
        if (entities.size() != 1 || !StringTag.valueOf("minecraft:pig").equals(((CompoundTag)entities.get(0)).get("id")))
            throw new AssertionError("falling entity not removed or unrelated entity lost");
        var codec = PalettedContainer.codecRW(BlockState.CODEC,
            Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState());
        var blocks = codec.parse(NbtOps.INSTANCE, NbtIo.read(Path.of(args[1]))).getOrThrow();
        for (int y=0; y<16; y++) for (int z=0; z<16; z++) for (int x=0; x<16; x++) {
            var expected = x==3 && y==1 && z==3 ? Blocks.SAND.defaultBlockState()
                : y==0 || (x==8 && y==2 && z==6) ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
            if (!blocks.get(x,y,z).equals(expected)) throw new AssertionError("unexpected terrain at " + x + "," + y + "," + z);
        }
    }
}
