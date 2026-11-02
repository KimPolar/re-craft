import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;

public class PlayerBubblePhysicsOracle {
    static final BlockPos POS=new BlockPos(8,10,8);
    static class World extends ServerLevel {
        BlockState above; RandomSource random; int packets;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos pos) { return pos.equals(POS.above())?above:Blocks.AIR.defaultBlockState(); }
        @Override public RandomSource getRandom() { return random; }
        @Override public <T extends ParticleOptions> int sendParticles(T type,double x,double y,double z,int count,double dx,double dy,double dz,double speed) { packets++; return 0; }
    }
    static class TestPlayer extends Player {
        Abilities abilities; Vec3 velocity;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public Abilities getAbilities() { return abilities; }
        @Override public MoveSimulationType getMoveSimulationType() { return MoveSimulationType.SERVER_AND_CLIENT; }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public void setDeltaMovement(double x,double y,double z) { velocity=new Vec3(x,y,z); }
        @Override public void setDeltaMovement(Vec3 value) { velocity=value; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.abilities=new Abilities();
        var level=Entity.class.getDeclaredField("level"); unsafe.putObject(player,unsafe.objectFieldOffset(level),world);
        var method=BubbleColumnBlock.class.getDeclaredMethod("entityInside",BlockState.class,Level.class,BlockPos.class,Entity.class,InsideBlockEffectApplier.class,boolean.class); method.setAccessible(true);
        var rows=new JsonArray();
        for(int above=0;above<4;above++) for(boolean drag:new boolean[]{false,true}) for(boolean flying:new boolean[]{false,true}) for(double velocity:new double[]{-2,-0.91,-0.9,-0.31,-0.3,0,0.64,0.7,1.7,1.8,2}) {
            world.above=switch(above){case 0->Blocks.AIR.defaultBlockState(); case 1->Blocks.WATER.defaultBlockState(); case 2->Blocks.STONE.defaultBlockState(); default->Blocks.BUBBLE_COLUMN.defaultBlockState();};
            world.random=new LegacyRandomSource(77); world.packets=0;
            player.abilities.flying=flying; player.velocity=new Vec3(0.2,velocity,-0.3); player.fallDistance=5;
            var state=Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN,drag);
            method.invoke(Blocks.BUBBLE_COLUMN,state,world,POS,player,null,true);
            var row=new JsonObject(); row.addProperty("above",above); row.addProperty("drag",drag); row.addProperty("flying",flying); row.addProperty("initial",velocity); row.addProperty("velocity",player.velocity.y); row.addProperty("fall",player.fallDistance); row.addProperty("packets",world.packets); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
