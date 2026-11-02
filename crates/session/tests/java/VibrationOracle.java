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
        long now; Set<BlockPos> occluders; int signal, delay, notifications, sounds, events; BlockState sensorState, movementState, affectedState; RandomSource sensorRandom; BlockPos signalPosition; Direction signalDirection; float soundPitch; String soundId; net.minecraft.world.level.block.entity.BlockEntity sensorEntity; String movementEvent; Vec3 eventPosition; Entity eventSource;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public long getGameTime() { return now; }
        @Override public boolean shouldTickBlocksAt(long pos) { return false; }
        @Override public BlockState getBlockState(BlockPos pos) { if(movementState != null && pos.equals(BlockPos.ZERO)) return movementState; return occluders != null && occluders.contains(pos) ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState(); }
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
        @Override public void gameEvent(Holder<GameEvent> event, Vec3 pos, GameEvent.Context context) { movementEvent=event==GameEvent.STEP ? "step" : event==GameEvent.HIT_GROUND ? "hit_ground" : event==GameEvent.BLOCK_PLACE ? "block_place" : event==GameEvent.BLOCK_DESTROY ? "block_destroy" : "other"; affectedState=context.affectedState();eventPosition=pos;eventSource=context.sourceEntity(); }
        @Override public net.minecraft.world.flag.FeatureFlagSet enabledFeatures() { return net.minecraft.world.flag.FeatureFlags.DEFAULT_FLAGS; }
        @Override public void levelEvent(Entity player, int id, BlockPos pos, int data) {}
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
    static class MovementSource extends Source {
        World testWorld; boolean ground, crouching, swimming, water, rails;
        MovementSource() { super(); }
        @Override public net.minecraft.world.level.Level level() { return testWorld; }
        @Override public BlockPos getOnPos() { return BlockPos.ZERO; }
        @Override public boolean onGround() { return ground; }
        @Override public boolean isCrouching() { return crouching; }
        @Override public boolean isSwimming() { return swimming; }
        @Override public boolean isInWater() { return water; }
        @Override public boolean isOnRails() { return rails; }
        @Override public void gameEvent(Holder<GameEvent> event) { testWorld.movementEvent=event==GameEvent.SWIM ? "swim" : "other"; }
        void applyFall(double vertical, boolean grounded, BlockState block) { super.checkFallDamage(vertical,grounded,block,BlockPos.ZERO); }
    }
    static class ProbeBlock extends Block {
        boolean landed;
        ProbeBlock() { super(net.minecraft.world.level.block.state.BlockBehaviour.Properties.of().setId(net.minecraft.resources.ResourceKey.create(net.minecraft.core.registries.Registries.BLOCK,net.minecraft.resources.Identifier.parse("test:fall_probe")))); }
        @Override public void fallOn(net.minecraft.world.level.Level level, BlockState state, BlockPos pos, Entity entity, double distance) { landed=true; }
    }
    static class ProbePlayer extends net.minecraft.world.entity.player.Player {
        boolean careful;
        ProbePlayer() { super(null,null); }
        @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
        @Override public boolean hasInfiniteMaterials() { return false; }
        @Override public boolean isSteppingCarefully() { return careful; }
    }
    static class ProbePlacement extends net.minecraft.world.item.context.BlockPlaceContext {
        World world; ProbePlayer player; ItemStack stack; boolean canPlace;
        ProbePlacement() { super(null,null,null,null); }
        @Override public net.minecraft.world.level.Level getLevel() { return world; }
        @Override public net.minecraft.world.entity.player.Player getPlayer() { return player; }
        @Override public ItemStack getItemInHand() { return stack; }
        @Override public BlockPos getClickedPos() { return BlockPos.ZERO; }
        @Override public boolean canPlace() { return canPlace; }
    }
    static class ProbeItem extends net.minecraft.world.item.BlockItem {
        boolean success;
        ProbeItem() { super(null,new net.minecraft.world.item.Item.Properties()); }
        @Override public Block getBlock() { return Blocks.STONE; }
        @Override protected BlockState getPlacementState(net.minecraft.world.item.context.BlockPlaceContext context) { return Blocks.STONE.defaultBlockState(); }
        @Override protected boolean placeBlock(net.minecraft.world.item.context.BlockPlaceContext context,BlockState state) { return success; }
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
        bind.invoke(Blocks.LADDER.builtInRegistryHolder(),List.of(BlockTags.CLIMBABLE));
        var mover=(MovementSource)unsafe.allocateInstance(MovementSource.class); mover.testWorld=world; mover.testPosition=Vec3.ZERO;
        var emissionClass=Class.forName("net.minecraft.world.entity.Entity$MovementEmission");
        var emission=emissionClass.getDeclaredField("EVENTS").get(null);
        var movementMethod=Entity.class.getDeclaredMethod("applyMovementEmissionAndPlaySound",emissionClass,Vec3.class,BlockPos.class,BlockState.class); movementMethod.setAccessible(true);
        var nextStep=Entity.class.getDeclaredField("nextStep"); nextStep.setAccessible(true);
        var movementRows=new JsonArray(); var movementRandom=new Random(26423);
        var states=new BlockState[]{Blocks.AIR.defaultBlockState(),Blocks.STONE.defaultBlockState(),Blocks.LADDER.defaultBlockState(),Blocks.POWDER_SNOW.defaultBlockState()};
        for(int group=0;group<128;group++) {
            mover.moveDist=0; mover.flyDist=0; nextStep.setFloat(mover,1.0f);
            for(int tick=0;tick<8;tick++) {
                int modern=movementRandom.nextInt(states.length), legacy=movementRandom.nextInt(states.length);
                boolean same=movementRandom.nextBoolean(); if(same)legacy=modern;
                mover.ground=movementRandom.nextBoolean();mover.crouching=movementRandom.nextBoolean();mover.swimming=movementRandom.nextBoolean();mover.water=movementRandom.nextBoolean();mover.rails=movementRandom.nextBoolean();
                boolean enabled=movementRandom.nextBoolean();
                double x=(movementRandom.nextInt(9)-4)*0.37,y=(movementRandom.nextInt(5)-2)*0.41,z=(movementRandom.nextInt(9)-4)*0.29;
                world.movementState=states[modern];world.movementEvent="none";
                if(enabled)movementMethod.invoke(mover,emission,new Vec3(x,y,z),same ? BlockPos.ZERO : new BlockPos(1,0,0),states[legacy]);
                var row=new JsonObject(); row.addProperty("group",group);row.addProperty("modern",modern);row.addProperty("legacy",legacy);row.addProperty("same",same);row.addProperty("enabled",enabled);row.addProperty("ground",mover.ground);row.addProperty("crouching",mover.crouching);row.addProperty("swimming",mover.swimming);row.addProperty("water",mover.water);row.addProperty("rails",mover.rails);row.addProperty("x",x);row.addProperty("y",y);row.addProperty("z",z);row.addProperty("distance",Float.floatToRawIntBits(mover.moveDist));row.addProperty("flying_distance",Float.floatToRawIntBits(mover.flyDist));row.addProperty("next",Float.floatToRawIntBits(nextStep.getFloat(mover)));row.addProperty("event",world.movementEvent);movementRows.add(row);
            }
        }
        world.movementState=null;
        var probe=(ProbeBlock)unsafe.allocateInstance(ProbeBlock.class); var probeState=(BlockState)unsafe.allocateInstance(BlockState.class);
        var owner=net.minecraft.world.level.block.state.StateHolder.class.getDeclaredField("owner");owner.setAccessible(true);owner.set(probeState,probe);
        var fallRows=new JsonArray();
        var positionField=Entity.class.getDeclaredField("position");positionField.setAccessible(true);positionField.set(mover,Vec3.ZERO);
        for(boolean water:new boolean[]{false,true}) for(boolean ground:new boolean[]{false,true}) for(boolean support:new boolean[]{false,true})
        for(double distance:new double[]{0,0.01,0.5,3,100000000}) for(double vertical:new double[]{-3.141592653589793,-0.2,-1e-12,0,0.2}) {
            mover.water=water;mover.fallDistance=distance;mover.mainSupportingBlockPos=support ? Optional.of(BlockPos.ZERO) : Optional.empty();
            world.movementState=Blocks.STONE.defaultBlockState();world.movementEvent="none";world.affectedState=null;probe.landed=false;
            mover.applyFall(vertical,ground,probeState);
            var row=new JsonObject();row.addProperty("water",water);row.addProperty("ground",ground);row.addProperty("support",support);row.addProperty("initial",distance);row.addProperty("vertical",vertical);row.addProperty("fall_bits",Double.doubleToRawLongBits(mover.fallDistance));row.addProperty("event",world.movementEvent);row.addProperty("fall_on",probe.landed);row.addProperty("affected",world.affectedState==null ? "none" : world.affectedState==world.movementState ? "support" : "legacy");fallRows.add(row);
        }
        world.movementState=null;
        var player=(ProbePlayer)unsafe.allocateInstance(ProbePlayer.class);var placeContext=(ProbePlacement)unsafe.allocateInstance(ProbePlacement.class);var placeItem=(ProbeItem)unsafe.allocateInstance(ProbeItem.class);
        net.minecraft.world.item.Items.STONE.builtInRegistryHolder().bindComponents(net.minecraft.core.component.DataComponentMap.builder().set(net.minecraft.core.component.DataComponents.MAX_STACK_SIZE,64).build());
        placeContext.world=world;placeContext.player=player;world.movementState=Blocks.STONE.defaultBlockState();
        var blockEvents=new JsonArray();
        for(boolean careful:new boolean[]{false,true}) for(boolean allowed:new boolean[]{false,true}) for(boolean succeeds:new boolean[]{false,true}) {
            player.careful=careful;placeContext.canPlace=allowed;placeItem.success=succeeds;placeContext.stack=new ItemStack(net.minecraft.world.item.Items.STONE,2);
            world.movementEvent="none";world.affectedState=null;world.eventPosition=null;world.eventSource=null;
            placeItem.place(placeContext);
            var row=new JsonObject();row.addProperty("careful",careful);row.addProperty("allowed",allowed);row.addProperty("success",succeeds);row.addProperty("event",world.movementEvent);row.addProperty("affected",world.affectedState==world.movementState);row.addProperty("source",world.eventSource==player);row.addProperty("center",new Vec3(0.5,0.5,0.5).equals(world.eventPosition));row.addProperty("count",placeContext.stack.getCount());blockEvents.add(row);
        }
        for(boolean careful:new boolean[]{false,true}) {
            player.careful=careful;world.movementEvent="none";world.affectedState=null;world.eventPosition=null;world.eventSource=null;
            Blocks.STONE.playerWillDestroy(world,BlockPos.ZERO,world.movementState,player);
            var row=new JsonObject();row.addProperty("careful",careful);row.addProperty("event",world.movementEvent);row.addProperty("affected",world.affectedState==world.movementState);row.addProperty("source",world.eventSource==player);row.addProperty("center",new Vec3(0.5,0.5,0.5).equals(world.eventPosition));blockEvents.add(row);
        }
        world.movementState=null;
        var result=new JsonObject(); result.add("frequencies", frequencies); result.add("strengths", strengths); result.add("distances",distances); result.add("selections", selections); result.add("timing",timing); result.add("filter",filter); result.add("rays",rays); result.add("block_registry",blockRegistry); result.add("sensor_signals",sensorSignals);result.add("sensor_filter",sensorFilter);result.add("sensor_ticks",sensorTicks);result.add("sensor_arrivals",sensorArrivals);result.add("sensor_dispatch",sensorDispatch);result.add("sensor_step",sensorStep);result.add("movement",movementRows);result.add("landing",fallRows);result.add("block_events",blockEvents);
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
