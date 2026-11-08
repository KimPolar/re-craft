import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;

public class CropUseOracle {
    static class World extends ServerLevel {
        BlockState soil, written;
        int light;
        RandomSource random;
        World() { super(null,null,null,null,null,null,false,0,List.of(),false); }
        @Override public int getRawBrightness(BlockPos pos, int darken) { return light; }
        @Override public BlockState getBlockState(BlockPos pos) { return soil; }
        @Override public RandomSource getRandom() { return random; }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) {
            if (flags != 2) throw new AssertionError("wrong bonemeal update flags " + flags);
            written=state; return true;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world=(World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
        var result=new JsonObject(); var survival=new JsonArray(); var bonemeal=new JsonArray();
        var crops=List.of(Blocks.WHEAT, Blocks.CARROTS, Blocks.POTATOES, Blocks.BEETROOTS);
        for (int kind=0;kind<4;kind++) {
            var block=(BonemealableBlock)crops.get(kind); int max=kind==3 ? 3 : 7;
            var property=kind==3 ? BeetrootBlock.AGE : CropBlock.AGE;
            for (boolean tagged:new boolean[]{false,true}) {
                bind.invoke(Blocks.DIRT.builtInRegistryHolder(),tagged ? List.of(BlockTags.SUPPORTS_CROPS) : List.of());
                world.soil=Blocks.DIRT.defaultBlockState();
                for (int light:new int[]{0,7,8,9,15}) {
                    world.light=light;
                    var row=new JsonArray(); row.add(kind); row.add(tagged); row.add(light);
                    row.add(crops.get(kind).defaultBlockState().canSurvive(world,BlockPos.ZERO)); survival.add(row);
                }
            }
            for (int age=0;age<=max;age++) for (long seed:new long[]{-1,0,1,42,1234567,Long.MIN_VALUE}) {
                var state=crops.get(kind).defaultBlockState().setValue(property,age);
                world.random=RandomSource.create(seed); world.written=null;
                boolean valid=block.isValidBonemealTarget(world,BlockPos.ZERO,state,BonemealSource.INTERACTION);
                if (valid && block.isBonemealSuccess(world,world.random,BlockPos.ZERO,state,BonemealSource.INTERACTION))
                    block.performBonemeal(world,world.random,BlockPos.ZERO,state,BonemealSource.INTERACTION);
                var row=new JsonArray(); row.add(kind); row.add(age); row.add(seed); row.add(valid);
                row.add(world.written==null ? age : world.written.getValue(property)); row.add(world.random.nextInt()); bonemeal.add(row);
            }
        }
        result.add("survival",survival); result.add("bonemeal",bonemeal);
        Files.writeString(Path.of(args[0]),result.toString());
    }
}
