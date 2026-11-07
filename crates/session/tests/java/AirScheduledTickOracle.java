import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.Blocks;

public class AirScheduledTickOracle {
    public static void main(String[] args) {
        var out=System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        for(var block:new net.minecraft.world.level.block.Block[]{Blocks.AIR,Blocks.CAVE_AIR,Blocks.VOID_AIR}) {
            var random=RandomSource.create(42);
            block.defaultBlockState().tick(null,BlockPos.ZERO,random);
            out.printf("A %s %d%n",BuiltInRegistries.BLOCK.getKey(block),random.nextLong());
        }
    }
}
