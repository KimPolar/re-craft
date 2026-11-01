import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;

public class FrostedIceCascadeOracle {
    static final BlockPos CENTER = new BlockPos(0, 10, 0);
    static final Direction[] DIRECTIONS = Direction.values();
    static class World extends ServerLevel {
        Map<BlockPos,BlockState> blocks; JsonArray events; RandomSource random;
        EnvironmentAttributeSystem attributes; CollectingNeighborUpdater updater;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
        @Override public int getBrightness(LightLayer layer, BlockPos pos) { return 15; }
        @Override public int getMaxLocalRawBrightness(BlockPos pos) { return 15; }
        @Override public ResourceKey<Level> dimension() { return Level.OVERWORLD; }
        @Override public EnvironmentAttributeSystem environmentAttributes() { return attributes; }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.DEFAULT_FLAGS; }
        @Override public RandomSource getRandom() { return random; }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) {
            var old = getBlockState(pos); blocks.put(pos.immutable(), state);
            var event = new JsonArray(); event.add(state.is(Blocks.FROSTED_ICE) ? "age" : "melt");
            event.add(pos.getX()); event.add(pos.getY()); event.add(pos.getZ());
            if (state.is(Blocks.FROSTED_ICE)) event.add(state.getValue(FrostedIceBlock.AGE));
            events.add(event);
            if ((flags & 512) == 0 && state.is(Blocks.FROSTED_ICE)) state.onPlace(this, pos, old, false);
            if ((flags & 1) != 0) updater.updateNeighborsAtExceptFromFacing(pos, old.getBlock(), null, null);
            return true;
        }
        @Override public void scheduleTick(BlockPos pos, Block block, int delay) {
            var event = new JsonArray(); event.add("schedule"); event.add(pos.getX()); event.add(pos.getY()); event.add(pos.getZ()); event.add(delay); events.add(event);
        }
        @Override public void scheduleTick(BlockPos pos, net.minecraft.world.level.material.Fluid fluid, int delay) { }
        @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        @Override public void neighborChanged(BlockPos pos, Block block, Orientation orientation) { updater.neighborChanged(pos, block, orientation); }
        @Override public void gameEvent(Holder<GameEvent> event, Vec3 pos, GameEvent.Context context) { }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); world.attributes=EnvironmentAttributeSystem.builder().build();
        var method=FrostedIceBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class); method.setAccessible(true);
        var rows=new JsonArray();
        for (int seed=0; seed<32; seed++) for (int mask=0; mask<64; mask++) for (int outer=0;outer<2;outer++) {
            world.blocks=new HashMap<>(); world.events=new JsonArray(); world.random=new LegacyRandomSource(seed);
            world.updater=new CollectingNeighborUpdater(world,1000000);
            var center=Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,3); world.blocks.put(CENTER,center);
            for (int i=0;i<6;i++) if ((mask & (1<<i))!=0) {
                var pos=CENTER.relative(DIRECTIONS[i]);
                world.blocks.put(pos,Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,(seed+i)%4));
                if (outer!=0) world.blocks.put(pos.relative(DIRECTIONS[i]),Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,(seed+i+1)%4));
            }
            method.invoke(Blocks.FROSTED_ICE,center,world,CENTER,world.random);
            var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("mask",mask); row.addProperty("outer",outer); row.add("events",world.events); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
