import java.lang.reflect.Proxy;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.FluidTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.ChestBlock;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.material.Fluids;

public class OceanRuinChestMarkerOracle {
    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags", java.util.Collection.class);
        bind.setAccessible(true);
        bind.invoke(Fluids.WATER.builtInRegistryHolder(), List.of(FluidTags.WATER));
        var unsafeClass=Class.forName("sun.misc.Unsafe"); var f=unsafeClass.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var pieceClass=Class.forName("net.minecraft.world.level.levelgen.structure.structures.OceanRuinPieces$OceanRuinPiece");
        var piece=unsafeClass.getMethod("allocateInstance",Class.class).invoke(f.get(null),pieceClass);
        var largeField=pieceClass.getDeclaredField("isLarge"); largeField.setAccessible(true);
        var method=pieceClass.getDeclaredMethod("handleDataMarker",String.class,BlockPos.class,ServerLevelAccessor.class,RandomSource.class,BoundingBox.class);
        method.setAccessible(true);
        var position=new BlockPos(-7,56,21);
        for (int n=0;n<64;n++) {
            boolean wet=(n&1)!=0, large=(n&2)!=0, chestExists=(n&4)!=0, setResult=(n&8)!=0;
            long seed=(n/16)*99999999991L-123456789L;
            largeField.setBoolean(piece,large);
            var chest=new ChestBlockEntity(position,Blocks.CHEST.defaultBlockState());
            var events=new StringBuilder();
            var level=(ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)->switch(m.getName()){
                case "getFluidState" -> { events.append("F"); yield wet?Fluids.WATER.defaultFluidState():Fluids.EMPTY.defaultFluidState(); }
                case "setBlock" -> { BlockState state=(BlockState)v[1]; if (!state.is(Blocks.CHEST) || (int)v[2]!=2) throw new AssertionError("bad state/flags"); events.append(state.getValue(ChestBlock.WATERLOGGED)?"W":"D"); yield setResult; }
                case "getBlockEntity" -> { events.append("E"); yield chestExists?chest:null; }
                default -> throw new AssertionError("unexpected "+m);
            });
            var random=RandomSource.create(seed);
            method.invoke(piece,"chest",position,level,random,new BoundingBox(position));
            String table=chest.getLootTable()==null?"-":chest.getLootTable().identifier().toString();
            output.println("C\t"+n+"\t"+events+"\t"+table+"\t"+chest.getLootTableSeed()+"\t"+random.nextLong());
        }
    }
}
