import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.decoration.ArmorStand;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.item.enchantment.LevelBasedValue;
import net.minecraft.world.item.enchantment.effects.ReplaceDisk;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.VoxelShape;

public class FrostWalkerNeighborsOracle {
    static final BlockPos CENTER = new BlockPos(8,1,8);
    static final Direction[] DIRECTIONS = Direction.values();
    static class World extends ServerLevel {
        Map<BlockPos,BlockState> blocks; RandomSource random; JsonArray scheduled; CollectingNeighborUpdater updater;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public RandomSource getRandom() { return random; }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.DEFAULT_FLAGS; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
        @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        @Override public boolean isUnobstructed(Entity except, VoxelShape shape) { return true; }
        @Override public void neighborChanged(BlockPos pos, Block block, Orientation orientation) { updater.neighborChanged(pos, block, orientation); }
        @Override public void neighborShapeChanged(Direction direction, BlockPos target, BlockPos source, BlockState state, int flags, int depth) {
            updater.shapeUpdate(direction, state, target, source, flags, depth);
        }
        @Override public boolean setBlockAndUpdate(BlockPos pos, BlockState state) {
            var old = getBlockState(pos); if (old == state) return false;
            blocks.put(pos.immutable(),state); state.onPlace(this,pos,old,false);
            updater.updateNeighborsAtExceptFromFacing(pos,old.getBlock(),null,null);
            old.updateIndirectNeighbourShapes(this,pos,2,511);
            state.updateNeighbourShapes(this,pos,2,511);
            state.updateIndirectNeighbourShapes(this,pos,2,511);
            return true;
        }
        @Override public void scheduleTick(BlockPos pos, Block block, int delay) {
            var row = new JsonArray(); row.add(block == Blocks.FROSTED_ICE ? "ice" : "falling");
            row.add(pos.getX()); row.add(pos.getY()); row.add(pos.getZ()); row.add(delay); scheduled.add(row);
        }
        @Override public void scheduleTick(BlockPos pos, net.minecraft.world.level.material.Fluid fluid, int delay) { }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var entity=(ArmorStand)unsafe.allocateInstance(ArmorStand.class);
        var entityRandom=Entity.class.getDeclaredField("random"); unsafe.putObject(entity,unsafe.objectFieldOffset(entityRandom),new LegacyRandomSource(0));
        var effect=new ReplaceDisk(LevelBasedValue.constant(1),LevelBasedValue.constant(1),new Vec3i(0,-1,0),
            Optional.of(BlockPredicate.matchesBlocks(Blocks.WATER)),BlockStateProvider.holderOf(Blocks.FROSTED_ICE),Optional.empty());
        Block[] blocks={Blocks.SAND,Blocks.RED_SAND,Blocks.GRAVEL,Blocks.GLASS,Blocks.COBBLESTONE,Blocks.MOSSY_COBBLESTONE,
            Blocks.GRANITE,Blocks.DIORITE,Blocks.ANDESITE,Blocks.CALCITE,Blocks.TUFF,Blocks.SANDSTONE,Blocks.RED_SANDSTONE,Blocks.STONE_BRICKS,Blocks.CLAY}; var rows=new JsonArray();
        for (int seed=0;seed<4;seed++) for (int type=0;type<blocks.length;type++) for (int mask : new int[]{1,2,4,8,16,32,63}) {
            world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(seed); world.scheduled=new JsonArray(); world.updater=new CollectingNeighborUpdater(world,10000);
            world.blocks.put(CENTER,Blocks.WATER.defaultBlockState());
            for(int i=0;i<6;i++) world.blocks.put(CENTER.relative(DIRECTIONS[i]),((mask & (1<<i)) != 0 ? blocks[type] : Blocks.STONE).defaultBlockState());
            effect.apply(world,1,null,entity,new Vec3(8.5,2,8.5));
            var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("type",type); row.addProperty("mask",mask);
            row.add("scheduled",world.scheduled); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
