import java.lang.reflect.Proxy;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;

public class BonusChestSnowyOracle {
    public static void main(String[] args) {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var origin=new BlockPos(0,8,0);
        for(var block:new Block[]{Blocks.GRASS_BLOCK,Blocks.PODZOL,Blocks.MYCELIUM}) {
            if(!(block instanceof SnowyBlock)) throw new AssertionError("not a snowy block");
            for(boolean snowy:new boolean[]{false,true}) for(var direction:Direction.values())
                for(var replacement:new Block[]{Blocks.CHEST,Blocks.TORCH}) {
                    var state=block.defaultBlockState().setValue(BlockStateProperties.SNOWY,snowy);
                    var random=RandomSource.create(42);
                    var next=state.updateShape(null,null,origin,direction,origin.relative(direction),replacement.defaultBlockState(),random);
                    var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},
                        (proxy,method,values)->{if(method.getName().equals("getBlockState")) return state;throw new UnsupportedOperationException(method.toString());});
                    boolean survives=Blocks.TORCH.defaultBlockState().canSurvive(level,origin.above());
                    out.printf("S %s %s %s %s %s %s %d%n",BuiltInRegistries.BLOCK.getKey(block),snowy,
                        direction.getSerializedName(),BuiltInRegistries.BLOCK.getKey(replacement),state==next,survives,random.nextLong());
                }
        }
    }
}
