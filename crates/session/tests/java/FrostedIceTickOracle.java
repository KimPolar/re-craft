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
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.redstone.Orientation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;

public class FrostedIceTickOracle {
    static final BlockPos CENTER = new BlockPos(0, 10, 0);
    static final Direction[] DIRECTIONS = Direction.values();
    static class World extends ServerLevel {
        Map<BlockPos,BlockState> blocks; JsonArray events; int brightness; boolean end;
        RandomSource random; EnvironmentAttributeSystem attributes;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.STONE.defaultBlockState()); }
        @Override public int getBrightness(LightLayer layer, BlockPos pos) { return brightness; }
        @Override public int getMaxLocalRawBrightness(BlockPos pos) { return 15 - brightness; }
        @Override public ResourceKey<Level> dimension() { return end ? Level.END : Level.OVERWORLD; }
        @Override public EnvironmentAttributeSystem environmentAttributes() { return attributes; }
        @Override public RandomSource getRandom() { return random; }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) {
            var old = getBlockState(pos); blocks.put(pos.immutable(), state);
            var event = new JsonArray(); event.add(state.is(Blocks.FROSTED_ICE) ? "age" : "melt"); event.add(index(pos));
            if (state.is(Blocks.FROSTED_ICE)) event.add(state.getValue(FrostedIceBlock.AGE));
            events.add(event);
            if ((flags & 512) == 0 && state.is(Blocks.FROSTED_ICE)) state.onPlace(this, pos, old, false);
            return true;
        }
        @Override public void scheduleTick(BlockPos pos, Block block, int delay) {
            var event = new JsonArray(); event.add("schedule"); event.add(index(pos)); event.add(delay); events.add(event);
        }
        @Override public void neighborChanged(BlockPos pos, Block block, Orientation orientation) { }
        @Override public void gameEvent(Holder<GameEvent> event, Vec3 pos, GameEvent.Context context) { }
    }
    static int index(BlockPos pos) {
        if (pos.equals(CENTER)) return -1;
        for (int i = 0; i < DIRECTIONS.length; i++) if (pos.equals(CENTER.relative(DIRECTIONS[i]))) return i;
        throw new AssertionError("unexpected position " + pos);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        world.attributes = EnvironmentAttributeSystem.builder().build();
        var method=FrostedIceBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class); method.setAccessible(true);
        var rows = new JsonArray();
        for (int seed=0;seed<16;seed++) for (int age=0;age<4;age++) for (int light : new int[]{0,8,9,10,11,12,15})
        for (int mask : new int[]{0,1,3,15,31,63}) for (boolean end : new boolean[]{false,true}) {
            world.blocks=new HashMap<>(); world.events=new JsonArray(); world.brightness=light; world.end=end;
            world.random=new LegacyRandomSource(seed);
            var center=Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,age);
            world.blocks.put(CENTER,center);
            for (int i=0;i<6;i++) if ((mask & (1<<i))!=0) world.blocks.put(CENTER.relative(DIRECTIONS[i]),Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,(seed+i)%4));
            method.invoke(Blocks.FROSTED_ICE,center,world,CENTER,world.random);
            var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("age",age); row.addProperty("light",light); row.addProperty("mask",mask); row.addProperty("end",end);
            row.add("events",world.events); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        var placements = new JsonArray();
        for (int seed=0; seed<128; seed++) {
            world.blocks=new HashMap<>(); world.events=new JsonArray(); world.random=new LegacyRandomSource(seed);
            ((FrostedIceBlock)Blocks.FROSTED_ICE).onPlace(Blocks.FROSTED_ICE.defaultBlockState(),world,CENTER,Blocks.WATER.defaultBlockState(),false);
            var row=new JsonObject(); row.addProperty("seed",seed); row.add("events",world.events); row.addProperty("next",world.random.nextInt()); placements.add(row);
        }
        var neighborMethod=FrostedIceBlock.class.getDeclaredMethod("neighborChanged",BlockState.class,Level.class,BlockPos.class,Block.class,Orientation.class,boolean.class); neighborMethod.setAccessible(true);
        var neighbors = new JsonArray();
        for (int mask=0;mask<64;mask++) for (int age=0;age<4;age++) for (boolean changedByIce : new boolean[]{false,true}) {
            world.blocks=new HashMap<>(); world.events=new JsonArray();
            var center=Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,age); world.blocks.put(CENTER,center);
            for (int i=0;i<6;i++) if ((mask & (1<<i))!=0) world.blocks.put(CENTER.relative(DIRECTIONS[i]),Blocks.FROSTED_ICE.defaultBlockState());
            neighborMethod.invoke(Blocks.FROSTED_ICE,center,world,CENTER,changedByIce ? Blocks.FROSTED_ICE : Blocks.STONE,null,false);
            var row=new JsonObject(); row.addProperty("mask",mask); row.addProperty("age",age); row.addProperty("changed_by_ice",changedByIce); row.add("events",world.events); neighbors.add(row);
        }
        var output=new JsonObject(); output.add("ticks",rows); output.add("placements",placements); output.add("neighbors",neighbors);
        Files.writeString(Path.of(args[0]),output.toString());
    }
}
