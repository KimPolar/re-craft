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
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.SculkSensorBlock;
import net.minecraft.world.level.block.CalibratedSculkSensorBlock;
import net.minecraft.world.level.block.entity.SculkSensorBlockEntity;
import net.minecraft.world.level.block.entity.CalibratedSculkSensorBlockEntity;
import net.minecraft.world.level.block.state.properties.SculkSensorPhase;
import net.minecraft.world.level.redstone.Orientation;
import net.minecraft.util.RandomSource;
import net.minecraft.sounds.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.tags.*;
import net.minecraft.world.level.gameevent.*;
import net.minecraft.world.level.gameevent.vibrations.*;
import net.minecraft.world.phys.Vec3;

public class VibrationOracle {
    static class World extends ServerLevel {
        long now; Set<BlockPos> occluders; int signal, delay, notifications, sounds, events; BlockState sensorState; RandomSource sensorRandom; BlockPos signalPosition; Direction signalDirection; float soundPitch; String soundId; net.minecraft.world.level.block.entity.BlockEntity sensorEntity;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public long getGameTime() { return now; }
        @Override public boolean shouldTickBlocksAt(long pos) { return false; }
        @Override public BlockState getBlockState(BlockPos pos) { return occluders != null && occluders.contains(pos) ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState(); }
        @Override public net.minecraft.world.level.block.entity.BlockEntity getBlockEntity(BlockPos pos) { return sensorEntity; }
        @Override public int getSignal(BlockPos pos, Direction direction) { signalPosition=pos;signalDirection=direction;return signal; }
        @Override public boolean setBlockAndUpdate(BlockPos pos, BlockState state) { sensorState=state; return true; }
        @Override public void scheduleTick(BlockPos pos, Block block, int ticks) { delay=ticks; }
        @Override public void updateNeighborsAt(BlockPos pos, Block block) { notifications++; }
        @Override public void updateNeighborsAt(BlockPos pos, Block block, Orientation orientation) { notifications++; }
        @Override public RandomSource getRandom() { return sensorRandom; }
        @Override public void playSound(Entity entity, BlockPos pos, SoundEvent event, SoundSource source, float volume, float pitch) { sounds++;soundPitch=pitch;soundId=event.location().toString(); }
        @Override public void playSound(Entity entity, double x,double y,double z,SoundEvent event,SoundSource source,float volume,float pitch) { sounds++;soundPitch=pitch;soundId=event.location().toString(); }
        @Override public void gameEvent(Entity entity, Holder<GameEvent> event, BlockPos pos) { events++; }
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
        boolean spectator, careful, dampens; Vec3 testPosition;
        Source() { super(null,0,0,0,ItemStack.EMPTY); }
        @Override public boolean isSpectator() { return spectator; }
        @Override public boolean isSteppingCarefully() { return careful; }
        @Override public boolean dampensVibrations() { return dampens; }
        @Override public Vec3 position() { return testPosition; }
        @Override public UUID getUUID() { return new UUID(1,2); }
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
        bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.OCCLUDES_VIBRATION_SIGNALS));
        var isOccluded=VibrationSystem.Listener.class.getDeclaredMethod("isOccluded",net.minecraft.world.level.Level.class,Vec3.class,Vec3.class); isOccluded.setAccessible(true);
        var rays=new JsonArray(); var random=new Random(31337);
        for (int n=0; n<1200; n++) {
            int x=random.nextInt(9)-4,y=random.nextInt(9)-4,z=random.nextInt(9)-4;
            int shift=n%3==0 ? 30000000 : n%3==1 ? -30000000 : 0;
            var from=new Vec3(shift+0.25,0.75,shift+0.1); var to=new Vec3(shift+x+0.3,y+0.8,shift+z+0.9);
            world.occluders=new HashSet<>();
            if ((n%3)==0) {
                int plane=random.nextInt(7)-3;
                for (int a=-5;a<=5;a++) for (int b=-5;b<=5;b++)
                    if (random.nextInt(9)!=0) world.occluders.add(new BlockPos(shift+plane,a,shift+b));
            } else for(int i=0;i<40;i++) world.occluders.add(new BlockPos(shift+random.nextInt(11)-5,random.nextInt(11)-5,shift+random.nextInt(11)-5));
            var blocks=new JsonArray(); for (var pos:world.occluders) { var p=new JsonArray();p.add(pos.getX());p.add(pos.getY());p.add(pos.getZ());blocks.add(p); }
            var row=new JsonObject(); row.addProperty("x",x);row.addProperty("y",y);row.addProperty("z",z);row.addProperty("shift",shift);row.add("blocks",blocks);
            row.addProperty("occluded",(boolean)isOccluded.invoke(null,world,from,to));rays.add(row);
        }
        var blockRegistry=new JsonArray(); for (var entry:BuiltInRegistries.BLOCK.entrySet()) blockRegistry.add(entry.getKey().identifier().toString());
        var sensorSignals=new JsonArray(); var sensorFilter=new JsonArray(); var sensorTicks=new JsonArray(); var sensorArrivals=new JsonArray();
        world.sensorRandom=RandomSource.create(1); world.occluders=Set.of();
        for (int calibrated=0;calibrated<2;calibrated++) for (var phase:SculkSensorPhase.values()) for (int power=0;power<=15;power++) for(boolean wet:new boolean[]{false,true}) {
            var state=(calibrated==0 ? Blocks.SCULK_SENSOR : Blocks.CALIBRATED_SCULK_SENSOR).defaultBlockState()
                .setValue(SculkSensorBlock.PHASE,phase).setValue(SculkSensorBlock.POWER,power).setValue(SculkSensorBlock.WATERLOGGED,wet);
            if (calibrated!=0) state=state.setValue(CalibratedSculkSensorBlock.FACING,Direction.NORTH);
            for (var direction:Direction.values()) {
                var row=new JsonObject();row.addProperty("calibrated",calibrated!=0);row.addProperty("phase",phase.getSerializedName());row.addProperty("power",power);row.addProperty("wet",wet);row.addProperty("direction",direction.getName());
                if(direction==Direction.DOWN) {
                    var shape=state.getShape(world,BlockPos.ZERO).bounds(); var collision=state.getCollisionShape(world,BlockPos.ZERO).bounds();
                    var outline=new JsonArray();var boxes=new JsonArray();
                    for(double v:new double[]{shape.minX,shape.minY,shape.minZ,shape.maxX,shape.maxY,shape.maxZ})outline.add(v);
                    for(double v:new double[]{collision.minX,collision.minY,collision.minZ,collision.maxX,collision.maxY,collision.maxZ})boxes.add(v);
                    row.add("outline",outline);row.add("collision",boxes);row.addProperty("outline_count",state.getShape(world,BlockPos.ZERO).toAabbs().size());row.addProperty("collision_count",state.getCollisionShape(world,BlockPos.ZERO).toAabbs().size());
                }
                row.addProperty("weak",state.getSignal(world,BlockPos.ZERO,direction));row.addProperty("direct",state.getDirectSignal(world,BlockPos.ZERO,direction));sensorSignals.add(row);
            }
            world.sensorState=state;world.delay=0;world.notifications=0;world.sounds=0;world.events=0;world.soundId="";world.soundPitch=0;world.sensorRandom=RandomSource.create(42);
            state.tick(world,BlockPos.ZERO,world.sensorRandom);
            var row=new JsonObject();row.addProperty("calibrated",calibrated!=0);row.addProperty("phase",phase.getSerializedName());row.addProperty("power",power);row.addProperty("wet",wet);
            row.addProperty("after_phase",world.sensorState.getValue(SculkSensorBlock.PHASE).getSerializedName());row.addProperty("after_power",world.sensorState.getValue(SculkSensorBlock.POWER));
            row.addProperty("delay",world.delay);row.addProperty("notifications",world.notifications);row.addProperty("sounds",world.sounds);row.addProperty("sound",world.soundId);row.addProperty("pitch_bits",Float.floatToRawIntBits(world.soundPitch));row.addProperty("next_random",world.sensorRandom.nextInt());sensorTicks.add(row);
        }
        for (int calibrated=0;calibrated<2;calibrated++) for (var phase:SculkSensorPhase.values()) {
            var state=(calibrated==0 ? Blocks.SCULK_SENSOR : Blocks.CALIBRATED_SCULK_SENSOR).defaultBlockState().setValue(SculkSensorBlock.PHASE,phase);
            if (calibrated!=0) state=state.setValue(CalibratedSculkSensorBlock.FACING,Direction.NORTH);
            var blockEntity=calibrated==0 ? new SculkSensorBlockEntity(BlockPos.ZERO,state) : new CalibratedSculkSensorBlockEntity(BlockPos.ZERO,state);
            for (var event:events) for (boolean same:new boolean[]{false,true}) for(int input=0;input<(calibrated==0 ? 1 : 16);input++) {
                world.signal=input;var row=new JsonObject(); row.addProperty("calibrated",calibrated!=0);row.addProperty("phase",phase.getSerializedName());row.addProperty("event",event.key().identifier().toString());row.addProperty("same",same);row.addProperty("input",input);
                world.signalPosition=null;world.signalDirection=null;
                row.addProperty("accepted",blockEntity.getVibrationUser().canReceiveVibration(world,same ? BlockPos.ZERO : BlockPos.ZERO.north(),event,GameEvent.Context.of((Entity)null)));
                row.addProperty("back_is_south",world.signalPosition!=null && world.signalPosition.equals(BlockPos.ZERO.south()) && world.signalDirection==Direction.SOUTH);sensorFilter.add(row);
            }
        }
        for (int calibrated=0;calibrated<2;calibrated++) for (var phase:SculkSensorPhase.values()) for(boolean wet:new boolean[]{false,true}) for(var event:List.of(GameEvent.SPLASH,GameEvent.STEP,GameEvent.EXPLODE,GameEvent.BLOCK_DESTROY)) for(float distance:new float[]{0,1,4,8,16}) {
            var state=(calibrated==0 ? Blocks.SCULK_SENSOR : Blocks.CALIBRATED_SCULK_SENSOR).defaultBlockState().setValue(SculkSensorBlock.PHASE,phase).setValue(SculkSensorBlock.WATERLOGGED,wet);
            if(calibrated!=0)state=state.setValue(CalibratedSculkSensorBlock.FACING,Direction.NORTH);
            var blockEntity=calibrated==0 ? new SculkSensorBlockEntity(BlockPos.ZERO,state) : new CalibratedSculkSensorBlockEntity(BlockPos.ZERO,state);
            world.sensorState=state;world.delay=0;world.notifications=0;world.sounds=0;world.events=0;world.soundId="";world.soundPitch=0;world.sensorRandom=RandomSource.create(42);
            blockEntity.getVibrationUser().onReceiveVibration(world,BlockPos.ZERO,event,null,null,distance);
            var row=new JsonObject();row.addProperty("calibrated",calibrated!=0);row.addProperty("phase",phase.getSerializedName());row.addProperty("wet",wet);row.addProperty("event",event.key().identifier().toString());row.addProperty("distance",distance);
            row.addProperty("after_phase",world.sensorState.getValue(SculkSensorBlock.PHASE).getSerializedName());row.addProperty("after_power",world.sensorState.getValue(SculkSensorBlock.POWER));row.addProperty("delay",world.delay);row.addProperty("notifications",world.notifications);row.addProperty("sounds",world.sounds);row.addProperty("events",world.events);row.addProperty("frequency",blockEntity.getLastVibrationFrequency());row.addProperty("sound",world.soundId);row.addProperty("pitch_bits",Float.floatToRawIntBits(world.soundPitch));row.addProperty("next_random",world.sensorRandom.nextInt());sensorArrivals.add(row);
        }
        bind.invoke(GameEvent.SPLASH,List.of(GameEventTags.VIBRATIONS));
        var sensorDispatch=new JsonArray(); world.now=100;
        var listenerRange=net.minecraft.world.level.gameevent.EuclideanGameEventListenerRegistry.class.getDeclaredMethod("getPostableListenerPosition",ServerLevel.class,Vec3.class,net.minecraft.world.level.gameevent.GameEventListener.class); listenerRange.setAccessible(true);
        for (boolean calibrated:new boolean[]{false,true}) for(var phase:SculkSensorPhase.values()) for(int input:new int[]{0,2,7,15}) for(boolean wall:new boolean[]{false,true}) for(double eventX:new double[]{3.5,8.99,9.0,16.99,17.0}) {
            var state=(calibrated ? Blocks.CALIBRATED_SCULK_SENSOR : Blocks.SCULK_SENSOR).defaultBlockState().setValue(SculkSensorBlock.PHASE,phase);
            if(calibrated)state=state.setValue(CalibratedSculkSensorBlock.FACING,Direction.NORTH);
            var blockEntity=calibrated ? new CalibratedSculkSensorBlockEntity(BlockPos.ZERO,state) : new SculkSensorBlockEntity(BlockPos.ZERO,state);
            world.signal=input;world.occluders=new HashSet<>();
            if(wall)for(int y=-2;y<=2;y++)for(int z=-2;z<=2;z++)world.occluders.add(new BlockPos(1,y,z));
            var position=new Vec3(eventX,0.5,0.5);
            boolean inRange=((Optional<?>)listenerRange.invoke(null,world,position,blockEntity.getListener())).isPresent();
            boolean accepted=inRange && blockEntity.getListener().handleGameEvent(world,GameEvent.SPLASH,GameEvent.Context.of((Entity)null),position);
            var row=new JsonObject();row.addProperty("calibrated",calibrated);row.addProperty("phase",phase.getSerializedName());row.addProperty("input",input);row.addProperty("wall",wall);row.addProperty("x",eventX);row.addProperty("accepted",accepted);sensorDispatch.add(row);
        }
        bind.invoke(GameEvent.STEP,List.of());
        var entityTypeField=Entity.class.getDeclaredField("type");entityTypeField.setAccessible(true);entityTypeField.set(entity,net.minecraft.world.entity.EntityTypes.ITEM);
        var sensorStep=new JsonArray();
        for(boolean calibrated:new boolean[]{false,true}) for(var phase:SculkSensorPhase.values()) for(int input:new int[]{0,1,2,15}) for(boolean current:new boolean[]{false,true}) for(boolean careful:new boolean[]{false,true}) {
            var state=(calibrated ? Blocks.CALIBRATED_SCULK_SENSOR : Blocks.SCULK_SENSOR).defaultBlockState().setValue(SculkSensorBlock.PHASE,phase);
            if(calibrated)state=state.setValue(CalibratedSculkSensorBlock.FACING,Direction.NORTH);
            var blockEntity=calibrated ? new CalibratedSculkSensorBlockEntity(BlockPos.ZERO,state) : new SculkSensorBlockEntity(BlockPos.ZERO,state);
            if(current) { blockEntity.getVibrationData().setCurrentVibration(new VibrationInfo(GameEvent.SPLASH,3.0f,new Vec3(3,0,0),(UUID)null,null));blockEntity.getVibrationData().setTravelTimeInTicks(3); }
            entity.testPosition=new Vec3(0.5,0.5,0.5);entity.careful=careful;entity.spectator=false;entity.dampens=false;
            world.sensorEntity=blockEntity;world.signal=input;world.now=100;
            ((SculkSensorBlock)state.getBlock()).stepOn(world,BlockPos.ZERO,state,entity);
            var selected=blockEntity.getVibrationData().getSelectionStrategy().chosenCandidate(101);
            var row=new JsonObject();row.addProperty("calibrated",calibrated);row.addProperty("phase",phase.getSerializedName());row.addProperty("input",input);row.addProperty("current",current);row.addProperty("careful",careful);row.addProperty("scheduled",selected.isPresent());sensorStep.add(row);
        }
        var result=new JsonObject(); result.add("frequencies", frequencies); result.add("strengths", strengths); result.add("distances",distances); result.add("selections", selections); result.add("timing",timing); result.add("filter",filter); result.add("rays",rays); result.add("block_registry",blockRegistry); result.add("sensor_signals",sensorSignals);result.add("sensor_filter",sensorFilter);result.add("sensor_ticks",sensorTicks);result.add("sensor_arrivals",sensorArrivals);result.add("sensor_dispatch",sensorDispatch);result.add("sensor_step",sensorStep);
        Files.writeString(Path.of(args[0]), result.toString());
        var registryLookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
        var sensorNbt=new net.minecraft.nbt.CompoundTag();
        for(int n=0;n<4;n++) {
            boolean calibrated=(n&1)!=0;
            var position=new BlockPos(-1,42,3);
            var state=(calibrated ? Blocks.CALIBRATED_SCULK_SENSOR : Blocks.SCULK_SENSOR).defaultBlockState();
            var blockEntity=calibrated ? new CalibratedSculkSensorBlockEntity(position,state) : new SculkSensorBlockEntity(position,state);
            blockEntity.setLastVibrationFrequency(n+1);
            var vibration=blockEntity.getVibrationData();
            if(n==1 || n==3) vibration.getSelectionStrategy().addCandidate(new VibrationInfo(GameEvent.SPLASH,1.25f,new Vec3(-1.25,42.5,3.75),new UUID(1,2),null),99);
            if(n>=2) { vibration.setCurrentVibration(new VibrationInfo(GameEvent.STEP,4.5f,new Vec3(2.25,42.0,-3.5),new UUID(3,4),new UUID(5,6)));vibration.setTravelTimeInTicks(3); }
            sensorNbt.put("sensor"+n,blockEntity.saveWithFullMetadata(registryLookup));
        }
        net.minecraft.nbt.NbtIo.write(sensorNbt,Path.of(args[0]+".nbt"));
    }
}
