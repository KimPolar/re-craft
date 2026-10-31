import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.*;

public class AquiferDefaultStatesOracle {
    public static void main(String[] args) throws Exception {
        var out=System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        for(var block:BuiltInRegistries.BLOCK) for(var state:block.getStateDefinition().getPossibleStates()) if(state.isAir()) {
            String id=BuiltInRegistries.BLOCK.getKey(block).toString();
            if(!id.equals("minecraft:air") && !id.equals("minecraft:cave_air") && !id.equals("minecraft:void_air")) throw new AssertionError("unexpected air state "+state);
        }
        String[] states={"\"minecraft:water\"","{\"id\":\"minecraft:water\",\"properties\":{\"level\":\"1\"}}",
            "\"minecraft:lava\"","{\"id\":\"minecraft:lava\",\"properties\":{\"level\":\"1\"}}",
            "{\"id\":\"minecraft:lava\",\"properties\":{\"level\":\"15\"}}",
            "{\"id\":\"minecraft:lava\",\"properties\":{\"level\":\"99\"}}",
            "\"minecraft:air\"","\"minecraft:cave_air\"","\"minecraft:void_air\"","\"minecraft:stone\"","\"minecraft:sand\""};
        var zero=DensityFunctions.zero();
        var config=new Aquifer.Config(zero,DensityFunctions.constant(0.3f),DensityFunctions.constant(0.25f),
            DensityFunctions.constant(0.6f),zero,DensityFunctions.constant(20));
        var compiler=new DensityFunctionCompiler(null);
        DensitySamplerSet set=f -> new DensitySampler.Bound(compiler.getSampler(f),SamplerContext.EMPTY_UNCACHED);
        var factory=new LegacyRandomSource(12345).forkPositional().fromHashOf(Identifier.withDefaultNamespace("aquifer")).forkPositional();
        var volume=new DensityVolume(16,128,16,-16,-64,32);
        for(int s=0;s<states.length;s++) {
            BlockState fluid=BlockState.CODEC.parse(JsonOps.INSTANCE,JsonParser.parseString(states[s])).getOrThrow();
            var global=(Aquifer.FluidPicker)(x,y,z) -> y < -54 ? new Aquifer.FluidStatus(-54,Blocks.LAVA.defaultBlockState()) : new Aquifer.FluidStatus(32,fluid);
            var aquifer=config.create(set,factory,volume,global);
            out.printf("S %d%n",s);
            for(int n=0;n<2048;n++) {
                int x=(n*7)%16, z=(n*11+n/128)%16, y=(n*37)%128-64;
                BlockState state=aquifer.computeSubstance(-16+x,y,32+z,-1);
                int code=state==null?0:state==Blocks.AIR.defaultBlockState()?3:state==Blocks.LAVA.defaultBlockState()?2:state==fluid?1:-1;
                if(code<0) throw new AssertionError("unexpected state "+state);
                out.printf("P %d %d %d %d %d%n",x,y,z,code,aquifer.shouldScheduleFluidUpdate()?1:0);
            }
        }
    }
}
