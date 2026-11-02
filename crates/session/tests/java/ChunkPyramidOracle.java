import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.chunk.status.*;

public class ChunkPyramidOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        for (var step : ChunkPyramid.GENERATION_PYRAMID.steps()) {
            out.print(step.targetStatus().getName()+" "+step.blockStateWriteRadius()+" ");
            out.print(step.directDependencies().asList().stream().map(ChunkStatus::getName).reduce((a,b)->a+","+b).orElse("-"));
            out.print(" ");
            out.println(step.accumulatedDependencies().asList().stream().map(ChunkStatus::getName).reduce((a,b)->a+","+b).orElse("-"));
        }
    }
}
