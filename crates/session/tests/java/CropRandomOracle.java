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

public class CropRandomOracle {
    static final BlockPos POS = BlockPos.ZERO;
    static final int[][] NEIGHBORS = {{0,-1},{0,1},{-1,0},{1,0},{-1,-1},{1,-1},{1,1},{-1,1}};
    static class World extends ServerLevel {
        Block crop;
        int[] soil;
        int neighbors, brightness, written;
        World() { super(null,null,null,null,null,null,false,0,List.of(),false); }
        @Override public int getRawBrightness(BlockPos pos, int darken) { return brightness; }
        @Override public BlockState getBlockState(BlockPos pos) {
            int x=pos.getX(), y=pos.getY(), z=pos.getZ();
            if (y == -1 && Math.abs(x)<=1 && Math.abs(z)<=1) {
                int value=soil[(x+1)*3+z+1];
                return value==0 ? Blocks.STONE.defaultBlockState() : value==3 ? Blocks.DIRT.defaultBlockState() :
                    Blocks.FARMLAND.defaultBlockState().setValue(FarmlandBlock.MOISTURE,value==2 ? 7 : 0);
            }
            if (y == 0) for (int i=0;i<NEIGHBORS.length;i++)
                if (x==NEIGHBORS[i][0] && z==NEIGHBORS[i][1] && (neighbors & (1<<i))!=0)
                    return crop.defaultBlockState();
            return Blocks.AIR.defaultBlockState();
        }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) {
            written=state.getValue(crop==Blocks.BEETROOTS ? BeetrootBlock.AGE : CropBlock.AGE);
            if (flags!=2) throw new AssertionError("unexpected flags " + flags);
            return true;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        bind.invoke(Blocks.FARMLAND.builtInRegistryHolder(),List.of(BlockTags.GROWS_CROPS));
        bind.invoke(Blocks.DIRT.builtInRegistryHolder(),List.of(BlockTags.GROWS_CROPS));
        var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var speed=CropBlock.class.getDeclaredMethod("getGrowthSpeed",Block.class,net.minecraft.world.level.BlockGetter.class,BlockPos.class);
        speed.setAccessible(true);
        int[][] soils={{0,0,0,0,0,0,0,0,0},{1,1,1,1,1,1,1,1,1},
            {2,2,2,2,2,2,2,2,2},{0,1,2,1,2,0,2,0,1},{3,0,1,0,3,2,1,2,3}};
        int[] masks={0,1,3,5,16,64,255};
        var rows=new JsonArray();
        for (int kind=0;kind<4;kind++) {
            world.crop=List.of(Blocks.WHEAT,Blocks.CARROTS,Blocks.POTATOES,Blocks.BEETROOTS).get(kind);
            int max=kind==3 ? 3 : 7;
            for (int pattern=0;pattern<soils.length;pattern++) for (int mask:masks) {
                world.soil=soils[pattern]; world.neighbors=mask;
                float expectedSpeed=(float)speed.invoke(null,world.crop,world,POS);
                for (int light:new int[]{0,8,9,15}) for (int age:new int[]{0,max-1,max})
                for (long seed:new long[]{0,1,42,-1,1234567}) {
                    world.brightness=light; world.written=-1;
                    var state=world.crop.defaultBlockState().setValue(kind==3 ? BeetrootBlock.AGE : CropBlock.AGE,age);
                    var random=RandomSource.create(seed); state.randomTick(world,POS,random);
                    var row=new JsonArray(); row.add(kind); row.add(pattern); row.add(mask); row.add(light);
                    row.add(age); row.add(seed); row.add(Float.floatToRawIntBits(expectedSpeed));
                    row.add(world.written<0 ? age : world.written); row.add(random.nextInt()); rows.add(row);
                }
            }
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
