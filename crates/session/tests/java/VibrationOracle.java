import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.tags.*;
import net.minecraft.world.level.gameevent.*;
import net.minecraft.world.level.gameevent.vibrations.*;
import net.minecraft.world.phys.Vec3;

public class VibrationOracle {
    static class World extends ServerLevel {
        long now;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public long getGameTime() { return now; }
        @Override public boolean shouldTickBlocksAt(long pos) { return false; }
        @Override public <T extends ParticleOptions> int sendParticles(T type, double x, double y, double z, int count, double dx, double dy, double dz, double speed) { return 0; }
    }
    static class User implements VibrationSystem.User {
        int received, changed; boolean requireChunks;
        public int getListenerRadius() { return 8; }
        public PositionSource getPositionSource() { return new BlockPositionSource(BlockPos.ZERO); }
        public boolean canReceiveVibration(ServerLevel world, BlockPos pos, Holder<GameEvent> event, GameEvent.Context context) { return true; }
        public void onReceiveVibration(ServerLevel world, BlockPos pos, Holder<GameEvent> event, Entity entity, Entity owner, float distance) { received++; }
        public void onDataChanged() { changed++; }
        public boolean requiresAdjacentChunksToBeTicking() { return requireChunks; }
    }
    static class Source extends ItemEntity {
        boolean spectator, careful, dampens;
        Source() { super(null,0,0,0,ItemStack.EMPTY); }
        @Override public boolean isSpectator() { return spectator; }
        @Override public boolean isSteppingCarefully() { return careful; }
        @Override public boolean dampensVibrations() { return dampens; }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var frequencies = new JsonObject();
        for (var entry : BuiltInRegistries.GAME_EVENT.entrySet()) {
            frequencies.addProperty(entry.getKey().identifier().toString(), VibrationSystem.getGameEventFrequency(entry.getKey()));
        }
        var strengths = new JsonArray();
        for (int radius : new int[]{1, 8, 16, 32}) for (int step = 0; step <= 32; step++) {
            float center = radius * step / 16.0f;
            for (float distance : new float[]{Math.nextDown(center), center, Math.nextUp(center)}) {
                var row = new JsonObject(); row.addProperty("radius", radius); row.addProperty("distance", Float.floatToRawIntBits(distance));
                row.addProperty("strength", VibrationSystem.getRedstoneStrengthForDistance(distance, radius)); strengths.add(row);
            }
        }
        var distances = new JsonArray();
        for (double[] coords : new double[][]{
            {0.1,64.0,-0.9,8.5,64.5,0.5},
            {-30000000.3,-64.25,29999999.75,-29999992.5,-60.5,29999998.5},
            {1.99,2.99,3.99,0.5,0.5,0.5},
            {0,0,0,0,0,0},
            {-1.0,-1.0,-1.0,-0.01,-0.01,-0.01}
        }) {
            var a=new Vec3(coords[0],coords[1],coords[2]); var b=new Vec3(coords[3],coords[4],coords[5]);
            var row=new JsonObject(); var bits=new JsonArray();
            for (double coord : coords) bits.add(Double.doubleToRawLongBits(coord));
            row.add("coords",bits); row.addProperty("travel",Float.floatToRawIntBits((float)a.distanceTo(b)));
            row.addProperty("blocks",Float.floatToRawIntBits(VibrationSystem.Listener.distanceBetweenInBlocks(BlockPos.containing(a),BlockPos.containing(b))));
            distances.add(row);
        }
        var events = BuiltInRegistries.GAME_EVENT.listElements().toList();
        var selections = new JsonArray();
        for (var first : events) for (var second : events) {
            for (int scenario=0; scenario<5; scenario++) {
                float d1=2.0f, d2=switch(scenario){ case 0 -> 1.0f; case 1 -> 2.0f; default -> 3.0f; };
                long tick2=scenario==3 ? 6 : scenario==4 ? 4 : 5;
                var selector=new VibrationSelector();
                selector.addCandidate(new VibrationInfo(first, d1, new Vec3(1,0,0), (java.util.UUID)null, null), 5);
                selector.addCandidate(new VibrationInfo(second, d2, new Vec3(2,0,0), (java.util.UUID)null, null), tick2);
                var row=new JsonObject(); row.addProperty("first", first.key().identifier().toString());
                row.addProperty("second", second.key().identifier().toString()); row.addProperty("d1", d1); row.addProperty("d2", d2);
                row.addProperty("tick2", tick2); row.addProperty("same_tick", selector.chosenCandidate(5).isPresent());
                row.addProperty("chosen", selector.chosenCandidate(6).orElseThrow().pos().x); selections.add(row);
            }
        }
        var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)unsafeField.get(null); var world=(World)unsafe.allocateInstance(World.class);
        var timing = new JsonArray();
        for (boolean blocked : new boolean[]{false,true}) for (float distance : new float[]{0, 0.2f, 0.999f, 1, 1.2f, 2.9f, 8}) {
            var data = new VibrationSystem.Data(); var user = new User();
            data.getSelectionStrategy().addCandidate(new VibrationInfo(GameEvent.SPLASH, distance, new Vec3(1, 0, 0), (java.util.UUID)null, null), 100);
            for (int tick=100; tick<=110; tick++) {
                world.now=tick; user.requireChunks=blocked && tick<105; VibrationSystem.Ticker.tick(world,data,user);
                var row=new JsonObject(); row.addProperty("distance", Float.floatToRawIntBits(distance)); row.addProperty("tick", tick);
                row.addProperty("ready", !user.requireChunks);
                row.addProperty("active", data.getCurrentVibration()!=null); row.addProperty("remaining", data.getTravelTimeInTicks());
                row.addProperty("received", user.received); row.addProperty("changed", user.changed); timing.add(row);
            }
        }
        var filter = new JsonArray();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        var entity=(Source)unsafe.allocateInstance(Source.class); var user=new User();
        for (int mask=0; mask<128; mask++) {
            boolean vibration=(mask&1)!=0, ignored=(mask&2)!=0, present=(mask&4)!=0;
            boolean spectator=(mask&8)!=0, careful=(mask&16)!=0, dampens=(mask&32)!=0, block=(mask&64)!=0;
            var tags=new ArrayList<>(); if(vibration)tags.add(GameEventTags.VIBRATIONS); if(ignored)tags.add(GameEventTags.IGNORE_VIBRATIONS_SNEAKING);
            bind.invoke(GameEvent.SPLASH,tags);
            bind.invoke(Blocks.STONE.builtInRegistryHolder(),block ? List.of(BlockTags.DAMPENS_VIBRATIONS) : List.of());
            entity.spectator=spectator; entity.careful=careful; entity.dampens=dampens;
            var context=GameEvent.Context.of(present ? entity : null, Blocks.STONE.defaultBlockState());
            var row=new JsonObject(); row.addProperty("mask",mask); row.addProperty("accepted",user.isValidVibration(GameEvent.SPLASH,context)); filter.add(row);
        }
        var result=new JsonObject(); result.add("frequencies", frequencies); result.add("strengths", strengths); result.add("distances",distances); result.add("selections", selections); result.add("timing",timing); result.add("filter",filter);
        Files.writeString(Path.of(args[0]), result.toString());
    }
}
